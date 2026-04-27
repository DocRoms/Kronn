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
        None => tokio::process::Command::new(program),
    };
    #[cfg(not(target_os = "windows"))]
    let cmd = tokio::process::Command::new(program);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_cmd_creates_command() {
        let cmd = async_cmd("echo");
        // Just verify it doesn't panic — creation_flags is Windows-only
        drop(cmd);
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
        assert!(resolve_windows_program(&abs).is_none(),
            "absolute path with backslash + extension must skip resolution");
        // Relative path containing slash → skip
        let rel = OsString::from("./bin/foo");
        assert!(resolve_windows_program(&rel).is_none(),
            "path containing slash must skip resolution");
        // Bare extension → skip (Windows can resolve .exe natively)
        let with_ext = OsString::from("npx.cmd");
        // We skip on extension presence — caller passes the explicit form.
        assert!(resolve_windows_program(&with_ext).is_none(),
            "name with extension must skip resolution");
    }
}
