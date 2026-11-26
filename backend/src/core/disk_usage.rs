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

use crate::models::setup::{DEFAULT_DISK_STALLED_BUILD_DAYS, DEFAULT_DISK_SWEEP_IDLE_HOURS};

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

/// Free space a launch needs, in GiB. `0` = the guard is off, which is also the
/// state until [`set_disk_policy`] publishes the configured value at boot: a
/// process that never loaded a config (a unit test, a library user) must not be
/// refused by a threshold nobody chose.
static LAUNCH_MIN_FREE_GIB: AtomicU64 = AtomicU64::new(0);
static SWEEP_IDLE_HOURS: AtomicU64 = AtomicU64::new(DEFAULT_DISK_SWEEP_IDLE_HOURS);
/// KT-1691 — days a Blocked or Interrupted execution's build dir must be idle
/// before it is offered for a purge.
static STALLED_BUILD_DAYS: AtomicU64 = AtomicU64::new(DEFAULT_DISK_STALLED_BUILD_DAYS);

/// The shortest idle period after which a stalled execution's build dir is
/// offered for a purge: `0` would offer every Blocked execution's build dir at
/// once, so it is read as one day.
pub const MIN_STALLED_BUILD_DAYS: u64 = 1;

/// Publish `server.disk_stalled_build_days`. Called at boot and on every
/// config save.
pub fn set_stalled_build_days(days: u64) {
    STALLED_BUILD_DAYS.store(days.max(MIN_STALLED_BUILD_DAYS), Ordering::Relaxed);
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Same reason as `TEST_POLICY`: a test reads its own thread's value.
    static TEST_STALLED_DAYS: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// The configured number of days a Blocked or Interrupted execution's build
/// dir must be idle to be purgeable (the calling thread's own under `cfg(test)`).
pub fn stalled_build_days() -> u64 {
    if crate::core::test_mode::on() {
        #[cfg(any(test, feature = "test-support"))]
        {
            TEST_STALLED_DAYS
                .with(std::cell::Cell::get)
                .map_or(DEFAULT_DISK_STALLED_BUILD_DAYS, |days| {
                    days.max(MIN_STALLED_BUILD_DAYS)
                })
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            unreachable!()
        }
    } else {
        STALLED_BUILD_DAYS
            .load(Ordering::Relaxed)
            .max(MIN_STALLED_BUILD_DAYS)
    }
}

/// [`stalled_build_days`] as a duration.
pub fn stalled_build_idle() -> Duration {
    Duration::from_secs(stalled_build_days().saturating_mul(24 * 3600))
}

/// Sets the calling test thread's stalled-build period, restoring the previous
/// one on drop.
#[cfg(any(test, feature = "test-support"))]
pub struct TestStalledDays(Option<u64>);

#[cfg(any(test, feature = "test-support"))]
impl TestStalledDays {
    pub fn set(days: u64) -> Self {
        Self(TEST_STALLED_DAYS.with(|cell| cell.replace(Some(days))))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestStalledDays {
    fn drop(&mut self) {
        TEST_STALLED_DAYS.with(|cell| cell.set(self.0));
    }
}

/// The shortest quiet period a lane build dir must have before a sweep may take
/// it. A configured `0` would make every finished lane an orphan the instant it
/// was last written, i.e. turn the idle check off; it is read as this floor.
pub const MIN_SWEEP_IDLE_HOURS: u64 = 1;

/// The idle threshold actually applied for a configured number of hours.
pub fn effective_sweep_idle_hours(configured: u64) -> u64 {
    configured.max(MIN_SWEEP_IDLE_HOURS)
}

/// Publish `server.disk_launch_min_free_gib` and `server.disk_sweep_idle_hours`.
/// Called at boot and on every config save, so a changed setting applies
/// without a restart.
pub fn set_disk_policy(launch_min_free_gib: u64, sweep_idle_hours: u64) {
    LAUNCH_MIN_FREE_GIB.store(launch_min_free_gib, Ordering::Relaxed);
    SWEEP_IDLE_HOURS.store(
        effective_sweep_idle_hours(sweep_idle_hours),
        Ordering::Relaxed,
    );
}

/// What the process-wide policy currently holds.
fn published_policy() -> (u64, u64) {
    (
        LAUNCH_MIN_FREE_GIB.load(Ordering::Relaxed),
        effective_sweep_idle_hours(SWEEP_IDLE_HOURS.load(Ordering::Relaxed)),
    )
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// The policy a test runs under. The process-wide one is shared by every
    /// test thread (and `config::save` publishes into it), so a test never
    /// reads it: it reads this, which only its own thread can change.
    static TEST_POLICY: std::cell::Cell<Option<(u64, u64)>> =
        const { std::cell::Cell::new(None) };
}

/// The configured `(launch_min_free_gib, sweep_idle_hours)`.
///
/// Under `cfg(test)` this is the calling thread's own policy (the guard off and
/// the default idle period unless [`TestDiskPolicy`] set one), never the shared
/// one, so tests that provision workers cannot refuse one another.
pub fn disk_policy() -> (u64, u64) {
    if crate::core::test_mode::on() {
        #[cfg(any(test, feature = "test-support"))]
        {
            TEST_POLICY
                .with(std::cell::Cell::get)
                .map(|(gib, hours)| (gib, effective_sweep_idle_hours(hours)))
                .unwrap_or((0, DEFAULT_DISK_SWEEP_IDLE_HOURS))
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            unreachable!()
        }
    } else {
        published_policy()
    }
}

/// Sets the calling test thread's policy, restoring the previous one on drop
/// (also when an assertion panics).
#[cfg(any(test, feature = "test-support"))]
pub struct TestDiskPolicy(Option<(u64, u64)>);

#[cfg(any(test, feature = "test-support"))]
impl TestDiskPolicy {
    pub fn set(launch_min_free_gib: u64, sweep_idle_hours: u64) -> Self {
        let before =
            TEST_POLICY.with(|cell| cell.replace(Some((launch_min_free_gib, sweep_idle_hours))));
        Self(before)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestDiskPolicy {
    fn drop(&mut self) {
        TEST_POLICY.with(|cell| cell.set(self.0));
    }
}

/// How long a lane build dir must be unwritten before a sweep may remove it.
pub fn sweep_idle() -> Duration {
    Duration::from_secs(disk_policy().1.saturating_mul(3600))
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
    /// Not created by Kronn (no ownership marker) or owned by no workspace row.
    /// Reported; never touched by a sweep, only by a confirmed per-entry delete.
    Foreign,
    /// KT-1691 — the build dir of an execution that is Blocked or Interrupted
    /// and has not been written for `server.disk_stalled_build_days`. Only a
    /// confirmed per-entry delete takes it; its worktree and branch stay and the
    /// next resume rebuilds.
    Stalled,
}

impl DiskState {
    pub fn as_str(self) -> &'static str {
        match self {
            DiskState::Orphan => "orphan",
            DiskState::Reclaimable => "reclaimable",
            DiskState::Active => "active",
            DiskState::Kept => "kept",
            DiskState::Foreign => "foreign",
            DiskState::Stalled => "stalled",
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
///
/// `bytes` is the **apparent** size: what the files add up to, which counts the
/// blocks a copy-on-write clone shares with another tree in full (a lane cloned
/// from the seed `target/debug` reads as big as the seed). `unique_bytes` is the
/// **real** one: the blocks no other file shares, i.e. what deleting the tree
/// frees. `None` when the file system cannot say (anything but APFS, today), and
/// then only the apparent figure exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measured {
    pub bytes: u64,
    pub partial: bool,
    pub unique_bytes: Option<u64>,
}

impl Measured {
    /// What deleting frees when the file system can tell, else the apparent
    /// size (an upper bound).
    pub fn real_or_apparent(&self) -> u64 {
        self.unique_bytes.unwrap_or(self.bytes)
    }
}

/// Bytes of `path` no other file shares (APFS `ATTR_CMNEXT_PRIVATESIZE`), or
/// `None` when the file system cannot say. A plain file on a volume without
/// clones has nothing shared, but the file system is the one to say so.
#[cfg(target_os = "macos")]
pub fn private_size(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    /// `struct attrlist` of `<sys/attr.h>`.
    #[repr(C)]
    struct AttrList {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }
    extern "C" {
        fn getattrlist(
            path: *const libc::c_char,
            attr_list: *mut AttrList,
            attr_buf: *mut libc::c_void,
            attr_buf_size: libc::size_t,
            options: libc::c_ulong,
        ) -> libc::c_int;
    }
    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
    const FSOPT_NOFOLLOW: libc::c_ulong = 0x0000_0001;
    const FSOPT_ATTR_CMN_EXTENDED: libc::c_ulong = 0x0000_0020;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut list = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: ATTR_CMNEXT_PRIVATESIZE,
    };
    // The reply: a u32 length, then the off_t (8 bytes, 4-byte aligned).
    let mut buffer = [0u8; 16];
    // SAFETY: `c_path` is NUL-terminated, `list` and `buffer` outlive the call
    // and `buffer`'s size is passed.
    let status = unsafe {
        getattrlist(
            c_path.as_ptr(),
            &mut list,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            FSOPT_NOFOLLOW | FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    if status != 0 {
        return None;
    }
    let length = u32::from_ne_bytes(buffer[0..4].try_into().ok()?);
    if length < 12 {
        return None;
    }
    let size = i64::from_ne_bytes(buffer[4..12].try_into().ok()?);
    u64::try_from(size).ok()
}

#[cfg(not(target_os = "macos"))]
pub fn private_size(_path: &Path) -> Option<u64> {
    None
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

/// Sum the file sizes under `root`, stopping when the budget is spent (checked at every
/// entry). Top-level names in `skip` are not entered (the caller measures them as an entry
/// of their own). Rooted (KT-2084): every folder is listed through its own handle, so a
/// symlink, or a folder swapped for one mid-walk, is never followed. A folder it cannot read
/// is left out and the walk goes on; that, or a walk that stops short on its budget, is
/// `partial`.
pub fn measure_bounded(root: &Path, skip: &[&str], budget: MeasureBudget) -> Measured {
    use crate::core::fs_guard::{guarded_walk_skipping_unreadable, Visit, WalkBounds, WalkEnd};
    let mut measured = Measured::default();
    // Unique bytes are only reported when the file system answered for every
    // file: one it could not answer for would make the sum a lie.
    let mut unique = Some(0u64);
    let bounds = WalkBounds {
        max_entries: budget.max_entries,
        deadline: budget.deadline,
        max_depth: crate::core::rooted_fd::MAX_DEPTH,
    };
    // Opened from its parent: a measured folder that is a link is never followed (KT-2084).
    let parent = root.parent().unwrap_or(root);
    let walked = guarded_walk_skipping_unreadable(parent, root, bounds, &mut |entry| {
        if entry.depth == 0 {
            return Visit::Enter;
        }
        let skipped = entry.depth == 1
            && entry
                .path
                .file_name()
                .is_some_and(|name| skip.iter().any(|skip| name == *skip));
        if skipped || entry.is_symlink {
            return Visit::Skip;
        }
        if entry.is_dir {
            return Visit::Enter;
        }
        measured.bytes += entry.len;
        // The private size is still asked by path (a residual of the rooted walk).
        unique = match (unique, private_size(entry.path)) {
            (Some(total), Some(private)) => Some(total + private),
            _ => None,
        };
        Visit::Skip
    });
    measured.partial = !matches!(walked, Ok((WalkEnd::Done, 0)));
    measured.unique_bytes = unique;
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
