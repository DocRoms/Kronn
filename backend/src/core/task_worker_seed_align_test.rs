use super::*;
use crate::core::task_worker_env::ProbeOutcome;
use std::sync::Mutex;

struct Build {
    calls: Mutex<Vec<ProbeCommand>>,
    outcome: ProbeOutcome,
}

impl Build {
    fn new(success: bool, detail: &str) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outcome: ProbeOutcome {
                success,
                detail: detail.into(),
            },
        }
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl ProbeRunner for Build {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.calls.lock().unwrap().push(probe.clone());
        self.outcome.clone()
    }
}

struct Seed {
    _dir: tempfile::TempDir,
    crate_dir: std::path::PathBuf,
    target: std::path::PathBuf,
}

fn seed() -> Seed {
    let dir = tempfile::tempdir().unwrap();
    let crate_dir = dir.path().join("svc");
    let target = crate_dir.join("target");
    std::fs::create_dir_all(target.join("debug").join(".fingerprint")).unwrap();
    std::fs::write(target.join("debug").join(CARGO_BUILD_LOCK), "").unwrap();
    Seed {
        _dir: dir,
        crate_dir,
        target,
    }
}

fn env(flag: &str) -> Vec<(String, String)> {
    vec![("CFLAGS_x_y_z".to_string(), flag.to_string())]
}

fn request<'a>(
    seed: &'a Seed,
    compiler_env: &'a [(String, String)],
    lock: &str,
) -> AlignRequest<'a> {
    AlignRequest {
        main_crate_dir: &seed.crate_dir,
        target: &seed.target,
        lock_hash: Some(lock.to_string()),
        compiler_env,
        build_env: vec![(
            "CARGO_TARGET_DIR".into(),
            seed.target.clone().into_os_string(),
        )],
        timeout: Duration::from_secs(60),
        lock_wait: Duration::from_millis(100),
    }
}

/// A build script's fingerprint file with a chosen modification time.
fn script_ran(seed: &Seed, unit: &str, seconds_from_now: i64) {
    let dir = seed.target.join("debug").join(".fingerprint").join(unit);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("run-build-script-build-script-build");
    std::fs::write(&file, "x").unwrap();
    let when = if seconds_from_now >= 0 {
        SystemTime::now() + Duration::from_secs(seconds_from_now as u64)
    } else {
        SystemTime::now() - Duration::from_secs((-seconds_from_now) as u64)
    };
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn a_seed_with_no_debug_dir_or_no_environment_has_nothing_to_align() {
    let fixture = seed();
    std::fs::remove_dir_all(fixture.target.join("debug")).unwrap();
    let flags = env("-g");
    assert!(matches!(
        assess(&request(&fixture, &flags, "l1")),
        Assessment::Settled(SeedAlignment::Skipped { .. })
    ));

    let fixture = seed();
    let none: Vec<(String, String)> = Vec::new();
    assert!(matches!(
        assess(&request(&fixture, &none, "l1")),
        Assessment::Settled(SeedAlignment::Skipped { .. })
    ));
}

#[test]
fn an_unstamped_seed_is_built_once_then_vouched_for_by_its_stamp() {
    let fixture = seed();
    let flags = env("-save-temps=obj");
    let build = Build::new(true, "");

    let Assessment::NeedsBuild(why) = assess(&request(&fixture, &flags, "l1")) else {
        panic!("an unstamped seed needs a build");
    };
    assert!(why.contains("no Kronn stamp"), "{why}");

    let outcome = align_seed(
        &request(&fixture, &flags, "l1"),
        &build,
        &CancellationToken::new(),
    );
    assert!(
        matches!(outcome, SeedAlignment::Aligned { rebuilt: true, .. }),
        "{outcome:?}"
    );
    assert_eq!(build.calls(), 1);
    assert!(fixture.target.join(SEED_STAMP).is_file());

    // Same environment, same lock: the stamp is enough.
    assert_eq!(
        align_seed(
            &request(&fixture, &flags, "l1"),
            &build,
            &CancellationToken::new()
        ),
        SeedAlignment::Aligned {
            rebuilt: false,
            build_ms: 0
        }
    );
    assert_eq!(build.calls(), 1, "no second build");
}

#[test]
fn another_environment_or_another_lock_makes_the_stamp_stale() {
    let fixture = seed();
    let flags = env("-save-temps=obj");
    let build = Build::new(true, "");
    align_seed(
        &request(&fixture, &flags, "l1"),
        &build,
        &CancellationToken::new(),
    );

    let other_env = env("-g");
    let Assessment::NeedsBuild(why) = assess(&request(&fixture, &other_env, "l1")) else {
        panic!("another environment needs a build");
    };
    assert!(why.contains("another compiler environment"), "{why}");

    let Assessment::NeedsBuild(why) = assess(&request(&fixture, &flags, "l2")) else {
        panic!("another lock needs a build");
    };
    assert!(why.contains("another Cargo.lock"), "{why}");
}

/// The owner's own build, under an environment of its own (the dev backend's,
/// without the lane's flags), reruns the build scripts of the seed: the stamp no
/// longer describes it.
#[test]
fn a_build_script_that_ran_after_kronns_build_makes_the_stamp_stale() {
    let fixture = seed();
    let flags = env("-save-temps=obj");
    let build = Build::new(true, "");
    script_ran(&fixture, "aws-lc-sys-abc", -120);
    align_seed(
        &request(&fixture, &flags, "l1"),
        &build,
        &CancellationToken::new(),
    );
    assert!(
        matches!(
            assess(&request(&fixture, &flags, "l1")),
            Assessment::Settled(SeedAlignment::Aligned { rebuilt: false, .. })
        ),
        "nothing ran since"
    );

    script_ran(&fixture, "aws-lc-sys-abc", 60);
    let Assessment::NeedsBuild(why) = assess(&request(&fixture, &flags, "l1")) else {
        panic!("a rerun build script makes the seed stale");
    };
    assert!(why.contains("another environment built it since"), "{why}");
}

#[test]
fn a_held_cargo_lock_is_waited_for_within_the_bound_then_reported() {
    let fixture = seed();
    let flags = env("-g");
    let build = Build::new(true, "");
    let owner = std::fs::File::open(fixture.target.join("debug").join(CARGO_BUILD_LOCK)).unwrap();
    FileExt::try_lock_exclusive(&owner).unwrap();

    let started = Instant::now();
    let outcome = align_seed(
        &request(&fixture, &flags, "l1"),
        &build,
        &CancellationToken::new(),
    );

    assert!(started.elapsed() >= Duration::from_millis(100), "it waited");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "within the bound"
    );
    let SeedAlignment::NotAligned { reason } = outcome else {
        panic!("expected the fallback");
    };
    assert!(reason.contains("holds the seed's Cargo lock"), "{reason}");
    assert!(reason.contains("cloned as it is"), "{reason}");
    assert_eq!(build.calls(), 0, "nothing ran in the owner's target");
    FileExt::unlock(&owner).unwrap();
}

#[test]
fn a_lock_freed_while_waiting_lets_the_build_go_ahead() {
    let fixture = seed();
    let flags = env("-g");
    let build = Build::new(true, "");
    let lock_path = fixture.target.join("debug").join(CARGO_BUILD_LOCK);
    let owner = std::fs::File::open(&lock_path).unwrap();
    FileExt::try_lock_exclusive(&owner).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        FileExt::unlock(&owner).unwrap();
    });

    let mut waiting = request(&fixture, &flags, "l1");
    waiting.lock_wait = Duration::from_secs(10);
    let outcome = align_seed(&waiting, &build, &CancellationToken::new());
    release.join().unwrap();

    assert!(
        matches!(outcome, SeedAlignment::Aligned { rebuilt: true, .. }),
        "{outcome:?}"
    );
    assert_eq!(build.calls(), 1);
}

#[test]
fn a_failed_build_leaves_no_stamp_and_a_cancelled_wait_builds_nothing() {
    let fixture = seed();
    let flags = env("-g");
    let failing = Build::new(false, "error[E0463]: can't find crate");
    let outcome = align_seed(
        &request(&fixture, &flags, "l1"),
        &failing,
        &CancellationToken::new(),
    );
    let SeedAlignment::NotAligned { reason } = outcome else {
        panic!("expected a reported failure");
    };
    assert!(reason.contains("can't find crate"), "{reason}");
    assert!(!fixture.target.join(SEED_STAMP).exists());

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let build = Build::new(true, "");
    let outcome = align_seed(&request(&fixture, &flags, "l1"), &build, &cancelled);
    assert!(
        matches!(outcome, SeedAlignment::NotAligned { .. }),
        "{outcome:?}"
    );
    assert_eq!(build.calls(), 0);
}

#[test]
fn the_build_runs_in_the_seeds_own_crate_offline_and_locked_at_background_priority() {
    let fixture = seed();
    let flags = env("-g");
    let build = Build::new(true, "");
    align_seed(
        &request(&fixture, &flags, "l1"),
        &build,
        &CancellationToken::new(),
    );

    let call = build.calls.lock().unwrap()[0].clone();
    assert_eq!(call.program, "cargo");
    assert_eq!(call.args, ["build", "--locked", "--offline"]);
    assert_eq!(call.cwd, fixture.crate_dir);
    assert_eq!(call.priority, Priority::Background);
    assert!(!call.clean_env);
}
