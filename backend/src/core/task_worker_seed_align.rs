//! KT-1742 — a seed built with the lane's compiler environment.
//!
//! A lane is seeded with a copy-on-write clone of the project's own main-checkout
//! `target/debug` and then builds under a compiler environment of its own (on
//! macOS the C flags that keep clang's temporary files inside the lane). `cc`
//! records the environment it read in the fingerprint of every build script that
//! compiles C, so a seed built under another environment is invalid for all of
//! them and for everything that depends on them: most of a graph that uses TLS or
//! SQLite. Measured on the Kronn repository, that is the difference between a
//! warm-up of two minutes and one that ran into its bound.
//!
//! Nothing in the repository can say which environment its seed was built with,
//! so Kronn keeps a stamp beside the seed (`<target>/.kronn-seed-env.json`): the
//! environment, the hash of the `Cargo.lock` it was built from, and when its
//! build scripts last ran. A lane whose seed has a matching stamp clones it as
//! it is. When the stamp is missing or differs, Kronn runs ONE build in the
//! seed's own target, outside any sandbox, with the lane's environment, writes
//! the stamp, then clones. The next lane of the project finds the stamp and
//! reuses the seed.
//!
//! The seed is the owner's: their own builds and their running dev backend write
//! it. The build therefore waits (bounded) for Cargo's own lock on the profile
//! directory and, when it is still held, gives up and reports so, leaving the
//! seed to be cloned as it is.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::background_priority::Priority;
use crate::core::seed_prune::CARGO_BUILD_LOCK;
use crate::core::task_worker_env::{ProbeCommand, ProbeRunner};

/// The stamp, in the seed's target directory (not in `debug/`, which is what a
/// lane clones).
pub const SEED_STAMP: &str = ".kronn-seed-env.json";
const DEBUG_DIR: &str = "debug";
const STAMP_VERSION: u32 = 1;
/// How often a held Cargo lock is tried again.
const LOCK_POLL: Duration = Duration::from_millis(200);
/// Default wait for the seed's Cargo lock before falling back to the seed as it is.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(120);

/// What aligning one seed did, journalled on the lane's warm-start record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SeedAlignment {
    /// `aligned: true` — the seed is built with the lane's environment.
    /// `rebuilt: false` when its stamp already matched (no build was run).
    Aligned { rebuilt: bool, build_ms: u64 },
    /// `aligned: false` — alignment was due and could not be done (the seed's
    /// Cargo lock stayed held, the build failed or timed out): the lane is
    /// cloned from the seed as it is, as before KT-1742.
    NotAligned { reason: String },
    /// `aligned: skipped(<reason>)` — alignment was not attempted.
    Skipped { reason: String },
}

impl SeedAlignment {
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped {
            reason: reason.into(),
        }
    }

    fn not_aligned(reason: impl Into<String>) -> Self {
        Self::NotAligned {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SeedStamp {
    version: u32,
    /// The `Cargo.lock` the seed was built from (the main checkout's).
    lock_hash: Option<String>,
    /// The compiler environment, sorted by name.
    env: Vec<(String, String)>,
    built_unix: u64,
    /// The newest run of a build script in the seed when the stamp was written.
    build_script_unix: u64,
}

/// One seed to align.
pub struct AlignRequest<'a> {
    /// The seed's crate in the main checkout: the build runs there.
    pub main_crate_dir: &'a Path,
    /// The seed's target directory (the crate's own, from `cargo metadata`).
    pub target: &'a Path,
    /// The hash of the main checkout crate's `Cargo.lock`.
    pub lock_hash: Option<String>,
    /// The lane's compiler environment.
    pub compiler_env: &'a [(String, String)],
    /// The environment of the seed build: the compiler environment, the Cargo
    /// home and the offline switch, plus `CARGO_TARGET_DIR` set to `target`.
    pub build_env: Vec<(String, OsString)>,
    pub timeout: Duration,
    pub lock_wait: Duration,
}

fn sorted(env: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = env.to_vec();
    env.sort();
    env
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn mtime_unix(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
}

/// The newest time a build script ran in the seed: the modification time of the
/// newest `run-build-script-*` fingerprint file. A script reruns when an
/// environment variable it reads changes, so the owner's own build under another
/// environment (the dev backend's, without the lane's flags) moves it.
fn newest_build_script_run(debug: &Path) -> u64 {
    let mut newest = 0;
    for unit in std::fs::read_dir(debug.join(".fingerprint"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for file in std::fs::read_dir(unit.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if file
                .file_name()
                .to_string_lossy()
                .starts_with("run-build-script-")
            {
                newest = newest.max(mtime_unix(&file.path()).unwrap_or(0));
            }
        }
    }
    newest
}

/// Why the seed's stamp does not vouch for the lane's environment, or `Ok` when
/// it does.
fn stamp_verdict(request: &AlignRequest<'_>) -> Result<(), String> {
    let bytes = std::fs::read(request.target.join(SEED_STAMP))
        .map_err(|_| "the seed has no Kronn stamp".to_string())?;
    let stamp: SeedStamp =
        serde_json::from_slice(&bytes).map_err(|_| "the seed's stamp is unreadable".to_string())?;
    if stamp.version != STAMP_VERSION {
        return Err("the seed's stamp is of another version".into());
    }
    if stamp.env != sorted(request.compiler_env) {
        return Err("the seed was built with another compiler environment".into());
    }
    if stamp.lock_hash != request.lock_hash {
        return Err("the seed was built from another Cargo.lock".into());
    }
    if newest_build_script_run(&request.target.join(DEBUG_DIR)) > stamp.build_script_unix {
        return Err(
            "a build script ran in the seed after Kronn's build (another \
                    environment built it since)"
                .into(),
        );
    }
    Ok(())
}

fn write_stamp(request: &AlignRequest<'_>) -> io::Result<()> {
    let stamp = SeedStamp {
        version: STAMP_VERSION,
        lock_hash: request.lock_hash.clone(),
        env: sorted(request.compiler_env),
        built_unix: unix_now(),
        build_script_unix: newest_build_script_run(&request.target.join(DEBUG_DIR)),
    };
    let path = request.target.join(SEED_STAMP);
    let staging = request.target.join(format!("{SEED_STAMP}.tmp"));
    std::fs::write(
        &staging,
        serde_json::to_vec(&stamp).map_err(io::Error::other)?,
    )?;
    std::fs::rename(staging, path)
}

/// Wait, within `wait`, for Cargo's lock on the seed's profile directory to be
/// free, without keeping it: the build that follows takes it itself.
fn wait_for_free_lock(
    debug: &Path,
    wait: Duration,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let lock = std::fs::File::open(debug.join(CARGO_BUILD_LOCK))
        .map_err(|error| format!("the seed has no Cargo build lock ({error})"))?;
    let deadline = Instant::now() + wait;
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled while waiting for the seed's Cargo lock".into());
        }
        if FileExt::try_lock_exclusive(&lock).is_ok() {
            let _ = FileExt::unlock(&lock);
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "a build holds the seed's Cargo lock (waited {}s)",
                wait.as_secs()
            ));
        }
        std::thread::sleep(LOCK_POLL.min(wait.max(Duration::from_millis(10))));
    }
}

/// Make sure the seed of `request` was built with the lane's compiler
/// environment. Never raises: the answer says what happened, and every outcome
/// but `Aligned` leaves the seed to be cloned as it is.
pub fn align_seed(
    request: &AlignRequest<'_>,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
) -> SeedAlignment {
    match assess(request) {
        Assessment::Settled(alignment) => alignment,
        Assessment::NeedsBuild(stale) => build_aligned(request, &stale, runner, cancel),
    }
}

/// What looking at a seed settles without running anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assessment {
    /// Nothing to build: already aligned, or not applicable.
    Settled(SeedAlignment),
    /// A build is due, and why (what the stamp lacked).
    NeedsBuild(String),
}

/// Look at the seed: needs no lock, no slot and no process, so a caller can
/// decide whether to take a build slot before [`build_aligned`].
pub fn assess(request: &AlignRequest<'_>) -> Assessment {
    if !request.target.join(DEBUG_DIR).is_dir() {
        return Assessment::Settled(SeedAlignment::skipped(
            "the main checkout has no target/debug to align",
        ));
    }
    if request.compiler_env.is_empty() {
        return Assessment::Settled(SeedAlignment::skipped(
            "the lane sets no compiler environment: nothing to align",
        ));
    }
    match stamp_verdict(request) {
        Ok(()) => Assessment::Settled(SeedAlignment::Aligned {
            rebuilt: false,
            build_ms: 0,
        }),
        Err(reason) => Assessment::NeedsBuild(reason),
    }
}

/// The one build of [`align_seed`]: wait (bounded) for the seed's Cargo lock,
/// run `cargo build` in the seed's own crate with the lane's environment, write
/// the stamp.
pub fn build_aligned(
    request: &AlignRequest<'_>,
    stale: &str,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
) -> SeedAlignment {
    let debug = request.target.join(DEBUG_DIR);
    if let Err(reason) = wait_for_free_lock(&debug, request.lock_wait, cancel) {
        return SeedAlignment::not_aligned(format!("{reason}: the seed is cloned as it is"));
    }
    let started = Instant::now();
    let outcome = runner.run_cancellable(
        &ProbeCommand {
            program: "cargo".into(),
            args: ["build", "--locked", "--offline"]
                .map(String::from)
                .to_vec(),
            cwd: request.main_crate_dir.to_path_buf(),
            env: request.build_env.clone(),
            timeout: request.timeout,
            priority: Priority::Background,
            clean_env: false,
        },
        cancel,
    );
    let build_ms = started.elapsed().as_millis() as u64;
    if !outcome.success {
        let why = if cancel.is_cancelled() {
            "cancelled".to_string()
        } else {
            outcome.detail
        };
        return SeedAlignment::not_aligned(format!(
            "the seed build ({stale}) did not finish: {why}; the seed is cloned as it is"
        ));
    }
    if let Err(error) = write_stamp(request) {
        // The seed is aligned; the next lane will only rebuild (a no-op build).
        tracing::warn!(target = %request.target.display(), "seed stamp not written: {error}");
    }
    SeedAlignment::Aligned {
        rebuilt: true,
        build_ms,
    }
}

#[cfg(test)]
#[path = "task_worker_seed_align_test.rs"]
mod tests;
