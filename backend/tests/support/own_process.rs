//! A test that owns process-global state (`PATH`, `TMPDIR`, `KRONN_*`, a fake
//! CLI directory) gets its own process under `cargo test`, as nextest already
//! gives it (KT-1186).
//!
//! `tests/it.rs` links every test file into one binary. `cargo test` runs them
//! as threads of one process, so a test that sets `PATH` to its fake CLI
//! directory is observed by every other test that spawns a process or reads
//! `KRONN_DATA_DIR`, and a wall-clock wait in one is stretched by the builds
//! of the others. The guard re-runs the test alone in a child of this binary
//! and asserts on the child's exit: the body then runs with an environment
//! nobody else writes, exactly as under nextest.
// The child is this very test binary: the one place a test starts a process.
#![allow(clippy::disallowed_methods)]

use std::process::{Command, Stdio};

/// Set in the child only: this process is the test's own.
const CHILD_ENV: &str = "KRONN_TEST_OWN_PROCESS";

/// Call first in a test: `if own_process::delegated(module_path!(), "name") { return; }`.
///
/// Returns `true` when the test was run in a child process and passed (the
/// caller returns); `false` when this process is already the test's own (the
/// child, or a nextest worker) and the caller runs its body. Panics when the
/// child fails or did not run exactly the one test, so a renamed test cannot
/// pass without running.
pub fn delegated(module: &str, name: &str) -> bool {
    // nextest sets `NEXTEST` for every test it runs, one process each.
    if std::env::var_os(CHILD_ENV).is_some() || std::env::var_os("NEXTEST").is_some() {
        return false;
    }
    // libtest names a test without the crate: `it::a::b` is listed `a::b`.
    let path = match module.split_once("::") {
        Some((_, module)) => format!("{module}::{name}"),
        None => name.to_string(),
    };
    let child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([path.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .stdin(Stdio::null())
        .output()
        .expect("start the test's own process");
    let stdout = String::from_utf8_lossy(&child.stdout);
    let stderr = String::from_utf8_lossy(&child.stderr);
    assert!(
        child.status.success(),
        "{path} failed in its own process ({}):\n{stdout}\n{stderr}",
        child.status
    );
    assert!(
        stdout.contains("test result: ok. 1 passed;"),
        "{path} did not run exactly once in its own process:\n{stdout}\n{stderr}"
    );
    true
}
