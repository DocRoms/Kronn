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

/// Whether `program` is git, which runs repository-controlled code (hooks,
/// filters, fsmonitor, diff drivers) inside its own process.
fn is_git(program: &OsStr) -> bool {
    std::path::Path::new(program)
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|stem| stem.eq_ignore_ascii_case("git"))
}

/// Every git process gets the built git environment instead of the backend's
/// (KT-1006): a repository hook never sees Kronn's key or a provider key.
fn isolate_git(program: &OsStr, command: &mut std::process::Command) {
    if is_git(program) {
        crate::core::child_env::isolate(command, crate::core::child_env::ChildRoute::Git);
    }
}

/// Create a `tokio::process::Command` that won't flash a console window on Windows.
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, `PathBuf`, …)
/// so callers don't have to round-trip through `.to_str()` to invoke a binary by path.
pub fn async_cmd<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    let resolved = resolve_windows_program(program.as_ref());
    #[cfg(target_os = "windows")]
    let mut cmd = match resolved {
        Some(path) => tokio::process::Command::new(path),
        None => tokio::process::Command::new(program.as_ref()),
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = tokio::process::Command::new(&program);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    isolate_git(program.as_ref(), cmd.as_std_mut());
    cmd
}

/// Create a `std::process::Command` that won't flash a console window on Windows.
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, `PathBuf`, …).
pub fn sync_cmd<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    #[cfg(target_os = "windows")]
    let resolved = resolve_windows_program(program.as_ref());
    #[cfg(target_os = "windows")]
    let mut cmd = match resolved {
        Some(path) => std::process::Command::new(path),
        None => std::process::Command::new(program.as_ref()),
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = std::process::Command::new(&program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    isolate_git(program.as_ref(), &mut cmd);
    cmd
}

/// [`async_cmd`] for a program Kronn runs for itself (installer, system
/// probe): the base allow-list only, never the backend's environment.
pub fn tool_cmd<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    let mut cmd = async_cmd(program);
    crate::core::child_env::isolate(cmd.as_std_mut(), crate::core::child_env::ChildRoute::Tool);
    cmd
}

/// [`sync_cmd`] counterpart of [`tool_cmd`].
pub fn sync_tool_cmd<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    let mut cmd = sync_cmd(program);
    crate::core::child_env::isolate(&mut cmd, crate::core::child_env::ChildRoute::Tool);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_cmd_creates_command() {
        let cmd = async_cmd("echo");
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
                || sync_cmd("git").args(args).current_dir(repo.path()).output(),
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

    #[test]
    fn only_git_gets_the_git_policy() {
        assert!(is_git(OsStr::new("git")));
        assert!(is_git(OsStr::new("/usr/bin/git")));
        assert!(!is_git(OsStr::new("gitk")));
        assert!(!is_git(OsStr::new("echo")));
    }

    #[test]
    fn sync_cmd_creates_command() {
        let cmd = sync_cmd("echo");
        drop(cmd);
    }

    #[tokio::test]
    async fn async_cmd_runs_successfully() {
        let output = async_cmd("echo")
            .arg("hello")
            .output()
            .await
            .expect("echo should succeed");
        assert!(output.status.success());
    }

    #[test]
    fn sync_cmd_runs_successfully() {
        let output = sync_cmd("echo")
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
