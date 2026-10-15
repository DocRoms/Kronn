//! KT-1255 — run Kronn's own builds of a worker lane under the worker's sandbox.
//!
//! The lane's first build (`cargo test --no-run`, KT-1250) and the toolchain
//! smoke check (KT-1246) are started by Kronn, not by the worker, so they do
//! not inherit the worker's Claude sandbox. They used to run as the host user:
//! harmless while a lane is provisioned at `HEAD == base`, but any later
//! relaunch that rebuilds after the worker changed `Cargo.lock` or a build
//! script would have run worker-written code (`build.rs`, a proc-macro) with
//! the host's rights.
//!
//! [`SandboxedProbeRunner`] closes that: it wraps each probe in the host's OS
//! sandbox, with the same filesystem policy as
//! `agents::runner::claude_task_worker_settings` — write only the lane's
//! [`WorkerLane::write_roots`] (plus the temp directories a toolchain needs),
//! read as before, and network only on loopback. It fails closed: a host with
//! no usable sandbox mechanism refuses the command and says why, it never
//! falls back to running it unconfined.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::task_worker_env::{
    ProbeCommand, ProbeOutcome, ProbeRunner, ProcessProbeRunner, WorkerLane,
};

/// macOS' Seatbelt front-end.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
/// Linux' unprivileged namespace sandbox.
const BWRAP: &str = "bwrap";

/// The OS facility that confines a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    Seatbelt,
    Bubblewrap,
}

impl Mechanism {
    /// The mechanism of this host, when it has one Kronn drives.
    pub fn of_host() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Seatbelt)
        } else if cfg!(target_os = "linux") {
            Some(Self::Bubblewrap)
        } else {
            None
        }
    }
}

/// What a confined command may write. Everything else on disk is read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub write_roots: Vec<PathBuf>,
}

impl SandboxPolicy {
    /// The worker's own write roots: its worktree and its lane's Cargo target.
    /// The host's `CARGO_HOME` and dependency caches stay read-only.
    pub fn for_lane(lane: &WorkerLane) -> Self {
        Self {
            write_roots: lane.write_roots(),
        }
    }

    /// The policy of a task worktree, derived the same way the worker's
    /// sandbox settings derive theirs.
    pub fn for_worktree(work_dir: &Path) -> Result<Self, String> {
        let worktree = work_dir.canonicalize().map_err(|error| {
            format!(
                "cannot canonicalize task worktree {}: {error}",
                work_dir.display()
            )
        })?;
        Ok(Self::for_lane(&WorkerLane::for_worktree(&worktree)?))
    }

    /// The roots plus the scratch space every toolchain needs (linker and
    /// compiler temporaries): the process temp dir and, on macOS, its sibling
    /// per-user cache. Canonical, because Seatbelt matches real paths.
    fn writable(&self, probe_env: &[(String, OsString)]) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.write_roots.clone();
        let mut temps: Vec<PathBuf> = vec![std::env::temp_dir()];
        temps.extend(
            probe_env
                .iter()
                .filter(|(name, _)| name == "TMPDIR")
                .map(|(_, value)| PathBuf::from(value)),
        );
        for fixed in ["/tmp", "/var/tmp"] {
            temps.push(PathBuf::from(fixed));
        }
        for temp in temps {
            let Ok(real) = temp.canonicalize() else {
                continue;
            };
            if real.file_name().is_some_and(|name| name == "T") {
                if let Some(cache) = real.parent().map(|parent| parent.join("C")) {
                    if cache.is_dir() {
                        roots.push(cache);
                    }
                }
            }
            roots.push(real);
        }
        roots.sort();
        roots.dedup();
        roots
    }
}

/// A Seatbelt string literal: `\` and `"` escaped.
fn seatbelt_literal(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The Seatbelt profile of `writable`: reads and process control as usual,
/// writes only under the given roots (and the few device nodes a process
/// needs), network only on loopback.
pub fn seatbelt_profile(writable: &[PathBuf]) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    profile.push_str("(allow file-write*\n");
    for root in writable {
        profile.push_str(&format!("  (subpath {})\n", seatbelt_literal(root)));
    }
    profile.push_str(
        "  (literal \"/dev/null\")\n  (literal \"/dev/zero\")\n  \
         (literal \"/dev/dtracehelper\")\n  (regex #\"^/dev/tty\")\n  (regex #\"^/dev/fd/\"))\n",
    );
    profile.push_str(
        "(deny network*)\n\
         (allow network-outbound (remote ip \"localhost:*\"))\n\
         (allow network-bind (local ip \"localhost:*\"))\n\
         (allow network-inbound (local ip \"localhost:*\"))\n",
    );
    profile
}

/// `bwrap` arguments of `writable`: the whole filesystem read-only, the roots
/// writable, a private network namespace (loopback only), the process dies
/// with its parent.
pub fn bubblewrap_args(writable: &[PathBuf]) -> Vec<String> {
    let mut args: Vec<String> = ["--ro-bind", "/", "/", "--dev-bind", "/dev", "/dev"]
        .into_iter()
        .map(String::from)
        .collect();
    for root in writable {
        let root = root.to_string_lossy().into_owned();
        args.extend(["--bind".to_string(), root.clone(), root]);
    }
    args.extend(
        ["--unshare-net", "--die-with-parent", "--new-session", "--"]
            .into_iter()
            .map(String::from),
    );
    args
}

/// Wrap `probe` so the OS sandbox runs it. Pure: no process is started.
pub fn confine(mechanism: Mechanism, policy: &SandboxPolicy, probe: &ProbeCommand) -> ProbeCommand {
    let writable = policy.writable(&probe.env);
    let (program, mut args) = match mechanism {
        Mechanism::Seatbelt => (
            SANDBOX_EXEC.to_string(),
            vec!["-p".to_string(), seatbelt_profile(&writable)],
        ),
        Mechanism::Bubblewrap => (BWRAP.to_string(), bubblewrap_args(&writable)),
    };
    args.push(probe.program.clone());
    args.extend(probe.args.iter().cloned());
    ProbeCommand {
        program,
        args,
        cwd: probe.cwd.clone(),
        env: probe.env.clone(),
        timeout: probe.timeout,
    }
}

/// Runs probes under the worker's sandbox, through `inner`.
pub struct SandboxedProbeRunner {
    /// `Err` when the policy could not be derived: every probe is refused.
    policy: Result<SandboxPolicy, String>,
    mechanism: Option<Mechanism>,
    inner: Arc<dyn ProbeRunner>,
}

impl SandboxedProbeRunner {
    /// The production runner of a task worktree: this host's mechanism, real
    /// child processes.
    pub fn for_worktree(work_dir: &Path) -> Self {
        Self::new(
            SandboxPolicy::for_worktree(work_dir),
            Mechanism::of_host(),
            Arc::new(ProcessProbeRunner),
        )
    }

    pub fn new(
        policy: Result<SandboxPolicy, String>,
        mechanism: Option<Mechanism>,
        inner: Arc<dyn ProbeRunner>,
    ) -> Self {
        Self {
            policy,
            mechanism,
            inner,
        }
    }

    fn refuse(reason: String) -> ProbeOutcome {
        ProbeOutcome {
            success: false,
            detail: format!("not run: {reason}"),
        }
    }
}

impl ProbeRunner for SandboxedProbeRunner {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.run_cancellable(probe, &CancellationToken::new())
    }

    fn run_cancellable(&self, probe: &ProbeCommand, cancel: &CancellationToken) -> ProbeOutcome {
        let policy = match &self.policy {
            Ok(policy) => policy,
            Err(error) => {
                return Self::refuse(format!(
                    "the worker sandbox policy could not be derived: {error}"
                ));
            }
        };
        let Some(mechanism) = self.mechanism else {
            return Self::refuse(
                "this host has no sandbox Kronn can run a lane build under (macOS Seatbelt or \
                 Linux bubblewrap); a worker build is never run unconfined"
                    .to_string(),
            );
        };
        self.inner
            .run_cancellable(&confine(mechanism, policy, probe), cancel)
    }

    fn confined(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "task_worker_sandbox_test.rs"]
mod task_worker_sandbox_test;
