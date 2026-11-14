//! KT-1250 — a worker lane starts warm and compiles under a host-wide cap.
//!
//! KT-1246 gave every lane its own Cargo target, but an empty one: a worker
//! spent 38+ minutes compiling the dependency graph from cold, and four workers
//! doing it at once pushed the host's load average to 44. Two things fix that:
//!
//! * the lane target is seeded with a **copy-on-write clone** of the main
//!   checkout's warm `target/debug` (APFS `clonefile(2)`, a reflink elsewhere;
//!   where neither is supported the target stays empty, as before) so the
//!   worker only recompiles what it changes. A *lane* target is never a seed
//!   source: every worker may write its own lane target, and Kronn builds the
//!   seeded lane outside the worker sandbox, so a planted artifact would run
//!   as the host user (see [`seed_candidates`]);
//! * compilation pressure is capped: `CARGO_BUILD_JOBS` is injected into the
//!   lane environment, and the lane's first build — run by Kronn before the
//!   worker starts — goes through a host-wide gate, extra lanes queueing.
//!
//! Everything here is runtime-agnostic; [`crate::core::task_worker_env`] owns
//! the lane layout it works on.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::background_priority::Priority;
use crate::core::project_toolchains::{self, CargoMetadata, RustRoot};
use crate::core::seed_prune::{self, PruneReport};
use crate::core::task_worker_env::{
    prepare_launch, ProbeCommand, ProbeRunner, ProcessProbeRunner, Provisioned, WorkerLane,
};
use crate::core::task_worker_lane_settings;
use crate::core::task_worker_root_marks::{self, RootMark, RootState};
pub use crate::core::task_worker_seed_align::SeedAlignment;
use crate::core::task_worker_seed_align::{self, AlignRequest, Assessment};
use crate::models::WarmingStepKind;

/// Overrides the `CARGO_BUILD_JOBS` a worker gets.
pub const BUILD_JOBS_OVERRIDE_ENV: &str = "KRONN_WORKER_CARGO_JOBS";
/// Overrides how many task workers may compile at once on this host.
pub const MAX_CONCURRENT_BUILDS_ENV: &str = "KRONN_WORKER_MAX_CONCURRENT_BUILDS";
/// Overrides the first build's time limit, in seconds.
pub const FIRST_BUILD_TIMEOUT_ENV: &str = "KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS";

/// `task_execution_events.action` of the warm-start record (a sibling of the
/// `toolchain_capabilities` event, so the capability record keeps its shape).
pub const WARM_START_EVENT_ACTION: &str = "lane_warm_start";

const DEFAULT_MAX_CONCURRENT_BUILDS: usize = 2;
/// KT-1742: 60 min, not 30. Even from a perfect seed, the `kronn` crate's own
/// two compiles (lib with `test-support`, then the lib test binary) took 32 min
/// 56 s at host load 14 with 3 jobs, so a 30-min bound killed a healthy warm-up
/// and threw its work away. See `docs/operations/worktree-build-artifacts.md`.
const DEFAULT_FIRST_BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Written in a lane's target at the end of a successful first build.
const WARM_MARKER: &str = ".kronn-warm.json";
const DEBUG_DIR: &str = "debug";
/// Cargo's own advisory lock on a profile directory, held for a build's whole
/// duration (`<target>/debug/.cargo-lock`).
#[cfg(test)]
const CARGO_BUILD_LOCK: &str = seed_prune::CARGO_BUILD_LOCK;
const TIMED_OUT_DETAIL_PREFIX: &str = "timed out after";
/// How a probe's detail starts when the runner refused it (the sandboxed runner,
/// `not run: …`) or could not start the program: not a verdict on a root.
const NOT_STARTED_PREFIXES: [&str; 2] = ["not run:", "cannot start"];
/// How often a queued first build looks at its cancellation.
const GATE_POLL: Duration = Duration::from_millis(100);

// ─── Settings ─────────────────────────────────────────────────────────────

/// How much compilation a worker may cause, resolved from the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmStartSettings {
    /// `CARGO_BUILD_JOBS` of every worker process.
    pub build_jobs: u32,
    /// Task workers that may run their first build at the same time.
    pub max_concurrent_builds: usize,
    pub first_build_timeout: Duration,
}

impl WarmStartSettings {
    pub fn from_host() -> Self {
        let var = |name: &str| {
            crate::core::child_env::var_os(name).and_then(|value| value.into_string().ok())
        };
        Self::resolve(
            var(BUILD_JOBS_OVERRIDE_ENV).as_deref(),
            var(MAX_CONCURRENT_BUILDS_ENV).as_deref(),
            var(FIRST_BUILD_TIMEOUT_ENV).as_deref(),
            performance_cores(),
        )
    }

    /// [`WarmStartSettings::from_host`] for a project whose own setting (or the
    /// global one) gives the first build's bound: the operator's
    /// `KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS`, when set to a positive integer,
    /// still wins over it.
    pub fn from_host_with_timeout(setting: Duration) -> Self {
        let mut settings = Self::from_host();
        let operator = crate::core::child_env::var_os(FIRST_BUILD_TIMEOUT_ENV)
            .and_then(|value| value.into_string().ok())
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0);
        if operator.is_none() {
            settings.first_build_timeout = setting;
        }
        settings
    }

    /// The settings of the lane of `work_dir`: the host's, with the first
    /// build's bound of its project (Settings > Workers, then the project's own).
    pub fn for_worktree(work_dir: &Path) -> Self {
        let lane = work_dir
            .canonicalize()
            .ok()
            .and_then(|worktree| WorkerLane::for_worktree(&worktree).ok());
        match lane {
            Some(lane) => {
                let effective = crate::core::task_worker_lane_settings::effective_for(
                    &lane.worktree,
                    lane.main_checkout.as_deref(),
                );
                Self::from_host_with_timeout(effective.first_build_timeout)
            }
            None => Self::from_host(),
        }
    }

    /// Pure resolution. Default jobs: half of the performance cores, so two
    /// compiling workers use what the host can really run in parallel. An
    /// override that is not a positive integer is ignored.
    pub fn resolve(
        jobs: Option<&str>,
        max_concurrent: Option<&str>,
        timeout_secs: Option<&str>,
        performance_cores: usize,
    ) -> Self {
        let positive = |value: Option<&str>| {
            value
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
        };
        Self {
            build_jobs: positive(jobs)
                .map(|jobs| jobs.min(u32::MAX as u64) as u32)
                .unwrap_or_else(|| (performance_cores / 2).max(1) as u32),
            max_concurrent_builds: positive(max_concurrent)
                .map(|max| max as usize)
                .unwrap_or(DEFAULT_MAX_CONCURRENT_BUILDS),
            first_build_timeout: positive(timeout_secs)
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_FIRST_BUILD_TIMEOUT),
        }
    }
}

/// The `CARGO_BUILD_JOBS` a worker of this host gets.
pub fn worker_build_jobs() -> u32 {
    WarmStartSettings::from_host().build_jobs
}

/// Performance cores of the host: Apple silicon's `perflevel0`, else every
/// logical CPU the process may use.
fn performance_cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let perf =
                crate::core::cmd::sync_cmd("sysctl", crate::core::child_env::ChildRoute::Tool)
                    .args(["-n", "hw.perflevel0.physicalcpu"])
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .and_then(|text| text.trim().parse::<usize>().ok())
                    .filter(|cores| *cores > 0);
            if let Some(perf) = perf {
                return perf;
            }
        }
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
    })
}

// ─── Host-wide build gate ─────────────────────────────────────────────────

/// A counting semaphore over the first builds of the task workers.
pub struct BuildGate {
    running: Mutex<usize>,
    released: Condvar,
}

/// Held for the duration of one first build; dropping it frees the slot.
pub struct BuildPermit<'a> {
    gate: &'a BuildGate,
}

impl BuildGate {
    pub const fn new() -> Self {
        Self {
            running: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// Builds currently holding a permit.
    pub fn running(&self) -> usize {
        *self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn try_acquire(&self, limit: usize) -> Option<BuildPermit<'_>> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if *running >= limit.max(1) {
            return None;
        }
        *running += 1;
        Some(BuildPermit { gate: self })
    }

    /// Take a slot, waiting for one when `limit` builds are already running.
    /// `on_wait` is called once, before the wait, with the number of builds
    /// ahead — the caller says so in the worker's room.
    pub fn acquire(&self, limit: usize, on_wait: impl FnOnce(usize)) -> BuildPermit<'_> {
        let limit = limit.max(1);
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if *running >= limit {
            on_wait(*running);
            while *running >= limit {
                running = self
                    .released
                    .wait(running)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
        *running += 1;
        BuildPermit { gate: self }
    }
}

/// How a bounded, cancellable wait for the gate ended.
pub enum GateWait<'a> {
    Acquired(BuildPermit<'a>),
    /// No slot freed within the bound.
    TimedOut,
    /// The wait was cancelled (the execution was cancelled while it queued).
    Cancelled,
}

impl BuildGate {
    /// [`BuildGate::acquire`] that gives up after `max_wait`, so a queue that
    /// never drains cannot hold a worker in `Provisioning` forever, and that
    /// stops waiting as soon as `cancel` fires, taking no slot.
    pub fn acquire_within(
        &self,
        limit: usize,
        max_wait: Duration,
        cancel: &CancellationToken,
        on_wait: impl FnOnce(usize),
    ) -> GateWait<'_> {
        let limit = limit.max(1);
        let deadline = Instant::now() + max_wait;
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if cancel.is_cancelled() {
            return GateWait::Cancelled;
        }
        if *running >= limit {
            on_wait(*running);
            while *running >= limit {
                if cancel.is_cancelled() {
                    return GateWait::Cancelled;
                }
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return GateWait::TimedOut;
                };
                running = self
                    .released
                    .wait_timeout(running, remaining.min(GATE_POLL))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        *running += 1;
        GateWait::Acquired(BuildPermit { gate: self })
    }
}

impl Default for BuildGate {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BuildPermit<'_> {
    fn drop(&mut self) {
        let mut running = self.gate.running.lock().unwrap_or_else(|e| e.into_inner());
        *running = running.saturating_sub(1);
        self.gate.released.notify_one();
    }
}

/// The gate every task worker of this backend process goes through.
pub fn host_build_gate() -> &'static BuildGate {
    static GATE: BuildGate = BuildGate::new();
    &GATE
}

// ─── The record ───────────────────────────────────────────────────────────

/// Where a warm lane's `debug/` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSourceKind {
    /// The main checkout's `target/`: the only source, because no worker's
    /// sandbox can write it.
    MainCheckout,
    /// Journal rows written before lane-to-lane seeding was removed (a lane
    /// target is writable by its worker). Never produced.
    #[serde(other)]
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SeedOutcome {
    /// `debug/` was cloned (copy-on-write) from `source`.
    Cloned {
        source: PathBuf,
        kind: SeedSourceKind,
        clone_ms: u64,
        /// KT-1691 — what pruning the seed to its latest build removed before
        /// this clone (nothing when it had no stale artifact, `None` when it
        /// was not pruned).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pruned: Option<PruneReport>,
        /// KT-1742 — whether the seed was built with the lane's compiler
        /// environment before this clone (`aligned: true`), could not be
        /// (`false`, with why) or was not attempted (`skipped(<reason>)`). The
        /// first Rust root's; every root's is in `WarmStartRecord::crates`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aligned: Option<SeedAlignment>,
    },
    /// The lane target already had a `debug/`; left untouched.
    AlreadyPresent,
    /// No warm target is available to clone from.
    NoSource {
        reason: String,
    },
    /// A source exists but the file system cannot clone it: the lane starts
    /// empty (a full copy would cost the disk it is meant to save).
    Unsupported {
        reason: String,
    },
    NotApplicable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FirstBuild {
    Succeeded {
        duration_ms: u64,
    },
    Failed {
        duration_ms: u64,
        detail: String,
    },
    TimedOut {
        duration_ms: u64,
    },
    /// The execution was cancelled while the build queued or ran: the process
    /// was stopped and its gate slot freed.
    Cancelled {
        duration_ms: u64,
    },
    Skipped {
        reason: String,
    },
}

/// What starting a lane warm did, journalled on the execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarmStartRecord {
    /// SHA-256 of the worktree's `Cargo.lock`, when it has one.
    pub lock_hash: Option<String>,
    pub seed: SeedOutcome,
    pub build_jobs: u32,
    pub first_build: FirstBuild,
    /// How long the first build waited for a slot of the host's gate.
    pub queued_ms: u64,
    /// KT-1637 — how long provisioning the lane took (Cargo home, caches).
    /// `None` when the lane was not provisioned or the row predates the timing.
    #[serde(default)]
    pub provision_ms: Option<u64>,
    /// KT-1637 — how long seeding the lane target took
    /// (`SeedOutcome::Cloned::clone_ms` is the clone alone). `None` when no seed
    /// was attempted or the row predates the timing.
    #[serde(default)]
    pub seed_ms: Option<u64>,
    /// KT-1742 — what each Rust root of the repository (or the project's
    /// declared first-build command) did: where its seed was, whether it was
    /// aligned, how its build ended. `first_build` is their aggregate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crates: Vec<WarmedCrate>,
}

/// One Rust root (a workspace root or a standalone crate, found from the
/// manifests) or the declared first-build command, as warmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarmedCrate {
    /// The root's directory relative to the repository (`.` for the root
    /// itself), or `command` for a declared first-build command.
    pub path: String,
    /// Its seed: the target directory `cargo metadata` gave for the root in the
    /// main checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_target: Option<PathBuf>,
    pub seed_aligned: SeedAlignment,
    pub build: FirstBuild,
}

impl WarmStartRecord {
    /// The warming sub-steps this record timed (KT-1637), in the order they run.
    /// A step that did not run is absent, never reported as zero: a lane with no
    /// first build has no gate wait, a lane the warm-up never prepared has no seed.
    pub fn timed_steps(&self) -> Vec<(WarmingStepKind, u64)> {
        let mut steps = Vec::new();
        if let Some(provision_ms) = self.provision_ms {
            steps.push((WarmingStepKind::LaneProvision, provision_ms));
        }
        // Only an outcome that went looking at a warm target spent seed time; a
        // row journalled before `seed_ms` still knows its clone.
        let seed_ms = match &self.seed {
            SeedOutcome::Cloned { clone_ms, .. } => Some(self.seed_ms.unwrap_or(*clone_ms)),
            SeedOutcome::AlreadyPresent | SeedOutcome::Unsupported { .. } => self.seed_ms,
            SeedOutcome::NoSource { .. } | SeedOutcome::NotApplicable { .. } => None,
        };
        if let Some(seed_ms) = seed_ms {
            steps.push((WarmingStepKind::SeedClone, seed_ms));
        }
        // A build cancelled in the queue never started: it has no build time.
        let build_ms = match &self.first_build {
            FirstBuild::Succeeded { duration_ms }
            | FirstBuild::Failed { duration_ms, .. }
            | FirstBuild::TimedOut { duration_ms } => Some(*duration_ms),
            FirstBuild::Cancelled { duration_ms } => Some(*duration_ms).filter(|ms| *ms > 0),
            FirstBuild::Skipped { .. } => None,
        };
        // A build that was skipped after waiting for a slot (the queue did not
        // drain) still spent that wait.
        if build_ms.is_some() || self.queued_ms > 0 {
            steps.push((WarmingStepKind::GateWait, self.queued_ms));
        }
        if let Some(duration_ms) = build_ms {
            steps.push((WarmingStepKind::FirstBuild, duration_ms));
        }
        steps
    }

    fn not_applicable(reason: &str, build_jobs: u32) -> Self {
        Self {
            lock_hash: None,
            seed: SeedOutcome::NotApplicable {
                reason: reason.into(),
            },
            build_jobs,
            first_build: FirstBuild::Skipped {
                reason: reason.into(),
            },
            queued_ms: 0,
            provision_ms: None,
            seed_ms: None,
            crates: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WarmMarker {
    lock_hash: String,
    completed_unix: u64,
    /// The `CARGO_HOME` the build ran under. Cargo fingerprints a registry
    /// crate by the absolute path of its sources, so a target is only reusable
    /// under the same home: a clone made under another one recompiles every
    /// dependency (measured on this host: no reuse at all).
    #[serde(default)]
    cargo_home: Option<PathBuf>,
    /// KT-1742 — what each root did in this lane, per root: one root's success
    /// survives another's failure. Empty in a marker written before.
    #[serde(default)]
    roots: Vec<RootMark>,
}

/// Which Cargo home a lane's builds run under, for judging whether a warm
/// target can be reused by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeKey {
    /// The `CARGO_HOME` of the lane's environment; `None` leaves Cargo's host
    /// default, which is the home the main checkout's builds use.
    cargo_home: Option<PathBuf>,
    /// Whether the main checkout's `target/` was built under that same home.
    main_checkout_reusable: bool,
}

impl HomeKey {
    pub fn of(lane: &WorkerLane, provisioned: &Provisioned) -> Self {
        let private = provisioned.cargo_home_ready && provisioned.shared_cargo_home.is_none();
        Self {
            cargo_home: lane.effective_cargo_home(provisioned),
            main_checkout_reusable: !private,
        }
    }
}

// ─── Seeding ──────────────────────────────────────────────────────────────

/// Clones the tree at the first path to the (absent) second one, atomically
/// and copy-on-write, or fails. Injectable so the fallback is testable.
pub type CloneTree<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

/// APFS `clonefile(2)`: the whole tree in one call, no data copied.
#[cfg(target_os = "macos")]
pub fn clone_tree(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn clonefile(src: *const libc::c_char, dst: *const libc::c_char, flags: u32)
            -> libc::c_int;
    }
    let to_c = |path: &Path| {
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
    };
    let (source, destination) = (to_c(source)?, to_c(destination)?);
    // SAFETY: both pointers are valid NUL-terminated strings for the call.
    let status = unsafe { clonefile(source.as_ptr(), destination.as_ptr(), 0) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A reflink where the file system supports it (`cp --reflink=always` fails
/// rather than copying, which is exactly the fallback this needs).
#[cfg(all(unix, not(target_os = "macos")))]
pub fn clone_tree(source: &Path, destination: &Path) -> io::Result<()> {
    cp_reflink_tree(std::ffi::OsStr::new("cp"), source, destination)
}

/// [`clone_tree`] through the `cp` at `cp` (a test gives a stand-in).
#[cfg(any(all(unix, not(target_os = "macos")), all(unix, test)))]
fn cp_reflink_tree(cp: &std::ffi::OsStr, source: &Path, destination: &Path) -> io::Result<()> {
    // Like `clonefile(2)`: `cp` would copy into an existing directory.
    if destination.symlink_metadata().is_ok() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    let output = crate::core::cmd::sync_cmd(cp, crate::core::child_env::ChildRoute::Tool)
        .args(["-a", "--reflink=always", "--"])
        .arg(source)
        .arg(destination)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    // `cp` creates the directories before its first reflink fails: a failed
    // clone must leave nothing, as `clonefile(2)` does.
    let _ = std::fs::remove_dir_all(destination);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

#[cfg(not(unix))]
pub fn clone_tree(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copy-on-write cloning is not implemented on this platform",
    ))
}

#[derive(Debug, Clone)]
struct SeedCandidate {
    /// The target directory (holding `debug/`).
    target: PathBuf,
    kind: SeedSourceKind,
}

fn read_marker(target: &Path) -> Option<WarmMarker> {
    serde_json::from_slice(&std::fs::read(target.join(WARM_MARKER)).ok()?).ok()
}

/// The sources a lane may be seeded from: the main checkout's own targets (one
/// per Rust root, each resolved by `cargo metadata` in the MAIN checkout) that
/// hold a `debug/` built under a Cargo home this lane shares.
///
/// **Never another lane's target.** Each worker's sandbox may write its own
/// lane target (and the marker in it); Kronn then builds the seeded lane
/// OUTSIDE that sandbox. A worker that planted a proc-macro or a build artifact
/// with a newer mtime in its `debug/` (Cargo trusts mtimes, not contents) would
/// get it loaded by that build and run as the host user. A main checkout's
/// target is outside every worker's write roots (its worktree and its own lane
/// target), and the target is asked of Cargo in the main checkout, where only the
/// owner's `.cargo/config.toml` speaks: a worker's edit of its own worktree's
/// configuration cannot point the seed anywhere.
fn seed_candidates(lane: &WorkerLane, home: &HomeKey, targets: &[PathBuf]) -> Vec<SeedCandidate> {
    if lane.main_checkout.is_none() {
        return Vec::new();
    }
    let mut candidates: Vec<SeedCandidate> = Vec::new();
    for target in targets {
        let source = target.join(DEBUG_DIR);
        // Belt and braces for the layout: whatever the worker may write is never a source.
        let writable_by_the_worker = lane
            .write_roots()
            .iter()
            .any(|root| source.starts_with(root));
        let known = candidates
            .iter()
            .any(|candidate| candidate.target == *target);
        if writable_by_the_worker || !home.main_checkout_reusable || !source.is_dir() || known {
            continue;
        }
        candidates.push(SeedCandidate {
            target: target.clone(),
            kind: SeedSourceKind::MainCheckout,
        });
    }
    candidates
}

/// The package names the main checkout's `Cargo.lock` files list (one per Rust
/// root, wherever it is in the repository: a checkout may hold several
/// workspaces), or `None` when none could be read.
fn main_checkout_locked_packages(lane: &WorkerLane) -> Option<std::collections::HashSet<String>> {
    let main = lane.main_checkout.as_ref()?;
    let texts: Vec<String> = project_toolchains::rust_roots(main)
        .iter()
        .filter_map(|root| std::fs::read_to_string(root.manifest_dir.join("Cargo.lock")).ok())
        .collect();
    seed_prune::locked_package_names(texts.iter().map(String::as_str))
}

/// Merge a further seed's `debug/` into the lane's, entry by entry: what a
/// repository with several workspaces, each with its own target, needs, since
/// the lane has one `debug/`. Cargo names every unit's files by a hash, so two
/// seeds' entries do not collide; what the lane already holds is kept.
fn merge_seed(source: &Path, destination: &Path, clone: CloneTree<'_>) -> io::Result<usize> {
    let mut cloned = 0;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == seed_prune::CARGO_BUILD_LOCK {
            continue;
        }
        let target = destination.join(entry.file_name());
        if target.symlink_metadata().is_err() {
            clone(&entry.path(), &target)?;
            cloned += 1;
        } else if entry.file_type()?.is_dir() && target.is_dir() {
            for child in std::fs::read_dir(entry.path())? {
                let child = child?;
                let child_target = target.join(child.file_name());
                if child_target.symlink_metadata().is_err() {
                    clone(&child.path(), &child_target)?;
                    cloned += 1;
                }
            }
        }
    }
    Ok(cloned)
}

/// Seed `lane`'s `debug/` from the best warm source of `targets`. Never raises:
/// the outcome says what happened and an unseedable lane just starts empty.
fn seed_lane(
    lane: &WorkerLane,
    home: &HomeKey,
    targets: &[PathBuf],
    clone: CloneTree<'_>,
) -> SeedOutcome {
    let destination = lane.cargo_target_dir.join(DEBUG_DIR);
    if destination.exists() {
        return SeedOutcome::AlreadyPresent;
    }
    let candidates = seed_candidates(lane, home, targets);
    if candidates.is_empty() {
        let reason = if home.main_checkout_reusable {
            "the main checkout has no target/debug (lane targets are never a seed source)"
        } else {
            "the main checkout's target was built under the host's CARGO_HOME, which this \
             lane does not use (lane targets are never a seed source)"
        };
        return SeedOutcome::NoSource {
            reason: reason.into(),
        };
    }
    let staging = lane
        .cargo_target_dir
        .join(format!(".{DEBUG_DIR}-seeding-{}", std::process::id()));
    let mut skipped: Vec<String> = Vec::new();
    let mut unsupported: Option<String> = None;
    let mut seeded: Option<SeedOutcome> = None;
    for candidate in candidates {
        let source = candidate.target.join(DEBUG_DIR);
        // KT-1691 — prune the seed to its latest build first: Cargo never removes
        // an old artifact, and every lane would clone (then rewrite) all of it.
        // Cargo's own build lock is taken exclusively for that, so a seed a build
        // is writing is never pruned nor cloned; it is then held shared for the
        // clone, so a build that starts waits.
        let (lock, pruned) = match seed_prune::prune_and_lock_shared(
            &source,
            main_checkout_locked_packages(lane).as_ref(),
        ) {
            Ok((lock, report)) => (lock, Some(report)),
            Err(reason) => {
                skipped.push(format!("{}: {reason}", candidate.target.display()));
                continue;
            }
        };
        if let Some(report) = pruned.as_ref().filter(|report| report.removed_entries > 0) {
            tracing::info!(
                seed = %source.display(),
                units = report.removed_units,
                entries = report.removed_entries,
                bytes = report.removed_bytes,
                "pruned stale artifacts from the seed build dir before cloning it"
            );
        }
        if seeded.is_some() {
            // KT-1742 — a further workspace's target: its units join the lane's.
            let merged = merge_seed(&source, &destination, clone);
            let _ = FileExt::unlock(&lock);
            if let Err(error) = merged {
                tracing::warn!(
                    seed = %source.display(),
                    "a further seed target could not be merged into the lane: {error}"
                );
            }
            continue;
        }
        let _ = std::fs::remove_dir_all(&staging);
        let started = Instant::now();
        let cloned =
            clone(&source, &staging).and_then(|()| std::fs::rename(&staging, &destination));
        let _ = FileExt::unlock(&lock);
        match cloned {
            Ok(()) => {
                seeded = Some(SeedOutcome::Cloned {
                    source: candidate.target,
                    kind: candidate.kind,
                    clone_ms: started.elapsed().as_millis() as u64,
                    pruned,
                    aligned: None,
                });
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                unsupported = Some(format!(
                    "cannot clone {}: {error}",
                    candidate.target.display()
                ));
                // The file system cannot clone: no other source will do better.
                break;
            }
        }
    }
    if let Some(seeded) = seeded {
        return seeded;
    }
    match unsupported {
        Some(reason) => SeedOutcome::Unsupported { reason },
        None => SeedOutcome::NoSource {
            reason: format!("no usable source: {}", skipped.join("; ")),
        },
    }
}

// ─── Warm start ───────────────────────────────────────────────────────────

/// Writes the lane's marker: `lock_hash` is the aggregate key (what a marker
/// always carried), `roots` what each root did.
fn write_marker(
    lane: &WorkerLane,
    lock_hash: &str,
    home: &HomeKey,
    roots: Vec<RootMark>,
) -> io::Result<()> {
    let marker = WarmMarker {
        lock_hash: lock_hash.to_string(),
        cargo_home: home.cargo_home.clone(),
        roots,
        completed_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
    };
    let path = lane.cargo_target_dir.join(WARM_MARKER);
    let staging = lane.cargo_target_dir.join(format!("{WARM_MARKER}.tmp"));
    std::fs::write(
        &staging,
        serde_json::to_vec(&marker).map_err(io::Error::other)?,
    )?;
    std::fs::rename(&staging, path)
}

/// What a warm-up needs besides the lane itself: the project's settings, the way
/// to ask Cargo where a crate builds, and the runner of the seed's own build.
/// Everything here is injectable, so the warm-up is testable without Cargo.
pub struct WarmHost<'a> {
    /// Build each Rust root's seed with `compiler_env` before cloning it.
    pub seed_alignment: bool,
    /// The lane's C compiler environment (the owner's settings, or detected).
    pub compiler_env: Vec<(String, String)>,
    /// A shell command that replaces the detected first build. It runs only
    /// under the lane's OS sandbox ([`ProbeRunner::confined`]): the worktree it
    /// runs in holds files the worker wrote.
    pub first_build_command: Option<String>,
    /// The Rust roots to warm (relative directories); `None` = every detected one.
    pub rust_roots: Option<Vec<String>>,
    /// Resolves a crate directory of the MAIN checkout to its target directory.
    pub metadata: project_toolchains::MetadataResolver<'a>,
    /// Runs the seed's alignment build. The seed is the owner's own target,
    /// which no worker's sandbox may write: this runner must not be confined.
    pub seed_runner: &'a dyn ProbeRunner,
    /// How long that build waits for Cargo's lock on the seed.
    pub lock_wait: Duration,
}

#[derive(Debug, Clone, Copy)]
enum SlotFailure {
    Cancelled,
    TimedOut,
}

/// The lane's place in the host's build gate, taken at most once and only when
/// something is about to build (the seed's alignment, or the first build).
struct Slot<'a, F: FnOnce(usize)> {
    gate: &'a BuildGate,
    limit: usize,
    max_wait: Duration,
    cancel: &'a CancellationToken,
    on_wait: Option<F>,
    permit: Option<BuildPermit<'a>>,
    failure: Option<SlotFailure>,
    queued_ms: u64,
    held_since: Option<Instant>,
}

impl<'a, F: FnOnce(usize)> Slot<'a, F> {
    fn take(&mut self) -> Result<(), SlotFailure> {
        if self.permit.is_some() {
            return Ok(());
        }
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let queued = Instant::now();
        let mut waited = false;
        let on_wait = self.on_wait.take();
        let acquired = self
            .gate
            .acquire_within(self.limit, self.max_wait, self.cancel, |ahead| {
                waited = true;
                if let Some(on_wait) = on_wait {
                    on_wait(ahead);
                }
            });
        let waited_ms = queued.elapsed().as_millis() as u64;
        match acquired {
            GateWait::Acquired(permit) => {
                self.permit = Some(permit);
                self.held_since = Some(Instant::now());
                self.queued_ms = if waited { waited_ms } else { 0 };
                Ok(())
            }
            GateWait::Cancelled => {
                self.queued_ms = waited_ms;
                self.failure = Some(SlotFailure::Cancelled);
                Err(SlotFailure::Cancelled)
            }
            GateWait::TimedOut => {
                self.queued_ms = waited_ms;
                self.failure = Some(SlotFailure::TimedOut);
                Err(SlotFailure::TimedOut)
            }
        }
    }
}

/// One Rust root of the worktree while it is warmed.
struct CrateState {
    root: RustRoot,
    seed_target: Option<PathBuf>,
    /// Why the root has no seed target (Cargo could not say where it builds).
    unresolved: Option<String>,
    alignment: SeedAlignment,
}

impl CrateState {
    fn new(root: &RustRoot, reason: &str) -> Self {
        Self {
            root: root.clone(),
            seed_target: None,
            unresolved: None,
            alignment: SeedAlignment::skipped(reason),
        }
    }

    fn warmed(&self, build: FirstBuild) -> WarmedCrate {
        WarmedCrate {
            path: self.root.label(),
            seed_target: self.seed_target.clone(),
            seed_aligned: self.alignment.clone(),
            build,
        }
    }
}

/// The marker's key: the roots' lock hash; with a declared first-build command,
/// a hash over the command and the locks, so changing either re-warms the lane.
fn warm_key(roots: &[RustRoot], command: Option<&str>) -> Option<String> {
    let locks = project_toolchains::combined_lock_hash(roots);
    match command {
        None => locks,
        Some(command) => Some(project_toolchains::text_hash(&format!(
            "{command}\n{}",
            locks.unwrap_or_default()
        ))),
    }
}

/// One thing the warm-up builds, and the key its outcome is remembered under: a
/// Rust root, or the project's declared command.
#[derive(Debug, Clone)]
struct Unit {
    label: String,
    key: String,
}

/// The units of a warm-up, in the order of `roots` (or the single command unit
/// when a command replaces the roots' builds).
fn warm_units(
    roots: &[RustRoot],
    command: Option<&str>,
    aggregate: Option<&str>,
    compiler_env: &[(String, String)],
) -> Vec<Unit> {
    if command.is_some() {
        return vec![Unit {
            label: "command".into(),
            key: task_worker_root_marks::root_key("command", aggregate, compiler_env),
        }];
    }
    roots
        .iter()
        .map(|root| Unit {
            label: root.label(),
            key: task_worker_root_marks::root_key(
                &root.label(),
                root.lock_hash().as_deref(),
                compiler_env,
            ),
        })
        .collect()
}

/// The lane's marker, when what it vouches for is still there: a Cargo lane
/// needs its `debug/`.
fn usable_marker(lane: &WorkerLane, no_cargo_roots: bool) -> Option<WarmMarker> {
    if !no_cargo_roots && !lane.cargo_target_dir.join(DEBUG_DIR).is_dir() {
        return None;
    }
    read_marker(&lane.cargo_target_dir)
}

/// What the lane's marker says `unit` did, for its key. A marker written before
/// per-root entries (none) vouches for the whole lane at its aggregate key.
fn marked(marker: Option<&WarmMarker>, unit: &Unit, aggregate: Option<&str>) -> Option<RootState> {
    let marker = marker?;
    if marker.roots.is_empty() {
        return (Some(marker.lock_hash.as_str()) == aggregate).then_some(RootState::Built);
    }
    marker
        .roots
        .iter()
        .find(|mark| mark.path == unit.label && mark.key == unit.key)
        .map(|mark| mark.state.clone())
}

/// The environment of a lane's own builds: the lane's, with the compiler
/// environment the seed was aligned to, so the two agree by construction.
fn lane_build_env(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    host: &WarmHost<'_>,
) -> Vec<(String, OsString)> {
    let mut env: Vec<(String, OsString)> = lane
        .env(provisioned)
        .into_iter()
        .filter(|(name, _)| {
            !host
                .compiler_env
                .iter()
                .any(|(own, _)| own.as_str() == *name)
        })
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    env.extend(
        host.compiler_env
            .iter()
            .map(|(name, value)| (name.clone(), OsString::from(value))),
    );
    env
}

/// The environment of a seed's alignment build: what changes a build script's
/// fingerprint (the compiler environment, the Cargo home) and what keeps it
/// offline and as light as a lane's, aimed at the seed's own target. Nothing of
/// the lane's ports, data directory or tooling.
fn seed_build_env(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    host: &WarmHost<'_>,
    target: &Path,
) -> Vec<(String, OsString)> {
    const KEPT: [&str; 3] = ["CARGO_HOME", "CARGO_NET_OFFLINE", CARGO_BUILD_JOBS_NAME];
    let mut env: Vec<(String, OsString)> = lane
        .env(provisioned)
        .into_iter()
        .filter(|(name, _)| KEPT.contains(name))
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    env.extend(
        host.compiler_env
            .iter()
            .map(|(name, value)| (name.clone(), OsString::from(value))),
    );
    env.push((
        "CARGO_TARGET_DIR".to_string(),
        target.as_os_str().to_os_string(),
    ));
    env
}

const CARGO_BUILD_JOBS_NAME: &str = "CARGO_BUILD_JOBS";

#[allow(clippy::too_many_arguments)]
fn record_of(
    lock_hash: Option<String>,
    seed: SeedOutcome,
    jobs: u32,
    first_build: FirstBuild,
    queued_ms: u64,
    seed_ms: Option<u64>,
    crates: Vec<WarmedCrate>,
) -> WarmStartRecord {
    WarmStartRecord {
        lock_hash,
        seed,
        build_jobs: jobs,
        first_build,
        queued_ms,
        provision_ms: None,
        seed_ms,
        crates,
    }
}

fn classify(
    outcome: crate::core::task_worker_env::ProbeOutcome,
    started: Instant,
    cancel: &CancellationToken,
) -> FirstBuild {
    let duration_ms = started.elapsed().as_millis() as u64;
    if !outcome.success && cancel.is_cancelled() {
        FirstBuild::Cancelled { duration_ms }
    } else if outcome.success {
        FirstBuild::Succeeded { duration_ms }
    } else if outcome.detail.starts_with(TIMED_OUT_DETAIL_PREFIX) {
        FirstBuild::TimedOut { duration_ms }
    } else {
        FirstBuild::Failed {
            duration_ms,
            detail: outcome.detail,
        }
    }
}

/// Seed the lane target, then run its first build under the host gate.
///
/// `on_wait` is called once, with the number of builds ahead, when the gate is
/// full — before blocking. A build that fails or times out is recorded and
/// leaves no marker; the worker is launched regardless.
#[allow(clippy::too_many_arguments)]
pub fn warm_lane(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    settings: &WarmStartSettings,
    host: &WarmHost<'_>,
    gate: &BuildGate,
    runner: &dyn ProbeRunner,
    clone: CloneTree<'_>,
    on_wait: impl FnOnce(usize),
) -> WarmStartRecord {
    warm_lane_cancellable(
        lane,
        provisioned,
        settings,
        host,
        gate,
        runner,
        clone,
        &CancellationToken::new(),
        on_wait,
    )
}

/// [`warm_lane`] that stops when `cancel` fires: a queued first build leaves
/// the gate without taking a slot, a running one is killed (the runner owns
/// that) and its slot is freed. Nothing is marked warm.
///
/// The steps, for a repository that is not Kronn's: every Rust workspace root
/// found from the manifests is a unit of warm-up (`cargo metadata` says where its
/// seed is); its seed is built once with the lane's compiler environment when it
/// was not ([`task_worker_seed_align`]); the lane is cloned from the seeds; each
/// root is built (or the project's declared command is run), under one time
/// bound. A project with no Rust root and no declared command is not warmed, and
/// the record says why.
#[allow(clippy::too_many_arguments)]
pub fn warm_lane_cancellable(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    settings: &WarmStartSettings,
    host: &WarmHost<'_>,
    gate: &BuildGate,
    runner: &dyn ProbeRunner,
    clone: CloneTree<'_>,
    cancel: &CancellationToken,
    on_wait: impl FnOnce(usize),
) -> WarmStartRecord {
    let jobs = settings.build_jobs;
    let home = HomeKey::of(lane, provisioned);
    let (roots, unknown_roots) = project_toolchains::select_roots(
        project_toolchains::rust_roots(&lane.worktree),
        host.rust_roots.as_deref(),
    );
    let command = host.first_build_command.clone();
    if roots.is_empty() && command.is_none() {
        let reason = if host.rust_roots.is_some() {
            format!(
                "the project's Rust roots setting names no Rust workspace of the repository \
                 ({}): nothing to warm",
                unknown_roots.join(", ")
            )
        } else {
            "no Cargo.toml in the repository and no first-build command declared for this \
             project: nothing to warm"
                .to_string()
        };
        return WarmStartRecord::not_applicable(&reason, jobs);
    }
    let cargo_lock = project_toolchains::combined_lock_hash(&roots);
    let lock_hash = warm_key(&roots, command.as_deref());
    if let Err(error) = std::fs::create_dir_all(&lane.cargo_target_dir) {
        return WarmStartRecord::not_applicable(
            &format!(
                "the lane target {} cannot be created: {error}",
                lane.cargo_target_dir.display()
            ),
            jobs,
        );
    }
    if cancel.is_cancelled() {
        return record_of(
            lock_hash,
            SeedOutcome::NotApplicable {
                reason: "cancelled before the lane was seeded".into(),
            },
            jobs,
            FirstBuild::Cancelled { duration_ms: 0 },
            0,
            None,
            Vec::new(),
        );
    }
    let mut slot = Slot {
        gate,
        limit: settings.max_concurrent_builds,
        max_wait: settings.first_build_timeout,
        cancel,
        on_wait: Some(on_wait),
        permit: None,
        failure: None,
        queued_ms: 0,
        held_since: None,
    };

    let seeding = Instant::now();
    let destination_present = lane.cargo_target_dir.join(DEBUG_DIR).exists();
    let mut states: Vec<CrateState> = Vec::new();
    let mut seed;
    if roots.is_empty() {
        seed = SeedOutcome::NotApplicable {
            reason: "the project has no Cargo workspace: there is no target to seed".into(),
        };
    } else if destination_present {
        states.extend(
            roots
                .iter()
                .map(|root| CrateState::new(root, "the lane target already has a debug/")),
        );
        seed = SeedOutcome::AlreadyPresent;
    } else if cargo_lock.is_none() {
        states.extend(roots.iter().map(|root| {
            CrateState::new(
                root,
                "the worktree has no Cargo.lock to match a warm target",
            )
        }));
        seed = SeedOutcome::NoSource {
            reason: "the worktree has no Cargo.lock to match a warm target against".into(),
        };
    } else {
        states.extend(roots.iter().map(|root| CrateState::new(root, "")));
        // Where each root builds in the MAIN checkout: asked of Cargo there.
        for state in &mut states {
            match lane.main_checkout.as_ref() {
                None => {
                    state.unresolved =
                        Some("the worktree is not a managed one: there is no main checkout".into());
                }
                Some(main) => match (host.metadata)(&state.root.in_checkout(main)) {
                    Ok(CargoMetadata {
                        target_directory, ..
                    }) => state.seed_target = Some(target_directory),
                    Err(error) => {
                        state.unresolved = Some(format!("{}: {error}", state.root.label()));
                    }
                },
            }
        }
        // Align every seed with the lane's compiler environment, when due.
        let skip_reason = if !host.seed_alignment {
            Some("seed alignment is off for this project")
        } else if host.compiler_env.is_empty() {
            Some("the lane sets no compiler environment: nothing to align")
        } else if !home.main_checkout_reusable {
            Some("the lane does not use the host's Cargo home: the seed is not reusable")
        } else {
            None
        };
        for state in &mut states {
            let Some(target) = state.seed_target.clone() else {
                let why = state
                    .unresolved
                    .clone()
                    .unwrap_or_else(|| "no seed target".into());
                state.alignment = SeedAlignment::skipped(format!("no seed: {why}"));
                continue;
            };
            if let Some(reason) = skip_reason {
                state.alignment = SeedAlignment::skipped(reason);
                continue;
            }
            let Some(main) = lane.main_checkout.as_ref() else {
                continue;
            };
            let main_dir = state.root.in_checkout(main);
            let request = AlignRequest {
                main_crate_dir: &main_dir,
                target: &target,
                lock_hash: project_toolchains::file_hash(&main_dir.join("Cargo.lock")),
                compiler_env: &host.compiler_env,
                build_env: seed_build_env(lane, provisioned, host, &target),
                timeout: settings.first_build_timeout / 2,
                lock_wait: host.lock_wait,
            };
            state.alignment = match task_worker_seed_align::assess(&request) {
                Assessment::Settled(alignment) => alignment,
                Assessment::NeedsBuild(stale) => match slot.take() {
                    Ok(()) => task_worker_seed_align::build_aligned(
                        &request,
                        &stale,
                        host.seed_runner,
                        cancel,
                    ),
                    Err(SlotFailure::Cancelled) => {
                        return record_of(
                            lock_hash,
                            SeedOutcome::NotApplicable {
                                reason: "cancelled while the seed was aligned".into(),
                            },
                            jobs,
                            FirstBuild::Cancelled { duration_ms: 0 },
                            slot.queued_ms,
                            Some(seeding.elapsed().as_millis() as u64),
                            Vec::new(),
                        );
                    }
                    Err(SlotFailure::TimedOut) => SeedAlignment::NotAligned {
                        reason: format!(
                            "no build slot freed within {}s: the seed is cloned as it is",
                            settings.first_build_timeout.as_secs()
                        ),
                    },
                },
            };
        }
        let targets: Vec<PathBuf> = states
            .iter()
            .filter_map(|state| state.seed_target.clone())
            .collect();
        seed = if targets.is_empty() {
            let unresolved: Vec<String> = states
                .iter()
                .filter_map(|state| state.unresolved.clone())
                .collect();
            SeedOutcome::NoSource {
                reason: format!(
                    "no seed target could be resolved: {}",
                    unresolved.join("; ")
                ),
            }
        } else {
            seed_lane(lane, &home, &targets, clone)
        };
        if let SeedOutcome::Cloned {
            source, aligned, ..
        } = &mut seed
        {
            *aligned = states
                .iter()
                .find(|state| state.seed_target.as_deref() == Some(source.as_path()))
                .map(|state| state.alignment.clone());
        }
    }
    let seed_ms = seeding.elapsed().as_millis() as u64;
    let unknown_crates = || -> Vec<WarmedCrate> {
        unknown_roots
            .iter()
            .map(|path| WarmedCrate {
                path: path.clone(),
                seed_target: None,
                seed_aligned: SeedAlignment::skipped("not a Rust root of the repository"),
                build: FirstBuild::Skipped {
                    reason: "listed in the project's Rust roots setting, but there is no \
                             Cargo.toml there"
                        .into(),
                },
            })
            .collect()
    };
    let skipped_crates = |states: &[CrateState], reason: &str| -> Vec<WarmedCrate> {
        let mut crates: Vec<WarmedCrate> = states
            .iter()
            .map(|state| {
                state.warmed(FirstBuild::Skipped {
                    reason: reason.to_string(),
                })
            })
            .collect();
        crates.extend(unknown_crates());
        crates
    };

    // `--locked` cannot build without a lock file, and a build with no lock hash
    // could not be marked warm: nothing to run, nothing to wait for.
    if cargo_lock.is_none() && command.is_none() {
        let reason = "the worktree has no Cargo.lock: a locked first build is not possible";
        return record_of(
            lock_hash,
            seed,
            jobs,
            FirstBuild::Skipped {
                reason: reason.into(),
            },
            0,
            Some(seed_ms),
            skipped_crates(&states, reason),
        );
    }

    // What each unit (a Rust root, or the declared command) already is. Per unit,
    // so one root's success survives another's failure: it was built or failed in
    // this lane (the lane's marker), or failed before for this very key in another
    // lane of the project (the failure cache beside the seed). Neither is run again.
    let units = warm_units(
        &roots,
        command.as_deref(),
        lock_hash.as_deref(),
        &host.compiler_env,
    );
    let marker = usable_marker(lane, roots.is_empty());
    let mut marks: Vec<RootMark> = marker
        .as_ref()
        .map(|marker| marker.roots.clone())
        .unwrap_or_default();
    let mut marks_changed = false;
    let mut known: Vec<Option<WarmedCrate>> = vec![None; states.len()];
    let mut pending: Vec<usize> = Vec::new();
    let mut command_pending = false;
    let mut failed_before = 0usize;
    if command.is_some() {
        match marked(marker.as_ref(), &units[0], lock_hash.as_deref()) {
            None => command_pending = true,
            Some(RootState::Built) => {}
            Some(RootState::Failed { .. }) => failed_before += 1,
        }
    } else {
        for (index, state) in states.iter().enumerate() {
            let unit = &units[index];
            let prior = marked(marker.as_ref(), unit, lock_hash.as_deref()).or_else(|| {
                let seed = state.seed_target.as_deref()?;
                let reason = task_worker_root_marks::cached_failure(seed, &unit.label, &unit.key)?;
                task_worker_root_marks::upsert(
                    &mut marks,
                    RootMark::new(
                        &unit.label,
                        &unit.key,
                        RootState::Failed {
                            reason: reason.clone(),
                        },
                    ),
                );
                marks_changed = true;
                Some(RootState::Failed { reason })
            });
            match prior {
                None => pending.push(index),
                Some(RootState::Built) => {
                    known[index] = Some(state.warmed(FirstBuild::Skipped {
                        reason: "already built for this Cargo.lock and compiler environment".into(),
                    }));
                }
                Some(RootState::Failed { reason }) => {
                    failed_before += 1;
                    known[index] = Some(state.warmed(FirstBuild::Failed {
                        duration_ms: 0,
                        detail: format!(
                            "not retried: it failed before for this Cargo.lock and compiler \
                             environment: {reason}"
                        ),
                    }));
                }
            }
        }
    }
    if pending.is_empty() && !command_pending {
        if marks_changed {
            if let Err(error) = write_marker(lane, lock_hash.as_deref().unwrap_or(""), &home, marks)
            {
                tracing::warn!(lane = %lane.name, "warm marker not written: {error}");
            }
        }
        let reason = if failed_before == 0 {
            "the lane target was already built for this Cargo.lock".to_string()
        } else {
            format!(
                "nothing left to build: {failed_before} root(s) failed before for this \
                 Cargo.lock and compiler environment and are not retried"
            )
        };
        let mut crates: Vec<WarmedCrate> = known.into_iter().flatten().collect();
        if command.is_some() {
            crates = skipped_crates(
                &states,
                "replaced by the project's declared first-build command",
            );
            crates.push(WarmedCrate {
                path: "command".into(),
                seed_target: None,
                seed_aligned: SeedAlignment::skipped("a declared command builds no seed"),
                build: FirstBuild::Skipped {
                    reason: reason.clone(),
                },
            });
        } else {
            crates.extend(unknown_crates());
        }
        return record_of(
            lock_hash,
            seed,
            jobs,
            FirstBuild::Skipped { reason },
            0,
            Some(seed_ms),
            crates,
        );
    }

    // The declared command runs the worktree's own scripts (package.json, a
    // Makefile, composer scripts), and a resumed lane's worktree holds what the
    // worker wrote: never with the host's rights. A runner that does not confine
    // its commands to the lane's OS sandbox refuses it, and the record says so.
    if command_pending && !runner.confined() {
        let detail = "the declared first-build command was not run: it executes scripts of \
                      the worktree, which the worker writes, so it only runs under the lane's \
                      OS sandbox, and this runner does not confine its commands";
        return record_of(
            lock_hash,
            seed,
            jobs,
            FirstBuild::Failed {
                duration_ms: 0,
                detail: detail.into(),
            },
            0,
            Some(seed_ms),
            {
                let mut crates = skipped_crates(
                    &states,
                    "replaced by the project's declared first-build command",
                );
                crates.push(WarmedCrate {
                    path: "command".into(),
                    seed_target: None,
                    seed_aligned: SeedAlignment::skipped("a declared command builds no seed"),
                    build: FirstBuild::Failed {
                        duration_ms: 0,
                        detail: detail.into(),
                    },
                });
                crates
            },
        );
    }

    if let Err(failure) = slot.take() {
        return match failure {
            SlotFailure::Cancelled => record_of(
                lock_hash,
                seed,
                jobs,
                FirstBuild::Cancelled { duration_ms: 0 },
                slot.queued_ms,
                Some(seed_ms),
                Vec::new(),
            ),
            // Never stall a worker behind a queue that does not drain: it starts
            // with the target it has, as before KT-1250.
            SlotFailure::TimedOut => {
                let reason = format!(
                    "no build slot freed within {}s: the worker starts with the target it has",
                    settings.first_build_timeout.as_secs()
                );
                record_of(
                    lock_hash,
                    seed,
                    jobs,
                    FirstBuild::Skipped {
                        reason: reason.clone(),
                    },
                    slot.queued_ms,
                    Some(seed_ms),
                    skipped_crates(&states, &reason),
                )
            }
        };
    }
    let queued_ms = slot.queued_ms;
    let deadline = slot.held_since.unwrap_or_else(Instant::now) + settings.first_build_timeout;
    let env = lane_build_env(lane, provisioned, host);

    // What to run: the declared command, or one `cargo test` per pending Rust root.
    let mut attempted: Vec<(String, FirstBuild)> = Vec::new();
    let mut run = |label: &str, probe: ProbeCommand| -> bool {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let started = Instant::now();
        let build = if remaining.is_zero() {
            FirstBuild::TimedOut { duration_ms: 0 }
        } else {
            let outcome = runner.run_cancellable(
                &ProbeCommand {
                    timeout: remaining,
                    ..probe
                },
                cancel,
            );
            classify(outcome, started, cancel)
        };
        let stop = matches!(
            build,
            FirstBuild::TimedOut { .. } | FirstBuild::Cancelled { .. }
        );
        attempted.push((label.to_string(), build));
        stop
    };
    if let Some(command) = &command {
        let (program, args) = shell_for(command);
        let probe = ProbeCommand {
            program,
            args,
            cwd: lane.worktree.clone(),
            env: env.clone(),
            timeout: settings.first_build_timeout,
            priority: Priority::Background,
            clean_env: false,
        };
        run("command", probe);
    } else {
        for &index in &pending {
            let state = &states[index];
            let probe = ProbeCommand {
                program: "cargo".into(),
                args: first_build_args(&state.root.manifest_dir),
                cwd: state.root.manifest_dir.clone(),
                env: env.clone(),
                timeout: settings.first_build_timeout,
                // The lane's first build compiles the whole dependency graph.
                priority: Priority::Background,
                clean_env: false,
            };
            if run(&state.root.label(), probe) {
                break;
            }
        }
    }
    drop(slot);

    // What each attempted unit taught: a success is marked built (and forgets an
    // old failure); a failure is marked and cached for its key; a timeout and a
    // cancellation say nothing about the root and leave no trace.
    let mut warmed: Vec<WarmedCrate> = Vec::new();
    for (label, build) in &attempted {
        let unit = units.iter().find(|unit| &unit.label == label);
        let seed_dir = states
            .iter()
            .find(|state| &state.root.label() == label)
            .and_then(|state| state.seed_target.clone());
        let learned = match build {
            FirstBuild::Succeeded { .. } => Some(RootState::Built),
            // A build that never started (no sandbox on this host, no `cargo`)
            // says nothing about the root: it is tried again once that is fixed.
            FirstBuild::Failed { detail, .. }
                if !NOT_STARTED_PREFIXES.iter().any(|p| detail.starts_with(p)) =>
            {
                Some(RootState::Failed {
                    reason: detail.clone(),
                })
            }
            _ => None,
        };
        if let (Some(unit), Some(state)) = (unit, learned) {
            match (&state, &seed_dir) {
                (RootState::Failed { reason }, Some(seed)) => {
                    task_worker_root_marks::remember_failure(seed, label, &unit.key, reason);
                }
                (RootState::Built, Some(seed)) => {
                    task_worker_root_marks::forget_failure(seed, label)
                }
                _ => {}
            }
            task_worker_root_marks::upsert(&mut marks, RootMark::new(label, &unit.key, state));
            marks_changed = true;
        }
        if command.is_none() {
            if let Some(index) = states.iter().position(|state| &state.root.label() == label) {
                known[index] = Some(states[index].warmed(build.clone()));
            }
        }
    }
    if command.is_some() {
        // The declared command replaced the roots' builds; their seeds still count.
        warmed = skipped_crates(
            &states,
            "replaced by the project's declared first-build command",
        );
        for (label, build) in &attempted {
            warmed.push(WarmedCrate {
                path: label.clone(),
                seed_target: None,
                seed_aligned: SeedAlignment::skipped("a declared command builds no seed"),
                build: build.clone(),
            });
        }
    } else {
        // A root nothing attempted (the bound ran out first) is reported, not hidden.
        for (index, entry) in known.iter_mut().enumerate() {
            if entry.is_none() {
                *entry = Some(states[index].warmed(FirstBuild::Skipped {
                    reason: "not reached: the first build stopped before it".into(),
                }));
            }
        }
        warmed.extend(known.into_iter().flatten());
        warmed.extend(unknown_crates());
    }

    // The aggregate: the first attempted build that did not succeed, else the
    // total. A root that failed before and was not retried is in its own entry.
    let total_ms: u64 = attempted
        .iter()
        .map(|(_, build)| match build {
            FirstBuild::Succeeded { duration_ms }
            | FirstBuild::Failed { duration_ms, .. }
            | FirstBuild::TimedOut { duration_ms }
            | FirstBuild::Cancelled { duration_ms } => *duration_ms,
            FirstBuild::Skipped { .. } => 0,
        })
        .sum();
    let first_build = attempted
        .iter()
        .find(|(_, build)| !matches!(build, FirstBuild::Succeeded { .. }))
        .map(|(label, build)| match build {
            // Which root, when the repository has several.
            FirstBuild::Failed {
                duration_ms,
                detail,
            } if roots.len() > 1 && command.is_none() => FirstBuild::Failed {
                duration_ms: *duration_ms,
                detail: format!("{label}: {detail}"),
            },
            other => other.clone(),
        })
        .unwrap_or(FirstBuild::Succeeded {
            duration_ms: total_ms,
        });
    if marks_changed {
        if let Some(hash) = &lock_hash {
            if let Err(error) = write_marker(lane, hash, &home, marks) {
                tracing::warn!(lane = %lane.name, "warm marker not written: {error}");
            }
        }
    }
    record_of(
        lock_hash,
        seed,
        jobs,
        first_build,
        queued_ms,
        Some(seed_ms),
        warmed,
    )
}

/// The program and arguments that run a declared command through the host's shell.
fn shell_for(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd".into(), vec!["/C".into(), command.into()])
    } else {
        ("sh".into(), vec!["-c".into(), command.into()])
    }
}

/// KT-1736 — the arguments of the lane's first build in `manifest_dir`.
///
/// A worker's first command is a targeted `cargo test --lib <filter>`, so the
/// warm-up builds what that needs: the dependency graph (dev profile, which the
/// test profile inherits) and the crate's own **lib test binary**. It no longer
/// builds every `tests/*.rs` integration binary: each one links the whole crate
/// again, and on a loaded host that did not fit the bound (the warm-ups of
/// 2026-11-12 ended `timed_out`). The integration binaries are built by the
/// validations that run them.
///
/// A manifest with no library target has no lib test binary to name (`--lib`
/// would be an error), so it keeps the whole-package build.
fn first_build_args(manifest_dir: &Path) -> Vec<String> {
    let mut args = vec!["test".to_string()];
    if project_toolchains::has_lib_target(manifest_dir) {
        args.push("--lib".into());
    }
    args.extend(["--no-run", "--locked", "--offline"].map(String::from));
    args
}

/// Will warming this worktree's lane run a first build? Side-effect free: the
/// launch uses it to decide whether the worker's start has to wait for one (and
/// is therefore handed to a background task) or can go on at once.
pub fn first_build_pending(work_dir: &Path) -> bool {
    let Ok(worktree) = work_dir.canonicalize() else {
        return false;
    };
    let Ok(lane) = WorkerLane::for_worktree(&worktree) else {
        return false;
    };
    let effective =
        task_worker_lane_settings::effective_for(&lane.worktree, lane.main_checkout.as_deref());
    let (roots, _) = project_toolchains::select_roots(
        project_toolchains::rust_roots(&lane.worktree),
        effective.rust_roots.as_deref(),
    );
    let command = effective.first_build_command.as_deref();
    let Some(hash) = warm_key(&roots, command) else {
        return false;
    };
    let units = warm_units(&roots, command, Some(&hash), &effective.compiler_env);
    let marker = usable_marker(&lane, roots.is_empty());
    // A root that failed before in another lane is only known once the seed's
    // target is resolved (a `cargo metadata` run): it counts as pending here, and
    // the warm-up itself then skips it at once.
    units
        .iter()
        .any(|unit| marked(marker.as_ref(), unit, Some(&hash)).is_none())
}

/// Provision the lane of `work_dir`, then [`warm_lane`] it with the host's
/// settings and gate. Never fails: a lane that cannot be prepared is recorded
/// as not applicable, with the reason.
pub fn warm_worktree(
    work_dir: &Path,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
    on_wait: impl FnOnce(usize),
) -> WarmStartRecord {
    let provisioning = Instant::now();
    match prepare_launch(work_dir) {
        Ok((lane, provisioned)) => {
            let provision_ms = provisioning.elapsed().as_millis() as u64;
            let effective = task_worker_lane_settings::effective_for(
                &lane.worktree,
                lane.main_checkout.as_deref(),
            );
            let settings = WarmStartSettings::from_host_with_timeout(effective.first_build_timeout);
            let host = WarmHost {
                seed_alignment: effective.seed_alignment,
                compiler_env: effective.compiler_env,
                first_build_command: effective.first_build_command,
                rust_roots: effective.rust_roots,
                metadata: &project_toolchains::host_cargo_metadata,
                seed_runner: &ProcessProbeRunner,
                lock_wait: task_worker_seed_align::DEFAULT_LOCK_WAIT,
            };
            WarmStartRecord {
                provision_ms: Some(provision_ms),
                ..warm_lane_cancellable(
                    &lane,
                    &provisioned,
                    &settings,
                    &host,
                    host_build_gate(),
                    runner,
                    &clone_tree,
                    cancel,
                    on_wait,
                )
            }
        }
        Err(error) => WarmStartRecord::not_applicable(
            &format!("the worker lane could not be prepared: {error}"),
            WarmStartSettings::from_host().build_jobs,
        ),
    }
}

#[cfg(test)]
#[path = "task_worker_lane_warm_test.rs"]
mod task_worker_lane_warm_test;
