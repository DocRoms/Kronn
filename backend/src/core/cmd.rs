//! Cross-platform command helpers.
//!
//! On Windows, every `Command::new()` spawns a visible console window by default.
//! These helpers apply the `CREATE_NO_WINDOW` flag so background processes (git, wsl.exe, etc.)
//! run invisibly — critical for the Tauri desktop app experience.

use std::ffi::OsStr;

/// Windows: CREATE_NO_WINDOW flag prevents a console window from appearing.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Create a `tokio::process::Command` that won't flash a console window on Windows.
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, `PathBuf`, …)
/// so callers don't have to round-trip through `.to_str()` to invoke a binary by path.
pub fn async_cmd<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Create a `std::process::Command` that won't flash a console window on Windows.
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, `PathBuf`, …).
pub fn sync_cmd<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
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
}
