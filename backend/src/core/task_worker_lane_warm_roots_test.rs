// KT-1742 round 4: per-root markers and cached failures, the roots setting, and
// the declared command that only runs under the lane's OS sandbox.
use super::*;
use crate::core::task_worker_root_marks::FAILURES_FILE;
use crate::core::task_worker_seed_align::SEED_FAILURE;

const DESKTOP: &str = "desktop/src-tauri";
const NO_GTK: &str = "pkg-config: the system library `gtk+-3.0` was not found";

/// A host whose seeds are resolved but never aligned: these tests are about the
/// lane's own builds.
fn plain_host(seed_runner: &dyn ProbeRunner) -> WarmHost<'_> {
    let mut host = aligned_host(seed_runner);
    host.seed_alignment = false;
    host
}

fn mark_of<'a>(marker: &'a WarmMarker, path: &str) -> &'a RootMark {
    marker
        .roots
        .iter()
        .find(|mark| mark.path == path)
        .unwrap_or_else(|| panic!("no mark for {path}: {marker:?}"))
}

fn two_root_lane(fx: &Fixture, name: &str, desktop_lock: &str) -> WorkerLane {
    fx.crate_lane(name, &[("backend", LOCK_A), (DESKTOP, desktop_lock)])
}

/// Kronn's own repository has two Rust roots; the desktop one needs system
/// libraries the host may not have. Its failure must not erase the backend's
/// success, nor be paid again by every relaunch and every new lane.
#[test]
fn one_roots_success_survives_another_roots_failure_and_the_failure_is_not_retried() {
    let fx = fixture();
    fx.main_crate("backend", LOCK_A);
    let desktop_seed = fx.main_crate(DESKTOP, LOCK_B);
    let seed_build = ScriptedBuild::ok();
    let host = plain_host(&seed_build);

    // First lane: backend builds, desktop fails.
    let first = two_root_lane(&fx, "kt-1742-m1", LOCK_B);
    let runner = ScriptedBuild::ok().failing_in(DESKTOP, NO_GTK);
    let record = warm_with(&first, &host, &runner, &RecordingClone::new());

    assert_eq!(runner.calls().len(), 2, "one build per root");
    let by_path = |path: &str| record.crates.iter().find(|c| c.path == path).unwrap();
    assert!(matches!(
        by_path("backend").build,
        FirstBuild::Succeeded { .. }
    ));
    assert!(matches!(by_path(DESKTOP).build, FirstBuild::Failed { .. }));
    let FirstBuild::Failed { detail, .. } = &record.first_build else {
        panic!("the failed root is the aggregate: {record:?}");
    };
    assert!(
        detail.starts_with(DESKTOP) && detail.contains("gtk"),
        "the failure names its root: {detail}"
    );
    let marker = read_marker(&first.cargo_target_dir).expect("per-root marks are written");
    assert_eq!(mark_of(&marker, "backend").state, RootState::Built);
    assert!(
        matches!(&mark_of(&marker, DESKTOP).state, RootState::Failed { reason } if reason.contains("gtk")),
        "{marker:?}"
    );
    assert!(
        desktop_seed.join(FAILURES_FILE).is_file(),
        "the failure is cached beside the root's seed, for the project's later lanes"
    );

    // A relaunch of that lane builds neither: the success is kept, the failure
    // is not retried, and the record says both.
    let relaunch_runner = ScriptedBuild::ok();
    let relaunch = warm_with(&first, &host, &relaunch_runner, &RecordingClone::new());
    assert!(relaunch_runner.calls().is_empty(), "nothing to build again");
    let FirstBuild::Skipped { reason } = &relaunch.first_build else {
        panic!("{relaunch:?}");
    };
    assert!(reason.contains("not retried"), "{reason}");
    let desktop = relaunch.crates.iter().find(|c| c.path == DESKTOP).unwrap();
    assert!(
        matches!(&desktop.build, FirstBuild::Failed { detail, .. } if detail.contains("not retried") && detail.contains("gtk")),
        "{desktop:?}"
    );

    // A NEW lane with the same locks builds the backend (its own target is
    // empty) and does not try the desktop root again.
    let second = two_root_lane(&fx, "kt-1742-m2", LOCK_B);
    let second_runner = ScriptedBuild::ok().failing_in(DESKTOP, NO_GTK);
    let record = warm_with(&second, &host, &second_runner, &RecordingClone::new());
    let calls = second_runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].cwd, second.worktree.join("backend"));
    assert!(
        matches!(record.first_build, FirstBuild::Succeeded { .. }),
        "the cached failure is not this lane's failure: {record:?}"
    );
    let desktop = record.crates.iter().find(|c| c.path == DESKTOP).unwrap();
    assert!(
        matches!(&desktop.build, FirstBuild::Failed { detail, .. } if detail.contains("not retried")),
        "{desktop:?}"
    );
    let marker = read_marker(&second.cargo_target_dir).unwrap();
    assert!(matches!(
        mark_of(&marker, DESKTOP).state,
        RootState::Failed { .. }
    ));

    // The desktop's Cargo.lock changed: another key, tried again.
    let third = two_root_lane(&fx, "kt-1742-m3", LOCK_A);
    let third_runner = ScriptedBuild::ok();
    let record = warm_with(&third, &host, &third_runner, &RecordingClone::new());
    assert_eq!(third_runner.calls().len(), 2, "the changed root is retried");
    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));
    assert!(
        !desktop_seed.join(FAILURES_FILE).exists(),
        "a root that builds again has no failure left to remember"
    );
}

/// The environment is part of the key too: another compiler environment is a new
/// build, so a failure is not carried over to it.
#[test]
fn a_failure_is_tried_again_when_the_compiler_environment_changes() {
    let fx = fixture();
    fx.main_crate("backend", LOCK_A);
    fx.main_crate(DESKTOP, LOCK_B);
    let seed_build = ScriptedBuild::ok();
    let mut host = plain_host(&seed_build);

    let first = two_root_lane(&fx, "kt-1742-e1", LOCK_B);
    let failing = ScriptedBuild::ok().failing_in(DESKTOP, NO_GTK);
    warm_with(&first, &host, &failing, &RecordingClone::new());

    host.compiler_env = vec![(CC_ENV.to_string(), "-O2".to_string())];
    let second = two_root_lane(&fx, "kt-1742-e2", LOCK_B);
    let retry = ScriptedBuild::ok().failing_in(DESKTOP, NO_GTK);
    let record = warm_with(&second, &host, &retry, &RecordingClone::new());
    assert_eq!(retry.calls().len(), 2, "both roots build under a new env");
    assert!(matches!(record.first_build, FirstBuild::Failed { .. }));
}

/// One root's lock changing rebuilds that root only: the other's mark is kept.
#[test]
fn a_root_whose_lock_changed_is_the_only_one_rebuilt_in_a_marked_lane() {
    let fx = fixture();
    fx.main_crate("backend", LOCK_A);
    fx.main_crate(DESKTOP, LOCK_B);
    let seed_build = ScriptedBuild::ok();
    let host = plain_host(&seed_build);
    let lane = two_root_lane(&fx, "kt-1742-partial", LOCK_B);
    warm_with(&lane, &host, &ScriptedBuild::ok(), &RecordingClone::new());

    std::fs::write(lane.worktree.join(DESKTOP).join("Cargo.lock"), LOCK_A).unwrap();
    let runner = ScriptedBuild::ok();
    warm_with(&lane, &host, &runner, &RecordingClone::new());

    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].cwd, lane.worktree.join(DESKTOP));
    let marker = read_marker(&lane.cargo_target_dir).unwrap();
    assert_eq!(marker.roots.len(), 2, "{marker:?}");
    assert_eq!(mark_of(&marker, "backend").state, RootState::Built);
    assert_eq!(mark_of(&marker, DESKTOP).state, RootState::Built);
}

/// A timeout and a cancellation say nothing about a root: neither is remembered.
#[test]
fn a_timed_out_root_is_not_remembered_as_a_failure() {
    let fx = fixture();
    fx.main_crate("backend", LOCK_A);
    let seed_build = ScriptedBuild::ok();
    let host = plain_host(&seed_build);
    let lane = fx.crate_lane("kt-1742-slow", &[("backend", LOCK_A)]);
    let record = warm_with(
        &lane,
        &host,
        &ScriptedBuild::with(false, "timed out after 60s"),
        &RecordingClone::new(),
    );
    assert!(matches!(record.first_build, FirstBuild::TimedOut { .. }));
    assert!(read_marker(&lane.cargo_target_dir).is_none());
    assert!(!fx.repo.join("backend/target").join(FAILURES_FILE).exists());
}

/// The launch asks whether a lane still has something to build: per root.
#[test]
fn a_first_build_is_pending_until_every_root_has_a_mark_for_its_lock() {
    let fx = fixture();
    fx.main_crate("backend", LOCK_A);
    fx.main_crate(DESKTOP, LOCK_B);
    let seed_build = ScriptedBuild::ok();
    let mut host = plain_host(&seed_build);
    // What the settings detect for a project that does not build C.
    host.compiler_env = Vec::new();
    let lane = two_root_lane(&fx, "kt-1742-pending", LOCK_B);
    assert!(first_build_pending(&lane.worktree));

    let runner = ScriptedBuild::ok().failing_in(DESKTOP, NO_GTK);
    warm_with(&lane, &host, &runner, &RecordingClone::new());
    assert!(
        !first_build_pending(&lane.worktree),
        "a built root and a failed one are both settled for their locks"
    );

    std::fs::write(lane.worktree.join(DESKTOP).join("Cargo.lock"), LOCK_A).unwrap();
    assert!(
        first_build_pending(&lane.worktree),
        "the changed root is pending again"
    );
}

/// The project's setting names the Rust roots to warm; the others are left alone.
#[test]
fn the_rust_roots_setting_selects_the_roots_to_warm_and_reports_an_unknown_one() {
    let fx = fixture();
    fx.main_crate("backend", LOCK_A);
    fx.main_crate(DESKTOP, LOCK_B);
    let seed_build = ScriptedBuild::ok();
    let mut host = plain_host(&seed_build);
    host.rust_roots = Some(vec!["backend".into(), "./gone/".into()]);
    let lane = two_root_lane(&fx, "kt-1742-select", LOCK_B);
    let runner = ScriptedBuild::ok();

    let record = warm_with(&lane, &host, &runner, &RecordingClone::new());

    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].cwd, lane.worktree.join("backend"));
    let paths: Vec<&str> = record.crates.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["backend", "gone"], "{record:?}");
    assert!(
        matches!(&record.crates[1].build, FirstBuild::Skipped { reason } if reason.contains("no Cargo.toml")),
        "{:?}",
        record.crates[1]
    );
    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));

    // A setting that names no root of the repository warms nothing, and says why.
    host.rust_roots = Some(vec!["nowhere".into()]);
    let empty = fx.crate_lane("kt-1742-none", &[("backend", LOCK_A)]);
    let record = warm_with(&empty, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    let FirstBuild::Skipped { reason } = &record.first_build else {
        panic!("{record:?}");
    };
    assert!(reason.contains("nowhere"), "{reason}");
}

/// The declared command executes the worktree's own scripts: a runner that does
/// not confine its commands to the lane's OS sandbox is never asked to run it.
#[test]
fn a_declared_command_is_refused_by_a_runner_that_does_not_confine_it() {
    let fx = fixture();
    let lane = fx.plain_lane("kt-1742-unconfined");
    let seed_build = ScriptedBuild::ok();
    let mut host = aligned_host(&seed_build);
    host.first_build_command = Some("npm ci".into());
    let unconfined = ScriptedBuild::ok();
    let gate = BuildGate::new();

    let record = warm_lane(
        &lane,
        &provisioned(),
        &settings(),
        &host,
        &gate,
        &unconfined,
        &|source, destination| copy_dir(source, destination),
        |_| {},
    );

    assert!(unconfined.calls().is_empty(), "the command never ran");
    assert_eq!(gate.running(), 0, "no build slot was taken for a refusal");
    let FirstBuild::Failed { detail, .. } = &record.first_build else {
        panic!("{record:?}");
    };
    assert!(detail.contains("OS sandbox"), "{detail}");
    let command = record.crates.iter().find(|c| c.path == "command").unwrap();
    assert!(matches!(command.build, FirstBuild::Failed { .. }));
    assert!(
        read_marker(&lane.cargo_target_dir).is_none(),
        "nothing is marked warm"
    );

    // The same lane under a confined runner runs it: the refusal was not cached.
    let confined = ScriptedBuild::ok().sandboxed();
    let record = warm_with(&lane, &host, &confined, &RecordingClone::new());
    assert_eq!(confined.calls().len(), 1);
    assert!(matches!(record.first_build, FirstBuild::Succeeded { .. }));
}

/// The seed's alignment build is the same: a project whose seed cannot be built
/// (a missing system library) is not rebuilt by every lane.
#[test]
fn a_failed_seed_build_is_not_retried_for_the_same_lock_and_environment() {
    let fx = fixture();
    let target = fx.main_crate("redis-flush", LOCK_CC);
    let seed_build = ScriptedBuild::with(false, "error: failed to run custom build command");
    let host = aligned_host(&seed_build);

    let first = fx.crate_lane("kt-1742-sf1", &[("redis-flush", LOCK_CC)]);
    warm_with(&first, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    assert_eq!(seed_build.calls().len(), 1);
    assert!(target.join(SEED_FAILURE).is_file());

    let second = fx.crate_lane("kt-1742-sf2", &[("redis-flush", LOCK_CC)]);
    let record = warm_with(&second, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    assert_eq!(seed_build.calls().len(), 1, "not built again");
    let SeedAlignment::NotAligned { reason } = &record.crates[0].seed_aligned else {
        panic!("{record:?}");
    };
    assert!(reason.contains("not retried"), "{reason}");
    assert!(
        matches!(record.seed, SeedOutcome::Cloned { .. }),
        "the seed is still cloned as it is: {record:?}"
    );

    // Another lock in the main checkout is another build.
    std::fs::write(
        fx.repo.join("redis-flush").join("Cargo.lock"),
        format!("{LOCK_CC}\n[[package]]\nname = \"extra\"\nversion = \"1.0.0\"\n"),
    )
    .unwrap();
    let third = fx.crate_lane("kt-1742-sf3", &[("redis-flush", LOCK_CC)]);
    warm_with(&third, &host, &ScriptedBuild::ok(), &RecordingClone::new());
    assert_eq!(seed_build.calls().len(), 2, "a changed lock is tried again");
}
