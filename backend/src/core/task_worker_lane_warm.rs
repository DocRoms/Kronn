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
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::core::task_worker_env::{
    prepare_launch, ProbeCommand, ProbeRunner, Provisioned, WorkerLane,
};

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
const DEFAULT_FIRST_BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Written in a lane's target at the end of a successful first build.
const WARM_MARKER: &str = ".kronn-warm.json";
const DEBUG_DIR: &str = "debug";
/// Cargo's own advisory lock on a profile directory, held for a build's whole
/// duration (`<target>/debug/.cargo-lock`).
const CARGO_BUILD_LOCK: &str = ".cargo-lock";
const TIMED_OUT_DETAIL_PREFIX: &str = "timed out after";
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
}

impl WarmStartRecord {
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
    let output = crate::core::cmd::sync_cmd("cp", crate::core::child_env::ChildRoute::Tool)
        .args(["-a", "--reflink=always", "--"])
        .arg(source)
        .arg(destination)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
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

/// The `Cargo.lock` hash of the worktree's crate and the crate directory.
fn manifest_and_lock_hash(lane: &WorkerLane) -> Option<(PathBuf, Option<String>)> {
    let manifest_dir = [lane.worktree.clone(), lane.worktree.join("backend")]
        .into_iter()
        .find(|dir| dir.join("Cargo.toml").is_file())?;
    let hash = std::fs::read(manifest_dir.join("Cargo.lock"))
        .ok()
        .map(|bytes| {
            Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        });
    Some((manifest_dir, hash))
}

fn read_marker(target: &Path) -> Option<WarmMarker> {
    serde_json::from_slice(&std::fs::read(target.join(WARM_MARKER)).ok()?).ok()
}

/// The sources a lane may be seeded from: the main checkout's `target/` when it
/// holds a `debug/` built under a Cargo home this lane shares.
///
/// **Never another lane's target.** Each worker's sandbox may write its own
/// lane target (and the marker in it); Kronn then builds the seeded lane
/// OUTSIDE that sandbox. A worker that planted a proc-macro or a build artifact
/// with a newer mtime in its `debug/` (Cargo trusts mtimes, not contents) would
/// get it loaded by that build and run as the host user. The main checkout's
/// `target/` is outside every worker's write roots (its worktree and its own
/// lane target), so it is the one source a worker cannot have touched.
fn seed_candidates(lane: &WorkerLane, home: &HomeKey) -> Vec<SeedCandidate> {
    let Some(target_root) = lane
        .main_checkout
        .as_ref()
        .and_then(|_| lane.cargo_target_dir.parent())
    else {
        return Vec::new();
    };
    let source = target_root.join(DEBUG_DIR);
    // Belt and braces for the layout: whatever the worker may write is never a source.
    let writable_by_the_worker = lane
        .write_roots()
        .iter()
        .any(|root| source.starts_with(root));
    if writable_by_the_worker || !home.main_checkout_reusable || !source.is_dir() {
        return Vec::new();
    }
    vec![SeedCandidate {
        target: target_root.to_path_buf(),
        kind: SeedSourceKind::MainCheckout,
    }]
}

/// Hold Cargo's own build lock shared. It is refused while a build writes the
/// profile directory (so a target being written is never a source), and while
/// it is held a starting build waits instead of changing what is being cloned.
fn lock_source_shared(debug_dir: &Path) -> Result<std::fs::File, String> {
    let lock = std::fs::File::open(debug_dir.join(CARGO_BUILD_LOCK))
        .map_err(|error| format!("no Cargo build lock ({error})"))?;
    FileExt::try_lock_shared(&lock).map_err(|_| "a build is writing it".to_string())?;
    Ok(lock)
}

/// Seed `lane`'s `debug/` from the best warm source. Never raises: the outcome
/// says what happened and an unseedable lane just starts empty.
fn seed_lane(lane: &WorkerLane, home: &HomeKey, clone: CloneTree<'_>) -> SeedOutcome {
    let destination = lane.cargo_target_dir.join(DEBUG_DIR);
    if destination.exists() {
        return SeedOutcome::AlreadyPresent;
    }
    let candidates = seed_candidates(lane, home);
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
    for candidate in candidates {
        let source = candidate.target.join(DEBUG_DIR);
        let lock = match lock_source_shared(&source) {
            Ok(lock) => lock,
            Err(reason) => {
                skipped.push(format!("{}: {reason}", candidate.target.display()));
                continue;
            }
        };
        let _ = std::fs::remove_dir_all(&staging);
        let started = Instant::now();
        let cloned =
            clone(&source, &staging).and_then(|()| std::fs::rename(&staging, &destination));
        let _ = FileExt::unlock(&lock);
        match cloned {
            Ok(()) => {
                return SeedOutcome::Cloned {
                    source: candidate.target,
                    kind: candidate.kind,
                    clone_ms: started.elapsed().as_millis() as u64,
                };
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
    match unsupported {
        Some(reason) => SeedOutcome::Unsupported { reason },
        None => SeedOutcome::NoSource {
            reason: format!("no usable source: {}", skipped.join("; ")),
        },
    }
}

// ─── Warm start ───────────────────────────────────────────────────────────

fn write_marker(lane: &WorkerLane, lock_hash: &str, home: &HomeKey) -> io::Result<()> {
    let marker = WarmMarker {
        lock_hash: lock_hash.to_string(),
        cargo_home: home.cargo_home.clone(),
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

/// Seed the lane target, then run its first build under the host gate.
///
/// `on_wait` is called once, with the number of builds ahead, when the gate is
/// full — before blocking. A build that fails or times out is recorded and
/// leaves no marker; the worker is launched regardless.
pub fn warm_lane(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    settings: &WarmStartSettings,
    gate: &BuildGate,
    runner: &dyn ProbeRunner,
    clone: CloneTree<'_>,
    on_wait: impl FnOnce(usize),
) -> WarmStartRecord {
    warm_lane_cancellable(
        lane,
        provisioned,
        settings,
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
#[allow(clippy::too_many_arguments)]
pub fn warm_lane_cancellable(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    settings: &WarmStartSettings,
    gate: &BuildGate,
    runner: &dyn ProbeRunner,
    clone: CloneTree<'_>,
    cancel: &CancellationToken,
    on_wait: impl FnOnce(usize),
) -> WarmStartRecord {
    let jobs = settings.build_jobs;
    let home = HomeKey::of(lane, provisioned);
    let Some((manifest_dir, lock_hash)) = manifest_and_lock_hash(lane) else {
        return WarmStartRecord::not_applicable(
            "no Cargo.toml at the worktree root or in backend/",
            jobs,
        );
    };
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
        return WarmStartRecord {
            lock_hash,
            seed: SeedOutcome::NotApplicable {
                reason: "cancelled before the lane was seeded".into(),
            },
            build_jobs: jobs,
            first_build: FirstBuild::Cancelled { duration_ms: 0 },
            queued_ms: 0,
        };
    }
    let seed = match &lock_hash {
        Some(_) => seed_lane(lane, &home, clone),
        None => SeedOutcome::NoSource {
            reason: "the worktree has no Cargo.lock to match a warm target against".into(),
        },
    };

    // `--locked` cannot build without a lock file, and a build with no lock hash
    // could not be marked warm: nothing to run, nothing to wait for.
    if lock_hash.is_none() {
        return WarmStartRecord {
            lock_hash,
            seed,
            build_jobs: jobs,
            first_build: FirstBuild::Skipped {
                reason: "the worktree has no Cargo.lock: a locked first build is not possible"
                    .into(),
            },
            queued_ms: 0,
        };
    }

    // A lane already built for this exact Cargo.lock (a relaunch) is warm.
    let already_built = lock_hash.as_ref().is_some_and(|hash| {
        lane.cargo_target_dir.join(DEBUG_DIR).is_dir()
            && read_marker(&lane.cargo_target_dir).is_some_and(|marker| marker.lock_hash == *hash)
    });
    if already_built {
        return WarmStartRecord {
            lock_hash,
            seed,
            build_jobs: jobs,
            first_build: FirstBuild::Skipped {
                reason: "the lane target was already built for this Cargo.lock".into(),
            },
            queued_ms: 0,
        };
    }

    let queued = Instant::now();
    let mut waited = false;
    let permit = match gate.acquire_within(
        settings.max_concurrent_builds,
        settings.first_build_timeout,
        cancel,
        |ahead| {
            waited = true;
            on_wait(ahead);
        },
    ) {
        GateWait::Acquired(permit) => permit,
        GateWait::Cancelled => {
            return WarmStartRecord {
                lock_hash,
                seed,
                build_jobs: jobs,
                first_build: FirstBuild::Cancelled { duration_ms: 0 },
                queued_ms: queued.elapsed().as_millis() as u64,
            };
        }
        // Never stall a worker behind a queue that does not drain: it starts
        // with the target it has, as before KT-1250.
        GateWait::TimedOut => {
            return WarmStartRecord {
                lock_hash,
                seed,
                build_jobs: jobs,
                first_build: FirstBuild::Skipped {
                    reason: format!(
                        "no build slot freed within {}s: the worker starts with the target it \
                         has",
                        settings.first_build_timeout.as_secs()
                    ),
                },
                queued_ms: queued.elapsed().as_millis() as u64,
            };
        }
    };
    let queued_ms = if waited {
        queued.elapsed().as_millis() as u64
    } else {
        0
    };

    let env: Vec<(String, OsString)> = lane
        .env(provisioned)
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    let started = Instant::now();
    let outcome = runner.run_cancellable(
        &ProbeCommand {
            program: "cargo".into(),
            args: ["test", "--no-run", "--locked", "--offline"]
                .into_iter()
                .map(String::from)
                .collect(),
            cwd: manifest_dir,
            env,
            timeout: settings.first_build_timeout,
        },
        cancel,
    );
    let duration_ms = started.elapsed().as_millis() as u64;
    drop(permit);

    let first_build = if !outcome.success && cancel.is_cancelled() {
        FirstBuild::Cancelled { duration_ms }
    } else if outcome.success {
        if let Some(hash) = &lock_hash {
            if let Err(error) = write_marker(lane, hash, &home) {
                tracing::warn!(lane = %lane.name, "warm marker not written: {error}");
            }
        }
        FirstBuild::Succeeded { duration_ms }
    } else if outcome.detail.starts_with(TIMED_OUT_DETAIL_PREFIX) {
        FirstBuild::TimedOut { duration_ms }
    } else {
        FirstBuild::Failed {
            duration_ms,
            detail: outcome.detail,
        }
    };
    WarmStartRecord {
        lock_hash,
        seed,
        build_jobs: jobs,
        first_build,
        queued_ms,
    }
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
    let Some((_, Some(hash))) = manifest_and_lock_hash(&lane) else {
        return false;
    };
    !(lane.cargo_target_dir.join(DEBUG_DIR).is_dir()
        && read_marker(&lane.cargo_target_dir).is_some_and(|marker| marker.lock_hash == hash))
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
    let settings = WarmStartSettings::from_host();
    match prepare_launch(work_dir) {
        Ok((lane, provisioned)) => warm_lane_cancellable(
            &lane,
            &provisioned,
            &settings,
            host_build_gate(),
            runner,
            &clone_tree,
            cancel,
            on_wait,
        ),
        Err(error) => WarmStartRecord::not_applicable(
            &format!("the worker lane could not be prepared: {error}"),
            settings.build_jobs,
        ),
    }
}

#[cfg(test)]
#[path = "task_worker_lane_warm_test.rs"]
mod task_worker_lane_warm_test;
