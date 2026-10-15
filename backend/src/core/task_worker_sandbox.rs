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
//! [`WorkerLane::write_roots`] and the lane's own temp dir (no global
//! `/tmp`), read as before — and is stricter on the channels that settings
//! file cannot express: no outbound connection at all, no Mach lookup beyond
//! a short list, fresh PID/IPC namespaces on Linux. It fails closed: a host
//! with no usable sandbox mechanism refuses the command and says why, it
//! never falls back to running it unconfined.
//!
//! KT-1268: the principal's persisted validations (`cargo test`, `vitest`, the
//! `cargo metadata` that sizes their target) are Quick Exec commands run in
//! the worker's tree at integration — the same worker-written `build.rs`,
//! proc-macros and tests. [`Confinement`] is this profile and this launcher
//! (own session, no terminal) for a Quick Exec command: there is no second
//! profile.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::child_env::ChildRoute;
use crate::core::task_worker_env::{
    prepare_launch, ProbeCommand, ProbeOutcome, ProbeRunner, ProcessProbeRunner, WorkerLane,
};

/// macOS' Seatbelt front-end.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
/// Linux' unprivileged namespace sandbox.
const BWRAP: &str = "bwrap";
const TMPDIR_ENV: &str = "TMPDIR";

/// Mach services a confined build may look up, and why. Anything else — in
/// particular `launchd` (`launchctl submit`), Launch Services (`open`), XPC
/// and `cfprefsd` — is refused: a lookup is how a process asks the host to
/// start something outside the sandbox. A toolchain that needs one more fails
/// closed (the build is recorded failed, the worker launches cold).
pub const SEATBELT_MACH_SERVICES: &[&str] = &[
    // Logging of every libSystem process.
    "com.apple.logd",
    // libnotify, initialised by libSystem.
    "com.apple.system.notification_center",
    // getpwuid / getgrgid, used for the home directory and the user name.
    "com.apple.system.opendirectoryd.libinfo",
    // confstr(_CS_DARWIN_USER_TEMP_DIR): the per-user temp path of a tool
    // that ignores `TMPDIR`.
    "com.apple.bsd.dirhelper",
];

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

    /// The program that applies this mechanism.
    fn launcher(self) -> &'static str {
        match self {
            Self::Seatbelt => SANDBOX_EXEC,
            Self::Bubblewrap => BWRAP,
        }
    }

    /// `Ok` when this host can apply the mechanism to a command, otherwise why
    /// not (KT-1268). [`Mechanism::of_host`] only names the mechanism the OS
    /// family has: a Linux host without `bwrap`, or a container that forbids
    /// user namespaces, has none that works, and a validation must then be
    /// refused for that reason, not fail on a spawn error.
    pub fn check_usable(self) -> Result<(), String> {
        self.check_launcher(self.launcher())
    }

    /// [`Mechanism::check_usable`] with the launcher program injected: runs
    /// `true` under the real profile with nothing writable.
    fn check_launcher(self, program: &str) -> Result<(), String> {
        let (mut args, true_program) = match self {
            Self::Seatbelt => (
                vec!["-p".to_string(), seatbelt_profile(&[])],
                "/usr/bin/true",
            ),
            Self::Bubblewrap => (bubblewrap_args(&[]), "true"),
        };
        args.push(true_program.to_string());
        let route = ChildRoute::QuickExec;
        let mut command = crate::core::cmd::sync_cmd(program, route);
        command
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        crate::core::child_env::seal(&mut command, route, &[]);
        let unusable = |why: String| {
            format!(
                "the sandbox launcher `{program}` is unusable on this host ({why}); \
                 worker-written code is never run unconfined"
            )
        };
        match command.output() {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr.lines().next().unwrap_or("").trim();
                let status = output.status;
                Err(unusable(if detail.is_empty() {
                    format!("it exited with {status}")
                } else {
                    format!("it exited with {status}: {detail}")
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(unusable("it is not installed".to_string()))
            }
            Err(error) => Err(unusable(format!("it cannot be started: {error}"))),
        }
    }
}

/// What a confined command may write. Everything else on disk is read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub write_roots: Vec<PathBuf>,
    /// The lane's own scratch directory, inside a write root. The confined
    /// command gets it as `TMPDIR`; no global temp directory is writable.
    pub temp_dir: PathBuf,
}

impl SandboxPolicy {
    /// The worker's own write roots: its worktree and its lane's Cargo target.
    /// The host's `CARGO_HOME` and dependency caches stay read-only.
    pub fn for_lane(lane: &WorkerLane) -> Self {
        Self {
            write_roots: lane.write_roots(),
            temp_dir: lane.tmp_dir(),
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

    /// The directories the command may write: the lane's roots and its own
    /// temp dir, nothing else (no `/tmp`, `/var/tmp` or per-user cache).
    /// Resolved through symlinks, because Seatbelt matches real paths.
    fn writable(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .write_roots
            .iter()
            .chain(std::iter::once(&self.temp_dir))
            .map(|root| real_path(root))
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
}

/// `path` with its longest existing ancestor canonicalized: a Cargo target that
/// does not exist yet still resolves through `/tmp` → `/private/tmp`.
fn real_path(path: &Path) -> PathBuf {
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = current.canonicalize() {
            return missing.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// A Seatbelt string literal: `\` and `"` escaped.
fn seatbelt_literal(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The Seatbelt profile of `writable`: **deny by default**, then only what a
/// toolchain needs.
///
/// - files: read anywhere (as the worker does), write only under `writable`
///   and the few device nodes a process needs;
/// - network: no outbound connection of any kind (loopback and Unix sockets
///   included — the Kronn API, Ollama and a local database are all reachable
///   that way); a loopback socket may be bound and accepted on;
/// - IPC: Mach lookups only for [`SEATBELT_MACH_SERVICES`], no POSIX shared
///   memory or semaphores, no IOKit;
/// - processes: may exec and fork, may signal only its own sandbox;
/// - terminals: no read, write or ioctl on any tty (`/dev/tty`, `ttysNNN`,
///   `ptyXX`, `/dev/console`, `/dev/ptmx`) and no `pseudo-tty`: with the
///   probe in its own session ([`crate::core::task_worker_env::detach_from_terminal`])
///   there is no controlling terminal to inject keystrokes into, and this
///   closes the path by name for a terminal it could still reach. `/dev/null`
///   and `/dev/fd/N` (its own piped stdio, `/dev/stdout`) are what a build
///   writes to; `file-ioctl` stays open for them and for plain files.
pub fn seatbelt_profile(writable: &[PathBuf]) -> String {
    let mut profile = String::from(
        "(version 1)\n\
         (deny default)\n\
         (allow process-exec)\n\
         (allow process-fork)\n\
         (allow process-info* (target same-sandbox))\n\
         (allow signal (target same-sandbox))\n\
         (allow sysctl-read)\n\
         (allow file-read*)\n\
         (allow file-map-executable)\n\
         (allow file-ioctl)\n",
    );
    profile.push_str("(allow mach-lookup\n");
    for service in SEATBELT_MACH_SERVICES {
        profile.push_str(&format!("  (global-name \"{service}\")\n"));
    }
    profile.push_str(")\n(allow file-write*\n");
    for root in writable {
        profile.push_str(&format!("  (subpath {})\n", seatbelt_literal(root)));
    }
    profile.push_str(
        "  (literal \"/dev/null\")\n  (literal \"/dev/zero\")\n  \
         (literal \"/dev/dtracehelper\")\n  (regex #\"^/dev/fd/\"))\n",
    );
    profile.push_str(
        "(allow network-bind (local ip \"localhost:*\"))\n\
         (allow network-inbound (local ip \"localhost:*\"))\n",
    );
    // Last rule wins, so this must stay the final rule: the terminals are out
    // of reach whatever the allowances above say. A pty the build would start
    // for itself is not reachable either — `pseudo-tty` is not granted, and a
    // cargo compile has no use for one.
    profile.push_str(
        "(deny file-read-data file-write* file-ioctl\n  (vnode-type TTY)\n  \
         (literal \"/dev/tty\")\n  (literal \"/dev/console\")\n  (literal \"/dev/ptmx\")\n  \
         (regex #\"^/dev/tty\")\n  (regex #\"^/dev/pty\"))\n",
    );
    profile
}

/// `bwrap` arguments of `writable`: the filesystem read-only with only the
/// roots writable, and fresh network, PID and IPC namespaces (no connection
/// out, no host process visible or signalable, no shared SysV IPC). `/dev` is
/// a private minimal one (its `/dev/shm` is not the host's), and `/run` and
/// `/tmp` are emptied so the host's service sockets (the systemd user bus, the
/// Docker socket, an SSH agent) cannot be reached by path.
pub fn bubblewrap_args(writable: &[PathBuf]) -> Vec<String> {
    let mut args: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/run",
        "--tmpfs",
        "/tmp",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    // After the tmpfs mounts: a root under `/tmp` is bound back on top.
    for root in writable {
        let root = root.to_string_lossy().into_owned();
        args.extend(["--bind".to_string(), root.clone(), root]);
    }
    args.extend(
        [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--die-with-parent",
            "--new-session",
            "--",
        ]
        .into_iter()
        .map(String::from),
    );
    args
}

/// Wrap `probe` so the OS sandbox runs it, with the lane's temp dir as its
/// `TMPDIR`. Pure: no process is started.
pub fn confine(mechanism: Mechanism, policy: &SandboxPolicy, probe: &ProbeCommand) -> ProbeCommand {
    let writable = policy.writable();
    let (program, mut args) = match mechanism {
        Mechanism::Seatbelt => (
            mechanism.launcher().to_string(),
            vec!["-p".to_string(), seatbelt_profile(&writable)],
        ),
        Mechanism::Bubblewrap => (mechanism.launcher().to_string(), bubblewrap_args(&writable)),
    };
    args.push(probe.program.clone());
    args.extend(probe.args.iter().cloned());
    let mut env: Vec<(String, OsString)> = probe
        .env
        .iter()
        .filter(|(name, _)| name != TMPDIR_ENV)
        .cloned()
        .collect();
    env.push((
        TMPDIR_ENV.to_string(),
        real_path(&policy.temp_dir).into_os_string(),
    ));
    ProbeCommand {
        program,
        args,
        cwd: probe.cwd.clone(),
        env,
        timeout: probe.timeout,
    }
}

/// Why nothing can be confined on this host. `subject` is what would have run.
fn no_mechanism_reason(subject: &str) -> String {
    format!(
        "this host has no sandbox Kronn can run {subject} under (macOS Seatbelt or Linux \
         bubblewrap); worker-written code is never run unconfined"
    )
}

/// The command's only scratch space must exist before it starts: the sandbox
/// grants no other.
fn create_temp_dir(policy: &SandboxPolicy) -> Result<(), String> {
    let temp_dir = real_path(&policy.temp_dir);
    std::fs::create_dir_all(&temp_dir).map_err(|error| {
        format!(
            "cannot create the lane's temp dir {}: {error}",
            temp_dir.display()
        )
    })
}

/// The lane's sandbox, ready to launch a Quick Exec command in a worker's tree
/// (KT-1268): the profile of [`SandboxPolicy::for_lane`] through the host's
/// [`Mechanism`], and the lane's own environment. It exists only when the host
/// has a mechanism, so holding one is the proof that a command it launches is
/// confined; without one, [`Confinement::for_worktree`] says why instead.
#[derive(Debug, Clone)]
pub struct Confinement {
    mechanism: Mechanism,
    policy: SandboxPolicy,
    lane_env: Vec<(String, OsString)>,
}

impl Confinement {
    /// The confinement of a task worktree on this host: its mechanism, once
    /// the host proved it can apply it ([`Mechanism::check_usable`]).
    pub fn for_worktree(work_dir: &Path) -> Result<Self, String> {
        Self::with_usable_mechanism(work_dir, Mechanism::of_host(), Mechanism::check_usable)
    }

    /// [`Confinement::for_worktree`] with the host's mechanism and the check of
    /// its launcher injected. A mechanism that is not usable refuses before
    /// the lane is provisioned: nothing is prepared for a host that cannot
    /// confine.
    fn with_usable_mechanism(
        work_dir: &Path,
        mechanism: Option<Mechanism>,
        check_usable: impl FnOnce(Mechanism) -> Result<(), String>,
    ) -> Result<Self, String> {
        if let Some(mechanism) = mechanism {
            check_usable(mechanism)?;
        }
        Self::with_mechanism(work_dir, mechanism)
    }

    /// [`Confinement::for_worktree`] with the host's mechanism injected, so the
    /// refusal of a host without one is testable on any host. Nothing is
    /// provisioned for a host that has none.
    pub fn with_mechanism(work_dir: &Path, mechanism: Option<Mechanism>) -> Result<Self, String> {
        let mechanism =
            mechanism.ok_or_else(|| no_mechanism_reason("a validation of a worker's tree"))?;
        let (lane, provisioned) = prepare_launch(work_dir)?;
        for warning in &provisioned.warnings {
            tracing::warn!(lane = %lane.name, "task worker lane: {warning}");
        }
        let policy = SandboxPolicy::for_lane(&lane);
        create_temp_dir(&policy)?;
        let lane_env = lane
            .env(&provisioned)
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect();
        Ok(Self {
            mechanism,
            policy,
            lane_env,
        })
    }

    pub fn mechanism(&self) -> Mechanism {
        self.mechanism
    }

    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// `probe` under the OS sandbox, with the lane's environment beneath its
    /// own: a value the probe sets (a `--target-dir` override) wins. Pure.
    pub fn wrap(&self, probe: &ProbeCommand) -> ProbeCommand {
        let mut env: Vec<(String, OsString)> = self
            .lane_env
            .iter()
            .filter(|(name, _)| !probe.env.iter().any(|(set, _)| set == name))
            .cloned()
            .collect();
        env.extend(probe.env.iter().cloned());
        confine(
            self.mechanism,
            &self.policy,
            &ProbeCommand {
                env,
                ..probe.clone()
            },
        )
    }

    /// The process that runs `probe` confined: the sealed environment of
    /// `route`, in a session of its own with no controlling terminal
    /// ([`crate::core::task_worker_env::detach_from_terminal`], the launcher of
    /// the lane's first build). Not started.
    pub fn command(&self, probe: &ProbeCommand, route: ChildRoute) -> tokio::process::Command {
        let wrapped = self.wrap(probe);
        let mut command = crate::core::cmd::async_cmd(&wrapped.program, route);
        command.args(&wrapped.args).current_dir(&wrapped.cwd);
        for (name, value) in &wrapped.env {
            command.env(name, value);
        }
        crate::core::child_env::seal(command.as_std_mut(), route, &[]);
        #[cfg(unix)]
        crate::core::task_worker_env::detach_from_terminal(command.as_std_mut());
        command
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
            return Self::refuse(no_mechanism_reason("a lane build"));
        };
        if let Err(error) = create_temp_dir(policy) {
            return Self::refuse(error);
        }
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
