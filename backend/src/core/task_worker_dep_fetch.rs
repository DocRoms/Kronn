//! KT-1649 — fetch what a worker's lock files reference and the host cache lacks.
//!
//! A task worker is offline (`CARGO_NET_OFFLINE`, a sandbox with no outbound
//! connection) and reads the host's dependency caches read-only. A worker that
//! bumped a dependency (KT-1647: `utoipa-swagger-ui` 10.0.1 → 10.0.2) therefore
//! referenced a crate no cache held, could neither compile nor test, and
//! delivered blind until the principal ran `cargo fetch --locked` by hand.
//!
//! Before the lane's first build, Kronn now fills the cache itself, **outside
//! the worker's sandbox, from the lock file, and from nowhere else**:
//!
//! * the lock file is the whole input. `Cargo.lock` is parsed and every
//!   package source must be crates.io; `pnpm-lock.yaml` must name no tarball or
//!   git resolution of its own. A lock referencing anything else (a git
//!   dependency, an alternate registry, a tarball URL) is refused as a whole
//!   and nothing is fetched: the network stays opened to the public registries
//!   and no host the worker could name;
//! * a probe comes first: `cargo fetch --offline --locked` (and its pnpm
//!   twin). A cache that already holds everything never touches the network;
//! * what the worker controls is kept away from the command. Cargo reads its
//!   configuration (source replacement, proxies, registries, credential
//!   helpers) from the working directory upwards, so it runs from the host's own
//!   `CARGO_HOME` with `--manifest-path`, never from the worktree. pnpm reads
//!   `.npmrc` and `pnpm-workspace.yaml` from the project, so it runs in a
//!   scratch directory holding a copy of the lock file, the workspace file
//!   (refused when it names a registry, a proxy, a store, a hook or a
//!   certificate) and an empty manifest, with the registry and the store given
//!   on the command line. Neither command runs a build script or a lifecycle
//!   script (`cargo fetch` builds nothing, pnpm gets `--ignore-scripts`);
//! * every step is journalled on the execution (`dependency_fetch`): the lock,
//!   the registries it was bounded to, the outcome and its duration.
//!
//! Best effort, like the rest of the warm-up: a failed or refused fetch is
//! recorded and the worker still starts, with the cache it has.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::background_priority::Priority;
use crate::core::task_worker_env::{
    ProbeCommand, ProbeRunner, SharedCaches, WorkerLane, CARGO_HOME_ENV, CARGO_NET_OFFLINE_ENV,
};

/// `task_execution_events.action` of the fetch record.
pub const DEP_FETCH_EVENT_ACTION: &str = "dependency_fetch";

/// The only `source` a `Cargo.lock` package may carry for Kronn to fetch it:
/// crates.io, in its git-index and its sparse form.
pub const CRATES_IO_SOURCES: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// The registry a pnpm fetch is pinned to on the command line.
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

const CARGO_PROBE_TIMEOUT: Duration = Duration::from_secs(120);
const CARGO_FETCH_TIMEOUT: Duration = Duration::from_secs(600);
const PNPM_PROBE_TIMEOUT: Duration = Duration::from_secs(120);
const PNPM_FETCH_TIMEOUT: Duration = Duration::from_secs(600);
/// A lock file larger than this is not one a launch parses.
const MAX_LOCK_BYTES: u64 = 32 * 1024 * 1024;

const FRONTEND_DIR: &str = "frontend";
const PNPM_LOCK: &str = "pnpm-lock.yaml";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";
/// What a staged pnpm project holds instead of the worker's `package.json`.
const STAGED_MANIFEST: &str = "{\"name\":\"kronn-lane-dependency-fetch\",\"private\":true}\n";
/// A setting of `pnpm-workspace.yaml` that would move the fetch somewhere the
/// operator did not choose: another registry or proxy, another store or cache,
/// a hook (`pnpmfile` runs JavaScript), a certificate or TLS switch. Compared
/// case-insensitively, against every non-comment line.
const WORKSPACE_REFUSED: [&str; 12] = [
    "registr",
    "proxy",
    "storedir",
    "store-dir",
    "cachedir",
    "cache-dir",
    "statedir",
    "pnpmfile",
    "hooks",
    "cafile",
    "cert",
    "strict-ssl",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Cargo,
    Pnpm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FetchOutcome {
    /// The cache already held everything the lock references: no network.
    Cached,
    /// The missing packages were fetched into the host cache.
    Fetched,
    /// The lock references something outside the allow-list (or the step could
    /// not be set up safely): nothing was fetched.
    Refused { reason: String },
    /// The fetch ran and failed.
    Failed { detail: String },
    /// The execution was cancelled during the fetch.
    Cancelled,
}

/// One lock file's fetch step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepFetchStep {
    pub ecosystem: Ecosystem,
    /// The lock file, relative to the worktree.
    pub lockfile: String,
    /// The registries the step was bounded to; the only hosts it could reach.
    pub registries: Vec<String>,
    pub outcome: FetchOutcome,
    pub duration_ms: u64,
}

/// What the launch fetched, one step per lock file that had something to fetch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepFetchRecord {
    pub steps: Vec<DepFetchStep>,
}

impl DepFetchRecord {
    /// Whether a step reached the network and brought something in.
    pub fn fetched_anything(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.outcome == FetchOutcome::Fetched)
    }

    pub fn cancelled(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.outcome == FetchOutcome::Cancelled)
    }
}

// ─── Lock files ───────────────────────────────────────────────────────────

/// What a `Cargo.lock` asks Cargo to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoLockScan {
    /// Packages with a registry source (path dependencies have none).
    pub registry_packages: usize,
    /// The distinct registries they come from, all allow-listed.
    pub registries: Vec<String>,
}

/// Read a `Cargo.lock`: the registries it references, or why it is out of
/// bounds. Any source other than crates.io — a git dependency, an alternate
/// registry — refuses the whole lock, because `cargo fetch` fetches all of it.
pub fn scan_cargo_lock(text: &str) -> Result<CargoLockScan, String> {
    let table: toml::Table = toml::from_str(text)
        .map_err(|error| format!("Cargo.lock is not valid TOML: {}", first_line(&error)))?;
    let packages = match table.get("package") {
        None => return Ok(CargoLockScan::empty()),
        Some(toml::Value::Array(packages)) => packages,
        Some(_) => return Err("Cargo.lock `package` is not an array".into()),
    };
    let mut scan = CargoLockScan::empty();
    for package in packages {
        let Some(source) = package.get("source") else {
            continue;
        };
        let Some(source) = source.as_str() else {
            return Err("a Cargo.lock package has a `source` that is not a string".into());
        };
        if !CRATES_IO_SOURCES.contains(&source) {
            let name = package
                .get("name")
                .and_then(toml::Value::as_str)
                .unwrap_or("?");
            return Err(format!(
                "`{name}` comes from `{source}`, which is not crates.io: Kronn only fetches \
                 from the lock's crates.io packages"
            ));
        }
        scan.registry_packages += 1;
        if !scan.registries.iter().any(|known| known == source) {
            scan.registries.push(source.to_string());
        }
    }
    scan.registries.sort();
    Ok(scan)
}

impl CargoLockScan {
    fn empty() -> Self {
        Self {
            registry_packages: 0,
            registries: Vec::new(),
        }
    }
}

/// Whether a `pnpm-lock.yaml` lists packages, or why it is out of bounds. The
/// lock must name no resolution of its own: a `tarball:` URL, a git
/// `repo:`/`commit:`/`type: git`, a `directory:` or `file:` resolution is how a
/// lock points a download at a host other than the registry.
pub fn scan_pnpm_lock(text: &str) -> Result<bool, String> {
    let mut has_packages = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if line.starts_with("packages:") {
            has_packages = true;
        }
        let lowered = trimmed.to_ascii_lowercase();
        for marker in [
            "tarball:",
            "repo:",
            "commit:",
            "type: git",
            "type: directory",
            "directory:",
        ] {
            if lowered.contains(marker) {
                return Err(format!(
                    "pnpm-lock.yaml carries a `{}` resolution, which is not the npm registry: \
                     Kronn only fetches packages the registry resolves",
                    marker.trim_end_matches(':')
                ));
            }
        }
    }
    Ok(has_packages)
}

/// `Ok` when `pnpm-workspace.yaml` sets nothing that would move the fetch off
/// the registry and store Kronn chose.
fn check_pnpm_workspace(text: &str) -> Result<(), String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let lowered = trimmed.to_ascii_lowercase();
        if let Some(setting) = WORKSPACE_REFUSED
            .iter()
            .find(|setting| lowered.contains(*setting))
        {
            return Err(format!(
                "pnpm-workspace.yaml sets `{setting}`, which would redirect the fetch"
            ));
        }
    }
    Ok(())
}

fn first_line(error: &impl std::fmt::Display) -> String {
    error.to_string().lines().next().unwrap_or("").to_string()
}

fn read_lock(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .len();
    if size > MAX_LOCK_BYTES {
        return Err(format!(
            "{} is {size} bytes, over the {MAX_LOCK_BYTES} a launch reads",
            path.display()
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

// ─── Running a step ───────────────────────────────────────────────────────

fn probe(
    program: &str,
    args: Vec<String>,
    cwd: &Path,
    env: Vec<(String, OsString)>,
    timeout: Duration,
) -> ProbeCommand {
    ProbeCommand {
        program: program.into(),
        args,
        cwd: cwd.to_path_buf(),
        env,
        timeout,
        priority: Priority::Normal,
    }
}

/// The probe-then-fetch shape both ecosystems share: `offline` succeeding means
/// the cache is complete; otherwise `online` runs, and its result is the step's.
fn probe_then_fetch(
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
    offline: &ProbeCommand,
    online: &ProbeCommand,
) -> FetchOutcome {
    if cancel.is_cancelled() {
        return FetchOutcome::Cancelled;
    }
    if runner.run_cancellable(offline, cancel).success {
        return FetchOutcome::Cached;
    }
    if cancel.is_cancelled() {
        return FetchOutcome::Cancelled;
    }
    let fetched = runner.run_cancellable(online, cancel);
    if fetched.success {
        FetchOutcome::Fetched
    } else if cancel.is_cancelled() {
        FetchOutcome::Cancelled
    } else {
        FetchOutcome::Failed {
            detail: fetched.detail,
        }
    }
}

fn step(
    ecosystem: Ecosystem,
    lockfile: String,
    registries: Vec<String>,
    started: Instant,
    outcome: FetchOutcome,
) -> DepFetchStep {
    DepFetchStep {
        ecosystem,
        lockfile,
        registries,
        outcome,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// The crates the lock at `manifest_dir/Cargo.lock` references and `cargo_home`
/// lacks, fetched from crates.io. `None` when there is nothing to fetch (no
/// lock, or no registry package in it).
pub fn fetch_cargo(
    worktree: &Path,
    manifest_dir: &Path,
    cargo_home: Option<&Path>,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
) -> Option<DepFetchStep> {
    let started = Instant::now();
    let lock = manifest_dir.join("Cargo.lock");
    if !lock.is_file() {
        return None;
    }
    let lockfile = relative(worktree, &lock);
    let refused = |reason: String| {
        step(
            Ecosystem::Cargo,
            lockfile.clone(),
            Vec::new(),
            started,
            FetchOutcome::Refused { reason },
        )
    };
    let scan = match read_lock(&lock).and_then(|text| scan_cargo_lock(&text)) {
        Ok(scan) => scan,
        Err(reason) => return Some(refused(reason)),
    };
    if scan.registry_packages == 0 {
        return None;
    }
    let Some(cargo_home) = cargo_home else {
        return Some(refused(
            "the host has no CARGO_HOME to fill: the lane has no shared registry".into(),
        ));
    };
    let args = |offline: bool| {
        let mut args = vec!["fetch".to_string()];
        if offline {
            args.push("--offline".into());
        }
        args.extend([
            "--locked".into(),
            "--manifest-path".into(),
            manifest_dir
                .join("Cargo.toml")
                .to_string_lossy()
                .into_owned(),
        ]);
        args
    };
    let env = |offline: bool| {
        vec![
            (
                CARGO_HOME_ENV.to_string(),
                cargo_home.as_os_str().to_os_string(),
            ),
            (
                CARGO_NET_OFFLINE_ENV.to_string(),
                OsString::from(if offline { "true" } else { "false" }),
            ),
        ]
    };
    // From the host's own CARGO_HOME, never the worktree: Cargo reads its
    // configuration from the working directory upwards, and the worker writes
    // `.cargo/config.toml` there.
    let outcome = probe_then_fetch(
        runner,
        cancel,
        &probe(
            "cargo",
            args(true),
            cargo_home,
            env(true),
            CARGO_PROBE_TIMEOUT,
        ),
        &probe(
            "cargo",
            args(false),
            cargo_home,
            env(false),
            CARGO_FETCH_TIMEOUT,
        ),
    );
    Some(step(
        Ecosystem::Cargo,
        lockfile,
        scan.registries,
        started,
        outcome,
    ))
}

/// The packages `frontend/pnpm-lock.yaml` references and the pnpm `store` lacks,
/// fetched from the npm registry. `None` when there is nothing to fetch.
pub fn fetch_pnpm(
    worktree: &Path,
    frontend: &Path,
    store: Option<&Path>,
    scratch_root: &Path,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
) -> Option<DepFetchStep> {
    let started = Instant::now();
    let lock = frontend.join(PNPM_LOCK);
    if !lock.is_file() {
        return None;
    }
    let lockfile = relative(worktree, &lock);
    let registries = vec![NPM_REGISTRY.to_string()];
    let refused = |reason: String| {
        step(
            Ecosystem::Pnpm,
            lockfile.clone(),
            Vec::new(),
            started,
            FetchOutcome::Refused { reason },
        )
    };
    let lock_text = match read_lock(&lock).and_then(|text| scan_pnpm_lock(&text).map(|p| (text, p)))
    {
        Ok((text, true)) => text,
        Ok((_, false)) => return None,
        Err(reason) => return Some(refused(reason)),
    };
    let Some(store) = store else {
        return Some(refused(
            "the host has no pnpm store to fill: the lane has none to read".into(),
        ));
    };
    let workspace = match std::fs::read_to_string(frontend.join(PNPM_WORKSPACE)) {
        Ok(text) => match check_pnpm_workspace(&text) {
            Ok(()) => Some(text),
            Err(reason) => return Some(refused(reason)),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Some(refused(format!("cannot read {PNPM_WORKSPACE}: {error}"))),
    };
    let scratch = match Scratch::create(scratch_root, &lock_text, workspace.as_deref()) {
        Ok(scratch) => scratch,
        Err(error) => return Some(refused(format!("cannot stage the pnpm fetch: {error}"))),
    };
    let store_arg = store.to_string_lossy().into_owned();
    let common = |offline: bool| {
        let mut args = vec!["fetch".to_string(), "--ignore-scripts".into()];
        if offline {
            args.push("--offline".into());
        } else {
            args.extend(["--registry".into(), NPM_REGISTRY.into()]);
        }
        args.extend(["--store-dir".into(), store_arg.clone()]);
        args
    };
    let outcome = probe_then_fetch(
        runner,
        cancel,
        &probe(
            "pnpm",
            common(true),
            &scratch.dir,
            Vec::new(),
            PNPM_PROBE_TIMEOUT,
        ),
        &probe(
            "pnpm",
            common(false),
            &scratch.dir,
            Vec::new(),
            PNPM_FETCH_TIMEOUT,
        ),
    );
    Some(step(
        Ecosystem::Pnpm,
        lockfile,
        registries,
        started,
        outcome,
    ))
}

/// A scratch pnpm project, removed when dropped.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn create(root: &Path, lock: &str, workspace: Option<&str>) -> std::io::Result<Self> {
        let dir = root.join(format!("kronn-dep-fetch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        let scratch = Self { dir };
        std::fs::write(scratch.dir.join(PNPM_LOCK), lock)?;
        std::fs::write(scratch.dir.join("package.json"), STAGED_MANIFEST)?;
        if let Some(workspace) = workspace {
            std::fs::write(scratch.dir.join(PNPM_WORKSPACE), workspace)?;
        }
        Ok(scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn relative(worktree: &Path, path: &Path) -> String {
    path.strip_prefix(worktree)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Fetch what the lock files of `work_dir` reference and the host caches lack.
/// Never fails: a worktree the lane cannot be derived from has nothing to
/// fetch, and every step says how it ended.
///
/// `runner` must be one that may reach the network — Kronn's own process
/// runner, not the worker's confined one — which is why this is a step of its
/// own before the lane's first build rather than a probe of the smoke check.
pub fn fetch_missing_dependencies(
    work_dir: &Path,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
) -> DepFetchRecord {
    let Ok(worktree) = work_dir.canonicalize() else {
        return DepFetchRecord::default();
    };
    let Ok(lane) = WorkerLane::for_worktree(&worktree) else {
        return DepFetchRecord::default();
    };
    let caches = SharedCaches::discover(lane.main_checkout.as_deref());
    let mut record = DepFetchRecord::default();
    if let Some(manifest_dir) = [worktree.clone(), worktree.join("backend")]
        .into_iter()
        .find(|dir| dir.join("Cargo.toml").is_file())
    {
        record.steps.extend(fetch_cargo(
            &worktree,
            &manifest_dir,
            caches.cargo_home.as_deref(),
            runner,
            cancel,
        ));
    }
    if !record.cancelled() {
        record.steps.extend(fetch_pnpm(
            &worktree,
            &worktree.join(FRONTEND_DIR),
            caches.pnpm_stores.first().map(PathBuf::as_path),
            &std::env::temp_dir(),
            runner,
            cancel,
        ));
    }
    record
}

#[cfg(test)]
#[path = "task_worker_dep_fetch_test.rs"]
mod task_worker_dep_fetch_test;
