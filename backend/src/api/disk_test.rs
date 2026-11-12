//! KT-1318 — the ownership / age rules of the disk view, without a database
//! fixture. The DB-driven cases (sweeps, report, launch guard, provisioning)
//! live with the other lane tests in `api::orchestration::tests`.

use super::*;
use crate::core::worktree::LaneOwner;

fn owner() -> LaneOwner {
    LaneOwner {
        version: 1,
        lane: "kt-1-aaaa".into(),
        execution_id: "exec-1".into(),
        worktree: "/repo/.kronn/worktrees/task-kt-1-aaaa".into(),
    }
}

const SIX_HOURS: Duration = Duration::from_secs(6 * 3600);

#[test]
fn only_a_marked_finished_idle_lane_without_worktree_is_an_orphan() {
    let gone = LaneTargetState::Reclaimable {
        owner: owner(),
        worktree_present: false,
    };
    let (state, reason, purgeable) = lane_entry_state(
        &gone,
        Some(SIX_HOURS + Duration::from_secs(1)),
        SIX_HOURS,
        None,
    );
    assert_eq!((state, reason, purgeable), (DiskState::Orphan, None, true));

    // Exactly at the threshold counts as idle; a minute short does not.
    assert_eq!(
        lane_entry_state(&gone, Some(SIX_HOURS), SIX_HOURS, None).0,
        DiskState::Orphan
    );
    let (state, reason, purgeable) = lane_entry_state(
        &gone,
        Some(SIX_HOURS - Duration::from_secs(60)),
        SIX_HOURS,
        None,
    );
    assert_eq!(state, DiskState::Reclaimable);
    assert!(reason.unwrap().contains("idle threshold"));
    assert!(purgeable, "an explicit, confirmed delete may still take it");

    // An unreadable age is never proof of idleness.
    let (state, reason, _) = lane_entry_state(&gone, None, SIX_HOURS, None);
    assert_eq!(state, DiskState::Reclaimable);
    assert!(reason.unwrap().contains("not provably idle"));
}

#[test]
fn a_lane_whose_clean_worktree_is_kept_is_reclaimable_never_an_orphan() {
    let kept = LaneTargetState::Reclaimable {
        owner: owner(),
        worktree_present: true,
    };
    let (state, reason, purgeable) = lane_entry_state(&kept, Some(SIX_HOURS * 10), SIX_HOURS, None);
    assert_eq!(state, DiskState::Reclaimable);
    assert!(reason.unwrap().contains("worktree is kept"));
    assert!(purgeable);
}

#[test]
fn an_active_or_kept_lane_is_never_purgeable_whatever_its_age() {
    let old = Some(SIX_HOURS * 100);
    let active = LaneTargetState::Claimed(owner(), "execution exec-1 is Working".into());
    let (state, reason, purgeable) = lane_entry_state(&active, old, SIX_HOURS, None);
    assert_eq!(state, DiskState::Active);
    assert!(reason.unwrap().contains("Working"));
    assert!(!purgeable);

    let dirty = LaneTargetState::Kept(Some(owner()), "kept with uncommitted changes".into());
    let (state, _, purgeable) = lane_entry_state(&dirty, old, SIX_HOURS, None);
    assert_eq!((state, purgeable), (DiskState::Kept, false));
}

fn entry(kind: DiskKind, state: DiskState, bytes: u64, partial: bool) -> DiskEntry {
    DiskEntry {
        project_id: "p".into(),
        kind,
        path: format!("/repo/{bytes}"),
        name: "n".into(),
        bytes,
        unique_bytes: None,
        bytes_are_partial: partial,
        idle_seconds: None,
        owner: None,
        state,
        reason: None,
        purgeable: false,
    }
}

#[test]
fn totals_group_by_kind_and_count_only_orphans_as_reclaimable() {
    let entries = vec![
        entry(DiskKind::LaneBuildDir, DiskState::Orphan, 30, false),
        entry(DiskKind::LaneBuildDir, DiskState::Active, 20, true),
        entry(DiskKind::Worktree, DiskState::Kept, 5, false),
    ];
    let totals = totals_by_kind(&entries);
    assert_eq!(totals.len(), 3, "every kind is reported, even when empty");
    let lane = totals
        .iter()
        .find(|t| t.kind == DiskKind::LaneBuildDir)
        .unwrap();
    assert_eq!((lane.entries, lane.bytes, lane.orphan_bytes), (2, 50, 30));
    assert!(lane.bytes_are_partial, "one floored entry floors the total");
    let cache = totals.iter().find(|t| t.kind == DiskKind::Cache).unwrap();
    assert_eq!((cache.entries, cache.bytes), (0, 0));
}

fn report_of_two_projects() -> DiskReport {
    let mut mine = entry(DiskKind::LaneBuildDir, DiskState::Orphan, 30, false);
    mine.project_id = "p1".into();
    let mut theirs = entry(DiskKind::Worktree, DiskState::Active, 70, false);
    theirs.project_id = "p2".into();
    theirs.owner = Some(DiskOwner {
        task_ref: Some("KT-9002".into()),
        execution_id: "exec-b".into(),
        status: "running".into(),
    });
    let entries = vec![theirs, mine];
    let volume = |id: &str| DiskVolume {
        project_id: id.into(),
        project_name: id.into(),
        path: format!("/repos/{id}"),
        available_bytes: Some(10),
        total_bytes: Some(100),
    };
    DiskReport {
        available_bytes: Some(10),
        under_pressure: false,
        policy: thresholds_policy(),
        volumes: vec![volume("p1"), volume("p2")],
        totals: totals_by_kind(&entries),
        apparent_bytes: 100,
        unique_bytes: None,
        orphan_bytes: 30,
        orphan_unique_bytes: 30,
        orphan_count: 1,
        entries,
        scoped: false,
    }
}

/// KT-1347 — a bridge token keeps the aggregates but sees no other project's
/// lanes, worktrees, paths or task refs.
#[test]
fn a_bridge_view_keeps_the_aggregates_and_its_own_project_s_detail_only() {
    let scoped = scoped_for_bridge(report_of_two_projects(), Some("p1"));
    assert!(scoped.scoped);
    assert_eq!(scoped.available_bytes, Some(10));
    assert_eq!((scoped.orphan_bytes, scoped.orphan_count), (30, 1));
    let totals: u64 = scoped.totals.iter().map(|total| total.bytes).sum();
    assert_eq!(totals, 100, "totals by kind stay whole-instance aggregates");
    assert_eq!(scoped.entries.len(), 1);
    assert_eq!(scoped.entries[0].project_id, "p1");
    assert_eq!(scoped.volumes.len(), 1);
    let json = serde_json::to_string(&scoped).unwrap();
    for hidden in ["p2", "KT-9002", "exec-b", "/repos/p2"] {
        assert!(!json.contains(hidden), "{hidden} leaked: {json}");
    }
}

#[test]
fn a_projectless_bridge_view_is_aggregates_only_and_the_full_view_is_unflagged() {
    let scoped = scoped_for_bridge(report_of_two_projects(), None);
    assert!(scoped.entries.is_empty() && scoped.volumes.is_empty());
    assert_eq!(scoped.orphan_count, 1);
    let full = serde_json::to_value(report_of_two_projects()).unwrap();
    assert!(full.get("scoped").is_none(), "the UI's report is unchanged");
    assert_eq!(full["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn the_refusal_names_the_number_the_setting_and_what_was_swept() {
    let reason = DiskPressure {
        available_gib: 7,
        min_gib: 30,
        swept_lanes: 4,
        swept_bytes: 12 * 1024 * 1024 * 1024,
    }
    .reason("launch a task worker");
    assert!(
        reason.contains("refusing to launch a task worker"),
        "{reason}"
    );
    assert!(reason.contains("only 7 GiB free"), "{reason}");
    assert!(reason.contains("30 GiB"), "{reason}");
    assert!(
        reason.contains("server.disk_launch_min_free_gib"),
        "{reason}"
    );
    assert!(
        reason.contains("4 removed, 12288 MiB really freed"),
        "{reason}"
    );
}

#[tokio::test]
async fn named_entries_need_confirmation_and_nothing_outside_a_project_is_deleted() {
    let db = Database::open_in_memory().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("target").join("wt-not-kronns");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("artefact"), b"x").unwrap();
    let path = target.to_string_lossy().to_string();

    let unconfirmed = run_purge(
        &db,
        PurgeRequest {
            project_id: None,
            paths: vec![path.clone()],
            confirm: false,
        },
    )
    .await;
    assert!(unconfirmed.unwrap_err().contains("confirm"));

    let confirmed = run_purge(
        &db,
        PurgeRequest {
            project_id: None,
            paths: vec![path],
            confirm: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(confirmed.mode, "entries");
    assert!(!confirmed.items[0].purged);
    assert!(confirmed.items[0]
        .reason
        .as_ref()
        .unwrap()
        .contains("not inside any Kronn project"));
    assert!(target.join("artefact").exists());
}

#[tokio::test]
async fn the_default_purge_and_the_report_of_an_unknown_project_are_refused_cleanly() {
    let db = Database::open_in_memory().unwrap();
    assert!(purge_orphans(&db, Some("nope")).await.is_err());
    assert!(build_disk_report(&db, Some("nope")).await.is_err());
    let empty = build_disk_report(&db, None).await.unwrap();
    assert!(empty.entries.is_empty() && empty.orphan_count == 0 && !empty.under_pressure);
    assert_eq!(empty.totals.len(), 3);
}

const THREE_DAYS: Duration = Duration::from_secs(3 * 24 * 3600);

/// KT-1691 — a foreign `wt-*` (no marker, no durable owner) is never touched by
/// a sweep, but the owner may delete it, one entry at a time, once it has been
/// quiet for the idle threshold: a build may be running in it.
#[test]
fn a_foreign_lane_is_offered_to_the_owner_only_once_quiet() {
    let old = Some(SIX_HOURS * 100);
    let (state, reason, purgeable) =
        lane_entry_state(&LaneTargetState::Unmarked, old, SIX_HOURS, None);
    assert_eq!((state, purgeable), (DiskState::Foreign, true));
    assert!(reason.unwrap().contains(worktree::LANE_OWNER_MARKER));

    for recent in [Some(Duration::from_secs(60)), None] {
        let (state, reason, purgeable) =
            lane_entry_state(&LaneTargetState::Unmarked, recent, SIX_HOURS, None);
        assert_eq!((state, purgeable), (DiskState::Foreign, false));
        assert!(reason.unwrap().contains("may be in use"));
    }
}

/// KT-1691 — a Blocked/Interrupted execution's build dir becomes purgeable once
/// idle for N days; before that, or when durable state says anything works for
/// it (`stalled_after: None`), it stays active and unpurgeable.
#[test]
fn a_blocked_executions_lane_becomes_purgeable_after_n_idle_days_only() {
    let claimed = LaneTargetState::Claimed(owner(), "execution exec-1 is Blocked".into());

    let (state, reason, purgeable) = lane_entry_state(
        &claimed,
        Some(THREE_DAYS + Duration::from_secs(1)),
        SIX_HOURS,
        Some(THREE_DAYS),
    );
    assert_eq!((state, purgeable), (DiskState::Stalled, true));
    assert!(reason.unwrap().contains("next resume rebuilds"));

    // Exactly at the threshold counts; a minute short does not.
    assert_eq!(
        lane_entry_state(&claimed, Some(THREE_DAYS), SIX_HOURS, Some(THREE_DAYS)).0,
        DiskState::Stalled
    );
    let (state, reason, purgeable) = lane_entry_state(
        &claimed,
        Some(THREE_DAYS - Duration::from_secs(60)),
        SIX_HOURS,
        Some(THREE_DAYS),
    );
    assert_eq!((state, purgeable), (DiskState::Active, false));
    assert!(reason.unwrap().contains("3 day(s)"));

    // An unreadable age is never idle.
    let (state, _, purgeable) = lane_entry_state(&claimed, None, SIX_HOURS, Some(THREE_DAYS));
    assert_eq!((state, purgeable), (DiskState::Active, false));

    // Anything durable says is working (or any other status): never.
    let old = Some(THREE_DAYS * 100);
    let (state, _, purgeable) = lane_entry_state(&claimed, old, SIX_HOURS, None);
    assert_eq!((state, purgeable), (DiskState::Active, false));
}

#[test]
fn an_ownerless_checkout_is_kept_and_offered_once_quiet() {
    let (state, reason, purgeable) = ownerless_entry_state(Some(SIX_HOURS), SIX_HOURS);
    assert_eq!((state, purgeable), (DiskState::Kept, true));
    assert!(reason.unwrap().contains("no workspace row"));
    for recent in [Some(Duration::from_secs(60)), None] {
        let (state, _, purgeable) = ownerless_entry_state(recent, SIX_HOURS);
        assert_eq!((state, purgeable), (DiskState::Kept, false));
    }
}

fn sized(bytes: u64, unique: Option<u64>) -> DiskEntry {
    DiskEntry {
        unique_bytes: unique,
        ..entry(DiskKind::LaneBuildDir, DiskState::Orphan, bytes, false)
    }
}

/// KT-1691 — 33 lanes cloned from one seed read ~190 GB each (1.85 TB) while the
/// disk really held a fraction: totals carry both figures, and the real one is
/// `None` (not a guess) as soon as one entry cannot say.
#[test]
fn totals_carry_apparent_and_real_bytes_and_never_guess_the_real_one() {
    let cloned = vec![
        sized(190, Some(12)),
        sized(190, Some(8)),
        sized(190, Some(0)),
    ];
    let lanes = totals_by_kind(&cloned)
        .into_iter()
        .find(|total| total.kind == DiskKind::LaneBuildDir)
        .unwrap();
    assert_eq!(lanes.bytes, 570, "apparent counts shared blocks in full");
    assert_eq!(
        lanes.unique_bytes,
        Some(20),
        "real counts only what is owned"
    );
    assert_eq!((lanes.orphan_bytes, lanes.orphan_unique_bytes), (570, 20));

    let mixed = vec![sized(100, Some(5)), sized(100, None)];
    let lanes = totals_by_kind(&mixed)
        .into_iter()
        .find(|total| total.kind == DiskKind::LaneBuildDir)
        .unwrap();
    assert_eq!(lanes.unique_bytes, None);
    // The orphan figure falls back to apparent for the unknown one: an upper bound.
    assert_eq!(lanes.orphan_unique_bytes, 105);
    assert_eq!(sum_unique(&mixed), None);
    let empty = totals_by_kind(&[]);
    assert!(empty.iter().all(|total| total.unique_bytes == Some(0)));
}

#[test]
fn a_purge_reports_what_it_really_freed() {
    let item = |bytes, unique| PurgeItem {
        path: "p".into(),
        purged: true,
        bytes,
        unique_bytes: unique,
        bytes_are_partial: false,
        reason: None,
    };
    let result = finish_purge(
        "entries",
        vec![item(190, Some(12)), item(190, None), refused("q", "no")],
        0,
        &[],
        Some(100),
    );
    assert_eq!(result.freed_bytes, 380);
    assert_eq!(
        result.freed_unique_bytes, 202,
        "unknown falls back to apparent"
    );
    assert_eq!(result.available_bytes_before, Some(100));
}
