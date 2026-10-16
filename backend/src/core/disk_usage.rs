//! KT-1318 — Kronn's disk footprint: the policy, the free-space guard and the
//! bounded size measure behind the "Disk" surface (API, MCP tool, UI).
//!
//! Kronn creates three kinds of artefact on the operator's disk:
//! - the managed worktrees (`<repo>/.kronn/worktrees/<name>`),
//! - the per-lane Cargo build dirs (`<repo>/target/wt-<lane>`, 5-30 GB each),
//! - the caches inside a worktree (its own `target/`).
//!
//! What is Kronn's to delete is decided elsewhere (`core::worktree`: an
//! ownership marker plus durable execution state). This module only holds the
//! numbers: how much room a launch needs, how long a lane must have been quiet,
//! and a size measure that always returns — a floor, flagged, rather than a walk
//! that outlives the disk it is trying to report on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::models::setup::DEFAULT_DISK_SWEEP_IDLE_HOURS;

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

/// Free space a launch needs, in GiB. `0` = the guard is off, which is also the
/// state until [`set_disk_policy`] publishes the configured value at boot: a
/// process that never loaded a config (a unit test, a library user) must not be
/// refused by a threshold nobody chose.
static LAUNCH_MIN_FREE_GIB: AtomicU64 = AtomicU64::new(0);
static SWEEP_IDLE_HOURS: AtomicU64 = AtomicU64::new(DEFAULT_DISK_SWEEP_IDLE_HOURS);

/// Publish `server.disk_launch_min_free_gib` and `server.disk_sweep_idle_hours`.
/// Called at boot and on every config save, so a changed setting applies
/// without a restart.
pub fn set_disk_policy(launch_min_free_gib: u64, sweep_idle_hours: u64) {
    LAUNCH_MIN_FREE_GIB.store(launch_min_free_gib, Ordering::Relaxed);
    SWEEP_IDLE_HOURS.store(sweep_idle_hours, Ordering::Relaxed);
}

/// The configured `(launch_min_free_gib, sweep_idle_hours)`.
pub fn disk_policy() -> (u64, u64) {
    (
        LAUNCH_MIN_FREE_GIB.load(Ordering::Relaxed),
        SWEEP_IDLE_HOURS.load(Ordering::Relaxed),
    )
}

/// How long a lane build dir must be unwritten before a sweep may remove it.
pub fn sweep_idle() -> Duration {
    Duration::from_secs(
        SWEEP_IDLE_HOURS
            .load(Ordering::Relaxed)
            .saturating_mul(3600),
    )
}

/// The three families of artefact Kronn leaves on the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskKind {
    /// A managed task worktree (`<repo>/.kronn/worktrees/<name>`), sources only.
    Worktree,
    /// A lane's private Cargo target (`<repo>/target/wt-<lane>`).
    LaneBuildDir,
    /// A regenerable cache inside a worktree (its own `target/`).
    Cache,
}

impl DiskKind {
    pub const ALL: [DiskKind; 3] = [DiskKind::Worktree, DiskKind::LaneBuildDir, DiskKind::Cache];

    pub fn as_str(self) -> &'static str {
        match self {
            DiskKind::Worktree => "worktree",
            DiskKind::LaneBuildDir => "lane_build_dir",
            DiskKind::Cache => "cache",
        }
    }
}

/// Where an artefact stands, from the point of view of a purge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskState {
    /// Kronn's, finished, no live worktree, idle past the threshold: the
    /// default purge removes it.
    Orphan,
    /// Kronn's and finished, but not yet an orphan (written recently, or its
    /// clean worktree is kept). Only an explicit, confirmed per-entry delete
    /// removes it.
    Reclaimable,
    /// A live execution, session or lease still claims it. Never removed.
    Active,
    /// Kept on purpose or by a refusal (dirty worktree, unreadable state).
    Kept,
    /// Not created by Kronn (no ownership marker). Reported, never touched.
    Foreign,
}

impl DiskState {
    pub fn as_str(self) -> &'static str {
        match self {
            DiskState::Orphan => "orphan",
            DiskState::Reclaimable => "reclaimable",
            DiskState::Active => "active",
            DiskState::Kept => "kept",
            DiskState::Foreign => "foreign",
        }
    }
}

/// What a launch found on the volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchHeadroom {
    /// Enough room, the guard is off, or the free space cannot be read (the
    /// build itself then reports the real error).
    Ok,
    Low {
        available_gib: u64,
        min_gib: u64,
    },
}

/// Pure decision of the launch guard: `available_bytes` is `None` when the
/// filesystem could not answer.
pub fn classify_launch_headroom(available_bytes: Option<u64>, min_gib: u64) -> LaunchHeadroom {
    if min_gib == 0 {
        return LaunchHeadroom::Ok;
    }
    match available_bytes {
        Some(bytes) if bytes < min_gib.saturating_mul(BYTES_PER_GIB) => LaunchHeadroom::Low {
            available_gib: bytes / BYTES_PER_GIB,
            min_gib,
        },
        _ => LaunchHeadroom::Ok,
    }
}

/// The nearest ancestor of `path` that exists: a repository's `target/` is not
/// there before its first build, but its volume is the repository's.
pub fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.exists())
        .map(Path::to_path_buf)
}

/// Free bytes on the volume holding `path` (O(1), no walk).
pub fn available_bytes(path: &Path) -> Option<u64> {
    fs2::available_space(existing_ancestor(path)?).ok()
}

/// Total bytes of the volume holding `path`.
pub fn total_bytes(path: &Path) -> Option<u64> {
    fs2::total_space(existing_ancestor(path)?).ok()
}

/// A size that is exact, or a floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measured {
    pub bytes: u64,
    pub partial: bool,
}

/// What one measure may spend. A Rust target holds more than a million files,
/// so no budget is generous enough to be exact for all of them: the point is
/// that it stops, and says so.
#[derive(Debug, Clone, Copy)]
pub struct MeasureBudget {
    pub max_entries: usize,
    pub deadline: Instant,
}

impl MeasureBudget {
    pub fn within(max_entries: usize, time: Duration) -> Self {
        Self {
            max_entries,
            deadline: Instant::now() + time,
        }
    }
}

/// Sum the file sizes under `root`, never following a symlink, stopping when
/// the budget is spent. Top-level names in `skip` are not entered (the caller
/// measures them as an entry of their own).
pub fn measure_bounded(root: &Path, skip: &[&str], budget: MeasureBudget) -> Measured {
    let mut measured = Measured::default();
    let mut visited = 0usize;
    let mut stack = vec![(root.to_path_buf(), true)];
    while let Some((dir, top_level)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            // The clock is read every few hundred entries, not at every one.
            if visited > budget.max_entries
                || (visited.is_multiple_of(256) && Instant::now() > budget.deadline)
            {
                measured.partial = true;
                return measured;
            }
            if top_level && skip.iter().any(|name| entry.file_name() == *name) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push((entry.path(), false));
            } else {
                measured.bytes += meta.len();
            }
        }
    }
    measured
}

/// Seconds since `path` was last modified, when the filesystem says.
pub fn idle_seconds(path: &Path) -> Option<u64> {
    let modified = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    Some(
        std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_secs(),
    )
}

#[cfg(test)]
#[path = "disk_usage_test.rs"]
mod disk_usage_test;
