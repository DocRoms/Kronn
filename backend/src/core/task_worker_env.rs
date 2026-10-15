//! KT-1246 — what a task worker needs to build and test inside its own lane.
//!
//! A *lane* is one managed task worktree plus the few places outside it that
//! its toolchain legitimately writes: a private Cargo target directory (outside
//! `backend/`, which the dev backend's file watcher restarts on) and a private
//! `CARGO_HOME`. Everything here is runtime-agnostic — Claude's sandbox policy
//! (`agents::runner::claude_task_worker_settings`) and its launch environment
//! consume it, and another runtime can reuse the same provisioning and the same
//! smoke check without a line of Claude-specific code.
//!
//! Nothing here widens the network: dependencies are reused read-only from the
//! main checkout and from the host's caches, and Cargo is told to stay offline.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Where the worker's tests keep their Kronn state, so they never touch the
/// operator's real data directory.
pub const KRONN_DATA_DIR_ENV: &str = "KRONN_DATA_DIR";
/// The lane's private Cargo target directory.
pub const CARGO_TARGET_DIR_ENV: &str = "CARGO_TARGET_DIR";
/// Cargo never reaches a registry from a worker (supply chain).
pub const CARGO_NET_OFFLINE_ENV: &str = "CARGO_NET_OFFLINE";
/// The lane's Cargo home. By default the host's own, which the sandbox only
/// reads: Cargo fingerprints a registry crate by the absolute path of its
/// sources, so a lane under another `CARGO_HOME` can never reuse a warm target
/// and recompiles every dependency (KT-1250, measured). With
/// [`PRIVATE_CARGO_HOME_ENV`] set it is the lane's private home instead: lock
/// files land there, the registry is linked, credentials never are.
pub const CARGO_HOME_ENV: &str = "CARGO_HOME";
/// Set to `1`/`true` to give every lane the private `CARGO_HOME` of KT-1246
/// instead of the host's. A warm target is then reusable by no other lane (nor
/// by the main checkout's), so seeding is skipped.
pub const PRIVATE_CARGO_HOME_ENV: &str = "KRONN_WORKER_PRIVATE_CARGO_HOME";
/// Caps one worker's compilation (KT-1250); the host-wide cap on concurrently
/// compiling workers is `core::task_worker_lane_warm`.
pub const CARGO_BUILD_JOBS_ENV: &str = "CARGO_BUILD_JOBS";
/// The port a Kronn backend started by the worker listens on (never the host's).
pub const KRONN_PORT_ENV: &str = "KRONN_PORT";
/// Where the worker's Vite proxies `/api`: its own backend. Never
/// `KRONN_BACKEND_URL`, which is the address the worker's MCP bridge uses to
/// reach the host Kronn.
pub const VITE_BACKEND_URL_ENV: &str = "KRONN_VITE_BACKEND_URL";
/// The worker's Vite (and Playwright) port.
pub const VITE_DEV_PORT_ENV: &str = "VITE_DEV_PORT";
/// Vite's cache: the linked node_modules is read-only to the worker.
pub const VITE_CACHE_DIR_ENV: &str = "VITE_CACHE_DIR";
/// Playwright's Chromium runs single-process: the sandbox denies its rendezvous.
pub const E2E_CHROMIUM_SINGLE_PROCESS_ENV: &str = "KRONN_E2E_CHROMIUM_SINGLE_PROCESS";
/// Lane ports live in `[LANE_PORT_BASE, LANE_PORT_BASE + 2 * LANE_PORT_SLOTS)`,
/// away from the host's Kronn (3140) and Vite (5173).
const LANE_PORT_BASE: u16 = 20_000;
const LANE_PORT_SLOTS: u16 = 15_000;

/// `task_execution_events.action` under which the capability record is journalled.
pub const TOOLCHAIN_EVENT_ACTION: &str = "toolchain_capabilities";

/// What the main checkout's local Git exclude learns so lane targets
/// (`<repo>/target/wt-<lane>`) are never untracked files of the checkout.
const LANE_TARGET_EXCLUDE: &str = "/target/wt-*/";

const FRONTEND_DEPS_EXCLUDE: &str = "/frontend/node_modules";

/// Sub-directory of the worktree holding the frontend's `package.json`.
const FRONTEND_DIR: &str = "frontend";
const NODE_MODULES: &str = "node_modules";

const NEXTEST_TIMEOUT: Duration = Duration::from_secs(20);
const OFFLINE_FETCH_TIMEOUT: Duration = Duration::from_secs(90);
const VITEST_TIMEOUT: Duration = Duration::from_secs(20);
/// What a failed probe's reason keeps of the command's output.
const PROBE_DETAIL_BYTES: usize = 300;

/// One task worktree and the lane-private locations derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLane {
    /// Path-safe lane id: the worktree directory name without its `task-` prefix.
    pub name: String,
    pub worktree: PathBuf,
    /// The checkout that owns the worktree (`<repo>/.kronn/worktrees/<name>`),
    /// when the worktree follows the managed layout.
    pub main_checkout: Option<PathBuf>,
    pub cargo_target_dir: PathBuf,
    pub cargo_home: PathBuf,
    pub data_dir: PathBuf,
}

impl WorkerLane {
    /// The lane of an absolute (canonical) worktree path. A managed worktree
    /// gets `<repo>/target/wt-<lane>`; any other directory keeps its target
    /// inside itself, so the lane is still private and still writable.
    pub fn for_worktree(worktree: &Path) -> Result<Self, String> {
        if !worktree.is_absolute() {
            return Err(format!(
                "task worker lane needs an absolute worktree path, got {}",
                worktree.display()
            ));
        }
        let dir_name = worktree
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                format!(
                    "task worker lane cannot be derived from {}",
                    worktree.display()
                )
            })?;
        // The id becomes a path segment of a directory Kronn creates and the
        // sandbox then trusts: only plain characters survive.
        let sanitized: String = dir_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let name = sanitized
            .strip_prefix("task-")
            .filter(|rest| !rest.is_empty())
            .unwrap_or(&sanitized)
            .to_string();
        let main_checkout = worktree
            .parent()
            .filter(|parent| parent.file_name().is_some_and(|n| n == "worktrees"))
            .and_then(Path::parent)
            .filter(|parent| parent.file_name().is_some_and(|n| n == ".kronn"))
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let cargo_target_dir = match &main_checkout {
            Some(root) => root.join("target").join(format!("wt-{name}")),
            None => worktree.join(".kronn").join("target"),
        };
        Ok(Self {
            cargo_home: cargo_target_dir.join(".kronn-cargo-home"),
            data_dir: worktree.join(".kronn").join("data"),
            cargo_target_dir,
            main_checkout,
            worktree: worktree.to_path_buf(),
            name,
        })
    }

    /// Directories the worker may write: its worktree and, when it lives
    /// outside it, the lane's Cargo target (the private `CARGO_HOME`, when one
    /// is used, is inside). The host's own `CARGO_HOME` is never writable.
    pub fn write_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.worktree.clone()];
        if !self.cargo_target_dir.starts_with(&self.worktree) {
            roots.push(self.cargo_target_dir.clone());
        }
        roots
    }

    /// Directories the worker reads but never writes: the host's dependency
    /// caches and the main checkout's installed frontend dependencies.
    pub fn read_roots(&self, caches: &SharedCaches) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        roots.extend(caches.cargo_registry());
        roots.extend(caches.cargo_git());
        roots.extend(caches.pnpm_stores.iter().cloned());
        if let Some(source) = self.frontend_deps_source().filter(|path| path.is_dir()) {
            roots.push(source);
        }
        roots
    }

    /// Environment a task worker gets so no env-prefixed command is needed.
    /// `CARGO_HOME` is set only when its registry link is in place; otherwise
    /// Cargo keeps the host's default and the smoke check reports what happens.
    pub fn env(&self, provisioned: &Provisioned) -> Vec<(&'static str, OsString)> {
        let mut env = vec![
            (KRONN_DATA_DIR_ENV, self.data_dir.clone().into_os_string()),
            (
                CARGO_TARGET_DIR_ENV,
                self.cargo_target_dir.clone().into_os_string(),
            ),
            (CARGO_NET_OFFLINE_ENV, OsString::from("true")),
            (
                CARGO_BUILD_JOBS_ENV,
                OsString::from(crate::core::task_worker_lane_warm::worker_build_jobs().to_string()),
            ),
        ];
        let (backend, vite) = self.ports();
        env.push((KRONN_PORT_ENV, OsString::from(backend.to_string())));
        env.push((
            VITE_BACKEND_URL_ENV,
            OsString::from(format!("http://127.0.0.1:{backend}")),
        ));
        env.push((VITE_DEV_PORT_ENV, OsString::from(vite.to_string())));
        env.push((
            VITE_CACHE_DIR_ENV,
            self.data_dir.join("vite-cache").into_os_string(),
        ));
        env.push((E2E_CHROMIUM_SINGLE_PROCESS_ENV, OsString::from("1")));
        if let Some(home) = self.effective_cargo_home(provisioned) {
            env.push((CARGO_HOME_ENV, home.into_os_string()));
        }
        env
    }

    /// The `CARGO_HOME` this lane's Cargo runs with, when one is set at all: the
    /// host's own (same absolute registry paths as the main checkout's builds,
    /// so a warm target is reusable) unless the private home was asked for.
    pub fn effective_cargo_home(&self, provisioned: &Provisioned) -> Option<PathBuf> {
        if !provisioned.cargo_home_ready {
            return None;
        }
        Some(
            provisioned
                .shared_cargo_home
                .clone()
                .unwrap_or_else(|| self.cargo_home.clone()),
        )
    }

    /// The lane's own backend and Vite ports, stable for its name, so whatever
    /// the worker serves never takes the host Kronn's port.
    pub fn ports(&self) -> (u16, u16) {
        // FNV-1a: stable across processes, unlike the std hasher.
        let hash = self
            .name
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
        let slot = (hash % u64::from(LANE_PORT_SLOTS)) as u16;
        let backend = LANE_PORT_BASE + 2 * slot;
        (backend, backend + 1)
    }

    fn frontend_dir(&self) -> PathBuf {
        self.worktree.join(FRONTEND_DIR)
    }

    fn frontend_deps_source(&self) -> Option<PathBuf> {
        self.main_checkout
            .as_ref()
            .map(|root| root.join(FRONTEND_DIR).join(NODE_MODULES))
    }

    /// Create the lane's directories and link the read-only dependency caches.
    /// Idempotent and best effort: what could not be prepared is reported in the
    /// result (and ends up in the capability record), never raised, so a missing
    /// cache degrades to "cannot run those tests" instead of "cannot start".
    pub fn provision(&self, caches: &SharedCaches) -> Provisioned {
        self.provision_with(caches, private_cargo_home_requested())
    }

    /// [`WorkerLane::provision`] with the `CARGO_HOME` choice made explicit:
    /// `private_cargo_home` keeps KT-1246's lane-private home.
    pub fn provision_with(&self, caches: &SharedCaches, private_cargo_home: bool) -> Provisioned {
        let mut warnings = Vec::new();
        // A lane target beside the repo must never make the main checkout look
        // dirty: the integration preflight refuses a dirty target. A project
        // that already ignores `/target/` writes nothing.
        if let Some(root) = self
            .main_checkout
            .as_ref()
            .filter(|root| root.join(".git").exists())
        {
            if let Err(error) =
                crate::core::worktree::ensure_local_git_exclude(root, LANE_TARGET_EXCLUDE)
            {
                warnings.push(format!(
                    "cannot ignore the lane target directories: {error}"
                ));
            }
        }
        for dir in [&self.cargo_target_dir, &self.data_dir] {
            if let Err(error) = std::fs::create_dir_all(dir) {
                warnings.push(format!("cannot create {}: {error}", dir.display()));
            }
        }
        let cargo_home_ready = self.link_cargo_home(caches, &mut warnings);
        let shared_cargo_home = caches
            .cargo_home
            .clone()
            .filter(|_| cargo_home_ready && !private_cargo_home);
        let frontend_deps = self.link_frontend_deps();
        // A `node_modules/` rule matches directories only, so the link would
        // show as an untracked file and the delivery would find the worktree
        // dirty. Judged from inside the worktree, where the link exists.
        if matches!(frontend_deps, FrontendDeps::Linked(_)) && self.worktree.join(".git").exists() {
            if let Err(error) = crate::core::worktree::ensure_local_git_exclude(
                &self.worktree,
                FRONTEND_DEPS_EXCLUDE,
            ) {
                warnings.push(format!("cannot ignore the node_modules link: {error}"));
            }
        }
        Provisioned {
            frontend_deps,
            cargo_home_ready,
            shared_cargo_home,
            warnings,
        }
    }

    fn link_cargo_home(&self, caches: &SharedCaches, warnings: &mut Vec<String>) -> bool {
        let Some(registry) = caches.cargo_registry() else {
            return false;
        };
        if let Err(error) = std::fs::create_dir_all(&self.cargo_home) {
            warnings.push(format!(
                "cannot create {}: {error}",
                self.cargo_home.display()
            ));
            return false;
        }
        let mut links = vec![("registry", Some(registry))];
        links.push(("git", caches.cargo_git()));
        // Source replacement (a mirror, vendored crates) lives in the user's
        // config; credentials are deliberately not linked.
        links.push(("config.toml", caches.cargo_config()));
        let mut registry_ready = false;
        for (name, source) in links {
            let Some(source) = source else { continue };
            match ensure_symlink(&source, &self.cargo_home.join(name)) {
                Ok(_) => registry_ready |= name == "registry",
                Err(error) => warnings.push(error),
            }
        }
        registry_ready
    }

    fn link_frontend_deps(&self) -> FrontendDeps {
        let frontend = self.frontend_dir();
        if !frontend.join("package.json").is_file() {
            return FrontendDeps::NotApplicable;
        }
        let destination = frontend.join(NODE_MODULES);
        match std::fs::symlink_metadata(&destination) {
            // A real directory (or a link that still resolves) is left alone.
            Ok(_) if destination.is_dir() => return FrontendDeps::AlreadyPresent,
            Ok(_) => {} // dangling link: replaced below
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return FrontendDeps::Failed(error.to_string()),
        }
        let Some(source) = self.frontend_deps_source().filter(|path| path.is_dir()) else {
            return FrontendDeps::SourceMissing;
        };
        match ensure_symlink(&source, &destination) {
            Ok(_) => FrontendDeps::Linked(source),
            Err(error) => FrontendDeps::Failed(error),
        }
    }
}

/// How the worktree's frontend dependencies were made available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendDeps {
    /// The worktree has no `frontend/package.json`.
    NotApplicable,
    /// `frontend/node_modules` is a read-only link to the main checkout's.
    Linked(PathBuf),
    /// `frontend/node_modules` already existed; left untouched.
    AlreadyPresent,
    /// The main checkout has nothing to reuse (and no network install exists).
    SourceMissing,
    Failed(String),
}

fn private_cargo_home_requested() -> bool {
    crate::core::child_env::var_os(PRIVATE_CARGO_HOME_ENV)
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes"))
}

/// What [`WorkerLane::provision`] managed to prepare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    pub frontend_deps: FrontendDeps,
    pub cargo_home_ready: bool,
    /// The host's own `CARGO_HOME`, when the lane runs with it rather than with
    /// its private one (see [`CARGO_HOME_ENV`]).
    pub shared_cargo_home: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// The host's reusable dependency caches, read-only for a worker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedCaches {
    /// The operator's real `CARGO_HOME`, when it exists.
    pub cargo_home: Option<PathBuf>,
    /// pnpm content stores that exist on this host.
    pub pnpm_stores: Vec<PathBuf>,
}

impl SharedCaches {
    /// Resolve the caches from the backend's own environment.
    pub fn discover(main_checkout: Option<&Path>) -> Self {
        let var = |name: &str| crate::core::child_env::var_os(name).map(PathBuf::from);
        let home = var("HOME").or_else(|| var("USERPROFILE"));
        Self::from_roots(
            home.as_deref(),
            var("CARGO_HOME").as_deref(),
            var("PNPM_HOME").as_deref(),
            var("XDG_DATA_HOME").as_deref(),
            main_checkout,
        )
    }

    /// Pure resolution: only directories that exist are kept.
    pub fn from_roots(
        home: Option<&Path>,
        cargo_home: Option<&Path>,
        pnpm_home: Option<&Path>,
        xdg_data_home: Option<&Path>,
        main_checkout: Option<&Path>,
    ) -> Self {
        let cargo_home = cargo_home
            .map(Path::to_path_buf)
            .or_else(|| home.map(|home| home.join(".cargo")))
            .filter(|path| path.is_dir())
            .and_then(|path| path.canonicalize().ok());
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(pnpm_home) = pnpm_home {
            candidates.push(pnpm_home.join("store"));
        }
        if let Some(data) = xdg_data_home {
            candidates.push(data.join("pnpm").join("store"));
        }
        if let Some(home) = home {
            candidates.push(home.join("Library").join("pnpm").join("store"));
            candidates.push(home.join(".local").join("share").join("pnpm").join("store"));
        }
        if let Some(root) = main_checkout {
            candidates.push(root.join(".pnpm-store"));
        }
        let mut pnpm_stores: Vec<PathBuf> = Vec::new();
        for candidate in candidates {
            if let Ok(canonical) = candidate.canonicalize() {
                if canonical.is_dir() && !pnpm_stores.contains(&canonical) {
                    pnpm_stores.push(canonical);
                }
            }
        }
        Self {
            cargo_home,
            pnpm_stores,
        }
    }

    pub fn cargo_registry(&self) -> Option<PathBuf> {
        self.cargo_home
            .as_ref()
            .map(|home| home.join("registry"))
            .filter(|path| path.is_dir())
    }

    pub fn cargo_git(&self) -> Option<PathBuf> {
        self.cargo_home
            .as_ref()
            .map(|home| home.join("git"))
            .filter(|path| path.is_dir())
    }

    fn cargo_config(&self) -> Option<PathBuf> {
        self.cargo_home
            .as_ref()
            .map(|home| home.join("config.toml"))
            .filter(|path| path.is_file())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LinkState {
    Created,
    AlreadyLinked,
}

/// Link `destination` to `source`, replacing a link that points elsewhere. A
/// real file or directory at `destination` is never touched: it is the
/// operator's (or the worker's) own and is reported as an error.
fn ensure_symlink(source: &Path, destination: &Path) -> Result<LinkState, String> {
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if std::fs::read_link(destination).is_ok_and(|current| current == source) {
                return Ok(LinkState::AlreadyLinked);
            }
            std::fs::remove_file(destination).map_err(|error| {
                format!("cannot replace the link {}: {error}", destination.display())
            })?;
        }
        Ok(_) => {
            return Err(format!(
                "{} already exists and is not a link; left untouched",
                destination.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("cannot inspect {}: {error}", destination.display()));
        }
    }
    symlink(source, destination).map_err(|error| {
        format!(
            "cannot link {} to {}: {error}",
            destination.display(),
            source.display()
        )
    })?;
    Ok(LinkState::Created)
}

#[cfg(unix)]
fn symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(source, destination)
    } else {
        std::os::windows::fs::symlink_file(source, destination)
    }
}

/// Prepare the lane of the worktree a task worker is about to start in.
pub fn prepare_launch(work_dir: &Path) -> Result<(WorkerLane, Provisioned), String> {
    let worktree = work_dir.canonicalize().map_err(|error| {
        format!(
            "cannot canonicalize task worktree {}: {error}",
            work_dir.display()
        )
    })?;
    let lane = WorkerLane::for_worktree(&worktree)?;
    let caches = SharedCaches::discover(lane.main_checkout.as_deref());
    let provisioned = lane.provision(&caches);
    Ok((lane, provisioned))
}

/// Give a task-worker process its lane environment. Provisioning failures are
/// logged and never refuse the launch: the capability record says what works.
pub fn apply_launch_env(command: &mut std::process::Command, work_dir: &Path) {
    match prepare_launch(work_dir) {
        Ok((lane, provisioned)) => {
            for warning in &provisioned.warnings {
                tracing::warn!(lane = %lane.name, "task worker lane: {warning}");
            }
            for (name, value) in lane.env(&provisioned) {
                command.env(name, value);
            }
        }
        Err(error) => tracing::warn!("task worker lane not applied: {error}"),
    }
}

// ─── Toolchain smoke check ────────────────────────────────────────────────

/// One capability a worker can rely on, with the reason it can or cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub available: bool,
    pub reason: String,
}

impl Capability {
    fn yes(reason: impl Into<String>) -> Self {
        Self {
            available: true,
            reason: reason.into(),
        }
    }

    fn no(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: reason.into(),
        }
    }
}

/// What a worker can run in its lane, recorded on the execution at launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainCapabilities {
    pub rust_tests: Capability,
    pub frontend_tests: Capability,
    pub port_binding: Capability,
}

impl ToolchainCapabilities {
    /// A capability set for a launch whose check could not run at all.
    pub fn unverified(reason: &str) -> Self {
        Self {
            rust_tests: Capability::no(reason),
            frontend_tests: Capability::no(reason),
            port_binding: Capability::no(reason),
        }
    }

    pub fn any_available(&self) -> bool {
        self.rust_tests.available || self.frontend_tests.available || self.port_binding.available
    }
}

/// A bounded command of the smoke check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, OsString)>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub success: bool,
    /// Trailing output (success) or the failure's diagnostic.
    pub detail: String,
}

/// How the smoke check runs a command; injectable so both outcomes are testable.
pub trait ProbeRunner: Send + Sync {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome;

    /// [`ProbeRunner::run`] that stops the command when `cancel` fires,
    /// reporting a failure. A runner that cannot interrupt a probe keeps this
    /// default and simply runs it to its end.
    fn run_cancellable(&self, probe: &ProbeCommand, cancel: &CancellationToken) -> ProbeOutcome {
        let _ = cancel;
        self.run(probe)
    }

    /// Whether every command this runner starts runs under the worker's sandbox
    /// profile (KT-1255, `core::task_worker_sandbox`). The plain process runner
    /// does not: it is for Kronn's own commands, never for a lane's build.
    fn confined(&self) -> bool {
        false
    }
}

/// Runs probes as real child processes with the sealed Kronn environment plus
/// the lane's own variables.
pub struct ProcessProbeRunner;

impl ProbeRunner for ProcessProbeRunner {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.run_cancellable(probe, &CancellationToken::new())
    }

    fn run_cancellable(&self, probe: &ProbeCommand, cancel: &CancellationToken) -> ProbeOutcome {
        let failure = |detail: String| ProbeOutcome {
            success: false,
            detail,
        };
        let mut command =
            crate::core::cmd::sync_cmd(&probe.program, crate::core::child_env::ChildRoute::Tool);
        command.args(&probe.args).current_dir(&probe.cwd);
        for (name, value) in &probe.env {
            command.env(name, value);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group, so a timeout or a cancel takes `cargo`'s
        // `rustc` children with it instead of leaving them compiling.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return failure(format!("cannot start `{}`: {error}", probe.program)),
        };
        let stdout = child.stdout.take().map(drain_tail);
        let stderr = child.stderr.take().map(drain_tail);
        let deadline = Instant::now() + probe.timeout;
        let mut cancelled = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if cancel.is_cancelled() => {
                    kill_process_tree(&mut child);
                    cancelled = true;
                    break None;
                }
                Ok(None) if Instant::now() >= deadline => {
                    kill_process_tree(&mut child);
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => return failure(format!("cannot wait for the probe: {error}")),
            }
        };
        let collect = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| {
            handle
                .and_then(|handle| handle.join().ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
                .unwrap_or_default()
        };
        let (stdout, stderr) = (collect(stdout), collect(stderr));
        let detail = if stderr.is_empty() { stdout } else { stderr };
        let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        match status {
            Some(status) if status.success() => ProbeOutcome {
                success: true,
                detail,
            },
            Some(status) => failure(format!("exited with {status}: {detail}")),
            None if cancelled => failure("cancelled".to_string()),
            None => failure(format!("timed out after {}s", probe.timeout.as_secs())),
        }
    }
}

/// Kill the probe and everything it started (it leads its own process group),
/// then reap it.
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // SAFETY: `killpg` takes a plain process-group id; the child is the
        // leader of the group it was spawned into.
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Read a pipe to its end, keeping only the last [`PROBE_DETAIL_BYTES`].
fn drain_tail<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(read) = pipe.read(&mut chunk) {
            if read == 0 {
                break;
            }
            tail.extend_from_slice(&chunk[..read]);
            if tail.len() > PROBE_DETAIL_BYTES {
                tail.drain(..tail.len() - PROBE_DETAIL_BYTES);
            }
        }
        tail
    })
}

/// Provision the lane of `work_dir` and smoke-check it. Never fails: a lane
/// that cannot even be prepared is reported as unverified, with the reason.
pub fn check_worktree(
    work_dir: &Path,
    loopback_binding_allowed: bool,
    runner: &dyn ProbeRunner,
) -> ToolchainCapabilities {
    match prepare_launch(work_dir) {
        Ok((lane, provisioned)) => {
            for warning in &provisioned.warnings {
                tracing::warn!(lane = %lane.name, "task worker lane: {warning}");
            }
            smoke_check(&lane, &provisioned, loopback_binding_allowed, runner)
        }
        Err(error) => ToolchainCapabilities::unverified(&format!(
            "the worker lane could not be prepared: {error}"
        )),
    }
}

/// Check, with the worker's own lane environment, that it can build and test.
///
/// `loopback_binding_allowed` is the sandbox policy's `allowLocalBinding`: the
/// port capability needs both the policy and a bind that really works.
pub fn smoke_check(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    loopback_binding_allowed: bool,
    runner: &dyn ProbeRunner,
) -> ToolchainCapabilities {
    smoke_check_with(
        lane,
        provisioned,
        loopback_binding_allowed,
        runner,
        &bind_loopback_port,
    )
}

/// [`smoke_check`] with the loopback bind injected, so a test never depends on
/// the host letting it bind a port.
pub(crate) fn smoke_check_with(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    loopback_binding_allowed: bool,
    runner: &dyn ProbeRunner,
    bind: &dyn Fn() -> std::io::Result<u16>,
) -> ToolchainCapabilities {
    let env: Vec<(String, OsString)> = lane
        .env(provisioned)
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    ToolchainCapabilities {
        rust_tests: check_rust(lane, &env, runner),
        frontend_tests: check_frontend(lane, provisioned, &env, runner),
        port_binding: check_port_binding(loopback_binding_allowed, bind),
    }
}

fn bind_loopback_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn check_rust(
    lane: &WorkerLane,
    env: &[(String, OsString)],
    runner: &dyn ProbeRunner,
) -> Capability {
    let Some(manifest_dir) = [lane.worktree.clone(), lane.worktree.join("backend")]
        .into_iter()
        .find(|dir| dir.join("Cargo.toml").is_file())
    else {
        return Capability::no("not applicable: no Cargo.toml at the worktree root or in backend/");
    };
    if let Err(error) = probe_writable(&lane.cargo_target_dir) {
        return Capability::no(format!(
            "the lane target directory {} is not writable: {error}",
            lane.cargo_target_dir.display()
        ));
    }
    let probe = |args: &[&str], timeout: Duration| ProbeCommand {
        program: "cargo".into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: manifest_dir.clone(),
        env: env.to_vec(),
        timeout,
    };
    let nextest = runner.run(&probe(&["nextest", "--version"], NEXTEST_TIMEOUT));
    if !nextest.success {
        return Capability::no(format!(
            "`cargo nextest --version` failed: {}",
            nextest.detail
        ));
    }
    // A cold `cargo check` would compile the whole dependency graph; resolving
    // it offline from the cache proves the part a sandbox actually breaks.
    let fetch = runner.run(&probe(
        &["fetch", "--offline", "--locked"],
        OFFLINE_FETCH_TIMEOUT,
    ));
    if !fetch.success {
        return Capability::no(format!(
            "offline dependency resolution failed (`cargo fetch --offline --locked`): {}",
            fetch.detail
        ));
    }
    Capability::yes(format!(
        "cargo nextest and the offline dependency cache work; lane target {}",
        lane.cargo_target_dir.display()
    ))
}

fn check_frontend(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    env: &[(String, OsString)],
    runner: &dyn ProbeRunner,
) -> Capability {
    match &provisioned.frontend_deps {
        FrontendDeps::NotApplicable => {
            Capability::no("not applicable: the worktree has no frontend/package.json")
        }
        FrontendDeps::SourceMissing => Capability::no(
            "the main checkout has no frontend/node_modules to reuse and a worker never installs \
             from the network",
        ),
        FrontendDeps::Failed(error) => Capability::no(format!(
            "frontend/node_modules could not be linked: {error}"
        )),
        FrontendDeps::Linked(_) | FrontendDeps::AlreadyPresent => {
            let frontend = lane.frontend_dir();
            let vitest = frontend.join(NODE_MODULES).join(".bin").join("vitest");
            if !vitest.exists() {
                return Capability::no("frontend/node_modules/.bin/vitest is missing");
            }
            let outcome = runner.run(&ProbeCommand {
                program: vitest.to_string_lossy().into_owned(),
                args: vec!["--version".into()],
                cwd: frontend,
                env: env.to_vec(),
                timeout: VITEST_TIMEOUT,
            });
            if outcome.success {
                Capability::yes(format!(
                    "vitest {} runs from frontend/node_modules",
                    outcome.detail
                ))
            } else {
                Capability::no(format!("`vitest --version` failed: {}", outcome.detail))
            }
        }
    }
}

fn check_port_binding(
    loopback_binding_allowed: bool,
    bind: &dyn Fn() -> std::io::Result<u16>,
) -> Capability {
    if !loopback_binding_allowed {
        return Capability::no("the sandbox policy does not allow loopback binding");
    }
    match bind() {
        Ok(port) => Capability::yes(format!(
            "loopback binding is allowed by the sandbox policy and works (probed 127.0.0.1:{port})"
        )),
        Err(error) => Capability::no(format!("binding 127.0.0.1 failed: {error}")),
    }
}

/// Prove a directory can be created and written, then leave it as it was.
fn probe_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let marker = dir.join(".kronn-write-probe");
    std::fs::write(&marker, b"")?;
    std::fs::remove_file(&marker)
}

#[cfg(test)]
#[path = "task_worker_env_test.rs"]
mod task_worker_env_test;
