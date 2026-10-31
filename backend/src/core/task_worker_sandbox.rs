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
use std::sync::{Arc, OnceLock};

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
    pub(crate) fn launcher(self) -> &'static str {
        match self {
            Self::Seatbelt => SANDBOX_EXEC,
            Self::Bubblewrap => BWRAP,
        }
    }

    /// The mechanism of this host once it proved it can apply it, otherwise
    /// why not (KT-1268): the one detection every confined launch goes
    /// through — a validation of a worker's tree and the lane's first build and
    /// toolchain smoke check alike — so none of them fails on a spawn error of
    /// a launcher that is missing or cannot apply its sandbox. Only a success
    /// is remembered: a host that gains `bwrap` is picked up without a restart.
    pub fn usable_on_host() -> Result<Self, String> {
        Self::usable_from(Self::of_host(), Self::check_usable_remembered)
    }

    /// [`Mechanism::usable_on_host`] with the host's mechanism and the check of
    /// its launcher injected.
    fn usable_from(
        mechanism: Option<Self>,
        check: impl FnOnce(Self) -> Result<(), String>,
    ) -> Result<Self, String> {
        let mechanism =
            mechanism.ok_or_else(|| no_mechanism_reason("a command of a worker's tree"))?;
        check(mechanism)?;
        Ok(mechanism)
    }

    /// [`Mechanism::check_usable`], answered from memory once it succeeded: the
    /// launcher is one `true` process per mechanism, not one per command.
    fn check_usable_remembered(self) -> Result<(), String> {
        static SEATBELT: OnceLock<()> = OnceLock::new();
        static BUBBLEWRAP: OnceLock<()> = OnceLock::new();
        let proven = match self {
            Self::Seatbelt => &SEATBELT,
            Self::Bubblewrap => &BUBBLEWRAP,
        };
        if proven.get().is_some() {
            return Ok(());
        }
        self.check_usable()?;
        let _ = proven.set(());
        Ok(())
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

/// A way to start a command under an OS sandbox: the worker's lane
/// ([`Confinement`]) or the profile of what a trust level grants
/// ([`crate::core::trust_sandbox::LevelConfinement`]).
pub trait CommandConfinement: Send + Sync {
    /// The process `probe` becomes under this confinement, not yet started.
    fn command(&self, probe: &ProbeCommand, route: ChildRoute) -> tokio::process::Command;
}

impl CommandConfinement for Confinement {
    fn command(&self, probe: &ProbeCommand, route: ChildRoute) -> tokio::process::Command {
        Confinement::command(self, probe, route)
    }
}

/// What a confined command may write. Everything else on disk is read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub write_roots: Vec<PathBuf>,
    /// The lane's own scratch directory, inside a write root. The confined
    /// command gets it as `TMPDIR`; no global temp directory is writable.
    pub temp_dir: PathBuf,
    /// The Docker socket the command may connect to, and nothing else on the
    /// network. Only a validation of a project whose human opened Docker to its
    /// validations carries one (KT-1433): the socket is the host's root.
    pub docker_socket: Option<PathBuf>,
    /// Directories the command can neither read nor write (KT-1492): the Kronn
    /// data directory, for a validation a trust level reached Docker for.
    pub hidden: Vec<PathBuf>,
}

impl SandboxPolicy {
    /// The worker's own write roots: its worktree and its lane's Cargo target.
    /// The host's `CARGO_HOME` and dependency caches stay read-only.
    pub fn for_lane(lane: &WorkerLane) -> Self {
        Self {
            write_roots: lane.write_roots(),
            temp_dir: lane.tmp_dir(),
            docker_socket: None,
            hidden: Vec::new(),
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
pub(crate) fn real_path(path: &Path) -> PathBuf {
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
pub(crate) fn seatbelt_literal(path: &Path) -> String {
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
    seatbelt_profile_with(writable, None)
}

/// [`seatbelt_profile`], plus outbound connections to `docker_socket` alone,
/// by its path and by the real path behind it.
pub fn seatbelt_profile_with(writable: &[PathBuf], docker_socket: Option<&Path>) -> String {
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
    if let Some(socket) = docker_socket {
        let mut paths = vec![socket.to_path_buf(), real_path(socket)];
        paths.dedup();
        profile.push_str("(allow network-outbound\n");
        for path in paths {
            profile.push_str(&format!(
                "  (remote unix-socket (path-literal {}))\n",
                seatbelt_literal(&path)
            ));
        }
        profile.push_str(")\n");
    }
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
    bubblewrap_args_with(writable, None)
}

/// [`bubblewrap_args`], plus `docker_socket` bound back at its path: a Unix
/// socket is reached by path, so the empty network namespace does not cut it.
pub fn bubblewrap_args_with(writable: &[PathBuf], docker_socket: Option<&Path>) -> Vec<String> {
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
    if let Some(socket) = docker_socket {
        let real = real_path(socket).to_string_lossy().into_owned();
        let at = socket.to_string_lossy().into_owned();
        args.extend(["--bind".to_string(), real, at]);
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
            vec![
                "-p".to_string(),
                seatbelt_profile_with(&writable, policy.docker_socket.as_deref()),
            ],
        ),
        Mechanism::Bubblewrap => (
            mechanism.launcher().to_string(),
            bubblewrap_args_with(&writable, policy.docker_socket.as_deref()),
        ),
    };
    hide_from(mechanism, &policy.hidden, &mut args);
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
    if let Some(socket) = &policy.docker_socket {
        env.retain(|(name, _)| name != "DOCKER_HOST");
        env.push((
            "DOCKER_HOST".to_string(),
            format!("unix://{}", socket.display()).into(),
        ));
    }
    ProbeCommand {
        program,
        args,
        cwd: probe.cwd.clone(),
        env,
        timeout: probe.timeout,
    }
}

/// KT-1492 — close `hidden` to the confined command: a Seatbelt deny appended
/// to the profile (the last matching rule wins), or an empty tmpfs mounted over
/// each existing directory before the namespaces are unshared.
pub(crate) fn hide_from(mechanism: Mechanism, hidden: &[PathBuf], args: &mut Vec<String>) {
    if hidden.is_empty() {
        return;
    }
    match mechanism {
        Mechanism::Seatbelt => {
            if let Some(profile) = args.get_mut(1) {
                profile.push_str(&crate::core::trust_sandbox::seatbelt_hide_rules(hidden));
            }
        }
        Mechanism::Bubblewrap => {
            let at = args
                .iter()
                .position(|arg| arg == "--unshare-net")
                .unwrap_or(args.len().saturating_sub(1));
            let tmpfs: Vec<String> = crate::core::trust_sandbox::bubblewrap_mount_points(hidden)
                .into_iter()
                .flat_map(|dir| ["--tmpfs".to_string(), dir])
                .collect();
            args.splice(at..at, tmpfs);
        }
    }
}

/// The Docker socket a validation of this host reaches: `DOCKER_HOST` when it
/// names a Unix socket, else the current `docker context`, else the first
/// existing socket of OrbStack, Docker Desktop, Colima or `/var/run`. A TCP
/// endpoint is refused: the sandbox opens one socket path, never the network.
pub fn host_docker_socket() -> Result<PathBuf, String> {
    let from_env = crate::core::child_env::var("DOCKER_HOST").ok();
    let from_context = || {
        crate::core::cmd::sync_cmd("docker", ChildRoute::Docker)
            .args([
                "context",
                "inspect",
                "--format",
                "{{.Endpoints.docker.Host}}",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let home = crate::core::child_env::var_os("HOME").map(PathBuf::from);
    docker_socket_from(from_env.as_deref(), from_context, home.as_deref(), |path| {
        path.exists()
    })
}

fn docker_socket_from(
    docker_host: Option<&str>,
    context_host: impl FnOnce() -> Option<String>,
    home: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf, String> {
    let unix = |host: &str| -> Result<Option<PathBuf>, String> {
        match host.strip_prefix("unix://") {
            Some(path) if !path.is_empty() => Ok(Some(PathBuf::from(path))),
            _ if host.trim().is_empty() => Ok(None),
            _ => Err(format!(
                "Docker is configured on {host}, not a local Unix socket: validations can only reach a socket"
            )),
        }
    };
    if let Some(host) = docker_host.filter(|host| !host.trim().is_empty()) {
        if let Some(path) = unix(host)? {
            return Ok(path);
        }
    }
    if let Some(host) = context_host() {
        if let Some(path) = unix(&host)? {
            return Ok(path);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".orbstack/run/docker.sock"));
        candidates.push(home.join(".docker/run/docker.sock"));
        candidates.push(home.join(".colima/default/docker.sock"));
    }
    candidates.push(PathBuf::from("/var/run/docker.sock"));
    candidates
        .into_iter()
        .find(|path| exists(path))
        .ok_or_else(|| {
            "no Docker socket found on this host: start Docker (OrbStack, Docker Desktop or Colima)"
                .to_string()
        })
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
        Self::with_mechanism(work_dir, Some(Mechanism::usable_on_host()?))
    }

    /// [`Confinement::for_worktree`] with the host's mechanism and the check of
    /// its launcher injected. A mechanism that is not usable refuses before
    /// the lane is provisioned: nothing is prepared for a host that cannot
    /// confine.
    #[cfg(test)]
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

    /// This confinement with `socket` reachable, for a project whose human
    /// opened Docker to its validations (KT-1433).
    pub fn with_docker_socket(mut self, socket: PathBuf) -> Self {
        self.policy.docker_socket = Some(socket);
        self
    }

    /// This confinement with `dirs` closed to the command (KT-1492).
    pub fn hiding(mut self, dirs: Vec<PathBuf>) -> Self {
        self.policy.hidden = dirs;
        self
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
    /// The host's usable mechanism, or why it has none. Asked when a probe
    /// runs, not when the runner is built: the check starts a process, and a
    /// runner is built on the async worker while a probe runs on a blocking one.
    mechanism: Arc<dyn Fn() -> Result<Mechanism, String> + Send + Sync>,
    inner: Arc<dyn ProbeRunner>,
}

impl SandboxedProbeRunner {
    /// The production runner of a task worktree: this host's mechanism, once
    /// proven usable ([`Mechanism::usable_on_host`], the detection a
    /// validation of a worker's tree goes through too), real child processes.
    pub fn for_worktree(work_dir: &Path) -> Self {
        Self::with_mechanism_check(
            SandboxPolicy::for_worktree(work_dir),
            Arc::new(Mechanism::usable_on_host),
            Arc::new(ProcessProbeRunner),
        )
    }

    pub fn new(
        policy: Result<SandboxPolicy, String>,
        mechanism: Option<Mechanism>,
        inner: Arc<dyn ProbeRunner>,
    ) -> Self {
        Self::with_mechanism_check(
            policy,
            Arc::new(move || mechanism.ok_or_else(|| no_mechanism_reason("a lane build"))),
            inner,
        )
    }

    /// [`SandboxedProbeRunner::new`] with the host's mechanism given as the
    /// check that proves it usable.
    pub fn with_mechanism_check(
        policy: Result<SandboxPolicy, String>,
        mechanism: Arc<dyn Fn() -> Result<Mechanism, String> + Send + Sync>,
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
        let mechanism = match (self.mechanism)() {
            Ok(mechanism) => mechanism,
            Err(reason) => return Self::refuse(reason),
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
