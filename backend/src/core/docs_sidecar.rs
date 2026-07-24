//! Kronn-docs sidecar manager.
//!
//! A small Python FastAPI server lives at `backend/sidecars/docs/`.
//! Desktop releases bundle it as a standalone executable; Docker bakes
//! its virtualenv into the image; native development can install that
//! virtualenv with `make docs-setup`. This module owns its lifecycle:
//!
//! * Discovers a free loopback port at startup and spawns the sidecar
//!   with `KRONN_DOCS_PORT=<port>` in its env.
//! * Waits up to 15 s for the sidecar to print `KRONN_DOCS_READY <port>`
//!   on stdout — the sidecar prints this once uvicorn has bound.
//! * Exposes `SidecarHandle` with the base URL so API handlers can
//!   reqwest directly against `127.0.0.1:<port>`.
//! * Kills the child on drop so we don't leak Python processes across
//!   backend restarts.
//!
//! Graceful degradation: if neither a bundled executable nor the
//! development virtualenv exists, we log a one-line hint and leave the
//! handle empty; `/api/docs/*` routes then return a 503. No crash and
//! no retry loop.

use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::core::cmd::async_cmd;

const BUNDLED_SIDECAR_ENV: &str = "KRONN_DOCS_SIDECAR";

#[derive(Debug, Clone, PartialEq, Eq)]
enum SidecarProgram {
    Bundled(PathBuf),
    PythonModule(PathBuf),
}

/// Read-only snapshot of a live sidecar. Cloned into each request
/// handler through `AppState.docs_sidecar`.
#[derive(Debug, Clone)]
pub struct SidecarHandle {
    /// `http://127.0.0.1:<port>` — pre-built so request handlers don't
    /// have to glue the string back together.
    pub base_url: String,
}

/// Runtime state for the sidecar. Optional because setup might not
/// have been run yet — in that case API routes report a clear error.
#[derive(Debug, Default)]
pub struct DocsSidecar {
    handle: Arc<Mutex<Option<SidecarHandle>>>,
    /// Kept alive so the OS doesn't reap the child while we're up.
    /// Dropping this Arc kills the child (via `Child`'s Drop impl,
    /// which we set to `kill_on_drop(true)` at spawn).
    #[allow(dead_code)]
    child: Arc<Mutex<Option<Child>>>,
}

impl DocsSidecar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only accessor used by API handlers to resolve the base URL.
    pub async fn handle(&self) -> Option<SidecarHandle> {
        self.handle.lock().await.clone()
    }

    /// Spawn the sidecar asynchronously. Intended to be called once
    /// from `backend::main` / `desktop::main` at startup, fire-and-
    /// forget — failure is logged, not propagated, so one broken
    /// sidecar never blocks the whole backend from coming up.
    ///
    /// Idempotent: if the handle is already set, this is a no-op.
    pub async fn start(&self) {
        if self.handle.lock().await.is_some() {
            return;
        }

        let program = resolve_sidecar_program(
            std::env::var_os(BUNDLED_SIDECAR_ENV),
            &source_bundle_path(),
            &venv_python_path(),
        );
        let Some(program) = program else {
            tracing::info!(
                "kronn-docs sidecar unavailable — desktop releases should bundle it; native developers can run `make docs-setup`. See backend/sidecars/docs/README.md"
            );
            return;
        };
        tracing::info!("starting resolved kronn-docs sidecar: {program:?}");

        // Ask the OS for a free TCP port. Bind + drop — the port is
        // then ours to hand to the child which will re-bind immediately.
        // Small race (another process could grab it between our drop
        // and the child's bind) but tiny in practice, and we log + skip
        // if the child fails to come up.
        let port = match pick_free_port() {
            Some(p) => p,
            None => {
                tracing::warn!("kronn-docs sidecar: could not find a free loopback port");
                return;
            }
        };

        // Desktop bundles execute directly. Docker/native development
        // use `python -m kronn_docs.server` from the dedicated venv.
        // stdout goes through a pipe so we can read the READY marker;
        // stderr is inherited so real errors land in the main backend
        // log without us having to re-forward them.
        let mut cmd = match &program {
            SidecarProgram::Bundled(path) => async_cmd(path),
            SidecarProgram::PythonModule(path) => {
                let mut cmd = async_cmd(path);
                cmd.arg("-m").arg("kronn_docs.server");
                cmd
            }
        };
        cmd.env("KRONN_DOCS_PORT", port.to_string())
            .env("PYTHONUNBUFFERED", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        // `cargo run` injects Rust build directories here on macOS. Passing
        // them to the frozen Python child masks its bundled Pango libraries.
        #[cfg(target_os = "macos")]
        cmd.env_remove("DYLD_FALLBACK_LIBRARY_PATH");

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("kronn-docs sidecar spawn failed: {e}");
                return;
            }
        };

        // Frozen Python has a slower first launch, especially while Windows
        // Defender scans a fresh install. This runs in a background task, so
        // allowing 15 seconds does not delay the Kronn backend itself.
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                tracing::warn!("kronn-docs sidecar: stdout pipe missing");
                return;
            }
        };

        let ready_check = timeout(Duration::from_secs(15), wait_for_ready(stdout, port)).await;
        match ready_check {
            Ok(Ok(())) => {
                let base_url = format!("http://127.0.0.1:{port}");
                tracing::info!("kronn-docs sidecar ready at {base_url}");
                *self.handle.lock().await = Some(SidecarHandle { base_url });
                *self.child.lock().await = Some(child);
            }
            Ok(Err(e)) => {
                tracing::warn!("kronn-docs sidecar failed to signal ready: {e}");
                let _ = child.kill().await;
            }
            Err(_) => {
                tracing::warn!(
                    "kronn-docs sidecar did not print READY marker within 15 s — aborting"
                );
                let _ = child.kill().await;
            }
        }
    }
}

/// Resolve a free loopback port by asking the kernel, then dropping
/// the listener. The race with a third party grabbing the port between
/// drop and child bind is irrelevant in practice (and loud if it
/// happens — the sidecar prints its own bind error to stderr).
fn pick_free_port() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// Pick the release-bundled sidecar first, then a development venv, then the
/// repo-local desktop bundle. The final fallback makes `kronn start-dev` use
/// the same exporter that a preceding desktop build already produced.
fn resolve_sidecar_program(
    bundled_override: Option<OsString>,
    source_bundle: &Path,
    venv_python: &Path,
) -> Option<SidecarProgram> {
    if let Some(path) = bundled_override.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(SidecarProgram::Bundled(path));
        }
        tracing::warn!(
            "{} points to a missing sidecar executable: {}",
            BUNDLED_SIDECAR_ENV,
            path.display()
        );
    }

    if venv_python.is_file() {
        return Some(SidecarProgram::PythonModule(venv_python.to_path_buf()));
    }

    source_bundle
        .is_file()
        .then(|| SidecarProgram::Bundled(source_bundle.to_path_buf()))
}

/// PyInstaller's one-directory output in a source checkout. Tauri relocates
/// this tree and sets `KRONN_DOCS_SIDECAR` in installed apps; this absolute
/// build-time path is only useful while running the backend from the checkout.
fn source_bundle_path() -> PathBuf {
    let executable = if cfg!(windows) {
        "kronn-docs.exe"
    } else {
        "kronn-docs"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../desktop/src-tauri/resources/docs-sidecar/kronn-docs")
        .join(executable)
}

/// Path to the venv Python interpreter created by `make docs-setup`.
/// `~/.kronn/venv/docs/bin/python` on Unix, `Scripts\python.exe` on
/// Windows.
fn venv_python_path() -> PathBuf {
    let home = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let base = home.join(".kronn").join("venv").join("docs");
    if cfg!(windows) {
        base.join("Scripts").join("python.exe")
    } else {
        base.join("bin").join("python")
    }
}

/// Poll the child's stdout line-by-line until we see `KRONN_DOCS_READY`.
/// Returns Ok on first match, Err if stdout closes before the marker.
async fn wait_for_ready(
    stdout: tokio::process::ChildStdout,
    expected_port: u16,
) -> Result<(), String> {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    let marker_prefix = format!("KRONN_DOCS_READY {}", expected_port);
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().starts_with(&marker_prefix) {
                    return Ok(());
                }
                // Log the sidecar's stdout as debug so operators diagnosing
                // a stuck boot see what the sidecar printed before the
                // timeout hit.
                tracing::debug!("kronn-docs stdout: {line}");
            }
            Ok(None) => {
                return Err("sidecar stdout closed before READY".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kronn-docs-sidecar-{label}-{}", Uuid::new_v4()))
    }

    #[test]
    fn venv_python_path_is_platform_specific() {
        let p = venv_python_path();
        let s = p.to_string_lossy();
        if cfg!(windows) {
            assert!(
                s.ends_with("python.exe"),
                "expected Scripts\\python.exe, got {s}"
            );
        } else {
            assert!(s.ends_with("bin/python"), "expected bin/python, got {s}");
        }
        assert!(
            s.contains(".kronn"),
            "path should be under ~/.kronn, got {s}"
        );
    }

    #[test]
    fn pick_free_port_returns_nonzero() {
        let p = pick_free_port().expect("kernel should give us some free port");
        assert!(p > 0);
    }

    #[test]
    fn bundled_program_wins_over_development_venv() {
        let root = test_dir("precedence");
        let bundled = root.join(if cfg!(windows) {
            "kronn-docs.exe"
        } else {
            "kronn-docs"
        });
        let venv = root.join("venv-python");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&bundled, b"bundle").unwrap();
        std::fs::write(&venv, b"venv").unwrap();

        let source = root.join("source-bundle");
        let resolved =
            resolve_sidecar_program(Some(bundled.clone().into_os_string()), &source, &venv);
        assert_eq!(resolved, Some(SidecarProgram::Bundled(bundled)));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_bundle_falls_back_to_development_venv() {
        let root = test_dir("fallback");
        let venv = root.join("venv-python");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&venv, b"venv").unwrap();

        let source = root.join("source-bundle");
        let resolved =
            resolve_sidecar_program(Some(root.join("missing").into_os_string()), &source, &venv);
        assert_eq!(resolved, Some(SidecarProgram::PythonModule(venv)));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_installed_program_returns_none() {
        let root = test_dir("missing");
        let resolved =
            resolve_sidecar_program(None, &root.join("source-bundle"), &root.join("venv-python"));
        assert_eq!(resolved, None);
    }

    #[test]
    fn repo_bundle_is_used_when_no_override_or_venv_exists() {
        let root = test_dir("source-bundle");
        let source = root.join("kronn-docs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"bundle").unwrap();

        let resolved = resolve_sidecar_program(None, &source, &root.join("missing-venv-python"));
        assert_eq!(resolved, Some(SidecarProgram::Bundled(source)));

        std::fs::remove_dir_all(root).unwrap();
    }
}
