//! KT-1318 — the policy, the launch guard's decision and the bounded measure.

use super::*;
use std::fs;

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn the_launch_guard_is_off_at_zero_and_unreadable_space_never_refuses() {
    assert_eq!(classify_launch_headroom(Some(0), 0), LaunchHeadroom::Ok);
    assert_eq!(classify_launch_headroom(None, 30), LaunchHeadroom::Ok);
}

#[test]
fn the_launch_guard_refuses_strictly_below_the_threshold() {
    assert_eq!(
        classify_launch_headroom(Some(30 * GIB - 1), 30),
        LaunchHeadroom::Low {
            available_gib: 29,
            min_gib: 30
        }
    );
    assert_eq!(
        classify_launch_headroom(Some(30 * GIB), 30),
        LaunchHeadroom::Ok
    );
    assert_eq!(
        classify_launch_headroom(Some(500 * GIB), 30),
        LaunchHeadroom::Ok
    );
    // An absurd threshold cannot overflow into "plenty of room".
    assert!(matches!(
        classify_launch_headroom(Some(500 * GIB), u64::MAX),
        LaunchHeadroom::Low { .. }
    ));
}

#[test]
fn the_idle_threshold_is_in_hours_and_a_test_thread_owns_its_policy() {
    assert_eq!(disk_policy(), (0, DEFAULT_DISK_SWEEP_IDLE_HOURS));
    {
        let _policy = TestDiskPolicy::set(42, 3);
        assert_eq!(disk_policy(), (42, 3));
        assert_eq!(sweep_idle(), Duration::from_secs(3 * 3600));
        // Another thread never sees it: tests running side by side cannot
        // refuse one another's launches.
        let elsewhere = std::thread::spawn(disk_policy).join().unwrap();
        assert_eq!(elsewhere, (0, DEFAULT_DISK_SWEEP_IDLE_HOURS));
    }
    assert_eq!(disk_policy(), (0, DEFAULT_DISK_SWEEP_IDLE_HOURS));
}

/// A configured `0` must not make every finished lane an instant orphan.
#[test]
fn zero_idle_hours_is_read_as_the_floor_not_as_no_idle_check() {
    assert_eq!(MIN_SWEEP_IDLE_HOURS, 1);
    assert_eq!(effective_sweep_idle_hours(0), 1);
    assert_eq!(effective_sweep_idle_hours(6), 6);
    let _policy = TestDiskPolicy::set(30, 0);
    assert_eq!(disk_policy(), (30, 1));
    assert_eq!(sweep_idle(), Duration::from_secs(3600));
}

#[test]
fn free_space_of_a_path_that_does_not_exist_yet_is_its_volumes() {
    let dir = tempfile::tempdir().unwrap();
    let not_yet = dir.path().join("target").join("wt-lane");
    assert_eq!(existing_ancestor(&not_yet).as_deref(), Some(dir.path()));
    assert!(available_bytes(&not_yet).is_some());
    assert!(total_bytes(&not_yet).unwrap() >= available_bytes(&not_yet).unwrap());
}

#[test]
fn measure_sums_files_skips_named_top_level_entries_and_never_follows_links() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::write(root.join("src/a.rs"), vec![0u8; 100]).unwrap();
    fs::write(root.join("src/deep/b.rs"), vec![0u8; 50]).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/big"), vec![0u8; 10_000]).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("huge"), vec![0u8; 50_000]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();

    let budget = || MeasureBudget::within(1_000, Duration::from_secs(5));
    let all = measure_bounded(root, &[], budget());
    assert_eq!((all.bytes, all.partial), (10_150, false));
    let sources = measure_bounded(root, &["target"], budget());
    assert_eq!((sources.bytes, sources.partial), (150, false));
}

#[test]
fn measure_stops_at_its_budget_and_says_it_is_a_floor() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..50 {
        fs::write(dir.path().join(format!("f{index}")), vec![0u8; 10]).unwrap();
    }
    let measured = measure_bounded(
        dir.path(),
        &[],
        MeasureBudget::within(10, Duration::from_secs(5)),
    );
    assert!(measured.partial);
    assert!(measured.bytes < 500);
}

#[test]
fn the_kinds_and_states_have_stable_wire_names() {
    assert_eq!(
        DiskKind::ALL.map(DiskKind::as_str),
        ["worktree", "lane_build_dir", "cache"]
    );
    assert_eq!(
        serde_json::to_string(&DiskKind::LaneBuildDir).unwrap(),
        "\"lane_build_dir\""
    );
    assert_eq!(
        serde_json::to_string(&DiskState::Orphan).unwrap(),
        "\"orphan\""
    );
    assert_eq!(DiskState::Foreign.as_str(), "foreign");
}

#[test]
fn stalled_days_floor_at_one_and_the_state_has_a_wire_name() {
    let _days = TestStalledDays::set(0);
    assert_eq!(stalled_build_days(), MIN_STALLED_BUILD_DAYS);
    drop(_days);
    let _days = TestStalledDays::set(5);
    assert_eq!(stalled_build_idle(), Duration::from_secs(5 * 24 * 3600));
    assert_eq!(DiskState::Stalled.as_str(), "stalled");
    assert_eq!(
        serde_json::to_string(&DiskState::Stalled).unwrap(),
        "\"stalled\""
    );
}

#[test]
fn real_or_apparent_prefers_the_unique_figure() {
    let known = Measured {
        bytes: 100,
        partial: false,
        unique_bytes: Some(10),
    };
    assert_eq!(known.real_or_apparent(), 10);
    let unknown = Measured {
        unique_bytes: None,
        ..known
    };
    assert_eq!(unknown.real_or_apparent(), 100);
}

/// KT-1691 — the 2026-11-11 incident: 33 lanes cloned from one seed each read
/// ~190 GB, 1.85 TB in all, and deleting five of them freed 39 GB. The apparent
/// size counts shared blocks in full; the unique one must not.
#[cfg(target_os = "macos")] // APFS clones (clonefile) exist only on macOS
#[test]
fn a_cloned_tree_reports_its_unique_bytes_not_the_blocks_it_shares() {
    use crate::core::task_worker_lane_warm::clone_tree;

    let dir = tempfile::tempdir().unwrap();
    let seed = dir.path().join("seed");
    fs::create_dir_all(&seed).unwrap();
    // Not compressible, not sparse: a real 4 MiB of blocks.
    let content: Vec<u8> = (0..4 * 1024 * 1024u32)
        .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    fs::write(seed.join("big.rlib"), &content).unwrap();
    let lane = dir.path().join("lane");
    if clone_tree(&seed, &lane).is_err() {
        // The temp dir's volume cannot clone: nothing shared to measure.
        return;
    }
    let budget = || MeasureBudget::within(1_000, Duration::from_secs(5));
    let cloned = measure_bounded(&lane, &[], budget());
    assert_eq!(cloned.bytes, content.len() as u64, "apparent counts it all");
    let unique = cloned.unique_bytes.expect("APFS answers");
    assert!(
        unique < cloned.bytes / 4,
        "a fresh clone owns almost nothing: {unique} unique of {} apparent",
        cloned.bytes
    );

    // The lane rewrites the file: now its blocks are its own.
    let mut rewritten = content.clone();
    rewritten
        .iter_mut()
        .for_each(|byte| *byte = byte.wrapping_add(1));
    fs::write(lane.join("big.rlib"), &rewritten).unwrap();
    let rewritten_unique = measure_bounded(&lane, &[], budget())
        .unique_bytes
        .expect("APFS answers");
    assert!(
        rewritten_unique >= (content.len() as u64) * 3 / 4,
        "a rewritten file is the lane's own: {rewritten_unique}"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn without_a_way_to_tell_the_unique_size_is_unknown_not_guessed() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("f"), vec![1u8; 100]).unwrap();
    let measured = measure_bounded(
        dir.path(),
        &[],
        MeasureBudget::within(100, Duration::from_secs(5)),
    );
    assert_eq!(measured.unique_bytes, None);
    assert_eq!(measured.real_or_apparent(), 100);
}
