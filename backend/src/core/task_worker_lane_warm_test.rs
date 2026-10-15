use super::*;
use crate::core::task_worker_env::ProbeOutcome;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

const LOCK_A: &str = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n";
const LOCK_B: &str = "[[package]]\nname = \"b\"\nversion = \"2.0.0\"\n";

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
        }
    }

    fn calls(&self) -> Vec<ProbeCommand> {
        self.calls.lock().unwrap().clone()
    }
}

impl ProbeRunner for ScriptedBuild {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.calls.lock().unwrap().push(probe.clone());
        self.outcome.clone()
    }
}

fn provisioned() -> Provisioned {
    Provisioned {
        frontend_deps: crate::core::task_worker_env::FrontendDeps::NotApplicable,
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
    let candidates = seed_candidates(&lane, &home);
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
    let outcome = seed_lane(&lane, &home, &|from, to| {
        let cargo = std::fs::File::open(&lock_path).unwrap();
        *observed.lock().unwrap() = Some(FileExt::try_lock_exclusive(&cargo).is_ok());
        copy_dir(from, to)
    });

    assert!(matches!(outcome, SeedOutcome::Cloned { .. }));
    assert_eq!(*build_could_start.lock().unwrap(), Some(false));
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

#[cfg(unix)]
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
fn a_failed_or_timed_out_build_leaves_no_marker() {
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
    assert!(read_marker(&failed_lane.cargo_target_dir).is_none());

    let slow_lane = fx.lane("kt-1250-slow", LOCK_A);
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
        },
        build_jobs: 4,
        first_build: FirstBuild::Succeeded {
            duration_ms: 90_000,
        },
        queued_ms: 5,
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

#[cfg(unix)]
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
    write_marker(&lane, &hash_of(LOCK_A), &HomeKey::of(&lane, &provisioned())).unwrap();
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
