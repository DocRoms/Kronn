use super::*;
use crate::core::project_toolchains::CargoMetadata;
use crate::core::task_worker_env::ProbeOutcome;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

const LOCK_A: &str = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n";
const LOCK_B: &str = "[[package]]\nname = \"b\"\nversion = \"2.0.0\"\n";
/// A lock that lists the `cc` crate: the project builds C from a build script.
const LOCK_CC: &str = "[[package]]\nname = \"cc\"\nversion = \"1.0.0\"\n";

fn hash_of(lock: &str) -> String {
    Sha256::digest(lock.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A repo layout in a temp dir with a managed worktree holding a Rust crate.
struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().canonicalize().unwrap();
    Fixture { _dir: dir, repo }
}

impl Fixture {
    fn lane(&self, name: &str, lock: &str) -> WorkerLane {
        let worktree = self
            .repo
            .join(".kronn")
            .join("worktrees")
            .join(format!("task-{name}"));
        std::fs::create_dir_all(worktree.join("backend")).unwrap();
        std::fs::write(worktree.join("backend").join("Cargo.toml"), "").unwrap();
        std::fs::write(worktree.join("backend").join("Cargo.lock"), lock).unwrap();
        WorkerLane::for_worktree(&worktree).unwrap()
    }

    fn target_root(&self) -> PathBuf {
        self.repo.join("target")
    }

    /// A lane target that finished a first build for `lock`.
    fn warm_lane_target(&self, name: &str, lock: &str) -> PathBuf {
        let target = self.target_root().join(format!("wt-{name}"));
        self.write_build(&target, name);
        std::fs::write(
            target.join(WARM_MARKER),
            serde_json::to_vec(&WarmMarker {
                lock_hash: hash_of(lock),
                completed_unix: 1,
                cargo_home: None,
                roots: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();
        target
    }

    /// The main checkout's own `target/`, with a build in it.
    fn main_target(&self) -> PathBuf {
        let target = self.target_root();
        self.write_build(&target, "main");
        target
    }

    fn write_build(&self, target: &Path, content: &str) {
        let debug = target.join(DEBUG_DIR);
        std::fs::create_dir_all(debug.join("deps")).unwrap();
        std::fs::write(debug.join(CARGO_BUILD_LOCK), "").unwrap();
        std::fs::write(debug.join("deps").join("libdep.rlib"), content).unwrap();
    }
}

fn settings() -> WarmStartSettings {
    WarmStartSettings::resolve(Some("3"), Some("2"), Some("60"), 8)
}

/// Where a crate of the fixture builds in the main checkout: `<repo>/target`,
/// the default of a repository whose crate is in `backend/` and whose
/// `.cargo/config.toml` says `target-dir = "target"` (Cargo itself is not run).
fn fixture_metadata(manifest_dir: &Path) -> Result<CargoMetadata, String> {
    let repo = manifest_dir
        .parent()
        .ok_or_else(|| "no parent".to_string())?;
    Ok(CargoMetadata {
        target_directory: repo.join("target"),
        workspace_root: manifest_dir.to_path_buf(),
    })
}

/// A seed-build runner that must not be reached: the host of `host()` aligns
/// nothing.
struct NeverRuns;

impl ProbeRunner for NeverRuns {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        panic!("no seed build expected, got {probe:?}");
    }
}

/// The warm host of a fixture project: no seed alignment, no compiler
/// environment, no declared command, Cargo's answer stubbed.
fn host() -> WarmHost<'static> {
    WarmHost {
        seed_alignment: false,
        compiler_env: Vec::new(),
        first_build_command: None,
        rust_roots: None,
        metadata: &fixture_metadata,
        seed_runner: &NeverRuns,
        lock_wait: Duration::from_millis(50),
    }
}

/// A clone that records its calls and copies (the test's stand-in for APFS).
struct RecordingClone {
    calls: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl RecordingClone {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn call(&self, source: &Path, destination: &Path) -> io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((source.to_path_buf(), destination.to_path_buf()));
        copy_dir(source, destination)
    }

    fn calls(&self) -> Vec<(PathBuf, PathBuf)> {
        self.calls.lock().unwrap().clone()
    }
}

fn copy_dir(source: &Path, destination: &Path) -> io::Result<()> {
    if source.is_file() {
        return std::fs::copy(source, destination).map(|_| ());
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

struct ScriptedBuild {
    calls: Mutex<Vec<ProbeCommand>>,
    outcome: ProbeOutcome,
    /// Whether this stand-in claims to run its commands under the OS sandbox.
    sandboxed: bool,
    /// A build whose directory ends with this fails with `failure_detail`.
    fails_in: Option<(PathBuf, String)>,
}

impl ScriptedBuild {
    fn ok() -> Self {
        Self::with(true, "")
    }

    fn with(success: bool, detail: &str) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outcome: ProbeOutcome {
                success,
                detail: detail.into(),
            },
            sandboxed: false,
            fails_in: None,
        }
    }

    /// The same runner, claiming the lane's OS sandbox (a declared command runs
    /// only under one).
    fn sandboxed(mut self) -> Self {
        self.sandboxed = true;
        self
    }

    /// A runner whose build in a directory ending with `dir` fails.
    fn failing_in(mut self, dir: &str, detail: &str) -> Self {
        self.fails_in = Some((PathBuf::from(dir), detail.to_string()));
        self
    }

    fn calls(&self) -> Vec<ProbeCommand> {
        self.calls.lock().unwrap().clone()
    }
}

impl ProbeRunner for ScriptedBuild {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.calls.lock().unwrap().push(probe.clone());
        if let Some((dir, detail)) = &self.fails_in {
            if probe.cwd.ends_with(dir) {
                return ProbeOutcome {
                    success: false,
                    detail: detail.clone(),
                };
            }
        }
        self.outcome.clone()
    }

    fn confined(&self) -> bool {
        self.sandboxed
    }
}

fn provisioned() -> Provisioned {
    Provisioned {
        frontend_deps: crate::core::task_worker_env::FrontendDeps::NotApplicable,
        dependency_layers: Vec::new(),
        cargo_home_ready: false,
        shared_cargo_home: None,
        warnings: Vec::new(),
    }
}

/// A lane whose Cargo home is ready: the host's own (`shared`) or its private one.
fn provisioned_with_home(shared: Option<&Path>) -> Provisioned {
    Provisioned {
        cargo_home_ready: true,
        shared_cargo_home: shared.map(Path::to_path_buf),
        ..provisioned()
    }
}

fn warm_under(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    runner: &dyn ProbeRunner,
    clone: &RecordingClone,
) -> WarmStartRecord {
    warm_lane(
        lane,
        provisioned,
        &settings(),
        &host(),
        &BuildGate::new(),
        runner,
        &|source, destination| clone.call(source, destination),
        |_| {},
    )
}

fn warm(
    lane: &WorkerLane,
    runner: &dyn ProbeRunner,
    clone: &RecordingClone,
    gate: &BuildGate,
) -> WarmStartRecord {
    warm_lane(
        lane,
        &provisioned(),
        &settings(),
        &host(),
        gate,
        runner,
        &|source, destination| clone.call(source, destination),
        |_| {},
    )
}

// ── Settings ─────────────────────────────────────────────────────────────

#[test]
fn jobs_default_to_half_of_the_performance_cores_and_never_below_one() {
    assert_eq!(
        WarmStartSettings::resolve(None, None, None, 10).build_jobs,
        5
    );
    assert_eq!(
        WarmStartSettings::resolve(None, None, None, 1).build_jobs,
        1
    );
    assert_eq!(
        WarmStartSettings::resolve(None, None, None, 0).build_jobs,
        1
    );
}

/// KT-1742: project setting > global setting > env var > built-in default. The
/// two settings are folded by `task_worker_lane_settings::resolve` into one
/// `configured` bound; the env var only fills what no setting gave.
#[test]
fn the_env_var_only_fills_an_unset_first_build_bound() {
    let env = WarmStartSettings::resolve(None, None, Some("5400"), 10);
    let configured = Some(Duration::from_secs(90 * 60));

    // a setting beats the env var
    assert_eq!(
        env.with_configured_timeout(configured).first_build_timeout,
        Duration::from_secs(90 * 60)
    );
    // no setting: the env var fills it
    assert_eq!(
        env.with_configured_timeout(None).first_build_timeout,
        Duration::from_secs(5400)
    );
    // no setting and no env var: the built-in default
    assert_eq!(
        WarmStartSettings::resolve(None, None, None, 10)
            .with_configured_timeout(None)
            .first_build_timeout,
        DEFAULT_FIRST_BUILD_TIMEOUT
    );
}

#[test]
fn jobs_the_limit_and_the_timeout_are_configurable() {
    let configured = WarmStartSettings::resolve(Some("6"), Some("3"), Some("90"), 10);
    assert_eq!(configured.build_jobs, 6);
    assert_eq!(configured.max_concurrent_builds, 3);
    assert_eq!(configured.first_build_timeout, Duration::from_secs(90));

    let invalid = WarmStartSettings::resolve(Some("0"), Some("many"), Some("-4"), 10);
    assert_eq!(invalid.build_jobs, 5, "a non-positive override is ignored");
    assert_eq!(invalid.max_concurrent_builds, DEFAULT_MAX_CONCURRENT_BUILDS);
    assert_eq!(invalid.first_build_timeout, DEFAULT_FIRST_BUILD_TIMEOUT);
}

/// KT-1742: the default bound must stay above the measured 32 min 56 s of the
/// kronn crate's own two compiles at load 14, or a healthy warm-up is killed.
#[test]
fn the_default_first_build_bound_is_sixty_minutes_and_stays_overridable() {
    let unset = WarmStartSettings::resolve(None, None, None, 10);
    assert_eq!(unset.first_build_timeout, Duration::from_secs(60 * 60));
    assert_eq!(unset.first_build_timeout, DEFAULT_FIRST_BUILD_TIMEOUT);
    let overridden = WarmStartSettings::resolve(None, None, Some("5400"), 10);
    assert_eq!(overridden.first_build_timeout, Duration::from_secs(5400));
}

#[test]
fn the_lane_environment_injects_cargo_build_jobs() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-env", LOCK_A);
    let env = lane.env(&provisioned());
    let jobs = env
        .iter()
        .find(|(name, _)| *name == crate::core::task_worker_env::CARGO_BUILD_JOBS_ENV)
        .and_then(|(_, value)| value.to_str())
        .and_then(|value| value.parse::<u32>().ok());
    assert!(jobs.is_some_and(|jobs| jobs >= 1), "{env:?}");
}

#[test]
fn the_first_build_runs_with_the_jobs_cap_and_the_lane_target() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-jobs", LOCK_A);
    let runner = ScriptedBuild::ok();
    warm(&lane, &runner, &RecordingClone::new(), &BuildGate::new());

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "cargo");
    // The fixture's manifest has no library target: the whole package is built.
    assert_eq!(calls[0].args, ["test", "--no-run", "--locked", "--offline"]);
    assert_eq!(calls[0].cwd, lane.worktree.join("backend"));
    let env = |name: &str| {
        calls[0]
            .env
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
    };
    assert!(env("CARGO_BUILD_JOBS").is_some());
    assert_eq!(
        env("CARGO_TARGET_DIR"),
        Some(lane.cargo_target_dir.clone().into_os_string())
    );
}

/// KT-1736: a worker's first command is `cargo test --lib <filter>`, so the
/// warm-up builds the lib test binary only — not every `tests/*.rs` binary,
/// which did not fit the 30-minute bound on a loaded host.
#[test]
fn the_first_build_of_a_crate_with_a_library_builds_the_lib_test_binary_only() {
    let fx = fixture();
    let lane = fx.lane("kt-1736-lib", LOCK_A);
    let backend = lane.worktree.join("backend");
    std::fs::create_dir_all(backend.join("src")).unwrap();
    std::fs::write(backend.join("src").join("lib.rs"), "").unwrap();
    let runner = ScriptedBuild::ok();
    warm(&lane, &runner, &RecordingClone::new(), &BuildGate::new());

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].args,
        ["test", "--lib", "--no-run", "--locked", "--offline"]
    );
}

/// KT-1736: a library at a custom path (`[lib] path = …`) is still a library.
#[test]
fn a_lib_table_in_the_manifest_is_a_library_target() {
    let fx = fixture();
    let lane = fx.lane("kt-1736-lib-table", LOCK_A);
    std::fs::write(
        lane.worktree.join("backend").join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[lib]\npath = \"core/mod.rs\"\n",
    )
    .unwrap();
    let runner = ScriptedBuild::ok();
    warm(&lane, &runner, &RecordingClone::new(), &BuildGate::new());

    assert!(runner.calls()[0].args.iter().any(|arg| arg == "--lib"));
}

/// KT-1736: on macOS the lane's builds keep clang's intermediate files beside the
/// object file (the OS sandbox grants no per-user temp dir), and every other
/// host's C flags are left alone.
///
/// KT-1742: under the target-scoped names, never a plain `CFLAGS` (a plain one
/// would reach every target), and only for a project that builds C through the
/// `cc` crate: a project whose `Cargo.lock` does not list it keeps its build
/// scripts' fingerprints untouched.
#[test]
fn the_lane_keeps_clang_temp_files_in_the_object_dir_on_macos_for_a_cc_project_only() {
    use crate::core::task_worker_env::{compiler_temp_env_for, MACOS_TARGET_CFLAGS_ENV};

    let expected: Vec<(&str, OsString)> = vec![
        (
            "CFLAGS_aarch64_apple_darwin",
            OsString::from("-save-temps=obj"),
        ),
        (
            "CFLAGS_x86_64_apple_darwin",
            OsString::from("-save-temps=obj"),
        ),
    ];
    assert_eq!(compiler_temp_env_for(true), expected);
    assert_eq!(MACOS_TARGET_CFLAGS_ENV.len(), expected.len());
    assert!(compiler_temp_env_for(false).is_empty());

    // …and the first build gets it through the lane environment, like the worker,
    // when the project's lock lists `cc`.
    let fx = fixture();
    let lane = fx.lane("kt-1736-cflags", LOCK_CC);
    let env = lane.env(&provisioned());
    assert!(
        !env.iter().any(|(name, _)| *name == "CFLAGS"),
        "a plain CFLAGS reaches every target and cannot match the seed's"
    );
    for (name, value) in &expected {
        let got = env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        if cfg!(target_os = "macos") {
            assert_eq!(got.as_ref(), Some(value), "{name}");
        } else {
            assert!(got.is_none(), "{name}");
        }
    }

    // A project that does not build C through `cc` gets nothing, on any host.
    let plain = fx.lane("kt-1742-no-cc", LOCK_A);
    let env = plain.env(&provisioned());
    assert!(
        !env.iter().any(|(name, _)| name.starts_with("CFLAGS")),
        "{env:?}"
    );
}

/// KT-1633: the lane's first build (a `cargo test --no-run` of the whole
/// graph) reaches the probe runner as a background command, so the runner —
/// the sandboxed one included — starts it behind the priority launcher.
#[test]
fn the_first_build_asks_the_runner_for_background_priority() {
    let fx = fixture();
    let lane = fx.lane("kt-1633-priority", LOCK_A);
    let runner = ScriptedBuild::ok();
    warm(&lane, &runner, &RecordingClone::new(), &BuildGate::new());

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].priority,
        crate::core::background_priority::Priority::Background
    );
}

// ── Seeding ──────────────────────────────────────────────────────────────

#[test]
fn a_new_lane_is_seeded_with_a_clone_of_the_main_checkout_target() {
    let fx = fixture();
    let source = fx.main_target();
    let lane = fx.lane("kt-1250-new", LOCK_A);
    let clone = RecordingClone::new();
    let runner = ScriptedBuild::ok();

    let record = warm(&lane, &runner, &clone, &BuildGate::new());

    match &record.seed {
        SeedOutcome::Cloned {
            source: from, kind, ..
        } => {
            assert_eq!(from, &source);
            assert_eq!(*kind, SeedSourceKind::MainCheckout);
        }
        other => panic!("expected a clone, got {other:?}"),
    }
    // One clone of `debug/` only — a clone call, never a file-by-file copy of
    // the lane's own target or of the source's markers.
    let calls = clone.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, source.join(DEBUG_DIR));
    assert_eq!(
        std::fs::read_to_string(lane.cargo_target_dir.join("debug/deps/libdep.rlib")).unwrap(),
        "main"
    );
    assert!(
        !lane.cargo_target_dir.join(WARM_MARKER).exists()
            || read_marker(&lane.cargo_target_dir).is_some(),
        "the marker is the lane's own"
    );
    let staging_left = std::fs::read_dir(&lane.cargo_target_dir)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains("seeding"));
    assert!(!staging_left, "the staging directory is renamed away");
    assert_eq!(record.lock_hash, Some(hash_of(LOCK_A)));
}

#[test]
fn a_lane_target_is_never_a_seed_source() {
    // Each worker may write its own lane target and the marker in it, and Kronn
    // builds the seeded lane outside the worker sandbox: a lane target seeding
    // another lane would run what the first worker planted as the host user.
    let fx = fixture();
    // A well-formed, newest, same-lock, same-home lane target: everything the
    // former lane-to-lane seeding looked for.
    let lane_a = fx.warm_lane_target("kt-1200-worker-a", LOCK_A);
    // And a forged one: a marker and a planted artifact written by a worker.
    let forged = fx.target_root().join("wt-kt-1200-forged");
    fx.write_build(&forged, "planted proc-macro");
    std::fs::write(
        forged.join(WARM_MARKER),
        serde_json::to_vec(&WarmMarker {
            lock_hash: hash_of(LOCK_A),
            completed_unix: u64::MAX / 2,
            cargo_home: None,
            roots: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();
    let lane_b = fx.lane("kt-1250-worker-b", LOCK_A);

    // No main checkout build: with no safe source the lane starts empty.
    let clone = RecordingClone::new();
    let record = warm(&lane_b, &ScriptedBuild::ok(), &clone, &BuildGate::new());
    assert!(
        matches!(record.seed, SeedOutcome::NoSource { .. }),
        "{:?}",
        record.seed
    );
    assert!(clone.calls().is_empty(), "no lane target was cloned");
    assert!(!lane_b.cargo_target_dir.join(DEBUG_DIR).exists());

    // With a main checkout build, that is the source, whatever the lanes hold.
    let main = fx.main_target();
    let lane_c = fx.lane("kt-1250-worker-c", LOCK_A);
    let clone = RecordingClone::new();
    let record = warm(&lane_c, &ScriptedBuild::ok(), &clone, &BuildGate::new());
    match record.seed {
        SeedOutcome::Cloned { source, kind, .. } => {
            assert_eq!(source, main);
            assert_eq!(kind, SeedSourceKind::MainCheckout);
        }
        other => panic!("expected a clone of the main target, got {other:?}"),
    }
    let calls = clone.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].0 != lane_a.join(DEBUG_DIR) && calls[0].0 != forged.join(DEBUG_DIR),
        "{calls:?}"
    );
    assert_eq!(
        std::fs::read_to_string(lane_c.cargo_target_dir.join("debug/deps/libdep.rlib")).unwrap(),
        "main",
        "nothing a lane holds reached the new lane"
    );
}

#[test]
fn the_seed_source_is_outside_every_path_the_worker_may_write() {
    let fx = fixture();
    let main = fx.main_target();
    let lane = fx.lane("kt-1250-roots", LOCK_A);
    let source = main.join(DEBUG_DIR);
    assert!(
        lane.write_roots()
            .iter()
            .all(|root| !source.starts_with(root)),
        "{:?} vs {source:?}",
        lane.write_roots()
    );
    let home = HomeKey::of(&lane, &provisioned());
    let candidates = seed_candidates(&lane, &home, &[fx.target_root()]);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].target, main);
    assert_eq!(candidates[0].kind, SeedSourceKind::MainCheckout);
}

#[test]
fn the_main_checkout_target_is_the_fallback_source() {
    let fx = fixture();
    let main = fx.main_target();
    let lane = fx.lane("kt-1250-main", LOCK_A);
    let clone = RecordingClone::new();

    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());

    match record.seed {
        SeedOutcome::Cloned { source, kind, .. } => {
            assert_eq!(source, main);
            assert_eq!(kind, SeedSourceKind::MainCheckout);
        }
        other => panic!("expected a clone of the main target, got {other:?}"),
    }
    // Lane targets nested in `target/` are not dragged along: only `debug/`.
    assert_eq!(clone.calls()[0].0, main.join(DEBUG_DIR));
}

#[test]
fn no_seed_when_the_lock_hash_differs() {
    let fx = fixture();
    fx.warm_lane_target("kt-1200-other-lock", LOCK_B);
    let lane = fx.lane("kt-1250-lock", LOCK_A);
    let clone = RecordingClone::new();

    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());

    assert!(
        matches!(record.seed, SeedOutcome::NoSource { .. }),
        "{record:?}"
    );
    assert!(clone.calls().is_empty());
    assert!(!lane.cargo_target_dir.join(DEBUG_DIR).exists());
}

#[test]
fn a_lane_target_without_a_finished_build_is_not_a_source() {
    let fx = fixture();
    // `debug/` exists but no marker: its first build never completed.
    fx.write_build(&fx.target_root().join("wt-kt-1200-unfinished"), "x");
    let lane = fx.lane("kt-1250-unfinished", LOCK_A);
    let clone = RecordingClone::new();

    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());

    assert!(matches!(record.seed, SeedOutcome::NoSource { .. }));
    assert!(clone.calls().is_empty());
}

#[test]
fn a_target_being_written_is_never_a_source() {
    let fx = fixture();
    let busy = fx.main_target();
    let lane = fx.lane("kt-1250-busy", LOCK_A);
    // Cargo holds its build lock exclusively for the whole build.
    let building = std::fs::File::create(busy.join(DEBUG_DIR).join(CARGO_BUILD_LOCK)).unwrap();
    FileExt::lock_exclusive(&building).unwrap();
    let clone = RecordingClone::new();

    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());
    assert!(
        matches!(record.seed, SeedOutcome::NoSource { .. }),
        "{record:?}"
    );
    assert!(clone.calls().is_empty(), "the busy target was not cloned");

    // Once the build is done the same target is a valid source.
    FileExt::unlock(&building).unwrap();
    let lane = fx.lane("kt-1250-after", LOCK_A);
    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());
    assert!(
        matches!(record.seed, SeedOutcome::Cloned { .. }),
        "{record:?}"
    );
}

#[test]
fn a_source_cannot_start_a_build_while_it_is_being_cloned() {
    let fx = fixture();
    let source = fx.main_target();
    let lane = fx.lane("kt-1250-held", LOCK_A);
    let build_could_start = Arc::new(Mutex::new(None));
    let observed = build_could_start.clone();
    let lock_path = source.join(DEBUG_DIR).join(CARGO_BUILD_LOCK);

    let home = HomeKey::of(&lane, &provisioned());
    let outcome = seed_lane(&lane, &home, &[fx.target_root()], &|from, to| {
        let cargo = std::fs::File::open(&lock_path).unwrap();
        *observed.lock().unwrap() = Some(FileExt::try_lock_exclusive(&cargo).is_ok());
        copy_dir(from, to)
    });

    assert!(matches!(outcome, SeedOutcome::Cloned { .. }));
    assert_eq!(*build_could_start.lock().unwrap(), Some(false));
}

/// A unit of the seed as Cargo lays it out, last written `age_days` ago.
fn seed_unit(debug: &Path, hash: &str, age_days: u64) {
    let age = std::time::SystemTime::now() - Duration::from_secs(age_days * 24 * 3600);
    let fingerprint = debug.join(".fingerprint").join(format!("kronn-{hash}"));
    std::fs::create_dir_all(&fingerprint).unwrap();
    std::fs::write(fingerprint.join("test-lib-kronn"), "x").unwrap();
    // The fingerprint Cargo compares: two builds of the same configuration.
    std::fs::write(
        fingerprint.join("test-lib-kronn.json"),
        "{\"rustc\":13462371384788945141,\"features\":\"[\\\"default\\\"]\",\
         \"declared_features\":\"[]\",\"target\":7848994504142944354,\
         \"profile\":11056920130593593451,\"path\":7897662090217625916,\"deps\":[],\
         \"local\":[],\"rustflags\":[],\"config\":9396254390672932401,\"compile_kind\":0}",
    )
    .unwrap();
    for file in ["test-lib-kronn", "test-lib-kronn.json"] {
        std::fs::File::open(fingerprint.join(file))
            .unwrap()
            .set_modified(age)
            .unwrap();
    }
    std::fs::File::open(&fingerprint)
        .unwrap()
        .set_modified(age)
        .unwrap();
    std::fs::write(debug.join("deps").join(format!("kronn-{hash}")), "bin").unwrap();
}

/// KT-1691 — Cargo never removes an old artifact, so the seed grew to 174 GB of
/// which 91 GB were months-old test binaries, and every lane cloned (and then
/// rewrote part of) all of it. The seed is pruned to its latest build first.
#[test]
fn a_lane_never_clones_stale_seed_artifacts() {
    const STALE: &str = "aaaaaaaaaaaaaaaa";
    const LATEST: &str = "bbbbbbbbbbbbbbbb";
    let fx = fixture();
    let source = fx.main_target();
    let debug = source.join(DEBUG_DIR);
    seed_unit(&debug, STALE, 90);
    seed_unit(&debug, LATEST, 0);
    let lane = fx.lane("kt-1691-slim", LOCK_A);
    let seen_by_the_clone = Arc::new(Mutex::new(Vec::new()));
    let seen = seen_by_the_clone.clone();

    let home = HomeKey::of(&lane, &provisioned());
    let outcome = seed_lane(&lane, &home, &[fx.target_root()], &|from, to| {
        // What the clone is about to take, listed at the moment it takes it.
        let mut names: Vec<String> = std::fs::read_dir(from.join("deps"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        *seen.lock().unwrap() = names;
        copy_dir(from, to)
    });

    let SeedOutcome::Cloned { pruned, .. } = outcome else {
        panic!("expected a clone, got {outcome:?}");
    };
    assert_eq!(pruned.map(|report| report.removed_units), Some(1));
    let seen = seen_by_the_clone.lock().unwrap().clone();
    assert!(
        !seen.iter().any(|name| name.contains(STALE)),
        "the clone saw a stale artifact: {seen:?}"
    );
    assert!(seen.contains(&format!("kronn-{LATEST}")), "{seen:?}");
    let lane_debug = lane.cargo_target_dir.join(DEBUG_DIR);
    assert!(!lane_debug
        .join("deps")
        .join(format!("kronn-{STALE}"))
        .exists());
    assert!(!lane_debug
        .join(".fingerprint")
        .join(format!("kronn-{STALE}"))
        .exists());
    assert!(lane_debug
        .join("deps")
        .join(format!("kronn-{LATEST}"))
        .exists());
    // The seed itself is slimmed, so the next lane starts from the same.
    assert!(!debug.join("deps").join(format!("kronn-{STALE}")).exists());
    // What was already there (the warm artifact) is untouched.
    assert!(debug.join("deps").join("libdep.rlib").exists());
}

/// ...and never while a build uses it: a busy seed is neither pruned nor cloned.
#[test]
fn a_seed_a_build_is_writing_is_not_pruned() {
    let fx = fixture();
    let source = fx.main_target();
    let debug = source.join(DEBUG_DIR);
    seed_unit(&debug, "aaaaaaaaaaaaaaaa", 90);
    seed_unit(&debug, "bbbbbbbbbbbbbbbb", 0);
    let lane = fx.lane("kt-1691-busy", LOCK_A);
    let building = std::fs::File::open(debug.join(CARGO_BUILD_LOCK)).unwrap();
    FileExt::lock_exclusive(&building).unwrap();
    let clone = RecordingClone::new();

    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());

    assert!(matches!(record.seed, SeedOutcome::NoSource { .. }));
    assert!(clone.calls().is_empty());
    assert!(
        debug.join("deps").join("kronn-aaaaaaaaaaaaaaaa").exists(),
        "a seed in use is left exactly as it is"
    );
}

#[test]
fn an_unsupported_clone_leaves_the_target_empty_and_still_builds() {
    let fx = fixture();
    fx.main_target();
    let lane = fx.lane("kt-1250-nocow", LOCK_A);
    let runner = ScriptedBuild::ok();

    let record = warm_lane(
        &lane,
        &provisioned(),
        &settings(),
        &host(),
        &BuildGate::new(),
        &runner,
        &|_, destination| {
            // A half-made destination must not survive the failure.
            std::fs::create_dir_all(destination).unwrap();
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no reflink here",
            ))
        },
        |_| {},
    );

    match &record.seed {
        SeedOutcome::Unsupported { reason } => assert!(reason.contains("no reflink"), "{reason}"),
        other => panic!("expected the unsupported fallback, got {other:?}"),
    }
    assert!(!lane.cargo_target_dir.join(DEBUG_DIR).exists());
    let leftovers = std::fs::read_dir(&lane.cargo_target_dir)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains("seeding"));
    assert!(!leftovers);
    assert_eq!(runner.calls().len(), 1, "the worker's lane is built cold");
    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));
}

#[test]
fn an_existing_debug_directory_is_never_overwritten() {
    let fx = fixture();
    fx.main_target();
    let lane = fx.lane("kt-1250-present", LOCK_A);
    std::fs::create_dir_all(lane.cargo_target_dir.join(DEBUG_DIR)).unwrap();
    let clone = RecordingClone::new();

    let record = warm(&lane, &ScriptedBuild::ok(), &clone, &BuildGate::new());

    assert_eq!(record.seed, SeedOutcome::AlreadyPresent);
    assert!(clone.calls().is_empty());
}

/// GNU `cp --reflink=always` on a file system without reflinks (ext4) makes
/// the directories, then fails on the first file: the clone must remove them.
#[cfg(unix)] // asserts POSIX mode bits, which Windows does not have; runs a POSIX shell script fixture
#[test]
fn a_failed_cp_reflink_leaves_no_partial_tree() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join("cp");
    // The last two arguments are the source and the destination.
    std::fs::write(
        &cp,
        "#!/bin/sh\nfor last; do :; done\nmkdir -p \"$last/deps\"\n\
         echo \"cp: failed to clone: Operation not supported\" >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&cp, std::fs::Permissions::from_mode(0o755)).unwrap();
    let source = dir.path().join("src");
    std::fs::create_dir_all(source.join("deps")).unwrap();
    let destination = dir.path().join("dst");

    let error = cp_reflink_tree(cp.as_os_str(), &source, &destination).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(error.to_string().contains("not supported"), "{error}");
    assert!(
        !destination.exists(),
        "a failed clone leaves nothing behind"
    );

    // An existing destination is refused, never filled nor removed.
    std::fs::create_dir_all(destination.join("keep")).unwrap();
    let error = cp_reflink_tree(cp.as_os_str(), &source, &destination).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(destination.join("keep").is_dir());
}

#[cfg(unix)] // reads unix inode metadata
#[test]
fn clone_tree_is_copy_on_write_not_a_shared_or_a_copied_tree() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("src");
    std::fs::create_dir_all(source.join("deps")).unwrap();
    std::fs::write(source.join("deps").join("lib.rlib"), "original").unwrap();
    let destination = dir.path().join("dst");

    match clone_tree(&source, &destination) {
        Ok(()) => {}
        // The temp dir's file system cannot clone (not APFS / no reflink):
        // the unsupported path is what the seeding fallback test covers.
        Err(error) => {
            eprintln!("clone unsupported on this file system: {error}");
            assert!(
                !destination.exists(),
                "a failed clone leaves nothing behind"
            );
            return;
        }
    }

    let cloned = destination.join("deps").join("lib.rlib");
    let original = source.join("deps").join("lib.rlib");
    assert_eq!(std::fs::read_to_string(&cloned).unwrap(), "original");
    assert_ne!(
        std::fs::metadata(&cloned).unwrap().ino(),
        std::fs::metadata(&original).unwrap().ino(),
        "a clone is its own file, not a hard link"
    );
    std::fs::write(&cloned, "changed in the lane").unwrap();
    assert_eq!(
        std::fs::read_to_string(&original).unwrap(),
        "original",
        "writing the clone never reaches the source"
    );
}

// ── First build, marker and record ───────────────────────────────────────

#[test]
fn a_successful_first_build_writes_the_marker_and_records_its_duration() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-marker", LOCK_A);

    let record = warm(
        &lane,
        &ScriptedBuild::ok(),
        &RecordingClone::new(),
        &BuildGate::new(),
    );

    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));
    assert_eq!(record.build_jobs, 3);
    assert_eq!(
        read_marker(&lane.cargo_target_dir).map(|marker| marker.lock_hash),
        Some(hash_of(LOCK_A))
    );
}

#[test]
fn a_failed_or_timed_out_build_is_never_marked_built() {
    let fx = fixture();
    let failed_lane = fx.lane("kt-1250-failed", LOCK_A);
    let failed = warm(
        &failed_lane,
        &ScriptedBuild::with(false, "exited with 101: error[E0432]"),
        &RecordingClone::new(),
        &BuildGate::new(),
    );
    assert!(
        matches!(&failed.first_build, FirstBuild::Failed { detail, .. } if detail.contains("E0432")),
        "{failed:?}"
    );
    // KT-1742: the failure is remembered per root (so a relaunch does not run it
    // again), never as a build that succeeded.
    let marker = read_marker(&failed_lane.cargo_target_dir).expect("the failure is recorded");
    assert_eq!(marker.roots.len(), 1);
    assert!(matches!(marker.roots[0].state, RootState::Failed { .. }));

    // Another lock: the first lane's failure is cached for LOCK_A (KT-1742).
    let slow_lane = fx.lane("kt-1250-slow", LOCK_B);
    let slow = warm(
        &slow_lane,
        &ScriptedBuild::with(false, "timed out after 60s"),
        &RecordingClone::new(),
        &BuildGate::new(),
    );
    assert!(
        matches!(slow.first_build, FirstBuild::TimedOut { .. }),
        "{slow:?}"
    );
    assert!(read_marker(&slow_lane.cargo_target_dir).is_none());
}

#[test]
fn a_relaunched_lane_that_is_already_built_is_not_built_again() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-relaunch", LOCK_A);
    let first = ScriptedBuild::ok();
    warm(&lane, &first, &RecordingClone::new(), &BuildGate::new());
    // The first build left a profile directory behind (as a real one does).
    std::fs::create_dir_all(lane.cargo_target_dir.join(DEBUG_DIR)).unwrap();

    let second = ScriptedBuild::ok();
    let record = warm(&lane, &second, &RecordingClone::new(), &BuildGate::new());

    assert!(second.calls().is_empty());
    assert!(matches!(record.first_build, FirstBuild::Skipped { .. }));
}

#[test]
fn a_changed_lock_rebuilds_even_in_a_marked_lane() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-relock", LOCK_A);
    warm(
        &lane,
        &ScriptedBuild::ok(),
        &RecordingClone::new(),
        &BuildGate::new(),
    );
    std::fs::create_dir_all(lane.cargo_target_dir.join(DEBUG_DIR)).unwrap();
    std::fs::write(lane.worktree.join("backend").join("Cargo.lock"), LOCK_B).unwrap();

    let runner = ScriptedBuild::ok();
    warm(&lane, &runner, &RecordingClone::new(), &BuildGate::new());

    assert_eq!(runner.calls().len(), 1);
    assert_eq!(
        read_marker(&lane.cargo_target_dir).map(|marker| marker.lock_hash),
        Some(hash_of(LOCK_B))
    );
}

#[test]
fn a_worktree_without_a_crate_is_not_applicable() {
    let fx = fixture();
    let worktree = fx.repo.join(".kronn/worktrees/task-kt-1250-nocrate");
    std::fs::create_dir_all(&worktree).unwrap();
    let lane = WorkerLane::for_worktree(&worktree).unwrap();
    let runner = ScriptedBuild::ok();

    let record = warm(&lane, &runner, &RecordingClone::new(), &BuildGate::new());

    assert!(matches!(record.seed, SeedOutcome::NotApplicable { .. }));
    assert!(runner.calls().is_empty());
}

#[test]
fn the_warm_start_record_roundtrips_as_json() {
    let record = WarmStartRecord {
        lock_hash: Some("abc".into()),
        seed: SeedOutcome::Cloned {
            source: PathBuf::from("/repo/target"),
            kind: SeedSourceKind::MainCheckout,
            clone_ms: 12,
            pruned: None,
            aligned: None,
        },
        build_jobs: 4,
        first_build: FirstBuild::Succeeded {
            duration_ms: 90_000,
        },
        queued_ms: 5,
        provision_ms: Some(7),
        seed_ms: Some(13),
        crates: Vec::new(),
    };
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["seed"]["outcome"], "cloned");
    assert_eq!(json["first_build"]["outcome"], "succeeded");
    assert_eq!(
        serde_json::from_value::<WarmStartRecord>(json).unwrap(),
        record
    );

    let cancelled = serde_json::to_value(FirstBuild::Cancelled { duration_ms: 7 }).unwrap();
    assert_eq!(cancelled["outcome"], "cancelled");
}

#[test]
fn a_warm_start_row_journalled_before_the_timings_still_reads() {
    // KT-1637: `provision_ms` and `seed_ms` were added; older rows have neither.
    let record: WarmStartRecord = serde_json::from_value(serde_json::json!({
        "lock_hash": "abc",
        "seed": { "outcome": "already_present" },
        "build_jobs": 4,
        "first_build": { "outcome": "succeeded", "duration_ms": 1 },
        "queued_ms": 2
    }))
    .unwrap();
    assert_eq!((record.provision_ms, record.seed_ms), (None, None));
    // An old row cannot say how long it provisioned or seeded: the breakdown
    // leaves both out rather than reporting 0 ms.
    use crate::models::WarmingStepKind as Step;
    assert_eq!(
        record.timed_steps(),
        vec![(Step::GateWait, 2), (Step::FirstBuild, 1)]
    );
}

fn timed(first_build: FirstBuild, queued_ms: u64) -> WarmStartRecord {
    WarmStartRecord {
        lock_hash: Some("abc".into()),
        seed: SeedOutcome::Cloned {
            source: PathBuf::from("/repo/target"),
            kind: SeedSourceKind::MainCheckout,
            clone_ms: 30,
            pruned: None,
            aligned: None,
        },
        build_jobs: 4,
        first_build,
        queued_ms,
        provision_ms: Some(11),
        seed_ms: Some(35),
        crates: Vec::new(),
    }
}

#[test]
fn a_built_lane_reports_each_warming_step_in_run_order() {
    use crate::models::WarmingStepKind as Step;
    let record = timed(FirstBuild::Succeeded { duration_ms: 400 }, 20);
    assert_eq!(
        record.timed_steps(),
        vec![
            (Step::LaneProvision, 11),
            (Step::SeedClone, 35),
            (Step::GateWait, 20),
            (Step::FirstBuild, 400),
        ]
    );
}

#[test]
fn a_step_that_did_not_run_is_absent_not_zero() {
    use crate::models::WarmingStepKind as Step;
    // Already built: no queue, no build, but the lane was still provisioned.
    let built = timed(
        FirstBuild::Skipped {
            reason: "already built".into(),
        },
        0,
    );
    assert_eq!(
        built.timed_steps(),
        vec![(Step::LaneProvision, 11), (Step::SeedClone, 35)]
    );
    // A queue that never drained spent its wait, and never built.
    let gave_up = timed(
        FirstBuild::Skipped {
            reason: "no slot".into(),
        },
        1_800_000,
    );
    assert!(gave_up.timed_steps().contains(&(Step::GateWait, 1_800_000)));
    assert!(!gave_up
        .timed_steps()
        .iter()
        .any(|(step, _)| *step == Step::FirstBuild));
    // Cancelled while queued: it never started compiling.
    let cancelled = timed(FirstBuild::Cancelled { duration_ms: 0 }, 5);
    assert!(!cancelled
        .timed_steps()
        .iter()
        .any(|(step, _)| *step == Step::FirstBuild));
    // Nothing prepared, nothing seeded, nothing provisioned.
    let none = WarmStartRecord::not_applicable("no Cargo.toml", 4);
    assert!(none.timed_steps().is_empty());
    // No warm target to clone from: no seed step either, even though the lane
    // was provisioned.
    let mut no_source = timed(FirstBuild::Succeeded { duration_ms: 9 }, 0);
    no_source.seed = SeedOutcome::NoSource {
        reason: "no warm target".into(),
    };
    assert!(!no_source
        .timed_steps()
        .iter()
        .any(|(step, _)| *step == Step::SeedClone));
    assert!(no_source.timed_steps().contains(&(Step::LaneProvision, 11)));
}

#[test]
fn a_row_journalled_before_seed_ms_falls_back_to_its_clone_time() {
    use crate::models::WarmingStepKind as Step;
    let mut record = timed(FirstBuild::Succeeded { duration_ms: 1 }, 0);
    record.seed_ms = None;
    assert!(record.timed_steps().contains(&(Step::SeedClone, 30)));
}

#[test]
fn a_journal_row_from_the_lane_seeding_era_still_reads() {
    let old: SeedOutcome = serde_json::from_value(serde_json::json!({
        "outcome": "cloned",
        "source": "/repo/target/wt-old",
        "kind": "lane",
        "clone_ms": 3
    }))
    .unwrap();
    assert!(matches!(
        old,
        SeedOutcome::Cloned {
            kind: SeedSourceKind::Retired,
            ..
        }
    ));
}

// ── Host gate ────────────────────────────────────────────────────────────

#[test]
fn the_gate_never_lets_more_than_the_limit_run() {
    let gate = BuildGate::new();
    let first = gate.try_acquire(2).expect("first slot");
    let second = gate.try_acquire(2).expect("second slot");
    assert!(gate.try_acquire(2).is_none(), "the third is refused");
    assert_eq!(gate.running(), 2);
    drop(first);
    assert!(gate.try_acquire(2).is_some());
    drop(second);
}

#[test]
fn the_extra_worker_queues_says_so_and_runs_when_a_slot_frees() {
    let gate = Arc::new(BuildGate::new());
    let held = gate.try_acquire(1).expect("the running build");
    let announced = Arc::new(AtomicUsize::new(0));
    let (done_tx, done_rx) = mpsc::channel();

    let waiter = {
        let gate = gate.clone();
        let announced = announced.clone();
        std::thread::spawn(move || {
            let _permit = gate.acquire(1, |ahead| {
                announced.store(ahead, Ordering::SeqCst);
            });
            done_tx.send(()).unwrap();
        })
    };

    // It said it was queued, and has not started.
    let deadline = Instant::now() + Duration::from_secs(5);
    while announced.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(announced.load(Ordering::SeqCst), 1, "one build ahead");
    assert!(
        done_rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "the extra worker waits while the slot is taken"
    );

    drop(held);
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the queued worker starts once the slot frees");
    waiter.join().unwrap();
    assert_eq!(gate.running(), 0);
}

#[test]
fn a_second_lane_waits_for_the_first_build_and_reports_the_queue() {
    let fx = fixture();
    let first_lane = fx.lane("kt-1250-first", LOCK_A);
    let second_lane = fx.lane("kt-1250-second", LOCK_A);
    let gate = Arc::new(BuildGate::new());
    let settings = WarmStartSettings::resolve(Some("2"), Some("1"), Some("60"), 8);

    // The first build blocks inside the runner until released.
    struct BlockingRunner {
        started: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        builds: AtomicUsize,
    }
    impl ProbeRunner for BlockingRunner {
        fn run(&self, _: &ProbeCommand) -> ProbeOutcome {
            if self.builds.fetch_add(1, Ordering::SeqCst) == 0 {
                self.started.send(()).unwrap();
                let _ = self.release.lock().unwrap().recv();
            }
            ProbeOutcome {
                success: true,
                detail: String::new(),
            }
        }
    }
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let runner = Arc::new(BlockingRunner {
        started: started_tx,
        release: Mutex::new(release_rx),
        builds: AtomicUsize::new(0),
    });

    let first = {
        let (gate, runner, settings) = (gate.clone(), runner.clone(), settings);
        std::thread::spawn(move || {
            warm_lane(
                &first_lane,
                &provisioned(),
                &settings,
                &host(),
                &gate,
                runner.as_ref(),
                &|_, _| Err(io::Error::from(io::ErrorKind::Unsupported)),
                |_| {},
            )
        })
    };
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the first build is running");

    let queued = Arc::new(AtomicUsize::new(0));
    let second = {
        let (gate, runner, settings, queued) =
            (gate.clone(), runner.clone(), settings, queued.clone());
        std::thread::spawn(move || {
            warm_lane(
                &second_lane,
                &provisioned(),
                &settings,
                &host(),
                &gate,
                runner.as_ref(),
                &|_, _| Err(io::Error::from(io::ErrorKind::Unsupported)),
                |ahead| queued.store(ahead, Ordering::SeqCst),
            )
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while queued.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(queued.load(Ordering::SeqCst), 1, "the second lane queued");
    assert_eq!(
        runner.builds.load(Ordering::SeqCst),
        1,
        "the second build has not started"
    );

    release_tx.send(()).unwrap();
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    assert_eq!(runner.builds.load(Ordering::SeqCst), 2);
    assert_eq!(first.queued_ms, 0);
    assert!(matches!(second.first_build, FirstBuild::Succeeded { .. }));
    assert_eq!(gate.running(), 0);
}

#[test]
fn a_lane_under_a_private_cargo_home_is_not_seeded_from_the_main_target() {
    // Cargo fingerprints a registry crate by the absolute path of its sources:
    // a clone of a target built under the host's CARGO_HOME recompiles every
    // dependency under another one, so cloning it would cost and gain nothing.
    let fx = fixture();
    fx.main_target();
    let lane = fx.lane("kt-1250-private", LOCK_A);
    let clone = RecordingClone::new();

    let record = warm_under(
        &lane,
        &provisioned_with_home(None),
        &ScriptedBuild::ok(),
        &clone,
    );

    assert!(
        matches!(record.seed, SeedOutcome::NoSource { .. }),
        "{:?}",
        record.seed
    );
    assert!(clone.calls().is_empty(), "nothing was cloned");
}

#[test]
fn a_lane_under_the_host_cargo_home_is_seeded_from_the_main_target() {
    let fx = fixture();
    let main = fx.main_target();
    let lane = fx.lane("kt-1250-shared", LOCK_A);
    let clone = RecordingClone::new();

    let record = warm_under(
        &lane,
        &provisioned_with_home(Some(Path::new("/host/.cargo"))),
        &ScriptedBuild::ok(),
        &clone,
    );

    match record.seed {
        SeedOutcome::Cloned { source, kind, .. } => {
            assert_eq!(source, main);
            assert_eq!(kind, SeedSourceKind::MainCheckout);
        }
        other => panic!("expected a clone of the main target, got {other:?}"),
    }
}

#[test]
fn a_warmed_lane_records_its_cargo_home_and_seeds_no_other_lane() {
    let fx = fixture();
    let host = Path::new("/host/.cargo");
    let ok = ScriptedBuild::ok();

    let first = fx.lane("kt-1250-a", LOCK_A);
    fx.write_build(&first.cargo_target_dir, "a");
    let built = warm_under(
        &first,
        &provisioned_with_home(Some(host)),
        &ok,
        &RecordingClone::new(),
    );
    assert!(matches!(built.first_build, FirstBuild::Succeeded { .. }));
    assert_eq!(
        read_marker(&first.cargo_target_dir)
            .unwrap()
            .cargo_home
            .as_deref(),
        Some(host),
        "the marker records the home the build ran under"
    );

    // Even a lane that just finished a build under the very same home, for the
    // very same lock, seeds nobody: lane targets are worker-writable.
    let same_home = fx.lane("kt-1250-b", LOCK_A);
    let clone = RecordingClone::new();
    let unseeded = warm_under(&same_home, &provisioned_with_home(Some(host)), &ok, &clone);
    assert!(
        matches!(unseeded.seed, SeedOutcome::NoSource { .. }),
        "{:?}",
        unseeded.seed
    );
    assert!(clone.calls().is_empty());
}

#[test]
fn a_bounded_wait_gives_up_when_the_gate_never_drains_and_runs_when_it_does() {
    let gate = BuildGate::new();
    let held = gate.try_acquire(1).expect("the running build");

    let never = CancellationToken::new();
    let mut announced = None;
    let refused = gate.acquire_within(1, Duration::from_millis(50), &never, |ahead| {
        announced = Some(ahead)
    });
    assert!(
        matches!(refused, GateWait::TimedOut),
        "a full gate refuses after the bound"
    );
    assert_eq!(announced, Some(1), "it still said it was queued");
    assert_eq!(gate.running(), 1, "a refused wait takes no slot");

    drop(held);
    let granted = gate.acquire_within(1, Duration::from_millis(50), &never, |_| {});
    assert!(
        matches!(granted, GateWait::Acquired(_)),
        "a free gate grants at once"
    );
    assert_eq!(gate.running(), 1);
}

#[test]
fn cancelling_a_queued_first_build_leaves_the_gate_without_taking_a_slot() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-cancel-queued", LOCK_A);
    let gate = Arc::new(BuildGate::new());
    let _held = gate.try_acquire(1).expect("the running build");
    let runner = Arc::new(ScriptedBuild::ok());
    let settings = WarmStartSettings::resolve(Some("2"), Some("1"), Some("600"), 8);
    let cancel = CancellationToken::new();
    let (queued_tx, queued_rx) = mpsc::channel();

    let waiter = {
        let (gate, runner, cancel) = (gate.clone(), runner.clone(), cancel.clone());
        std::thread::spawn(move || {
            warm_lane_cancellable(
                &lane,
                &provisioned(),
                &settings,
                &host(),
                &gate,
                runner.as_ref(),
                &|_, _| Err(io::Error::from(io::ErrorKind::Unsupported)),
                &cancel,
                |ahead| queued_tx.send(ahead).unwrap(),
            )
        })
    };
    queued_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the second build queued behind the running one");
    let cancelled_at = Instant::now();
    cancel.cancel();
    let record = waiter.join().unwrap();

    assert!(
        cancelled_at.elapsed() < Duration::from_secs(5),
        "the queue wait ended with the cancel, not with its 600s bound"
    );
    assert!(
        matches!(record.first_build, FirstBuild::Cancelled { .. }),
        "{record:?}"
    );
    assert!(runner.calls().is_empty(), "the queued build never started");
    assert_eq!(gate.running(), 1, "only the running build holds a slot");
}

#[test]
fn cancelling_a_running_first_build_stops_it_and_frees_its_slot() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-cancel-running", LOCK_A);
    let gate = Arc::new(BuildGate::new());
    let settings = WarmStartSettings::resolve(Some("2"), Some("1"), Some("600"), 8);
    let cancel = CancellationToken::new();

    /// A build that runs until its cancellation fires.
    struct UntilCancelled {
        started: mpsc::Sender<()>,
    }
    impl ProbeRunner for UntilCancelled {
        fn run(&self, _: &ProbeCommand) -> ProbeOutcome {
            panic!("the first build must go through run_cancellable");
        }

        fn run_cancellable(&self, _: &ProbeCommand, cancel: &CancellationToken) -> ProbeOutcome {
            self.started.send(()).unwrap();
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            ProbeOutcome {
                success: false,
                detail: "cancelled".into(),
            }
        }
    }
    let (started_tx, started_rx) = mpsc::channel();
    let runner = Arc::new(UntilCancelled {
        started: started_tx,
    });

    let build = {
        let (gate, runner, cancel, lane) =
            (gate.clone(), runner.clone(), cancel.clone(), lane.clone());
        std::thread::spawn(move || {
            warm_lane_cancellable(
                &lane,
                &provisioned(),
                &settings,
                &host(),
                &gate,
                runner.as_ref(),
                &|_, _| Err(io::Error::from(io::ErrorKind::Unsupported)),
                &cancel,
                |_| {},
            )
        })
    };
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the first build is running");
    assert_eq!(gate.running(), 1);

    cancel.cancel();
    let record = build.join().unwrap();

    assert!(
        matches!(record.first_build, FirstBuild::Cancelled { .. }),
        "{record:?}"
    );
    assert_eq!(gate.running(), 0, "the slot is free again");
    assert!(
        read_marker(&lane.cargo_target_dir).is_none(),
        "a cancelled build is never marked warm"
    );
    // The freed slot is usable at once by the next worker.
    assert!(gate.try_acquire(1).is_some());
}

#[cfg(unix)] // runs a POSIX shell script fixture; relies on POSIX process groups and signals
#[test]
fn the_process_runner_kills_a_cancelled_build_and_what_it_started() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    // A shell (standing for cargo) that starts a long-lived child (rustc).
    let script = format!("sleep 300 & echo $! > '{}'; wait", pid_file.display());
    let probe = ProbeCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script],
        cwd: dir.path().to_path_buf(),
        env: Vec::new(),
        timeout: Duration::from_secs(120),
        priority: crate::core::background_priority::Priority::Normal,
        clean_env: false,
    };
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let watcher = {
        let pid_file = pid_file.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !pid_file.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            trigger.cancel();
        })
    };

    let started = Instant::now();
    let outcome = crate::core::task_worker_env::ProcessProbeRunner.run_cancellable(&probe, &cancel);
    watcher.join().unwrap();

    assert!(!outcome.success);
    assert_eq!(outcome.detail, "cancelled");
    assert!(started.elapsed() < Duration::from_secs(30));
    let grandchild: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut alive = true;
    for _ in 0..100 {
        // SAFETY: signal 0 only probes whether the pid exists.
        alive = unsafe { libc::kill(grandchild, 0) } == 0;
        if !alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive, "the build's own child was killed with it");
}

#[test]
fn a_worker_stuck_behind_a_full_gate_starts_with_the_target_it_has() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-stuck", LOCK_A);
    let gate = BuildGate::new();
    let _held = gate.try_acquire(1).expect("the running build");
    let runner = ScriptedBuild::ok();
    let settings = WarmStartSettings::resolve(Some("2"), Some("1"), Some("1"), 8);

    let started = Instant::now();
    let record = warm_lane(
        &lane,
        &provisioned(),
        &settings,
        &host(),
        &gate,
        &runner,
        &|_, _| Err(io::Error::from(io::ErrorKind::Unsupported)),
        |_| {},
    );

    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(
        matches!(record.first_build, FirstBuild::Skipped { .. }),
        "{record:?}"
    );
    assert!(runner.calls().is_empty(), "no build ran outside the gate");
    assert!(
        read_marker(&lane.cargo_target_dir).is_none(),
        "an unbuilt lane is never marked warm"
    );
}

#[test]
fn a_first_build_is_pending_only_for_a_lock_that_is_not_already_built() {
    let fx = fixture();
    let lane = fx.lane("kt-1250-pending", LOCK_A);
    assert!(first_build_pending(&lane.worktree));

    fx.write_build(&lane.cargo_target_dir, "built");
    write_marker(
        &lane,
        &hash_of(LOCK_A),
        &HomeKey::of(&lane, &provisioned()),
        Vec::new(),
    )
    .unwrap();
    assert!(
        !first_build_pending(&lane.worktree),
        "a lane built for this lock has nothing left to build"
    );

    std::fs::write(lane.worktree.join("backend").join("Cargo.lock"), LOCK_B).unwrap();
    assert!(
        first_build_pending(&lane.worktree),
        "a changed lock needs a build again"
    );

    let none = fx.repo.join("no-crate");
    std::fs::create_dir_all(&none).unwrap();
    assert!(!first_build_pending(&none), "no crate, no first build");
}

// ── KT-1742: seed alignment, nested crates, projects that are not Cargo ones ──

use crate::core::task_worker_seed_align::SEED_STAMP;

const CC_ENV: &str = "CFLAGS_test_target";
const CC_FLAG: &str = "-save-temps=obj";

fn cc_env() -> Vec<(String, String)> {
    vec![(CC_ENV.to_string(), CC_FLAG.to_string())]
}

/// Where a crate builds in a main checkout whose crate has no `.cargo/config.toml`:
/// its own `target/` (what `cargo metadata` answers; Cargo itself is not run).
fn main_metadata(manifest_dir: &Path) -> Result<CargoMetadata, String> {
    Ok(CargoMetadata {
        target_directory: manifest_dir.join("target"),
        workspace_root: manifest_dir.to_path_buf(),
    })
}

fn aligned_host(seed_runner: &dyn ProbeRunner) -> WarmHost<'_> {
    WarmHost {
        seed_alignment: true,
        compiler_env: cc_env(),
        first_build_command: None,
        rust_roots: None,
        metadata: &main_metadata,
        seed_runner,
        lock_wait: Duration::from_millis(150),
    }
}

impl Fixture {
    /// A lane whose worktree holds a Rust crate at each `(dir, lock)`.
    fn crate_lane(&self, name: &str, crates: &[(&str, &str)]) -> WorkerLane {
        let worktree = self
            .repo
            .join(".kronn")
            .join("worktrees")
            .join(format!("task-{name}"));
        for (rel, lock) in crates {
            let dir = worktree.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("Cargo.toml"), "").unwrap();
            std::fs::write(dir.join("Cargo.lock"), lock).unwrap();
        }
        WorkerLane::for_worktree(&worktree).unwrap()
    }

    /// The same crate in the main checkout, with a build in the crate's own
    /// `target/`. Returns that target.
    fn main_crate(&self, rel: &str, lock: &str) -> PathBuf {
        let dir = self.repo.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("Cargo.lock"), lock).unwrap();
        let target = dir.join("target");
        self.write_build(&target, "main");
        target
    }

    /// A worktree with no Cargo workspace at all (a PHP / JavaScript project).
    fn plain_lane(&self, name: &str) -> WorkerLane {
        let worktree = self
            .repo
            .join(".kronn")
            .join("worktrees")
            .join(format!("task-{name}"));
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("package.json"), "{}").unwrap();
        WorkerLane::for_worktree(&worktree).unwrap()
    }
}

fn warm_with(
    lane: &WorkerLane,
    host: &WarmHost<'_>,
    runner: &dyn ProbeRunner,
    clone: &RecordingClone,
) -> WarmStartRecord {
    warm_lane(
        lane,
        &provisioned(),
        &settings(),
        host,
        &BuildGate::new(),
        runner,
        &|source, destination| clone.call(source, destination),
        |_| {},
    )
}

fn env_of(call: &ProbeCommand, name: &str) -> Option<OsString> {
    call.env
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.clone())
}

/// KT-1742: a project that is NOT Kronn (a crate in `redis-flush/`, its own
/// `target/`, no repository-level Cargo config) gets its seed built once with the
/// lane's C-flag environment, then cloned; the next lane finds the stamp and
/// rebuilds nothing; a changed `Cargo.lock` of the main checkout aligns it again.
#[test]
fn a_project_that_is_not_kronn_gets_its_seed_aligned_once_and_the_next_lane_reuses_it() {
    let fx = fixture();
    let target = fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::ok();
    let host = aligned_host(&seed_build);
    let first_build = ScriptedBuild::ok();

    let first = fx.crate_lane("kt-1742-a", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&first, &host, &first_build, &RecordingClone::new());

    let SeedOutcome::Cloned {
        source, aligned, ..
    } = &record.seed
    else {
        panic!("expected a clone, got {record:?}");
    };
    assert_eq!(source, &target, "the seed is the crate's own target");
    assert!(
        matches!(aligned, Some(SeedAlignment::Aligned { rebuilt: true, .. })),
        "{aligned:?}"
    );
    let calls = seed_build.calls();
    assert_eq!(calls.len(), 1, "one seed build");
    assert_eq!(calls[0].program, "cargo");
    assert_eq!(calls[0].args, ["build", "--locked", "--offline"]);
    assert_eq!(calls[0].cwd, fx.repo.join("redis-flush"));
    assert_eq!(env_of(&calls[0], CC_ENV), Some(OsString::from(CC_FLAG)));
    assert_eq!(
        env_of(&calls[0], "CARGO_TARGET_DIR"),
        Some(target.clone().into_os_string()),
        "the build writes the seed's own target, not the lane's"
    );
    assert!(
        env_of(&calls[0], "KRONN_PORT").is_none(),
        "nothing of the lane's ports or data reaches the owner's build"
    );
    assert!(
        target.join(SEED_STAMP).is_file(),
        "the stamp beside the seed"
    );
    // The lane's own build agrees with the seed's environment by construction.
    let lane_calls = first_build.calls();
    assert_eq!(lane_calls.len(), 1);
    assert_eq!(
        env_of(&lane_calls[0], CC_ENV),
        Some(OsString::from(CC_FLAG)),
        "the first build runs with the seed's compiler environment"
    );
    assert_eq!(lane_calls[0].cwd, first.worktree.join("redis-flush"));

    // A second lane of the project reuses the aligned seed: no build.
    let second = fx.crate_lane("kt-1742-b", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&second, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    assert_eq!(
        record.crates[0].seed_aligned,
        SeedAlignment::Aligned {
            rebuilt: false,
            build_ms: 0
        }
    );
    assert_eq!(seed_build.calls().len(), 1, "the stamp matched: no rebuild");

    // The main checkout moved on to another Cargo.lock: aligned again.
    std::fs::write(
        fx.repo.join("redis-flush").join("Cargo.lock"),
        format!("{LOCK_CC}\n[[package]]\nname = \"extra\"\nversion = \"1.0.0\"\n"),
    )
    .unwrap();
    let third = fx.crate_lane("kt-1742-c", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&third, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    assert!(
        matches!(
            record.crates[0].seed_aligned,
            SeedAlignment::Aligned { rebuilt: true, .. }
        ),
        "{:?}",
        record.crates[0].seed_aligned
    );
    assert_eq!(
        seed_build.calls().len(),
        2,
        "a changed lock aligns once more"
    );
}

/// KT-1742: the seed is the owner's own target; while a build of theirs holds
/// Cargo's lock on it Kronn builds nothing in it, says so in the record, and the
/// lane falls back to what it did before (a seed being written is not cloned).
#[test]
fn a_seed_the_owner_is_building_is_left_alone_and_the_fallback_is_reported() {
    let fx = fixture();
    let target = fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::ok();
    let host = aligned_host(&seed_build);

    let owner_build = std::fs::File::open(target.join(DEBUG_DIR).join(CARGO_BUILD_LOCK)).unwrap();
    FileExt::try_lock_exclusive(&owner_build).unwrap();
    let lane = fx.crate_lane("kt-1742-held", &[("redis-flush", LOCK_CC)]);
    let started = Instant::now();
    let record = warm_with(&lane, &host, &ScriptedBuild::ok(), &RecordingClone::new());

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the wait for the lock is bounded"
    );
    let SeedAlignment::NotAligned { reason } = &record.crates[0].seed_aligned else {
        panic!("expected a reported fallback, got {record:?}");
    };
    assert!(reason.contains("holds the seed's Cargo lock"), "{reason}");
    assert!(
        seed_build.calls().is_empty(),
        "nothing built in their target"
    );
    assert!(
        matches!(record.seed, SeedOutcome::NoSource { .. }),
        "a seed a build is writing is not cloned: {record:?}"
    );
    assert!(!lane.cargo_target_dir.join(DEBUG_DIR).exists());

    // Their build over, the next lane aligns and clones.
    FileExt::unlock(&owner_build).unwrap();
    let next = fx.crate_lane("kt-1742-free", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&next, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    assert!(
        matches!(
            record.crates[0].seed_aligned,
            SeedAlignment::Aligned { rebuilt: true, .. }
        ),
        "{record:?}"
    );
    assert!(matches!(record.seed, SeedOutcome::Cloned { .. }));
}

/// KT-1742: a failed seed build is reported and never blocks the lane.
#[test]
fn a_failed_seed_build_is_reported_and_the_seed_is_cloned_as_it_is() {
    let fx = fixture();
    fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::with(false, "error: linker `cc` not found");
    let host = aligned_host(&seed_build);
    let lane = fx.crate_lane("kt-1742-failed", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&lane, &host, &ScriptedBuild::ok(), &RecordingClone::new());

    let SeedAlignment::NotAligned { reason } = &record.crates[0].seed_aligned else {
        panic!("{record:?}");
    };
    assert!(reason.contains("linker `cc` not found"), "{reason}");
    assert!(
        matches!(record.seed, SeedOutcome::Cloned { .. }),
        "{record:?}"
    );
    assert!(
        !fx.repo
            .join("redis-flush")
            .join("target")
            .join(SEED_STAMP)
            .exists(),
        "no stamp for a build that did not finish"
    );
}

/// KT-1742: the warm-up does not assume Kronn's layout. Every Rust workspace
/// root found from the manifests is built, seeded from its own target and
/// aligned; the lane has one `debug/`, so the seeds are merged into it.
#[test]
fn every_rust_root_of_the_repository_is_warmed_seeded_and_aligned_wherever_it_is() {
    let fx = fixture();
    let redis = fx.main_crate("redis-flush", LOCK_CC);
    let exporter = fx.main_crate("tools/exporter", LOCK_B);
    std::fs::write(
        exporter
            .join(DEBUG_DIR)
            .join("deps")
            .join("libexporter.rlib"),
        "exporter",
    )
    .unwrap();
    let seed_build = ScriptedBuild::ok();
    let host = aligned_host(&seed_build);
    let runner = ScriptedBuild::ok();
    let lane = fx.crate_lane(
        "kt-1742-roots",
        &[("redis-flush", LOCK_CC), ("tools/exporter", LOCK_B)],
    );

    let record = warm_with(&lane, &host, &runner, &RecordingClone::new());

    let paths: Vec<&str> = record.crates.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["redis-flush", "tools/exporter"], "{record:?}");
    assert_eq!(record.crates[0].seed_target.as_ref(), Some(&redis));
    assert_eq!(record.crates[1].seed_target.as_ref(), Some(&exporter));
    assert!(record
        .crates
        .iter()
        .all(|c| matches!(c.seed_aligned, SeedAlignment::Aligned { rebuilt: true, .. })));
    let builds = runner.calls();
    assert_eq!(builds.len(), 2, "one build per root");
    assert_eq!(builds[0].cwd, lane.worktree.join("redis-flush"));
    assert_eq!(builds[1].cwd, lane.worktree.join("tools/exporter"));
    assert_eq!(seed_build.calls().len(), 2, "one seed build per root");
    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));
    let deps = lane.cargo_target_dir.join(DEBUG_DIR).join("deps");
    assert!(deps.join("libdep.rlib").is_file(), "the first seed's units");
    assert!(
        deps.join("libexporter.rlib").is_file(),
        "the second workspace's units are merged into the lane"
    );
    let roots = project_toolchains::rust_roots(&lane.worktree);
    assert_eq!(
        read_marker(&lane.cargo_target_dir).map(|m| m.lock_hash),
        project_toolchains::combined_lock_hash(&roots),
        "the marker covers every root's lock"
    );
}

/// KT-1742: where a crate builds is asked of Cargo, in the main checkout; when
/// it cannot say, the lane has no seed and the record names the crate.
#[test]
fn a_crate_whose_target_cannot_be_resolved_has_no_seed_and_the_record_says_which() {
    fn unresolved(_: &Path) -> Result<CargoMetadata, String> {
        Err("cargo metadata failed (exit status: 101)".into())
    }
    let fx = fixture();
    fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::ok();
    let mut host = aligned_host(&seed_build);
    host.metadata = &unresolved;
    let lane = fx.crate_lane("kt-1742-unresolved", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&lane, &host, &ScriptedBuild::ok(), &RecordingClone::new());

    let SeedOutcome::NoSource { reason } = &record.seed else {
        panic!("{record:?}");
    };
    assert!(
        reason.contains("no seed target could be resolved"),
        "{reason}"
    );
    assert!(reason.contains("redis-flush"), "{reason}");
    assert!(seed_build.calls().is_empty());
    assert!(matches!(
        record.crates[0].seed_aligned,
        SeedAlignment::Skipped { .. }
    ));
}

#[test]
fn seed_alignment_can_be_turned_off_and_has_nothing_to_align_without_a_compiler_environment() {
    let fx = fixture();
    fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::ok();

    let mut off = aligned_host(&seed_build);
    off.seed_alignment = false;
    let lane = fx.crate_lane("kt-1742-off", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&lane, &off, &ScriptedBuild::ok(), &RecordingClone::new());
    let SeedAlignment::Skipped { reason } = &record.crates[0].seed_aligned else {
        panic!("{record:?}");
    };
    assert!(reason.contains("off for this project"), "{reason}");
    assert!(matches!(record.seed, SeedOutcome::Cloned { .. }));

    let mut bare = aligned_host(&seed_build);
    bare.compiler_env = Vec::new();
    let lane = fx.crate_lane("kt-1742-bare", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&lane, &bare, &ScriptedBuild::ok(), &RecordingClone::new());
    let SeedAlignment::Skipped { reason } = &record.crates[0].seed_aligned else {
        panic!("{record:?}");
    };
    assert!(reason.contains("no compiler environment"), "{reason}");
    assert!(
        seed_build.calls().is_empty(),
        "nothing was ever built in the seed"
    );
}

/// KT-1742: a project with no Cargo workspace gets no cargo stub and no Cargo
/// warm-up, and the record says why.
#[test]
fn a_project_with_no_cargo_workspace_is_not_warmed_and_the_record_says_why() {
    fn no_cargo(_: &Path) -> Result<CargoMetadata, String> {
        panic!("cargo must not be asked about a project that has no Cargo workspace");
    }
    let fx = fixture();
    let lane = fx.plain_lane("kt-1742-js");
    let runner = ScriptedBuild::ok();
    let seed_build = ScriptedBuild::ok();
    let mut host = aligned_host(&seed_build);
    host.metadata = &no_cargo;
    let record = warm_with(&lane, &host, &runner, &RecordingClone::new());

    let FirstBuild::Skipped { reason } = &record.first_build else {
        panic!("{record:?}");
    };
    assert!(reason.contains("nothing to warm"), "{reason}");
    assert!(matches!(record.seed, SeedOutcome::NotApplicable { .. }));
    assert!(runner.calls().is_empty(), "no cargo");
    assert!(seed_build.calls().is_empty());
    assert!(!first_build_pending(&lane.worktree));
}

/// KT-1742: the owner declares the warm-up of a project that is not a Cargo one
/// (a composer or npm install): same bound, same record, same marker.
#[test]
fn a_declared_first_build_command_warms_a_project_that_is_not_a_cargo_one() {
    let fx = fixture();
    let lane = fx.plain_lane("kt-1742-composer");
    let runner = ScriptedBuild::ok().sandboxed();
    let seed_build = ScriptedBuild::ok();
    let mut host = aligned_host(&seed_build);
    host.first_build_command = Some("composer install --no-interaction".into());
    let record = warm_with(&lane, &host, &runner, &RecordingClone::new());

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    let (program, shell_flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    assert_eq!(calls[0].program, program);
    assert_eq!(
        calls[0].args,
        [shell_flag, "composer install --no-interaction"]
    );
    assert_eq!(calls[0].cwd, lane.worktree, "run in the worktree root");
    // The same bound as a cargo warm-up (what is left of it, a moment later).
    assert!(calls[0].timeout <= settings().first_build_timeout);
    assert!(calls[0].timeout > settings().first_build_timeout - Duration::from_secs(30));
    assert_eq!(
        calls[0].priority,
        crate::core::background_priority::Priority::Background
    );
    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));
    assert_eq!(record.crates.len(), 1);
    assert_eq!(record.crates[0].path, "command");
    assert!(matches!(record.seed, SeedOutcome::NotApplicable { .. }));
    assert!(read_marker(&lane.cargo_target_dir).is_some(), "marked warm");
    assert!(seed_build.calls().is_empty(), "a command builds no seed");

    // Warm for this command: a relaunch builds nothing; another command re-warms.
    let again = warm_with(&lane, &host, &runner, &RecordingClone::new());
    assert!(matches!(again.first_build, FirstBuild::Skipped { .. }));
    assert_eq!(runner.calls().len(), 1);
    host.first_build_command = Some("npm ci".into());
    let changed = warm_with(&lane, &host, &runner, &RecordingClone::new());
    assert!(matches!(changed.first_build, FirstBuild::Succeeded { .. }));
    assert_eq!(runner.calls().len(), 2);
}

/// KT-1742: the record's shape the journal and the UI read: `seed.aligned` and
/// each crate's own alignment, as tagged JSON.
#[test]
fn the_alignment_is_journalled_as_tagged_json() {
    let fx = fixture();
    fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::ok();
    let host = aligned_host(&seed_build);
    let lane = fx.crate_lane("kt-1742-json", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&lane, &host, &ScriptedBuild::ok(), &RecordingClone::new());

    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["seed"]["aligned"]["state"], "aligned");
    assert_eq!(json["seed"]["aligned"]["rebuilt"], true);
    assert_eq!(json["crates"][0]["path"], "redis-flush");
    assert_eq!(json["crates"][0]["seed_aligned"]["state"], "aligned");
    assert_eq!(
        serde_json::from_value::<WarmStartRecord>(json).unwrap(),
        record
    );
    let skipped = serde_json::to_value(SeedAlignment::skipped("why")).unwrap();
    assert_eq!(skipped["state"], "skipped");
    assert_eq!(skipped["reason"], "why");
    let refused = serde_json::to_value(SeedAlignment::NotAligned {
        reason: "held".into(),
    })
    .unwrap();
    assert_eq!(refused["state"], "not_aligned");
}

#[path = "task_worker_lane_warm_roots_test.rs"]
mod roots;
