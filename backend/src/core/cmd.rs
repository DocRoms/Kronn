//! Cross-platform command helpers.
//!
//! On Windows, every `Command::new()` spawns a visible console window by default.
//! These helpers apply the `CREATE_NO_WINDOW` flag so background processes (git, wsl.exe, etc.)
//! run invisibly — critical for the Tauri desktop app experience.
//!
//! On Windows, they also resolve bare program names (`"npx"`, `"git"`, `"node"`)
//! to their fully-qualified path (`npx.cmd`, `git.exe`, `node.exe`) via `which`.
//! Without this, `Command::new("npx")` fails with "program not found" because
//! Win32 `CreateProcess` refuses to execute `.cmd`/`.bat` wrappers when called
//! by their bare name — only `.exe` works without an extension. Reported by a
//! Windows user (npm-installed Node.js, `npx` accessible in PowerShell, but
//! Kronn raised "Spawn failed for npx: program not found").

use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

/// Windows: CREATE_NO_WINDOW flag prevents a console window from appearing.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// On Windows, resolve a bare program name to its full path so `.cmd`/`.bat`
/// wrappers (npx, npm, yarn, pnpm…) can be spawned. No-op for paths that
/// already point at a real file (absolute, contains `\` or `/`, or has an
/// extension). Returns `None` when `which` can't find the binary — caller
/// falls back to the original input so the existing "program not found"
/// error path still surfaces.
#[cfg(target_os = "windows")]
fn resolve_windows_program(program: &OsStr) -> Option<PathBuf> {
    let s = program.to_str()?;
    // Already an explicit path or has an extension — let CreateProcess handle it.
    if s.contains('\\') || s.contains('/') || s.contains('.') {
        return None;
    }
    which::which(s).ok()
}

pub use crate::core::child_env::ChildRoute;

/// The only place a process is constructed (clippy's `disallowed_methods`
/// refuses `Command::new` elsewhere). No environment is applied yet.
#[allow(clippy::disallowed_methods)]
fn raw_async(program: &OsStr) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    let mut cmd = match resolve_windows_program(program) {
        Some(path) => tokio::process::Command::new(path),
        None => tokio::process::Command::new(program),
    };
    #[cfg(not(target_os = "windows"))]
    let cmd = tokio::process::Command::new(program);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[allow(clippy::disallowed_methods)]
fn raw_sync(program: &OsStr) -> std::process::Command {
    #[cfg(target_os = "windows")]
    let mut cmd = match resolve_windows_program(program) {
        Some(path) => std::process::Command::new(path),
        None => std::process::Command::new(program),
    };
    #[cfg(not(target_os = "windows"))]
    let cmd = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A `tokio::process::Command` for `program` with the environment built for
/// `route` (KT-1006): nothing of the backend's environment beyond the route's
/// allow-list. Values the caller adds afterwards are its launch's own; a
/// caller adding any must seal again (`child_env::seal`).
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, …).
pub fn async_cmd<S: AsRef<OsStr>>(program: S, route: ChildRoute) -> tokio::process::Command {
    let mut cmd = raw_async(program.as_ref());
    crate::core::child_env::isolate(cmd.as_std_mut(), route);
    cmd
}

/// [`async_cmd`] for a `std::process::Command`.
pub fn sync_cmd<S: AsRef<OsStr>>(program: S, route: ChildRoute) -> std::process::Command {
    let mut cmd = raw_sync(program.as_ref());
    crate::core::child_env::isolate(&mut cmd, route);
    cmd
}

/// `git`, whose repository hooks, filters and drivers run inside it.
pub fn git_cmd() -> std::process::Command {
    sync_cmd("git", ChildRoute::Git)
}

/// [`git_cmd`] for tokio.
pub fn async_git_cmd() -> tokio::process::Command {
    async_cmd("git", ChildRoute::Git)
}

/// A program Kronn runs for itself (installer, system probe): the base
/// allow-list only.
pub fn tool_cmd<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    async_cmd(program, ChildRoute::Tool)
}

/// [`tool_cmd`] for a `std::process::Command`.
pub fn sync_tool_cmd<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    sync_cmd(program, ChildRoute::Tool)
}

/// Why a process inherits the backend's environment instead of a route's.
/// Each variant is a declared exception of the design note (§9); the test
/// `full_env_cmd_sites_are_exactly_the_declared_exceptions` lists the call
/// sites in both crates. No exception ever receives a secret: see
/// [`child_env::strip_inherited_secrets`](crate::core::child_env::strip_inherited_secrets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullEnvReason {
    /// The document sidecar Kronn ships.
    DocsSidecar,
    /// A CLI asked which models it serves.
    ModelDiscovery,
    /// A CLI asked for its version.
    VersionDiscovery,
    /// The desktop app relaunching itself: it keeps its own environment but
    /// the forbidden names (the caller hands the key override back).
    SelfRestart,
}

/// A process that inherits the backend's environment, for a declared
/// [`FullEnvReason`] only, with every secret removed. It starts in the
/// temporary directory, never in a repository (the backend's own directory
/// may be one).
pub fn full_env_cmd<S: AsRef<OsStr>>(program: S, reason: FullEnvReason) -> tokio::process::Command {
    let mut cmd = raw_async(program.as_ref());
    crate::core::child_env::strip_inherited_secrets(
        cmd.as_std_mut(),
        reason == FullEnvReason::SelfRestart,
    );
    if reason != FullEnvReason::SelfRestart {
        cmd.current_dir(std::env::temp_dir());
    }
    cmd
}

/// [`full_env_cmd`] for a `std::process::Command`.
pub fn full_env_sync_cmd<S: AsRef<OsStr>>(
    program: S,
    reason: FullEnvReason,
) -> std::process::Command {
    let mut cmd = raw_sync(program.as_ref());
    crate::core::child_env::strip_inherited_secrets(&mut cmd, reason == FullEnvReason::SelfRestart);
    if reason != FullEnvReason::SelfRestart {
        cmd.current_dir(std::env::temp_dir());
    }
    cmd
}

/// The system opener (`open`, `xdg-open`, `start`) for `target`, with the
/// base allow-list only. The `open` crate's own spawning entry points are
/// refused by clippy; its commands are taken here and rebuilt.
#[allow(clippy::disallowed_methods)] // open::commands: each command is isolated below
pub fn open_in_system<T: AsRef<OsStr>>(target: T) -> std::io::Result<()> {
    let mut last = Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no system opener",
    ));
    for command in open::commands(target) {
        let mut command = rebuild_for_tool(command);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        last = match command.status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => Err(std::io::Error::other(format!(
                "opener exited with {status}"
            ))),
            Err(error) => Err(error),
        };
    }
    last
}

/// The opener command `open::commands` built, isolated with the Tool route.
fn rebuild_for_tool(mut command: std::process::Command) -> std::process::Command {
    crate::core::child_env::isolate(&mut command, ChildRoute::Tool);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_cmd_creates_command() {
        let cmd = tool_cmd("echo");
        // Just verify it doesn't panic — creation_flags is Windows-only
        drop(cmd);
    }

    /// A repository hook runs inside git's process: it never sees a variable
    /// outside the git allow-list (synthetic sentinels, no real secret).
    #[cfg(unix)]
    #[test]
    fn a_git_hook_runs_without_the_backend_environment() {
        use std::os::unix::fs::PermissionsExt;
        let repo = tempfile::tempdir().unwrap();
        // Also in the real process environment: without the policy, git would
        // inherit it from there. A name nothing else in the suite reads.
        std::env::set_var("KRONN_HOOK_SENTINEL_API_KEY", "sentinel-real-env");
        let git = |args: &[&str]| {
            let output = crate::core::child_env::with_parent_env(
                &[
                    ("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"),
                    ("HOME", repo.path().to_str().unwrap()),
                    ("GIT_AUTHOR_NAME", "Fixture"),
                    ("GIT_AUTHOR_EMAIL", "fixture@example.invalid"),
                    ("GIT_COMMITTER_NAME", "Fixture"),
                    ("GIT_COMMITTER_EMAIL", "fixture@example.invalid"),
                    ("KRONN_AUTH_TOKEN", "sentinel-admin"),
                    ("KRONN_ENCRYPTION_KEK", "sentinel-kek"),
                    ("ANTHROPIC_API_KEY", "sentinel-provider"),
                    ("GH_TOKEN", "sentinel-github"),
                    ("GIT_DIR", "/sentinel/elsewhere"),
                ],
                || git_cmd().args(args).current_dir(repo.path()).output(),
            )
            .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        let hook = repo.path().join(".git/hooks/pre-commit");
        std::fs::write(
            &hook,
            "#!/bin/sh\nfor v in KRONN_HOOK_SENTINEL_API_KEY KRONN_AUTH_TOKEN KRONN_ENCRYPTION_KEK ANTHROPIC_API_KEY GH_TOKEN; do\n  eval \"val=\\${$v-}\"\n  if [ -n \"$val\" ]; then echo \"$v leaked\" >&2; exit 1; fi\ndone\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(repo.path().join("f.txt"), "x").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "fixture"]);
        std::env::remove_var("KRONN_HOOK_SENTINEL_API_KEY");
    }

    /// A program Kronn runs for itself gets the base allow-list only.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_command_runs_without_the_backend_environment() {
        use crate::core::child_env::probe;
        probe::plant_real_sentinel();
        let dir = tempfile::tempdir().unwrap();
        let out_async = probe::env_dumping_program(dir.path(), "kronn-tool");
        let path = format!("{}:/usr/bin:/bin", dir.path().display());
        let home = dir.path().to_str().unwrap();
        let (mut async_command, mut sync_command) = probe::with_secret_parent(&path, home, || {
            (tool_cmd("kronn-tool"), sync_tool_cmd("kronn-tool"))
        });
        probe::assert_built_without_secrets(async_command.as_std(), &path, &[]);
        probe::assert_built_without_secrets(&sync_command, &path, &[]);
        assert!(async_command.status().await.unwrap().success());
        let recorded = probe::read_dump(&out_async);
        probe::assert_dump_without_secrets(&recorded, &[]);
        assert_eq!(recorded.get("HOME").map(String::as_str), Some(home));
        std::fs::remove_file(&out_async).unwrap();
        assert!(sync_command.status().unwrap().success());
        probe::assert_dump_without_secrets(&probe::read_dump(&out_async), &[]);
    }

    /// The system opener is started with the Tool environment, every
    /// candidate the platform has (B3-11).
    #[test]
    fn the_system_opener_gets_no_backend_secret() {
        use crate::core::child_env::probe;
        let commands = probe::with_secret_parent("/usr/bin", "/home/u", || {
            open::commands("https://example.invalid")
                .into_iter()
                .map(rebuild_for_tool)
                .collect::<Vec<_>>()
        });
        assert!(!commands.is_empty());
        for command in &commands {
            probe::assert_built_without_secrets(command, "/usr/bin", &[]);
        }
    }

    /// Every Rust file of the backend (sources and integration tests) and of
    /// the desktop crate, as (`crate/relative/path`, text).
    fn rust_sources() -> Vec<(String, String)> {
        let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let roots = [
            ("backend/src", backend.join("src")),
            ("backend/tests", backend.join("tests")),
            ("desktop/src", backend.join("../desktop/src-tauri/src")),
        ];
        let mut files = Vec::new();
        for (label, root) in roots {
            assert!(root.is_dir(), "{} is missing", root.display());
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "rs") {
                        let rel = path.strip_prefix(&root).unwrap().to_string_lossy();
                        files.push((
                            format!("{label}/{}", rel.replace('\\', "/")),
                            std::fs::read_to_string(&path).unwrap(),
                        ));
                    }
                }
            }
        }
        files
    }

    /// Exactly the exceptions design §9 declares inherit the environment,
    /// in either crate; any other process needs a route to compile.
    #[test]
    fn full_env_cmd_sites_are_exactly_the_declared_exceptions() {
        let expected: &[(&str, &str, usize)] = &[
            ("backend/src/agents/mod.rs", "VersionDiscovery", 4),
            ("backend/src/core/docs_sidecar.rs", "DocsSidecar", 2),
            (
                "backend/src/core/model_catalog/claude_discovery.rs",
                "ModelDiscovery",
                2,
            ),
            (
                "backend/src/core/model_catalog/codex_discovery.rs",
                "ModelDiscovery",
                1,
            ),
            ("backend/src/core/versions.rs", "VersionDiscovery", 1),
            ("desktop/src/main.rs", "SelfRestart", 1),
        ];
        let mut found: std::collections::BTreeMap<(String, String), usize> = Default::default();
        for (rel, text) in rust_sources() {
            if rel == "backend/src/core/cmd.rs" {
                continue;
            }
            for (at, _) in text.match_indices("full_env_") {
                let rest = &text[at..];
                let call = ["full_env_cmd", "full_env_sync_cmd"]
                    .into_iter()
                    .find(|name| rest.starts_with(name))
                    .unwrap_or_else(|| panic!("{rel}: unknown full_env_ name"));
                let after = &rest[call.len()..];
                if after.starts_with(',') || after.starts_with('}') {
                    continue; // an import
                }
                assert!(after.starts_with('('), "{rel}: {call} used as a value");
                let reason_at = after
                    .find("FullEnvReason::")
                    .unwrap_or_else(|| panic!("{rel}: {call} without a FullEnvReason"));
                let reason: String = after[reason_at + "FullEnvReason::".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                *found.entry((rel.clone(), reason)).or_default() += 1;
            }
        }
        let expected: std::collections::BTreeMap<(String, String), usize> = expected
            .iter()
            .map(|(file, reason, n)| ((file.to_string(), reason.to_string()), *n))
            .collect();
        assert_eq!(found, expected);
    }

    /// The places allowed to bypass clippy's spawn ban are exactly these.
    #[test]
    fn clippy_spawn_ban_bypasses_are_exactly_these() {
        let needle = concat!("allow(clippy::", "disallowed_methods)");
        let found: std::collections::BTreeMap<String, usize> = rust_sources()
            .into_iter()
            .map(|(rel, text)| (rel, text.matches(needle).count()))
            .filter(|(_, n)| *n > 0)
            .collect();
        let expected: std::collections::BTreeMap<String, usize> = [
            // raw_async, raw_sync, open_in_system (open::commands, isolated)
            ("backend/src/core/cmd.rs", 3),
            // test builds only: fixtures
            ("backend/src/lib.rs", 1),
            ("backend/tests/api_tests.rs", 1),
        ]
        .into_iter()
        .map(|(file, n)| (file.to_string(), n))
        .collect();
        assert_eq!(found, expected);
    }

    /// No exception receives a secret; only Kronn relaunching itself keeps
    /// its provider keys, never the forbidden names (B4-04).
    #[test]
    fn full_env_commands_never_carry_a_secret() {
        use crate::core::child_env::{probe, FORBIDDEN};
        let removed = |command: &std::process::Command, name: &str| {
            command
                .get_envs()
                .any(|(key, value)| key == name && value.is_none())
        };
        let (sidecar, restart) = probe::with_secret_parent("/usr/bin", "/home/u", || {
            (
                full_env_cmd("x", FullEnvReason::DocsSidecar),
                full_env_sync_cmd("x", FullEnvReason::SelfRestart),
            )
        });
        for name in probe::SECRET_NAMES {
            assert!(
                removed(sidecar.as_std(), name),
                "{name} reaches an exception"
            );
        }
        for name in FORBIDDEN {
            assert!(removed(sidecar.as_std(), name), "{name}");
            assert!(removed(&restart, name), "{name} reaches the relaunch");
        }
        assert!(!removed(&restart, "ANTHROPIC_API_KEY"));
        assert_eq!(
            sidecar.as_std().get_current_dir(),
            Some(std::env::temp_dir().as_path())
        );
    }

    #[test]
    fn sync_cmd_creates_command() {
        let cmd = sync_tool_cmd("echo");
        drop(cmd);
    }

    #[tokio::test]
    async fn async_cmd_runs_successfully() {
        let output = tool_cmd("echo")
            .arg("hello")
            .output()
            .await
            .expect("echo should succeed");
        assert!(output.status.success());
    }

    #[test]
    fn sync_cmd_runs_successfully() {
        let output = sync_tool_cmd("echo")
            .arg("hello")
            .output()
            .expect("echo should succeed");
        assert!(output.status.success());
    }

    /// Verify that on Windows we skip resolution for paths that are already
    /// explicit (so we don't double-resolve `C:\Program Files\nodejs\npx.cmd`
    /// or break callers that pass a `PathBuf`). This is a unit test for the
    /// guard logic only — it runs on Windows (the function is `cfg`-gated
    /// out elsewhere) and exercises the early-return cases.
    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_windows_program_skips_explicit_paths() {
        use std::ffi::OsString;
        // Absolute path → skip
        let abs = OsString::from(r"C:\Program Files\nodejs\npx.cmd");
        assert!(
            resolve_windows_program(&abs).is_none(),
            "absolute path with backslash + extension must skip resolution"
        );
        // Relative path containing slash → skip
        let rel = OsString::from("./bin/foo");
        assert!(
            resolve_windows_program(&rel).is_none(),
            "path containing slash must skip resolution"
        );
        // Bare extension → skip (Windows can resolve .exe natively)
        let with_ext = OsString::from("npx.cmd");
        // We skip on extension presence — caller passes the explicit form.
        assert!(
            resolve_windows_program(&with_ext).is_none(),
            "name with extension must skip resolution"
        );
    }
}
