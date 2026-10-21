//! KT-1189 — no integration test starts an agent CLI installed on the machine.
//!
//! These files are compiled against the library without `cfg(test)`, so the
//! guard the library's own tests get from `cfg(test)` reaches them through
//! `KRONN_TEST_NO_REAL_AGENTS`, set for the whole binary by `tests/it.rs`.

use std::path::{Path, PathBuf};

use kronn::core::child_env::{AgentFamily, ChildRoute};

/// Directories a test controls: a stub written there is the test's own.
fn controlled_roots() -> Vec<PathBuf> {
    let real = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    vec![
        real(std::env::temp_dir()),
        real(PathBuf::from("/tmp")),
        real(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ]
}

fn is_controlled(path: &Path) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    controlled_roots().iter().any(|root| path.starts_with(root))
}

/// The agent CLIs of the machine are invisible to detection: what the version
/// probe would start is a stub the test wrote, never `~/.local/bin/claude`.
#[test]
fn detection_never_resolves_an_agent_cli_of_the_machine() {
    for name in [
        "claude", "codex", "gemini", "copilot", "vibe", "opencode", "kiro-cli", "ollama",
    ] {
        if let Some(found) = kronn::agents::find_binary(name) {
            assert!(
                is_controlled(Path::new(&found.path)),
                "{name} resolved to {}, a CLI of the machine",
                found.path
            );
        }
    }
}

/// The routes that sweep the machine for agents (`/api/agents`, the
/// version-check, the readiness probe) all go through `detect_all`: every
/// agent it reports as installed is under a directory the test controls.
#[tokio::test]
async fn the_detection_sweep_reports_no_agent_cli_of_the_machine() {
    for force in [false, true] {
        for agent in kronn::agents::detect_all_cached(force).await {
            if let Some(path) = agent.path.as_deref().filter(|_| agent.installed) {
                assert!(
                    is_controlled(Path::new(path)),
                    "{} detected at {path}, a CLI of the machine",
                    agent.name
                );
            }
        }
    }
}

/// The guard is armed here, in a build without `cfg(test)`: starting a program
/// that is not under a test-controlled directory on an agent route panics, with
/// every builder the launches go through.
#[cfg(unix)]
#[test]
fn a_real_agent_binary_cannot_be_started_from_an_integration_test() {
    let route = ChildRoute::Agent(AgentFamily::Copilot);
    let refused = [
        std::panic::catch_unwind(|| {
            let _ = kronn::core::cmd::async_cmd("/bin/ls", route);
        }),
        std::panic::catch_unwind(|| {
            let _ = kronn::core::cmd::sync_cmd("/bin/ls", route);
        }),
        std::panic::catch_unwind(|| {
            let _ = kronn::core::cmd::discovery_cmd("/bin/ls", AgentFamily::Vibe);
        }),
    ];
    assert!(refused.iter().all(Result::is_err), "{refused:?}");
}

/// A stub under the temporary directory — what a test puts on an agent route
/// instead of the real CLI — still starts.
#[cfg(unix)]
#[tokio::test]
async fn a_stub_agent_still_starts_from_an_integration_test() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("claude");
    std::fs::write(&stub, "#!/bin/sh\necho stub 1.0\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = kronn::core::cmd::discovery_cmd(&stub, AgentFamily::Claude)
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "stub 1.0");
}

/// A panic inside a detached `tokio::spawn` (the `detect_all_cached` refresh)
/// is swallowed: nobody joins the task, so the test would pass with a real CLI
/// started. There the guard aborts the process, so the run fails loudly.
///
/// The detached launch runs in a child process — the abort would otherwise
/// take the whole test binary with it — and the parent asserts it died with the
/// guard's message instead of finishing.
#[cfg(unix)]
#[test]
fn a_real_agent_started_in_a_detached_task_fails_the_run() {
    const CHILD: &str = "KT1189_DETACHED_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let _detached = tokio::spawn(async {
                let _ =
                    kronn::core::cmd::async_cmd("/bin/ls", ChildRoute::Agent(AgentFamily::Vibe));
            });
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "no_real_agent_cli::a_real_agent_started_in_a_detached_task_fails_the_run",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the detached launch of a real agent did not fail the run: {stderr}"
    );
    assert!(
        stderr.contains("a test started the real agent binary"),
        "the run died, but not through the guard: {stderr}"
    );
}
