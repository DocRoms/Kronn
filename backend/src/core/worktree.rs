//! Git worktree management for discussion isolation.
//!
//! Each isolated discussion gets its own git worktree so agents can make
//! changes without interfering with the main working tree or other discussions.

use std::path::{Path, PathBuf};

use crate::core::lane_git::plainly_spelled;
use crate::core::task_worker_env::LANE_TARGET_PREFIX;
use crate::models::WorkerCommitHooks;

// ── KT-373 — disk as a provisioning precondition ─────────────────────────────
//
// On 2026-08-21 the dev volume reached 100% with 753 MiB left: seven worktrees
// were each holding their own Rust `target/`, 7.5 to 24.6 GiB apiece.
// Provisioning never asked whether there was room, so it kept succeeding until
// nothing on the machine worked — and the recovery cost hours of manual
// deletion across millions of files.
//
// The guard is deliberately the cheap kind. It reads the filesystem's own free
// count, which is O(1), and never walks a directory: a recursive size estimate
// on an already-full disk of small Rust artefacts takes minutes, and would put
// the most expensive possible operation on the path that is failing.

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

/// What a free-space check concluded, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskHeadroom {
    /// Above every threshold, or unmeasurable — provisioning proceeds.
    Ok,
    /// Below the warning threshold: provision, but say so.
    Low {
        available_gib: u64,
        warning_gib: u64,
    },
    /// Below the critical threshold: refuse.
    Critical {
        available_gib: u64,
        critical_gib: u64,
    },
}

/// Classify free space at `path` against the configured thresholds.
///
/// An unreadable filesystem reports `Ok` rather than `Critical`. That is a
/// deliberate asymmetry: this guard exists to prevent a disk from filling, not
/// to become a new way for provisioning to fail. A platform whose free space we
/// cannot read is a platform where we have no evidence of a problem, and
/// refusing on no evidence would break every user to protect none.
pub fn disk_headroom(path: &Path, warning_gib: u64, critical_gib: u64) -> DiskHeadroom {
    let available = match fs2::available_space(path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            tracing::debug!(
                "disk headroom unreadable at {}: {error} — provisioning proceeds",
                path.display()
            );
            None
        }
    };
    classify_disk_headroom(available, warning_gib, critical_gib)
}

/// Apply the provisioning policy independently from the host filesystem probe.
///
/// Windows may report the containing volume's free space for a missing child
/// path while Unix `statvfs` rejects the same shape. Keeping the policy pure
/// lets the unreadable case stay testable without pretending those APIs have
/// identical path-resolution semantics.
fn classify_disk_headroom(
    available: Option<u64>,
    warning_gib: u64,
    critical_gib: u64,
) -> DiskHeadroom {
    let Some(available) = available else {
        return DiskHeadroom::Ok;
    };
    let available_gib = available / BYTES_PER_GIB;
    // A warning below the critical mark could never fire, and would read as a
    // configuration that warns when it in fact refuses. The critical threshold
    // wins, and the warning is pulled up to meet it.
    let warning_gib = warning_gib.max(critical_gib);
    if available_gib < critical_gib {
        return DiskHeadroom::Critical {
            available_gib,
            critical_gib,
        };
    }
    if available_gib < warning_gib {
        return DiskHeadroom::Low {
            available_gib,
            warning_gib,
        };
    }
    DiskHeadroom::Ok
}

/// One managed worktree's regenerable build artefacts, as found on disk.
///
/// Deliberately free of any judgement about whether it may be deleted: this
/// half only reports what exists. Whether a worktree is still in use is a
/// question the durable execution state answers, and the 2026-08-21 incident is
/// what happens when a filesystem scan is allowed to answer it instead — a
/// worktree with no visible `cargo` process was cleaned while an agent owned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildArtifactTarget {
    /// The managed worktree this belongs to.
    pub worktree_path: PathBuf,
    /// The `target/` directory itself — the only thing cleanup may ever remove.
    pub target_path: PathBuf,
    /// Last modification of the directory, when the filesystem will say.
    pub modified: Option<std::time::SystemTime>,
    /// Bytes counted while walking, which stops at `walk_budget`.
    pub bytes: u64,
    /// True when the walk hit its budget: `bytes` is then a floor, not a size.
    pub size_is_partial: bool,
    /// Set when this entry exists but must never be reclaimed, naming why.
    ///
    /// Refused entries are LISTED, not dropped. A dry run that silently omits a
    /// symlinked or unreadable target is how someone concludes the tool found
    /// nothing and goes deleting by hand — which is exactly the 2026-08-21
    /// sequence. What we refuse to touch is precisely what a human most needs
    /// to see.
    pub refusal: Option<String>,
}

/// How many directory entries one scan will visit before giving up on an exact
/// size. A Rust `target/` reached 1.69 million files in the incident, and
/// walking it took over 44 minutes — on a machine that was already unusable.
/// A partial answer delivered now beats an exact one delivered after the disk
/// has filled, so the walk stops and says so rather than finishing at any cost.
/// Same walk, a budget a unit test can exceed without writing 20,000 files.
fn scan_entry_budget() -> usize {
    if crate::core::test_mode::on() {
        500
    } else {
        20_000
    }
}

/// An entry that exists and must not be reclaimed, carried into the inventory
/// so a dry run never hides what it refuses to touch.
fn refused_entry(worktree_path: &Path, reason: String) -> BuildArtifactTarget {
    BuildArtifactTarget {
        worktree_path: worktree_path.to_path_buf(),
        target_path: worktree_path.join("target"),
        modified: None,
        // No size is claimed for something we would not measure or remove.
        bytes: 0,
        size_is_partial: false,
        refusal: Some(reason),
    }
}

/// Inventory the build artefacts of every managed worktree, oldest first.
///
/// Read-only and deterministic: it never deletes, and the order does not depend
/// on filesystem enumeration order, so a dry-run shown to a human matches what
/// a later pass would act on.
pub fn scan_build_artifacts(repo_path: &Path) -> Vec<BuildArtifactTarget> {
    let managed_root = worktree_base_dir(repo_path);
    let entries = match std::fs::read_dir(&managed_root) {
        Ok(entries) => entries,
        // No managed root means nothing was ever provisioned here. That is an
        // empty inventory, not an error to report to anyone.
        Err(_) => return Vec::new(),
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let worktree_path = entry.path();
        // Reuse the provisioning-side assertion rather than trusting the walk:
        // ownership, direct-child layout, symlinks and reparse points are all
        // decided by the same code that guards creation and removal.
        if let Err(reason) = assert_managed_task_worktree_path(repo_path, &worktree_path) {
            found.push(refused_entry(&worktree_path, reason));
            continue;
        }
        let target_path = worktree_path.join("target");
        let meta = match std::fs::symlink_metadata(&target_path) {
            Ok(meta) => meta,
            // Absent is genuinely nothing to report. Unreadable is not: say so
            // rather than let it vanish from the inventory.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                found.push(refused_entry(
                    &worktree_path,
                    format!("cannot read {}: {error}", target_path.display()),
                ));
                continue;
            }
        };
        // A `target` that is a symlink is not ours to follow, and never ours to
        // delete: it points at storage this repository does not own. Listed as
        // refused, because a human hunting for space needs to know it is there.
        if meta.file_type().is_symlink() {
            found.push(refused_entry(
                &worktree_path,
                format!(
                    "{} is a symlink — it points outside this repository",
                    target_path.display()
                ),
            ));
            continue;
        }
        if !meta.is_dir() {
            found.push(refused_entry(
                &worktree_path,
                format!("{} is not a directory", target_path.display()),
            ));
            continue;
        }
        let (bytes, size_is_partial) = measure_within_budget(&target_path);
        found.push(BuildArtifactTarget {
            worktree_path,
            target_path,
            modified: meta.modified().ok(),
            bytes,
            size_is_partial,
            refusal: None,
        });
    }
    // Oldest first: the least likely to be wanted back is the first offered.
    found.sort_by(|a, b| {
        a.modified
            .cmp(&b.modified)
            .then_with(|| a.target_path.cmp(&b.target_path))
    });
    found
}

/// Sum file sizes under `root`, stopping at `scan_entry_budget()` entries.
///
/// Returns `(bytes, partial)`. Symlinks are counted as entries but never
/// followed, so a link into a huge tree cannot make this walk unbounded.
pub fn measure_within_budget(root: &Path) -> (u64, bool) {
    let measured = measure_real_within_budget(root);
    (measured.bytes, measured.partial)
}

/// [`measure_within_budget`] with the unique (non-shared) bytes too: a lane
/// cloned from the seed counts the blocks it shares in full in `bytes`, and
/// only `unique_bytes` is what deleting it frees (KT-1691).
pub fn measure_real_within_budget(root: &Path) -> crate::core::disk_usage::Measured {
    crate::core::disk_usage::measure_bounded(
        root,
        &[],
        crate::core::disk_usage::MeasureBudget::within(
            scan_entry_budget(),
            std::time::Duration::from_secs(120),
        ),
    )
}

/// What the durable execution state says about a worktree, as the caller read
/// it from the database.
///
/// An enum rather than a bool on purpose. A boolean parameter at a call site
/// reads as `clean(path, true)` and is trivial to pass the wrong way round;
/// this makes the caller name what it actually knows, and makes `Unknown` — the
/// dangerous case — impossible to express by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionLiveness {
    /// Every durable condition for cleanup holds. Produced by
    /// `db::orchestration::worktree_cleanup_liveness`, which is the only place
    /// that knows all of them — a terminal execution is necessary and NOT
    /// sufficient: a finished execution can still have an attached session, a
    /// live worker lease, or an externally-owned workspace.
    Terminal,
    /// Something durable says this worktree is still in use. Carries the reason,
    /// because "refused" without "why" sends a human looking at the wrong thing.
    Active(String),
    /// Durable state could not answer. Refused, and it says what was missing.
    Unknown(String),
}

/// What one cleanup did, for the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub target_path: PathBuf,
    /// Bytes counted before deleting, floored at the scan budget.
    pub bytes_reclaimed: u64,
    /// True when `bytes_reclaimed` is a floor rather than the whole figure.
    pub bytes_are_partial: bool,
    /// KT-1691 — of those, the bytes no other file shared (what the delete
    /// really freed); `None` when the file system cannot say. `bytes_reclaimed`
    /// counts the blocks a copy-on-write clone shares in full.
    pub unique_bytes: Option<u64>,
}

/// Delete the regenerable build artefacts of one managed worktree.
///
/// Refuses unless every condition holds, and says which one failed. The order
/// matters: ownership is settled before anything is read, and liveness before
/// anything is removed.
///
/// `liveness` must come from the durable execution state. A process scan may
/// still refuse a cleanup this function would allow — a running compiler is
/// evidence of activity — but it must never be what authorises one. On
/// 2026-08-21 a worktree was cleaned because no `cargo` was visible while an
/// agent owned it; absence of a compiler is not evidence of absence of work.
pub fn clean_worktree_build_artifacts(
    repo_path: &Path,
    worktree_path: &Path,
    liveness: ExecutionLiveness,
) -> Result<CleanupReport, String> {
    // Ownership first: an unmanaged path is refused before it is even read.
    assert_managed_task_worktree_path(repo_path, worktree_path)?;

    match liveness {
        ExecutionLiveness::Terminal => {}
        ExecutionLiveness::Active(reason) => {
            return Err(format!(
                "refusing to clean {}: {reason}",
                worktree_path.display()
            ))
        }
        // Fail closed. A missing execution row is an inconsistency to report,
        // not a directory to delete.
        ExecutionLiveness::Unknown(reason) => {
            return Err(format!(
                "refusing to clean {}: {reason}. Cleanup is authorised by durable state \
                 alone, never by the absence of a running build.",
                worktree_path.display()
            ))
        }
    }

    let target_path = worktree_path.join("target");
    let meta = match std::fs::symlink_metadata(&target_path) {
        Ok(meta) => meta,
        // Absence is a success with a zero figure: the caller asked for the
        // artefacts to be gone, and they are. ONLY absence. Any other error —
        // permission, I/O, a path that cannot be resolved — means we do not know
        // what is there, and "I could not look" must never be reported as
        // "nothing to do" (DoD-7).
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CleanupReport {
                target_path,
                bytes_reclaimed: 0,
                bytes_are_partial: false,
                unique_bytes: Some(0),
            })
        }
        Err(error) => {
            return Err(format!(
                "refusing to clean {}: cannot read it ({error}). Nothing was attempted.",
                target_path.display()
            ))
        }
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing to clean {}: it is a symlink, so it points at storage this \
             repository does not own",
            target_path.display()
        ));
    }
    if !meta.is_dir() {
        return Err(format!(
            "refusing to clean {}: expected a directory",
            target_path.display()
        ));
    }
    assert_no_build_in_progress(&target_path)?;

    let measured = measure_real_within_budget(&target_path);
    let (bytes_reclaimed, bytes_are_partial) = (measured.bytes, measured.partial);
    // Rooted at the worktree, never through a link, bounded in depth (KT-2084): what the
    // removal stopped at stays.
    crate::core::fs_guard::guarded_remove_tree(worktree_path, &target_path).map_err(|error| {
        if crate::core::fs_guard::is_link_refusal(&error) {
            format!("refusing to clean {}: {error}", target_path.display())
        } else {
            format!(
                "failed to clean {} ({PARTLY_REMOVED} at most): {error}. The rest is left as \
                 it stands.",
                target_path.display()
            )
        }
    })?;

    tracing::info!(
        "Reclaimed build artefacts at {} ({} bytes{})",
        target_path.display(),
        bytes_reclaimed,
        if bytes_are_partial { "+, floored" } else { "" }
    );
    Ok(CleanupReport {
        target_path,
        bytes_reclaimed,
        bytes_are_partial,
        unique_bytes: measured.unique_bytes,
    })
}

// ── KT-1256 — lane targets outlive their worktree ────────────────────────────
//
// A task worker builds in `<repo>/target/wt-<lane>`, outside its worktree, so
// removing the worktree never took the lane target with it. Each one starts as
// a copy-on-write clone of the main target and becomes real disk once the main
// target is rebuilt or cleaned, plus several GB of its own artefacts.

/// The lane target a managed task worktree builds into.
///
/// Only the lane name is taken from the worktree: the directory is always a
/// direct child of the repository's own `target/`.
pub fn lane_target_for_worktree(repo_path: &Path, worktree_path: &Path) -> Result<PathBuf, String> {
    let lane = crate::core::task_worker_env::WorkerLane::for_worktree(worktree_path)?;
    Ok(repo_path
        .join("target")
        .join(format!("{}{}", LANE_TARGET_PREFIX, lane.name)))
}

/// Direct child of `<repo>/target` named `wt-<something>`, with no symlink on
/// the way. The deletion below is only ever offered such a path.
fn assert_lane_target_path(repo_path: &Path, lane_target: &Path) -> Result<(), String> {
    let target_root = repo_path.join("target");
    let relative = lane_target.strip_prefix(&target_root).map_err(|_| {
        format!(
            "{} is outside {} — not a lane target",
            lane_target.display(),
            target_root.display()
        )
    })?;
    let mut components = relative.components();
    let name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => name.to_string_lossy().to_string(),
        _ => {
            return Err(format!(
                "{} is not a direct child of {}",
                lane_target.display(),
                target_root.display()
            ))
        }
    };
    if name
        .strip_prefix(LANE_TARGET_PREFIX)
        .is_none_or(|lane| lane.is_empty())
    {
        return Err(format!(
            "{} is not named {LANE_TARGET_PREFIX}<lane> — not a lane target",
            lane_target.display()
        ));
    }
    crate::core::fs_guard::assert_contained_no_symlink(repo_path, lane_target)
}

/// Delete one lane target. Only the path shape is checked here: that Kronn
/// owns the lane (the worktree it was just removed with, or its ownership
/// marker, see [`sweep_lane_targets`]) and that it is no longer in use are the
/// caller's to prove, except that a running Cargo (its build lock) always
/// refuses.
///
/// Absent is a success with a zero figure; any other unreadable state is an
/// error, never "nothing to do".
pub fn remove_lane_target(repo_path: &Path, lane_target: &Path) -> Result<CleanupReport, String> {
    assert_lane_target_path(repo_path, lane_target)?;
    let meta = match std::fs::symlink_metadata(lane_target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CleanupReport {
                target_path: lane_target.to_path_buf(),
                bytes_reclaimed: 0,
                bytes_are_partial: false,
                unique_bytes: Some(0),
            })
        }
        Err(error) => {
            return Err(format!(
                "refusing to clean {}: cannot read it ({error}). Nothing was attempted.",
                lane_target.display()
            ))
        }
    };
    if !meta.is_dir() {
        return Err(format!(
            "refusing to clean {}: expected a directory",
            lane_target.display()
        ));
    }
    assert_no_build_in_progress(lane_target)?;

    let measured = measure_real_within_budget(lane_target);
    let (bytes_reclaimed, bytes_are_partial) = (measured.bytes, measured.partial);
    crate::core::fs_guard::guarded_remove_tree(repo_path, lane_target).map_err(|error| {
        format!(
            "failed to clean {}: {error}. The lane target is left as it stands.",
            lane_target.display()
        )
    })?;
    tracing::info!(
        "Reclaimed lane target {} ({} bytes{})",
        lane_target.display(),
        bytes_reclaimed,
        if bytes_are_partial { "+, floored" } else { "" }
    );
    Ok(CleanupReport {
        target_path: lane_target.to_path_buf(),
        bytes_reclaimed,
        bytes_are_partial,
        unique_bytes: measured.unique_bytes,
    })
}

// ── KT-1691 — a checkout no workspace row names ──────────────────────────────
//
// A directory under `<repo>/.kronn/worktrees` that durable state does not know
// (the database was reset or restored, a row was deleted) is judged by nothing:
// no execution finishes it, so no release ever takes it. It is only offered to
// the owner, who confirms each one; these two functions are what a confirmed
// delete runs. The caller has proven no row names the path.

/// Delete the whole checkout of an ownerless worktree and its registration (KT-2084):
/// Kronn's judgement ([`kept_by_git`]) keeps what git's removal without `--force` kept
/// (uncommitted or untracked files, a populated submodule, a repository kept inside it, a
/// lock); its ignored files go with it, and the branch stays. Refuses a running build.
pub fn remove_ownerless_worktree(
    repo_path: &Path,
    worktree_path: &Path,
) -> Result<CleanupReport, String> {
    assert_managed_task_worktree_path(repo_path, worktree_path)?;
    let meta = std::fs::symlink_metadata(worktree_path).map_err(|error| {
        format!(
            "refusing to remove {}: cannot read it ({error})",
            worktree_path.display()
        )
    })?;
    if !meta.is_dir() {
        return Err(format!(
            "refusing to remove {}: expected a directory",
            worktree_path.display()
        ));
    }
    // Its own entry, never followed (KT-2084).
    if std::fs::symlink_metadata(worktree_path.join(".git")).is_err() {
        return Err(format!(
            "{} is not a git worktree: remove it by hand once you know it is yours",
            worktree_path.display()
        ));
    }
    // A link named `target` is the checkout's own entry, never followed (KT-2084).
    let target = worktree_path.join("target");
    if std::fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_dir()) {
        assert_no_build_in_progress(&target)?;
    }
    let measured = measure_real_within_budget(worktree_path);
    // Kronn's judgement, not git's refusal: OS junk is not work, no exclude needed.
    let (root, checkout) = owned_checkout(repo_path, worktree_path, true)?;
    let judged = Judgement::of(&checkout)?;
    match remove_judged_checkout(repo_path, &root, &checkout, judged, Removal::Ownerless) {
        Ok(()) => {}
        Err(NotRemoved::Kept(reason) | NotRemoved::Failed(reason)) => return Err(reason),
    }
    tracing::info!(
        "Removed ownerless worktree {} ({} bytes{})",
        worktree_path.display(),
        measured.bytes,
        if measured.partial { "+, floored" } else { "" }
    );
    Ok(CleanupReport {
        target_path: worktree_path.to_path_buf(),
        bytes_reclaimed: measured.bytes,
        bytes_are_partial: measured.partial,
        unique_bytes: measured.unique_bytes,
    })
}

/// Delete the `target/` inside an ownerless worktree (regenerable). Refuses a
/// running build and a `target` that is not a plain directory.
pub fn remove_ownerless_worktree_cache(
    repo_path: &Path,
    worktree_path: &Path,
) -> Result<CleanupReport, String> {
    assert_managed_task_worktree_path(repo_path, worktree_path)?;
    let target_path = worktree_path.join("target");
    let meta = match std::fs::symlink_metadata(&target_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CleanupReport {
                target_path,
                bytes_reclaimed: 0,
                bytes_are_partial: false,
                unique_bytes: Some(0),
            })
        }
        Err(error) => {
            return Err(format!(
                "refusing to clean {}: cannot read it ({error})",
                target_path.display()
            ))
        }
    };
    if !meta.is_dir() {
        return Err(format!(
            "refusing to clean {}: it is not a plain directory",
            target_path.display()
        ));
    }
    assert_no_build_in_progress(&target_path)?;
    let measured = measure_real_within_budget(&target_path);
    crate::core::fs_guard::guarded_remove_tree(repo_path, &target_path).map_err(|error| {
        format!(
            "failed to clean {}: {error}. The target is left as it stands.",
            target_path.display()
        )
    })?;
    Ok(CleanupReport {
        target_path,
        bytes_reclaimed: measured.bytes,
        bytes_are_partial: measured.partial,
        unique_bytes: measured.unique_bytes,
    })
}

/// Name of the ownership marker inside a lane target.
///
/// A directory called `wt-*` proves nothing: humans and other tooling build in
/// `target/wt-0151-mutants` and the like. Only a lane Kronn provisioned for an
/// execution carries this file, and only such a lane is ever swept.
pub const LANE_OWNER_MARKER: &str = ".kronn-lane.json";
const LANE_OWNER_VERSION: u32 = 1;
/// A marker is a few hundred bytes; anything larger is not ours to parse.
const LANE_OWNER_MAX_BYTES: u64 = 4096;

/// What the ownership marker records: which execution a lane was made for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaneOwner {
    pub version: u32,
    /// The lane name; must equal the directory's `wt-<lane>` suffix.
    pub lane: String,
    pub execution_id: String,
    /// The task worktree the lane was provisioned for. Only its directory name
    /// is used, re-rooted under the swept repository's managed root.
    pub worktree: String,
}

/// Record that Kronn provisioned `worktree_path`'s lane target for
/// `execution_id`, creating the directory when it is not there yet.
///
/// Idempotent. Called once per launch, before anything is built in the lane.
/// A lane that could not be marked is simply never swept.
pub fn mark_lane_target(
    repo_path: &Path,
    worktree_path: &Path,
    execution_id: &str,
) -> Result<PathBuf, String> {
    if execution_id.trim().is_empty() {
        return Err("a lane target cannot be marked without an execution id".to_string());
    }
    let lane = crate::core::task_worker_env::WorkerLane::for_worktree(worktree_path)?;
    let lane_target = lane_target_for_worktree(repo_path, worktree_path)?;
    assert_lane_target_path(repo_path, &lane_target)?;
    crate::core::fs_guard::guarded_create_dir_all(repo_path, &lane_target)?;
    let owner = LaneOwner {
        version: LANE_OWNER_VERSION,
        lane: lane.name,
        execution_id: execution_id.to_string(),
        worktree: worktree_path.to_string_lossy().to_string(),
    };
    // The stamp is Kronn's, not the project's: in a checkout that does not
    // ignore `target/` it would read as uncommitted work and hold up the
    // integration preflight. A `*` ignore file keeps git blind to the whole lane.
    let ignore = lane_target.join(".gitignore");
    crate::core::fs_guard::guarded_write_new(repo_path, &ignore, b"*\n").map_err(|error| {
        format!(
            "cannot write the lane ignore file in {}: {error}",
            lane_target.display()
        )
    })?;
    // Already stamped for this very execution: leave the file (and the lane's
    // idle clock) alone, so a relaunch does not look like a build.
    if read_lane_owner(&lane_target).as_ref() == Some(&owner) {
        return Ok(lane_target);
    }
    let bytes = serde_json::to_vec(&owner).map_err(|error| error.to_string())?;
    crate::core::fs_guard::guarded_replace(repo_path, &lane_target.join(LANE_OWNER_MARKER), &bytes)
        .map_err(|error| {
            format!(
                "cannot write the lane marker in {}: {error}",
                lane_target.display()
            )
        })?;
    Ok(lane_target)
}

/// [`mark_lane_target`] for a worktree whose repository is not known to the
/// caller: it is derived from the managed layout
/// (`<repo>/.kronn/worktrees/<name>`). A worktree outside that layout builds
/// inside itself, has no `target/wt-*` lane, and is refused.
///
/// Every place that makes Kronn start a build in a worker's lane - the
/// worker's own launch (task or campaign), and the sandbox a validation runs
/// in - stamps it first, so no lane Kronn created is ever left unowned.
pub fn mark_lane_target_for_worktree(
    worktree_path: &Path,
    execution_id: &str,
) -> Result<PathBuf, String> {
    let worktree = worktree_path.canonicalize().map_err(|error| {
        format!(
            "cannot canonicalize worktree {}: {error}",
            worktree_path.display()
        )
    })?;
    let lane = crate::core::task_worker_env::WorkerLane::for_worktree(&worktree)?;
    let repo = lane.main_checkout.ok_or_else(|| {
        format!(
            "{} is not a managed worktree, so it has no lane target",
            worktree.display()
        )
    })?;
    mark_lane_target(&repo, &worktree, execution_id)
}

/// The marker of a lane target, when it is a plain, well-formed file whose
/// lane agrees with the directory name. Anything else is "not marked".
/// Read without following a link and never from a FIFO (KT-2084).
fn read_lane_owner(lane_target: &Path) -> Option<LaneOwner> {
    let marker = lane_target.join(LANE_OWNER_MARKER);
    // Rooted at its parent: a lane target that is a link is never followed (KT-2084).
    let parent = lane_target.parent()?;
    let bytes = crate::core::fs_guard::guarded_read_head(parent, &marker, LANE_OWNER_MAX_BYTES + 1)
        .ok()??;
    if bytes.len() as u64 > LANE_OWNER_MAX_BYTES {
        return None;
    }
    let owner: LaneOwner = serde_json::from_slice(&bytes).ok()?;
    let directory_lane = lane_target
        .file_name()?
        .to_string_lossy()
        .strip_prefix(LANE_TARGET_PREFIX)?
        .to_string();
    (owner.version == LANE_OWNER_VERSION
        && !owner.execution_id.trim().is_empty()
        && owner.lane == directory_lane)
        .then_some(owner)
}

/// Every `<repo>/target/wt-*` entry, sorted so a dry run matches the pass that
/// acts on it.
pub fn lane_target_dirs(repo_path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(repo_path.join("target")) else {
        // No target directory means no lane was ever built here.
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(LANE_TARGET_PREFIX)
                .is_some_and(|lane| !lane.is_empty())
        })
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs
}

/// The owners of every marked lane target of a repository: what the caller
/// needs to ask durable state about before a sweep.
pub fn lane_target_owners(repo_path: &Path) -> Vec<LaneOwner> {
    lane_target_dirs(repo_path)
        .iter()
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()))
        .filter_map(|path| read_lane_owner(path))
        .collect()
}

/// How far a sweep goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneSweepScope {
    /// Nothing is deleted: the report is the dry run.
    ReportOnly,
    /// Delete only the lanes whose worktree no longer exists.
    WorktreeGone,
    /// Also delete the lanes of a finished execution whose worktree is still
    /// there but clean. A dirty worktree always keeps its lane.
    WorktreeGoneOrClean,
}

/// What a lane target is, judged without touching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneTargetState {
    /// No ownership marker: Kronn cannot prove it created this directory, so
    /// it is reported and never touched.
    Unmarked,
    /// Its execution is alive, or durable state could not say otherwise.
    Claimed(LaneOwner, String),
    /// Not to be removed: a dirty worktree kept on purpose, an unreadable one,
    /// a path that is not a plain lane directory. The owner is known when the
    /// marker was.
    Kept(Option<LaneOwner>, String),
    /// Kronn's, finished, and nothing needs it.
    Reclaimable {
        owner: LaneOwner,
        worktree_present: bool,
    },
}

/// What was done with one lane target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneTargetOutcome {
    Removed(LaneOwner, CleanupReport),
    /// Reclaimable but not removed (dry run, or outside the scope); the report
    /// carries the measured size.
    WouldRemove(LaneOwner, CleanupReport),
    Claimed(LaneOwner, String),
    Kept(Option<LaneOwner>, String),
    Unmarked,
}

/// What one sweep of `<repo>/target/wt-*` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaneTargetSweep {
    /// Marked lanes of finished executions that were deleted.
    pub removed: Vec<(LaneOwner, CleanupReport)>,
    /// Marked lanes that are reclaimable and were not deleted, with their size.
    pub would_remove: Vec<(LaneOwner, CleanupReport)>,
    /// Marked lanes a live execution still claims, with the reason.
    pub claimed: Vec<(PathBuf, String)>,
    /// Lanes kept on purpose or by a refusal, with the reason.
    pub kept: Vec<(PathBuf, String)>,
    /// `wt-*` directories Kronn did not create. Reported only.
    pub unmarked: Vec<PathBuf>,
}

impl LaneTargetSweep {
    /// Bytes the deleted lanes held, floored at the scan budget.
    pub fn bytes_removed(&self) -> u64 {
        self.removed
            .iter()
            .map(|(_, report)| report.bytes_reclaimed)
            .sum()
    }
}

/// Judge one `<repo>/target/wt-*` directory.
///
/// `judge` answers, from durable state, whether the marker's execution is
/// finished (`Ok`) or why it is not (`Err(reason)`); a lane it has no answer
/// for must come back `Err`. The worktree is looked up under the repository's
/// own managed root, never at the path the marker spells.
pub fn classify_lane_target(
    repo_path: &Path,
    lane_target: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
) -> LaneTargetState {
    classify_lane_target_attributed(repo_path, lane_target, judge, &LaneAttributions::new())
}

/// Lane owners durable state names without a marker on disk, by lane name
/// (KT-1397): a managed workspace row of a finished execution remembers the
/// worktree it had, and the lane derives from that name. Built from the
/// database, never from the directory being judged.
pub type LaneAttributions = std::collections::HashMap<String, LaneOwner>;

/// [`classify_lane_target`] that also recognises an unmarked lane by its
/// durable lineage. The `target/wt-*` dirs of 20/10 predate the marker, or lost
/// it; their execution is nonetheless in the database. Such a lane is only
/// reclaimable once its worktree is gone: it never rides on a present checkout.
pub fn classify_lane_target_attributed(
    repo_path: &Path,
    lane_target: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    attributions: &LaneAttributions,
) -> LaneTargetState {
    if let Err(reason) = assert_lane_target_path(repo_path, lane_target) {
        return LaneTargetState::Kept(None, reason);
    }
    match std::fs::symlink_metadata(lane_target) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return LaneTargetState::Kept(None, "not a plain directory — left alone".to_string())
        }
        Err(error) => {
            return LaneTargetState::Kept(None, format!("cannot read it ({error})"));
        }
    }
    let (owner, by_lineage) = match read_lane_owner(lane_target) {
        Some(owner) => (owner, false),
        None => {
            let attributed = lane_target
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(LANE_TARGET_PREFIX))
                .and_then(|lane| attributions.get(lane));
            match attributed {
                Some(owner) => (owner.clone(), true),
                None => return LaneTargetState::Unmarked,
            }
        }
    };
    if let Err(reason) = judge(&owner) {
        return LaneTargetState::Claimed(owner, reason);
    }
    let Some(name) = Path::new(&owner.worktree).file_name() else {
        return LaneTargetState::Kept(Some(owner), "the marker names no worktree".to_string());
    };
    let worktree = worktree_base_dir(repo_path).join(name);
    match classify_lane_worktree(repo_path, &owner.lane, &worktree) {
        Ok(true) if by_lineage => LaneTargetState::Kept(
            Some(owner),
            "no ownership marker, attributed by its workspace lineage, and its worktree \
             still exists — left alone"
                .to_string(),
        ),
        Ok(worktree_present) => LaneTargetState::Reclaimable {
            owner,
            worktree_present,
        },
        Err(reason) => LaneTargetState::Kept(Some(owner), reason),
    }
}

/// `Ok(present)` when the lane's worktree is gone (`false`) or clean (`true`);
/// `Err` says why the lane has to stay.
fn classify_lane_worktree(repo_path: &Path, lane: &str, worktree: &Path) -> Result<bool, String> {
    // The managed name must derive this very lane, so a forged marker cannot
    // point the check at another checkout.
    let derived = crate::core::task_worker_env::WorkerLane::for_worktree(worktree)
        .map(|derived| derived.name);
    if derived.as_deref() != Ok(lane) {
        return Err(format!(
            "the marker's worktree does not derive lane {lane} — left alone"
        ));
    }
    assert_managed_task_worktree_path(repo_path, worktree)?;
    match std::fs::symlink_metadata(worktree) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "cannot tell whether worktree {} exists ({error})",
            worktree.display()
        )),
        Ok(_) => {
            // A directory that is not a git worktree would answer for the
            // enclosing repository: it is not evidence of anything.
            if !worktree.join(".git").exists() {
                return Err(format!(
                    "{} is present and is not a git worktree",
                    worktree.display()
                ));
            }
            if judged_dirty_files(&Snapshot::of_worktree(worktree)?)?.is_empty() {
                Ok(true)
            } else {
                Err(format!(
                    "worktree {} is kept with uncommitted changes, so is its lane target",
                    worktree.display()
                ))
            }
        }
    }
}

/// Judge one lane target and, when `scope` allows it, delete it.
pub fn process_lane_target(
    repo_path: &Path,
    lane_target: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    scope: LaneSweepScope,
) -> LaneTargetOutcome {
    process_lane_target_idle(
        repo_path,
        lane_target,
        judge,
        scope,
        std::time::Duration::ZERO,
    )
}

/// Most recent write inside a lane target, looked up without walking it: the
/// directory itself, its entries and their direct children (`debug/`,
/// `debug/.fingerprint`, `.cargo-lock`, the marker...). Cargo touches those at
/// every build, so a build that ran a minute ago moves this forward.
///
/// `None` when nothing could be read: callers must then treat the lane as
/// written just now, never as idle (KT-1318). Rooted at the lane's parent folder
/// (KT-2084): the lane, or a folder of it, swapped for a link is never followed, and an
/// unreadable one is `None`.
pub fn lane_target_last_write(lane_target: &Path) -> Option<std::time::SystemTime> {
    use crate::core::fs_guard::{guarded_walk, Visit, WalkBounds};
    const ENTRY_BUDGET: usize = 4_000;
    const TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
    let parent = lane_target.parent()?;
    let mut newest: Option<std::time::SystemTime> = None;
    // Past the budget the newest write seen so far stands, as before.
    guarded_walk(
        parent,
        lane_target,
        WalkBounds::within(ENTRY_BUDGET, TIME_BUDGET),
        &mut |entry| {
            newest = Some(newest.map_or(entry.modified, |known| known.max(entry.modified)));
            if entry.depth < 2 {
                Visit::Enter
            } else {
                Visit::Skip
            }
        },
    )
    .ok()?;
    newest
}

/// How long a lane target has gone without a write, or `None` when unknown.
pub fn lane_target_idle_for(lane_target: &Path) -> Option<std::time::Duration> {
    let written = lane_target_last_write(lane_target)?;
    // A clock set back reads as "written just now", never as idle.
    Some(
        std::time::SystemTime::now()
            .duration_since(written)
            .unwrap_or_default(),
    )
}

/// [`process_lane_target`] that also keeps a lane written less than `min_idle`
/// ago (KT-1318): "no live worktree" is durable state, "not written for N
/// hours" is the second, independent proof that nothing is still using it.
pub fn process_lane_target_idle(
    repo_path: &Path,
    lane_target: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    scope: LaneSweepScope,
    min_idle: std::time::Duration,
) -> LaneTargetOutcome {
    process_lane_target_attributed(
        repo_path,
        lane_target,
        judge,
        scope,
        min_idle,
        &LaneAttributions::new(),
    )
}

/// [`process_lane_target_idle`] that also takes an unmarked lane Kronn can
/// attribute from durable lineage (KT-1397).
pub fn process_lane_target_attributed(
    repo_path: &Path,
    lane_target: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    scope: LaneSweepScope,
    min_idle: std::time::Duration,
    attributions: &LaneAttributions,
) -> LaneTargetOutcome {
    match classify_lane_target_attributed(repo_path, lane_target, judge, attributions) {
        LaneTargetState::Unmarked => LaneTargetOutcome::Unmarked,
        LaneTargetState::Claimed(owner, reason) => LaneTargetOutcome::Claimed(owner, reason),
        LaneTargetState::Kept(owner, reason) => LaneTargetOutcome::Kept(owner, reason),
        LaneTargetState::Reclaimable {
            owner,
            worktree_present,
        } => {
            let allowed = match scope {
                LaneSweepScope::ReportOnly => false,
                LaneSweepScope::WorktreeGone => !worktree_present,
                LaneSweepScope::WorktreeGoneOrClean => true,
            };
            if allowed && !min_idle.is_zero() {
                match lane_target_idle_for(lane_target) {
                    Some(idle) if idle >= min_idle => {}
                    Some(idle) => {
                        return LaneTargetOutcome::Kept(
                            Some(owner),
                            format!(
                                "written {} minute(s) ago, under the {}-hour idle threshold",
                                idle.as_secs() / 60,
                                min_idle.as_secs() / 3600
                            ),
                        )
                    }
                    None => {
                        return LaneTargetOutcome::Kept(
                            Some(owner),
                            "its last write cannot be read, so it is not provably idle".to_string(),
                        )
                    }
                }
            }
            if !allowed {
                let measured = measure_real_within_budget(lane_target);
                return LaneTargetOutcome::WouldRemove(
                    owner,
                    CleanupReport {
                        target_path: lane_target.to_path_buf(),
                        bytes_reclaimed: measured.bytes,
                        bytes_are_partial: measured.partial,
                        unique_bytes: measured.unique_bytes,
                    },
                );
            }
            match remove_lane_target(repo_path, lane_target) {
                Ok(report) => LaneTargetOutcome::Removed(owner, report),
                Err(reason) => LaneTargetOutcome::Kept(Some(owner), reason),
            }
        }
    }
}

/// One pass over `<repo>/target/wt-*`.
///
/// Deletes only a lane Kronn marked, whose execution `judge` calls finished,
/// whose worktree is gone (or, with the wider scope, clean). Every other
/// directory is reported and left exactly as it was.
pub fn sweep_lane_targets(
    repo_path: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    scope: LaneSweepScope,
) -> LaneTargetSweep {
    sweep_lane_targets_idle(repo_path, judge, scope, std::time::Duration::ZERO)
}

/// [`sweep_lane_targets`] that leaves every lane written within `min_idle`.
pub fn sweep_lane_targets_idle(
    repo_path: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    scope: LaneSweepScope,
    min_idle: std::time::Duration,
) -> LaneTargetSweep {
    sweep_lane_targets_attributed(repo_path, judge, scope, min_idle, &LaneAttributions::new())
}

/// [`sweep_lane_targets_idle`] that also reclaims the unmarked lanes of
/// `attributions` once their worktree is gone (KT-1397).
pub fn sweep_lane_targets_attributed(
    repo_path: &Path,
    judge: &dyn Fn(&LaneOwner) -> Result<(), String>,
    scope: LaneSweepScope,
    min_idle: std::time::Duration,
    attributions: &LaneAttributions,
) -> LaneTargetSweep {
    let mut sweep = LaneTargetSweep::default();
    for path in lane_target_dirs(repo_path) {
        match process_lane_target_attributed(repo_path, &path, judge, scope, min_idle, attributions)
        {
            LaneTargetOutcome::Removed(owner, report) => sweep.removed.push((owner, report)),
            LaneTargetOutcome::WouldRemove(owner, report) => {
                sweep.would_remove.push((owner, report))
            }
            LaneTargetOutcome::Claimed(_, reason) => sweep.claimed.push((path, reason)),
            LaneTargetOutcome::Kept(_, reason) => sweep.kept.push((path, reason)),
            LaneTargetOutcome::Unmarked => sweep.unmarked.push(path),
        }
    }
    sweep
}

/// Refuse when Cargo is mid-build in this target.
///
/// Cargo takes an exclusive lock on these files for the duration of a build, so
/// trying to take it ourselves answers "is a build running" without scanning
/// processes at all — which the incident showed to be unreliable in the other
/// direction. This can only ever refuse: failing to open a lock file is not
/// evidence of activity, so it lets the cleanup proceed to its real guards.
fn assert_no_build_in_progress(target_path: &Path) -> Result<(), String> {
    use fs2::FileExt;
    for lock_name in [".cargo-lock", "debug/.cargo-lock", "release/.cargo-lock"] {
        let lock_path = target_path.join(lock_name);
        let Some(file) = open_build_lock(target_path, &lock_path)? else {
            continue;
        };
        match file.try_lock_exclusive() {
            Ok(()) => {
                // Held for an instant only; release it before Cargo needs it.
                let _ = fs2::FileExt::unlock(&file);
            }
            Err(_) => {
                return Err(format!(
                    "refusing to clean {}: Cargo holds a build lock at {}, so a build is \
                     running right now",
                    target_path.display(),
                    lock_path.display()
                ))
            }
        }
    }
    Ok(())
}

/// The Cargo lock `lock_path` of `target_path`, opened from the target's parent folder without
/// following a link and without waiting on a FIFO (KT-2084): `None` when there is none. A lock
/// name, or a folder on its way, that is not what Cargo makes refuses the clean.
#[cfg(unix)]
fn open_build_lock(target_path: &Path, lock_path: &Path) -> Result<Option<std::fs::File>, String> {
    let refuse = |why: String| {
        format!(
            "refusing to clean {}: its build lock {} {why}",
            target_path.display(),
            lock_path.display()
        )
    };
    let (Some(base), Some(target_name), Ok(below)) = (
        target_path.parent(),
        target_path.file_name(),
        lock_path.strip_prefix(target_path),
    ) else {
        return Err(refuse("is not below a named target folder".to_string()));
    };
    let lock_name = below
        .file_name()
        .ok_or_else(|| refuse("has no name".to_string()))?;
    let folder = Path::new(target_name).join(below.parent().unwrap_or(Path::new("")));
    let dir = match crate::core::rooted_fd::Dir::open_path(base, &folder, false) {
        Ok(Some(dir)) => dir,
        Ok(None) => return Ok(None),
        Err(error) => {
            return Err(refuse(format!(
                "cannot be reached as a plain folder ({error})"
            )))
        }
    };
    dir.open_regular(lock_name)
        .map_err(|error| refuse(format!("is not a regular file ({error})")))
}

/// The Cargo lock `lock_path`, opened by path: Windows has no rooted primitive here (the
/// residual risk of `core::fs_guard`). `None` when it cannot be opened.
#[cfg(not(unix))]
fn open_build_lock(_: &Path, lock_path: &Path) -> Result<Option<std::fs::File>, String> {
    Ok(std::fs::File::open(lock_path).ok())
}

/// The thresholds the running server was configured with.
///
/// Held here rather than threaded through `create_task_worktree`'s signature:
/// provisioning is reached from eight call sites, none of which carries the
/// config, and plumbing it through all of them would touch far more code than
/// the guard itself. Two atomics, written once at boot from the config and read
/// on the provisioning path, keep the user's setting authoritative without that
/// churn — and `disk_headroom` stays a pure function so the decision is tested
/// on its own.
static DISK_WARNING_GIB: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(crate::models::setup::DEFAULT_DISK_WARNING_GIB);
static DISK_CRITICAL_GIB: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(crate::models::setup::DEFAULT_DISK_CRITICAL_GIB);

/// Publish the configured thresholds. Called at boot, and again whenever the
/// config is saved, so a user who lowers the bar is not made to restart.
pub fn set_disk_thresholds(warning_gib: u64, critical_gib: u64) {
    DISK_WARNING_GIB.store(warning_gib, std::sync::atomic::Ordering::Relaxed);
    DISK_CRITICAL_GIB.store(critical_gib, std::sync::atomic::Ordering::Relaxed);
}

pub fn configured_disk_thresholds() -> (u64, u64) {
    (
        DISK_WARNING_GIB.load(std::sync::atomic::Ordering::Relaxed),
        DISK_CRITICAL_GIB.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// Gate a provisioning attempt on free space, logging a warning below the soft
/// threshold and refusing below the hard one.
///
/// The refusal names the number and the setting, because the person reading it
/// is by definition on a machine that is about to stop working and needs to know
/// both what is wrong and which knob changes it.
fn ensure_disk_headroom_for(
    path: &Path,
    warning_gib: u64,
    critical_gib: u64,
    operation: &str,
) -> Result<(), String> {
    match disk_headroom(path, warning_gib, critical_gib) {
        DiskHeadroom::Ok => Ok(()),
        DiskHeadroom::Low {
            available_gib,
            warning_gib,
        } => {
            tracing::warn!(
                "Low disk: {available_gib} GiB free at {} (warning below {warning_gib} GiB). \
                 Worktree build artefacts are the usual cause; {operation} continues.",
                path.display()
            );
            Ok(())
        }
        DiskHeadroom::Critical {
            available_gib,
            critical_gib,
        } => Err(format!(
            "refusing to {operation}: only {available_gib} GiB free at {} \
             (critical below {critical_gib} GiB, server.disk_critical_gib). Free space — \
             worktree `target/` directories are the usual cause — or lower the threshold.",
            path.display()
        )),
    }
}

fn ensure_disk_headroom(path: &Path, warning_gib: u64, critical_gib: u64) -> Result<(), String> {
    ensure_disk_headroom_for(path, warning_gib, critical_gib, "provision a worktree")
}

/// Gate a build or validation before it can create more compiler artefacts.
///
/// This uses the same configured thresholds as worktree provisioning. It only
/// observes free space and never attempts cleanup: the interactive target and
/// any unrecognised cache remain outside automatic deletion ownership.
pub fn ensure_build_disk_headroom(path: &Path) -> Result<(), String> {
    // Cargo creates a target on its first build. Inspect its existing real
    // parent in that case instead of creating output during a guard; Cargo
    // remains the sole writer of the exact target path.
    let measured_path = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "refusing to run a build or validation: build target {} is a symlink",
                path.display()
            ));
        }
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| {
                format!(
                    "refusing to run a build or validation: build target {} has no parent directory",
                    path.display()
                )
            })?;
            let parent_metadata = std::fs::symlink_metadata(parent).map_err(|error| {
                format!(
                    "refusing to run a build or validation: build target parent {} cannot be inspected: {error}",
                    parent.display()
                )
            })?;
            if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
                return Err(format!(
                    "refusing to run a build or validation: build target parent {} is not a real directory",
                    parent.display()
                ));
            }
            parent.to_path_buf()
        }
        Err(error) => {
            return Err(format!(
                "refusing to run a build or validation: build target {} cannot be inspected: {error}",
                path.display()
            ));
        }
    };
    let target = measured_path.canonicalize().map_err(|error| {
        format!(
            "refusing to run a build or validation: build target {} cannot be resolved: {error}",
            measured_path.display()
        )
    })?;
    if !target.is_dir() {
        return Err(format!(
            "refusing to run a build or validation: build target {} is not a directory",
            target.display()
        ));
    }
    let (warning_gib, critical_gib) = configured_disk_thresholds();
    ensure_disk_headroom_for(
        &target,
        warning_gib,
        critical_gib,
        "run a build or validation",
    )
}

/// Fix worktree cross-references so they work from the host, not just inside Docker.
///
/// Git worktrees use absolute paths in two places:
/// 1. `<worktree>/.git` file → points to `<repo>/.git/worktrees/<name>`
/// 2. `<repo>/.git/worktrees/<name>/gitdir` → points back to `<worktree>/.git`
///
/// When created inside Docker, these contain container paths (`/host-home/...`).
/// This rewrites them to RELATIVE paths so the same checkout resolves from both the
/// container and the host — the only form portable across the two filesystem views.
///
/// The forward reference must climb one `../` per level the worktree is nested under
/// the repo root. The canonical layout is `<repo>/.kronn/worktrees/<name>` (3 levels),
/// so the gitdir is `../../../.git/worktrees/<name>`. Writing two levels
/// (`../../.git/...`) resolves to `<repo>/.kronn/.git` and breaks EVERY git command run
/// inside the worktree. The depth is derived from the actual path so a layout change
/// cannot silently reintroduce the off-by-one (KT-331).
fn fix_worktree_paths(repo_path: &Path, worktree_path: &Path) {
    let wt_name = match worktree_path.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return,
    };

    // Relative path from the repo root down to the worktree (POSIX slashes), e.g.
    // ".kronn/worktrees/<name>". Falls back to the canonical layout when the worktree
    // is not a textual child of the repo path.
    let rel_from_repo = worktree_path
        .strip_prefix(repo_path)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| format!(".kronn/worktrees/{}", wt_name));
    // One `../` per path component between the repo root and the worktree dir.
    let depth = rel_from_repo.split('/').filter(|c| !c.is_empty()).count();
    let up = "../".repeat(depth);

    // 1. Fix <worktree>/.git — point to <up>.git/worktrees/<name>
    //    Use forward slashes (POSIX) because git always uses forward slashes in gitdir files,
    //    even on Windows (git normalizes internally).
    //    Rooted at the worktree: a link at `.git` is refused (and one planted after the check
    //    is replaced as an entry), never written through (KT-1595).
    let dot_git = worktree_path.join(".git");
    if dot_git.exists() {
        let content = format!("gitdir: {}.git/worktrees/{}", up, wt_name);
        if let Err(e) =
            crate::core::fs_guard::guarded_replace(worktree_path, &dot_git, content.as_bytes())
        {
            tracing::warn!("Failed to fix worktree .git file: {}", e);
        }
    }

    // 2. Fix <repo>/.git/worktrees/<name>/gitdir — point back to the worktree.
    //    Git resolves this file RELATIVE TO ITS OWN DIRECTORY (`<repo>/.git/worktrees/
    //    <name>/`), always 3 levels under the repo root — so climb 3, then descend the
    //    repo-root-relative path to the worktree's .git. A bare repo-root-relative path
    //    (no `../../../`) resolves under `.git/worktrees/<name>/` and git marks the
    //    worktree `prunable`, so `git worktree prune` can drop the admin entry (KT-331).
    //    Rooted at the repo's `.git` (git's own admin folder: trusted, and possibly reached
    //    through a link by the user's choice); `worktrees/<name>` below it is walked without
    //    following a link (KT-1595).
    let git_admin = repo_path.join(".git");
    let gitdir_file = git_admin.join("worktrees").join(&wt_name).join("gitdir");
    if gitdir_file.exists() {
        let content = format!("../../../{}/.git\n", rel_from_repo);
        if let Err(e) =
            crate::core::fs_guard::guarded_replace(&git_admin, &gitdir_file, content.as_bytes())
        {
            tracing::warn!("Failed to fix repo gitdir for worktree: {}", e);
        }
    }
}

/// Information about a created worktree.
#[derive(Debug)]
pub struct WorktreeInfo {
    /// Full path to the worktree directory
    pub path: String,
    /// Branch name (e.g., "kronn/fix-the-bug")
    pub branch: String,
    /// If true, workspace points to the main repo (branch already checked out there)
    pub is_main_repo: bool,
}

/// Check if a branch is checked out in any worktree (including the main repo).
/// Path equality that survives symlinks. git prints CANONICAL paths
/// (`/private/var/…` on macOS) while Kronn holds the user's spelling
/// (`/var/…`, `~/Sites` symlinks…). A naive `==` made the "branch checked
/// out in the main repo" guard misfire on macOS: instead of blocking, it
/// took the reuse path and handed the MAIN CHECKOUT back as a "worktree"
/// (is_main_repo: false) — the agent then edited the user's live tree
/// believing it was isolated.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

/// The registered worktree `branch` is checked out in, read from the registrations' files
/// (KT-2084). `Err` when one cannot be read: the branch is then not known to be free.
fn branch_checked_out_at(repo_path: &Path, branch: &str) -> Result<Option<WorktreeEntry>, String> {
    Ok(worktree_entries(repo_path)?
        .into_iter()
        .find(|entry| entry.branch.as_deref() == Some(branch)))
}

/// Whether `entry` is the user's own checkout `repo_path`: the main worktree, or the entry
/// naming `repo_path` itself. A path is resolved inside the checkout only, and never through
/// a link that leaves it (KT-2084).
fn is_main_checkout(entry: &WorktreeEntry, repo_path: &Path) -> bool {
    entry.admin.is_none()
        || (entry.inside_checkout
            && crate::core::lane_git::resolve_in_checkout(repo_path, &entry.path)
                .is_some_and(|(real, _)| real == repo_path))
}

/// Base directory for worktrees: `.kronn/worktrees/` inside the repo.
fn worktree_base_dir(repo_path: &Path) -> PathBuf {
    repo_path.join(".kronn/worktrees")
}

/// Ignore Kronn's machine-owned checkout directory without editing the user's
/// tracked `.gitignore`. Provisioning must not make the target dirty itself — a
/// dirty target is (correctly) refused later by the integration preflight.
///
/// The exclude file is the one git NAMES (`rev-parse --git-path`), which
/// follows `.git/commondir` and a `.git` file's `gitdir:` — files an agent can
/// write. Use this entry point only where no agent has touched the checkout yet
/// (worktree provisioning, the worker environment). Code that runs after an
/// agent uses [`ensure_local_git_exclude_pinned`].
pub fn ensure_local_git_exclude(repo_path: &Path, pattern: &str) -> Result<(), String> {
    // Already ignored (a committed rule, or a previous run): write nothing.
    // `repo_path` may be a worker's worktree (lane seeds, the lane's Cargo target):
    // pinned to its registration there, so neither the `.git` pointer nor a nested
    // repository the worker wrote decides what git reads.
    let lane = crate::core::lane_git::LaneGit::resolve(repo_path)?;
    if lane
        .command()
        .args([
            "check-ignore",
            "-q",
            "--no-index",
            pattern.trim_start_matches('/'),
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        return Ok(());
    }
    let output = lane
        .command()
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
        .map_err(|error| format!("cannot resolve git exclude path: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot resolve git exclude path: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Err("git returned an empty local exclude path".into());
    }
    let exclude = {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            repo_path.join(path)
        }
    };
    for path in [Some(exclude.as_path()), exclude.parent()]
        .into_iter()
        .flatten()
    {
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(format!(
                "{} is a symlink — refusing to update the local Git exclude",
                path.display()
            ));
        }
    }
    if let Some(pinned) = lane.pinned() {
        // A managed worktree's exclude stays inside the registered git directory.
        let inside = exclude
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .is_none_or(|real| real.starts_with(&pinned.common_dir));
        if !inside {
            return Err(format!(
                "{} leaves the repository's git directory — refusing to update the local Git exclude",
                exclude.display()
            ));
        }
    }
    // The git directory git itself named (`<git dir>/info/exclude`) is the
    // root: a link at `exclude` or `info`, or swapped in for either, is refused.
    let git_dir = exclude
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("{} is not inside a git directory", exclude.display()))?;
    write_local_exclude(git_dir, &exclude, pattern)
}

/// Whether git already ignores `pattern` (a committed rule, or a previous run):
/// then nothing is written. In a managed worktree git is pinned to its
/// registration (KT-1745).
fn already_ignored(repo_path: &Path, pattern: &str) -> bool {
    let mut command = crate::core::lane_git::git_or_refuse(repo_path);
    command.args([
        "check-ignore",
        "-q",
        "--no-index",
        "--",
        pattern.trim_start_matches('/'),
    ]);
    // Within a time bound (KT-2084): a FIFO `.gitignore` never holds the caller.
    bounded(command, "check-ignore").is_ok_and(|output| output.status.success())
}

/// [`ensure_local_git_exclude`] for code that runs AFTER an agent (every audit
/// state write, the removal preflight): the agent can write `.git/commondir`
/// and a `.git` file's `gitdir:`, so git's answer is not trusted. The exclude
/// is `<repo>/.git/info/exclude` only, reached through the rooted primitive
/// from the repo (a link at `.git`, `info` or `exclude`, planted or swapped in,
/// is refused), and only when `.git` is a plain directory that holds no
/// `commondir`. A linked worktree (a `.git` FILE) or a repo with a `commondir`
/// is refused with an error, never followed: the exclude of a linked worktree
/// is the main repository's, written when the worktree was provisioned.
pub fn ensure_local_git_exclude_pinned(repo_path: &Path, pattern: &str) -> Result<(), String> {
    if already_ignored(repo_path, pattern) {
        return Ok(());
    }
    let git_dir = repo_path.join(".git");
    match std::fs::symlink_metadata(&git_dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "{} is not a plain git directory (a link or a `gitdir:` file): its pointer is not trusted after an agent ran",
                git_dir.display()
            ))
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", git_dir.display())),
    }
    let commondir = git_dir.join("commondir");
    match crate::core::fs_guard::guarded_read(repo_path, &commondir) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Err(format!(
                "{} exists: a main git directory has no `commondir`, its pointer is not trusted",
                commondir.display()
            ))
        }
        Err(error) => {
            return Err(format!(
                "refusing to inspect {}: {error}",
                commondir.display()
            ))
        }
    }
    write_local_exclude(repo_path, &git_dir.join("info").join("exclude"), pattern)
}

/// [`ensure_local_git_exclude_pinned`] for a rule that concerns a LINKED
/// worktree (a task lane): whether git already ignores `pattern` is judged
/// from `judge` (the lane, where the file or link the rule hides exists), but
/// the rule is written to the REGISTERED checkout `repo_path` — the
/// `info/exclude` a linked worktree shares is the main repository's, so that
/// is the file git would have named, reached without asking git (the lane's
/// own `.git` pointer is agent-writable). `judge` only decides whether to
/// write at all: a wrong answer from it skips a rule, it never moves one.
///
/// The git directory is resolved from the REGISTERED checkout `repo_path` (the
/// project Kronn registered, never the lane): its common git directory, which is
/// also where a registered checkout that is itself a linked worktree, or a
/// project below the repository top, keeps `info/exclude`. The write is rooted
/// at that directory, so a link at `info` or `exclude` is refused. A project
/// below the repository top gets the rule anchored under its prefix; a path git
/// does not know as a repository is left alone.
pub fn ensure_local_git_exclude_pinned_for(
    judge: &Path,
    repo_path: &Path,
    pattern: &str,
) -> Result<(), String> {
    if already_ignored(judge, pattern) {
        return Ok(());
    }
    let Some((common_dir, prefix)) = registered_git_layout(repo_path)? else {
        return Ok(());
    };
    write_local_exclude(
        &common_dir,
        &common_dir.join("info").join("exclude"),
        &anchored_under_prefix(&prefix, pattern),
    )
}

/// `(common git directory, prefix of repo_path below the repository top)` as
/// the REGISTERED checkout `repo_path` answers, or `None` when it is not inside
/// a git work tree. The directory must be a real folder, not a link.
fn registered_git_layout(repo_path: &Path) -> Result<Option<(PathBuf, String)>, String> {
    // lane-git-allow: asked of the registered checkout, never a lane.
    let output = crate::core::cmd::git_cmd()
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--show-prefix",
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("cannot resolve the git directory: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(format!(
            "cannot resolve the git directory of {}: {}",
            repo_path.display(),
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let common = lines.next().map(str::trim).unwrap_or_default();
    let prefix = lines.next().map(str::trim).unwrap_or_default().to_string();
    if common.is_empty() {
        return Err("git returned an empty common git directory".into());
    }
    let common = PathBuf::from(common);
    if !common.is_absolute() {
        return Err(format!(
            "git returned a relative common git directory {}",
            common.display()
        ));
    }
    match std::fs::symlink_metadata(&common) {
        Ok(meta) if meta.file_type().is_dir() => Ok(Some((common, prefix))),
        Ok(_) => Err(format!(
            "{} is not a plain git directory (a link): nothing is written under it",
            common.display()
        )),
        Err(error) => Err(format!("cannot inspect {}: {error}", common.display())),
    }
}

/// A rule anchored with a leading `/` is relative to the repository top, so a
/// project below it needs its prefix in the rule.
fn anchored_under_prefix(prefix: &str, pattern: &str) -> String {
    match pattern.strip_prefix('/') {
        Some(rest) if !prefix.is_empty() => format!("/{prefix}{rest}"),
        _ => pattern.to_string(),
    }
}

/// The git administrative directory (`<main>/.git/worktrees/<id>`) the REGISTERED
/// checkout `main_checkout` recorded for the linked worktree `worktree`, for
/// code that writes into it after an agent ran (KT-1595).
///
/// The directory is found from the main repository's own record — the `gitdir`
/// file of each `worktrees/<id>` entry names the worktree's `.git` file — and
/// NOT from the worktree's `.git` pointer or from `git rev-parse`, both of
/// which an agent can rewrite (`gitdir: /other/repo/.git`). No git runs
/// (KT-2084): the pointer is read as a file and must name that directory as
/// written, or it is refused: a worktree whose pointer was moved is never
/// written for. Every read goes through the rooted primitive from
/// `main_checkout`, so a link at `.git`, `worktrees` or an entry is refused,
/// and a recorded path outside `main_checkout` is never looked at. The
/// returned path is under `main_checkout`: pass `main_checkout` as the root of
/// the `guarded_*` calls that use it.
pub fn registered_worktree_git_dir(
    main_checkout: &Path,
    worktree: &Path,
) -> Result<PathBuf, String> {
    use crate::core::fs_guard::guarded_list;
    let wanted = crate::core::lane_git::without_verbatim_prefix(
        worktree
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", worktree.display()))?,
    );
    let owner = crate::core::lane_git::without_verbatim_prefix(
        main_checkout
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", main_checkout.display()))?,
    );
    let registry = main_checkout.join(".git").join("worktrees");
    let entries = guarded_list(main_checkout, &registry)
        .map_err(|error| format!("cannot read the worktree registry of the checkout: {error}"))?
        .ok_or_else(|| "the checkout registers no linked worktree".to_string())?;
    let mut found: Vec<PathBuf> = Vec::new();
    for name in entries {
        let entry = registry.join(&name);
        // Under the registration cap and as written, one final newline apart (KT-2084): a
        // record another lane padded or grew is not this lane's.
        let Some(recorded) = crate::core::lane_git::registration_gitdir(main_checkout, &entry)
        else {
            continue;
        };
        let recorded = PathBuf::from(recorded);
        // `worktree.useRelativePaths` records it relative to the entry itself, taken from the
        // checkout's own spelling.
        let recorded = if recorded.is_relative() {
            owner
                .join(".git")
                .join("worktrees")
                .join(&name)
                .join(recorded)
        } else {
            recorded
        };
        // Resolved inside the checkout only (KT-2084).
        let is_ours = recorded.file_name().is_some_and(|n| n == ".git")
            && recorded
                .parent()
                .and_then(|parent| crate::core::lane_git::resolve_inside(&owner, parent))
                .is_some_and(|(parent, _)| parent == wanted);
        if is_ours {
            found.push(entry);
        }
    }
    let [registered] = found.as_slice() else {
        return Err(if found.is_empty() {
            "the checkout has no registered worktree entry for this lane".to_string()
        } else {
            "the checkout registers this lane more than once".to_string()
        });
    };
    let admin = crate::core::lane_git::without_verbatim_prefix(
        registered
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", registered.display()))?,
    );
    if let Some(why) = crate::core::lane_git::pointer_problem(&wanted, &admin) {
        return Err(format!(
            "the `.git` of this worktree {why} but the checkout registered {}: its `.git` \
             pointer was moved, so nothing is written for it",
            registered.display()
        ));
    }
    Ok(registered.clone())
}

/// Appends `pattern` to the exclude file `exclude` (created when missing)
/// through the rooted primitive under `root` (KT-1595): nothing is read,
/// created or appended outside `root`.
fn write_local_exclude(root: &Path, exclude: &Path, pattern: &str) -> Result<(), String> {
    let existing = match crate::core::fs_guard::guarded_read(root, exclude) {
        Ok(existing) => existing,
        Err(error) => {
            return Err(format!(
                "refusing to update the local Git exclude {}: {error}",
                exclude.display()
            ))
        }
    };
    let Some(existing) = existing else {
        // No exclude yet: create-only, so a file that appears meanwhile is
        // never overwritten.
        let created = crate::core::fs_guard::guarded_write_new(
            root,
            exclude,
            format!("{pattern}\n").as_bytes(),
        )
        .map_err(|error| format!("cannot create {}: {error}", exclude.display()))?;
        return if created {
            Ok(())
        } else {
            Err(format!(
                "{} appeared while it was being created; left as it is",
                exclude.display()
            ))
        };
    };
    let text = String::from_utf8(existing.clone())
        .map_err(|_| format!("{} is not UTF-8; left as it is", exclude.display()))?;
    if text.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    let mut updated = text;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(pattern);
    updated.push('\n');
    // Optimistic on the bytes read: a concurrent change is refused, not lost.
    crate::core::fs_guard::guarded_replace_if_unchanged_keeping_mode(
        root, exclude, &updated, &existing,
    )
    .map_err(|error| format!("cannot update {}: {error}", exclude.display()))
}

/// Files an operating system or a file manager drops in any folder it opens:
/// never work, never worth blocking a delivery or a cleanup (KT-1750). The
/// default of Settings > Workers > OS junk (`workers.os_junk_patterns`).
pub const OS_JUNK_PATTERNS: [&str; 4] = [".DS_Store", "Thumbs.db", "desktop.ini", "._*"];

/// The Settings > Workers list, published by `config::save` and at boot. `None`
/// until the config says otherwise, which reads as the defaults.
static OS_JUNK_SETTING: std::sync::RwLock<Option<Vec<String>>> = std::sync::RwLock::new(None);

/// Publish the owner's Settings > Workers choice to this process.
pub fn set_os_junk_patterns(patterns: &[String]) {
    let mut setting = OS_JUNK_SETTING
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *setting = Some(patterns.to_vec());
}

/// The OS-junk patterns in force: the setting's, else the defaults.
pub fn os_junk_patterns() -> Vec<String> {
    OS_JUNK_SETTING
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_else(default_os_junk_patterns)
}

/// [`OS_JUNK_PATTERNS`] as the owned list the setting holds.
pub fn default_os_junk_patterns() -> Vec<String> {
    OS_JUNK_PATTERNS.iter().map(|p| p.to_string()).collect()
}

/// Whether the last component of `path` is one of `patterns` (`*` stands for any
/// run of characters).
fn is_os_junk(path: &str, patterns: &[String]) -> bool {
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    patterns.iter().any(|pattern| glob_matches(pattern, name))
}

fn glob_matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let head = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(head) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(tail) = parts.pop() else {
        return rest.is_empty();
    };
    for middle in parts {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    rest.len() >= tail.len() && rest.ends_with(tail)
}

/// Keep OS junk out of `git status` for the repository and every worktree of it
/// (the local exclude is shared), without touching a tracked `.gitignore`.
pub fn ensure_os_junk_excluded(repo_path: &Path) -> Result<(), String> {
    for pattern in os_junk_patterns() {
        ensure_local_git_exclude(repo_path, &pattern)?;
    }
    Ok(())
}

/// What a build, an install or a tool makes again (KT-2084): names or globs matched
/// against the name of an ignored entry, so a discussion delete never counts it as work.
/// `server.regenerable_outputs` adds more, by name or by path. `.cc-writes` is an agent
/// CLI's write log; `vendor`, `Pods`, `.terraform`, `_build` and `.build` are installed
/// dependencies and build folders (Composer or Go, CocoaPods, Terraform, Mix, SwiftPM).
/// `bin`, `deps` and `obj` are not: generic names that may hold hand-written scripts or
/// hand-placed SDKs (C and C++ projects). A project names them in
/// `server.regenerable_outputs`.
pub const REGENERABLE_OUTPUTS: [&str; 34] = [
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    "coverage",
    ".gradle",
    ".tox",
    "*.tsbuildinfo",
    ".eslintcache",
    ".stylelintcache",
    "*.pyc",
    "test-results",
    "playwright-report",
    ".parcel-cache",
    ".svelte-kit",
    ".angular",
    ".dart_tool",
    ".cc-writes",
    "vendor",
    "Pods",
    ".terraform",
    "_build",
    ".build",
];

/// The entries `server.regenerable_outputs` adds, published by `config::save` and at boot.
static REGENERABLE_OUTPUT_EXTRA: std::sync::RwLock<Vec<String>> =
    std::sync::RwLock::new(Vec::new());

/// Publish `server.regenerable_outputs` to this process.
pub fn set_regenerable_outputs(extra: &[String]) {
    *REGENERABLE_OUTPUT_EXTRA
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = extra.to_vec();
}

/// [`REGENERABLE_OUTPUTS`] and the configured entries.
pub fn regenerable_outputs() -> Vec<String> {
    let extra = REGENERABLE_OUTPUT_EXTRA
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    regenerable_outputs_with(&extra)
}

/// [`REGENERABLE_OUTPUTS`] and `extra`, each trimmed of blanks and slashes.
pub fn regenerable_outputs_with(extra: &[String]) -> Vec<String> {
    let mut names: Vec<String> = REGENERABLE_OUTPUTS
        .iter()
        .map(|name| name.to_string())
        .collect();
    names.extend(
        extra
            .iter()
            .map(|name| name.trim().trim_matches('/').to_string())
            .filter(|name| !name.is_empty()),
    );
    names
}

/// Whether the ignored `path` is one of `regenerable`. An entry without `/` names the
/// entry itself, never a folder above it (a `.env` in a tracked `build/` is work); one
/// with `/` names a path from the worktree root (`backend/bindings`) and what is below it.
fn is_regenerable(path: &str, regenerable: &[String]) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let Some(name) = parts.last() else {
        return false;
    };
    regenerable.iter().any(|pattern| {
        if !pattern.contains('/') {
            return glob_matches(pattern, name);
        }
        let wanted: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
        !wanted.is_empty()
            && wanted.len() <= parts.len()
            && wanted
                .iter()
                .zip(&parts)
                .all(|(want, part)| glob_matches(want, part))
    })
}

/// `regenerable` for the submodule at `prefix`: names as they are, path entries below it
/// from the submodule's root.
fn regenerable_below(regenerable: &[String], prefix: &str) -> Vec<String> {
    regenerable
        .iter()
        .filter_map(|pattern| {
            if !pattern.contains('/') {
                return Some(pattern.clone());
            }
            pattern
                .strip_prefix(prefix)?
                .strip_prefix('/')
                .filter(|rest| !rest.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// What every Kronn checkout needs ignored: its own `.kronn/` directory (an
/// error when that fails) and OS junk (only logged: it never blocks a launch).
pub fn ensure_managed_excludes(repo_path: &Path) -> Result<(), String> {
    ensure_local_git_exclude(repo_path, ".kronn/")?;
    if let Err(error) = ensure_os_junk_excluded(repo_path) {
        tracing::warn!(path = %repo_path.display(), "cannot exclude OS junk files: {error}");
    }
    Ok(())
}

/// Maximum length of a single slug component (project / discussion).
///
/// Windows MAX_PATH is 260 characters by default. A worktree path looks like:
///   `<repo>\.kronn\worktrees\<project>--<discussion>\…\file`
/// With a typical repo path of ~80 chars and the `.kronn\worktrees\` prefix
/// (~17 chars), capping each slug at 60 chars leaves at least ~100 chars for
/// nested files inside the worktree before hitting the legacy limit.
const MAX_SLUG_LEN: usize = 60;

/// Slugify a string for use in paths and branch names.
///
/// Caps the result at `MAX_SLUG_LEN` so concatenations like
/// `<project>--<discussion>` stay safely below Windows MAX_PATH (260)
/// even before the long-path prefix kicks in.
fn slugify(s: &str) -> String {
    let raw: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    truncate_slug(&raw)
}

/// Truncate a slug to `MAX_SLUG_LEN` chars without splitting on a trailing dash.
fn truncate_slug(s: &str) -> String {
    if s.len() <= MAX_SLUG_LEN {
        return s.to_string();
    }
    // chars() to be unicode-safe (slugs can contain accented letters)
    let truncated: String = s.chars().take(MAX_SLUG_LEN).collect();
    truncated.trim_end_matches('-').to_string()
}

/// Apply the Windows extended-length path prefix `\\?\` so file APIs accept
/// paths longer than 260 chars. No-op on non-Windows. Idempotent.
#[allow(dead_code)]
pub fn long_path(p: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let s = p.to_string_lossy();
        if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
            return p.to_path_buf();
        }
        // Only meaningful for absolute drive paths (C:\...). UNC paths use
        // a different form: \\?\UNC\server\share\…
        if s.len() >= 3 && s.as_bytes()[1] == b':' {
            return PathBuf::from(format!(r"\\?\{}", s));
        }
        if let Some(rest) = s.strip_prefix(r"\\") {
            // \\server\share → \\?\UNC\server\share
            return PathBuf::from(format!(r"\\?\UNC\{}", rest));
        }
        p.to_path_buf()
    }
    #[cfg(not(target_os = "windows"))]
    {
        p.to_path_buf()
    }
}

/// Create a persistent worktree for a discussion.
///
/// - `repo_path`: the git repo path (resolved via resolve_host_path)
/// - `project_slug`: slugified project name
/// - `discussion_slug`: slugified discussion title or ID
/// - `base_branch`: branch to base the worktree on (e.g., "main")
pub fn create_discussion_worktree(
    repo_path: &Path,
    project_slug: &str,
    discussion_slug: &str,
    base_branch: &str,
) -> Result<WorktreeInfo, String> {
    let project_slug = slugify(project_slug);
    let discussion_slug = slugify(discussion_slug);
    let branch = format!("kronn/{}", discussion_slug);
    let dir_name = format!("{}--{}", project_slug, discussion_slug);
    let worktree_path = worktree_base_dir(repo_path).join(&dir_name);

    // If the branch is already checked out in the main repo, block — user must switch
    // branches before the agent can work. This avoids the agent modifying files under
    // a running dev environment.
    if let Some(existing) = branch_checked_out_at(repo_path, &branch)? {
        if is_main_checkout(&existing, repo_path) {
            return Err(format!(
                "Branch {} is currently checked out in the main repo. Please switch to another branch before continuing.",
                branch
            ));
        }
        // Outside the checkout: not Kronn's, never used as a discussion's worktree (KT-2084).
        if !existing.inside_checkout {
            return Err(format!(
                "Branch {branch} is checked out at {}, outside the project checkout: Kronn \
                 does not use that worktree",
                existing.path.display()
            ));
        }
        // Reused only when it is the checkout Kronn gives this discussion (KT-2084): that
        // folder of `.kronn/worktrees`, through no link, its `.git` naming this registration.
        // Any other folder of the checkout is the user's tree, or another lane's.
        let own = plain_path(&existing)
            .and_then(|path| owned_checkout(repo_path, &path, true).ok())
            .is_some_and(|(_, checkout)| {
                checkout
                    .file_name()
                    .is_some_and(|name| name == dir_name.as_str())
                    && std::fs::symlink_metadata(&checkout).is_ok_and(|meta| meta.is_dir())
                    && existing.admin.as_deref().is_some_and(|admin| {
                        crate::core::lane_git::pointer_problem(&checkout, admin).is_none()
                    })
            });
        if !own {
            return Err(format!(
                "Branch {branch} is checked out at {}, which is not the worktree Kronn gives this \
                 discussion ({}): Kronn does not use it. Switch that worktree to another branch, \
                 or remove it with `git worktree remove`, then retry.",
                existing.path.display(),
                worktree_path.display()
            ));
        }
        let existing_path = existing.path;
        // Already in a worktree (e.g. .kronn/worktrees/) — reuse it
        tracing::info!(
            "Branch {} already checked out at {}, reusing",
            branch,
            existing_path.display()
        );
        return Ok(WorktreeInfo {
            path: existing_path.to_string_lossy().to_string(),
            branch,
            is_main_repo: false,
        });
    }

    // Create base directory
    crate::core::fs_guard::guarded_create_dir_all(repo_path, &worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    // Ensure .kronn/worktrees/ is gitignored
    ensure_managed_excludes(repo_path)?;

    // Mark repo as safe directory (needed in Docker where mount owner differs)
    crate::core::git_global_config::ensure_safe_directory(repo_path);
    crate::core::git_global_config::ensure_safe_directory(&worktree_path);

    // Create the worktree with a new branch based on base_branch
    let output = crate::core::cmd::project_git_cmd()
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_path)
        .arg(base_branch)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    tracing::info!(
        "Created discussion worktree at {} (branch: {})",
        worktree_path.display(),
        branch
    );

    confirm_registration_id(repo_path, &worktree_path, Some(&branch))?;

    // Fix gitdir paths so the worktree works from the host too (not just inside Docker)
    fix_worktree_paths(repo_path, &worktree_path);

    copy_agent_configs(repo_path, &worktree_path);

    Ok(WorktreeInfo {
        path: worktree_path.to_string_lossy().to_string(),
        branch,
        is_main_repo: false,
    })
}

pub use crate::db::discussions::REVIEW_WORKSPACE_MODE;

/// What [`create_review_worktree`] hands back: the checkout and the commits the
/// review was pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewWorktree {
    pub path: String,
    pub head_sha: String,
    pub base_sha: Option<String>,
}

/// Create the detached worktree of a review discussion (KT-1645).
///
/// A reviewer's tools (and the subagents it starts) run in this checkout, so
/// nothing they do — `git checkout --detach`, `git reset`, a stray edit — can
/// move HEAD or touch the uncommitted files of the owner's main checkout. The
/// checkout is pinned to the exact commit `head` resolves to, with no branch:
/// there is nothing to switch back to and nothing to leave behind but the
/// directory, which the discussion's deletion removes.
///
/// `base` is only checked to exist (a review range is `base..head`; both ends
/// live in the repository the worktree shares). Any refusal — an unknown
/// revision, a dirty `git worktree add` — is an `Err`: the caller must fail the
/// creation, never run the review in the main checkout instead.
pub fn create_review_worktree(
    repo_path: &Path,
    project_slug: &str,
    discussion_id: &str,
    head: &str,
    base: Option<&str>,
) -> Result<ReviewWorktree, String> {
    let head_sha = resolve_commit(repo_path, head)?;
    let base_sha = base
        .map(|base| resolve_commit(repo_path, base))
        .transpose()?;

    let short_id: String = discussion_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let dir_name = format!("{}--review-{}", slugify(project_slug), short_id);
    let worktree_path = worktree_base_dir(repo_path).join(&dir_name);
    if worktree_path.exists() {
        return Err(format!(
            "review worktree {} already exists",
            worktree_path.display()
        ));
    }

    std::fs::create_dir_all(worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;
    ensure_managed_excludes(repo_path)?;

    crate::core::git_global_config::ensure_safe_directory(repo_path);
    crate::core::git_global_config::ensure_safe_directory(&worktree_path);

    let output = crate::core::cmd::project_git_cmd()
        .args(["worktree", "add", "--detach"])
        .arg(&worktree_path)
        .arg(&head_sha)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree add: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    confirm_registration_id(repo_path, &worktree_path, None)?;
    fix_worktree_paths(repo_path, &worktree_path);
    copy_agent_configs(repo_path, &worktree_path);
    tracing::info!(
        "Created review worktree at {} (detached at {})",
        worktree_path.display(),
        head_sha
    );

    Ok(ReviewWorktree {
        path: worktree_path.to_string_lossy().to_string(),
        head_sha,
        base_sha,
    })
}

/// What [`release_review_worktree`] did with a review's checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewRelease {
    /// The checkout (and its admin entry) is gone.
    Removed,
    /// Left in place, with the reason: it holds something a removal would lose.
    Kept(String),
}

/// Remove the worktree of a finished review, if it is disposable (KT-1645).
///
/// A review worktree is a throwaway view of a commit, so once the review is
/// over it must not pile up against the worktree cap. "Disposable" means a
/// removal loses nothing: HEAD is detached, every commit it holds is reachable
/// from a ref, and nothing is modified or untracked (the agent configs Kronn
/// copies in at creation do not count). Anything else is
/// [`ReviewRelease::Kept`] with the reason, never force-removed.
///
/// Only a directory [`create_review_worktree`] makes qualifies
/// (`<repo>/.kronn/worktrees/<slug>--review-<id>`); any other path is an `Err`.
pub fn release_review_worktree(
    repo_path: &Path,
    worktree_path: &str,
) -> Result<ReviewRelease, String> {
    let path = Path::new(worktree_path);
    let is_review_dir = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains("--review-"))
        && path.parent().is_some_and(|parent| {
            let base = worktree_base_dir(repo_path);
            parent == base
                || matches!(
                    (parent.canonicalize(), base.canonicalize()),
                    (Ok(parent), Ok(base)) if parent == base
                )
        });
    if !is_review_dir {
        return Err(format!(
            "{worktree_path} is not a review worktree of {}",
            repo_path.display()
        ));
    }

    // Already gone: only the admin entry may be left to prune.
    if path.exists() {
        if let Some(reason) = review_worktree_holds_something(path)? {
            return Ok(ReviewRelease::Kept(reason));
        }
    }

    remove_discussion_worktree(repo_path, worktree_path, None)?;
    Ok(ReviewRelease::Removed)
}

/// Why removing the review checkout at `path` would lose something, if it would.
fn review_worktree_holds_something(path: &Path) -> Result<Option<String>, String> {
    // Judged from Kronn's copy of its git state (KT-2084).
    let judged = Snapshot::of_worktree(path)?;
    let on_branch = judged
        .output(&["symbolic-ref", "-q", "HEAD"])?
        .status
        .success();
    if on_branch {
        return Ok(Some("its HEAD is on a branch, not detached".to_string()));
    }

    // `--glob=refs/*`, not `--all`: `--all` also counts this worktree's own
    // HEAD, which would make every commit it holds look reachable.
    let unreachable = judged.output(&["rev-list", "-n", "1", "HEAD", "--not", "--glob=refs/*"])?;
    if !unreachable.status.success() {
        return Ok(Some(format!(
            "its history cannot be read: {}",
            String::from_utf8_lossy(&unreachable.stderr).trim()
        )));
    }
    if !String::from_utf8_lossy(&unreachable.stdout)
        .trim()
        .is_empty()
    {
        return Ok(Some(
            "its HEAD holds a commit no branch or tag reaches".to_string(),
        ));
    }

    let status = judged.output(&[
        "status",
        "--ignore-submodules=dirty",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
    ])?;
    if !status.status.success() {
        return Ok(Some(format!(
            "its state cannot be read: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        )));
    }
    let copied: Vec<String> = AGENT_CONFIG_FILES
        .iter()
        .map(|parts| parts.join("/"))
        .collect();
    let dirty: Vec<DirtyFile> = parse_porcelain_z(&status.stdout)
        .into_iter()
        .filter(|file| !(file.status == "??" && copied.contains(&file.path)))
        .collect();
    Ok(dirty.first().map(|first| {
        format!(
            "it holds {} uncommitted change(s), e.g. {}",
            dirty.len(),
            first.path
        )
    }))
}

/// KT-1751 review — the worktrees git records for the REGISTERED checkout
/// `repo_path` (itself first), whatever directory they live in. Asked of the
/// registered checkout, never of a lane, so a lane's own `.git` pointer cannot
/// make it claim another project.
pub fn registered_worktree_paths(repo_path: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(worktree_entries(repo_path)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

type WorktreeEntry = crate::core::lane_git::RegisteredWorktree;

/// The path of `entry`, when Kronn may act on it as spelled: see [`plainly_spelled`], and
/// inside the owner's checkout (KT-2084).
fn plain_path(entry: &WorktreeEntry) -> Option<PathBuf> {
    (entry.readable && entry.inside_checkout && plainly_spelled(&entry.path))
        .then(|| entry.path.clone())
}

/// The worktrees registered for `repo_path`, read from the registrations' files (KT-2084): no
/// git runs, nothing is read through a link or from a FIFO, and a registration Kronn cannot
/// read that way is an error, never "nothing registered".
fn worktree_entries(repo_path: &Path) -> Result<Vec<WorktreeEntry>, String> {
    crate::core::lane_git::registered_worktrees(repo_path)
}

/// `<repo>/.kronn/worktrees`, canonical, with no link below the repository: the only folder
/// Kronn removes a checkout from.
fn managed_root(repo_path: &Path) -> Result<PathBuf, String> {
    let repo = repo_path
        .canonicalize()
        .map_err(|error| format!("{} cannot be resolved: {error}", repo_path.display()))?;
    let repo = crate::core::lane_git::without_verbatim_prefix(repo);
    let root = repo.join(".kronn").join("worktrees");
    crate::core::fs_guard::assert_contained_no_symlink(&repo, &root)?;
    assert_no_reparse_component(&repo, &root)?;
    Ok(root)
}

/// The checkout `path` of `repo_path`, as Kronn or git spells it, as Kronn removes it
/// (KT-2084): plainly spelled, below `<repo>/.kronn/worktrees` (`direct`: a directory of
/// its own there) and with no link from the repository down. Returns the canonical managed
/// root and the canonical checkout.
fn owned_checkout(
    repo_path: &Path,
    path: &Path,
    direct: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let refuse = |why: &str| {
        Err(format!(
            "{} is not a worktree Kronn removes: {why}",
            path.display()
        ))
    };
    if !plainly_spelled(path) {
        return refuse("its path has an empty, `.` or `..` part, or a control character");
    }
    let root = managed_root(repo_path)?;
    let Ok(relative) = path
        .strip_prefix(worktree_base_dir(repo_path))
        .or_else(|_| path.strip_prefix(&root))
    else {
        return refuse("it is not under .kronn/worktrees");
    };
    let depth = relative.components().count();
    if depth == 0 || (direct && depth != 1) {
        return refuse("it is not a checkout directory of .kronn/worktrees");
    }
    let checkout = root.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(&root, &checkout)?;
    assert_no_reparse_component(&root, &checkout)?;
    Ok((root, checkout))
}

/// Start of the reason a removal gives when it stopped with part of the worktree removed.
pub const PARTLY_REMOVED: &str = "partly removed";

/// Remove the one registration (`<common>/worktrees/<id>`) naming Kronn's checkout
/// `checkout` exactly, without following a link (KT-2084); when none names it, the one Kronn
/// gave it ([`registration_by_id`]). `Ok(false)`: neither. `checkout` comes from Kronn's own
/// records (a discussion's, a task's), never from a path a registration names.
fn remove_registration(repo_path: &Path, checkout: &Path) -> Result<bool, String> {
    let admin = match crate::core::lane_git::registration_of(repo_path, checkout)? {
        Some(admin) => Some(admin),
        None => registration_by_id(repo_path, checkout)?,
    };
    match admin {
        Some(admin) => remove_admin_dir(&admin).map(|()| true),
        None => Ok(false),
    }
}

/// When no registration names Kronn's checkout `checkout` (canonical, a folder of
/// `<repo>/.kronn/worktrees`), the one Kronn gave it at creation: its id is the checkout's
/// folder name, which [`confirm_registration_id`] checked when the checkout was made, and a
/// worker may have rewritten its `gitdir` to name another path (KT-2084). Removing the
/// checkout never leaves it behind. Kept only when it is another checkout's own: its `gitdir`
/// names another folder of the managed root whose `.git` names this registration. Only for a
/// `checkout` Kronn's own records give: the id is never taken from what a `gitdir` says.
fn registration_by_id(repo_path: &Path, checkout: &Path) -> Result<Option<PathBuf>, String> {
    let root = managed_root(repo_path)?;
    let (Some(id), true) = (
        checkout.file_name(),
        checkout.parent() == Some(root.as_path()),
    ) else {
        return Ok(None);
    };
    let Some((admin, named)) = crate::core::lane_git::registration_by_id(repo_path, id)? else {
        return Ok(None);
    };
    if let Some(other) = named.as_deref().filter(|other| *other != checkout) {
        let another_checkout = other.parent() == Some(root.as_path())
            && std::fs::symlink_metadata(other).is_ok_and(|meta| meta.is_dir());
        if another_checkout && crate::core::lane_git::pointer_problem(other, &admin).is_none() {
            tracing::warn!(
                checkout = %checkout.display(),
                registration = %admin.display(),
                "the registration with this checkout's name is {}'s: kept",
                other.display()
            );
            return Ok(None);
        }
    }
    tracing::warn!(
        checkout = %checkout.display(),
        registration = %admin.display(),
        "the registration Kronn gave this checkout names {}: removed with it",
        named.as_deref().map_or_else(
            || "no readable path".to_string(),
            |other| other.display().to_string()
        )
    );
    Ok(Some(admin))
}

/// The registration git gave the checkout `worktree_path` Kronn just created carries its
/// folder's name (KT-2084): git gives another id (`<name>1`) when that name is taken, and
/// Kronn later finds the registration it made by that name. Otherwise the creation is undone
/// (the checkout, its registration, and `created_branch`) and refused, naming the holder.
pub fn confirm_registration_id(
    repo_path: &Path,
    worktree_path: &Path,
    created_branch: Option<&str>,
) -> Result<(), String> {
    // A path Kronn would not remove by its registration's name either: nothing to confirm.
    let Ok((root, checkout)) = owned_checkout(repo_path, worktree_path, true) else {
        return Ok(());
    };
    let Some(id) = checkout.file_name() else {
        return Ok(());
    };
    // Read from the `.git` git just wrote, before any agent runs in the checkout.
    let Some(given) = crate::core::lane_git::pointer_registration_id(&checkout) else {
        return Ok(());
    };
    if id == given.as_str() {
        return Ok(());
    }
    let holder = match crate::core::lane_git::registration_by_id(repo_path, id) {
        Ok(Some((taken, Some(path)))) => {
            format!("{} (the worktree {})", taken.display(), path.display())
        }
        Ok(Some((taken, None))) => taken.display().to_string(),
        _ => "another registration".to_string(),
    };
    let mut left = String::new();
    if let Err(error) = remove_owned_checkout(repo_path, &root, &checkout, false) {
        left = format!(" The new checkout could not be undone: {error}.");
    } else if let Some(branch) = created_branch {
        let deleted = delete_git(repo_path).and_then(|mut command| {
            command.args(["branch", "-D", "--end-of-options", branch]);
            bounded(command, "branch -D")
        });
        if !deleted.is_ok_and(|output| output.status.success()) {
            left = format!(" The new branch {branch} is left: delete it before retrying.");
        }
    }
    Err(format!(
        "git registered {} as `{given}`, not under its folder's name, because {holder} already \
         holds that name: Kronn creates no worktree whose registration it could take for that \
         one. Remove that worktree (`git worktree remove`) or rename it, then retry.{left}",
        checkout.display()
    ))
}

/// Remove the registration `admin`, below its own `<common>/worktrees`.
fn remove_admin_dir(admin: &Path) -> Result<(), String> {
    let registrations = admin
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", admin.display()))?;
    crate::core::fs_guard::guarded_remove_tree(registrations, admin).map_err(|error| {
        format!(
            "its git registration {} could not be removed ({error})",
            admin.display()
        )
    })
}

/// Remove Kronn's checkout `checkout` (canonical, below the canonical managed `root`), then
/// the one registration naming it exactly, each without following a link (KT-2084). Neither
/// is found from the worktree's `.git` or from a path git lists. A checkout no registration
/// names goes when `unregistered` allows it, with the registration Kronn gave it if one is
/// left ([`registration_by_id`]); several registrations, or one naming it through a link,
/// keep it.
fn remove_owned_checkout(
    repo_path: &Path,
    root: &Path,
    checkout: &Path,
    unregistered: bool,
) -> Result<(), String> {
    let admin = crate::core::lane_git::registration_of(repo_path, checkout)?;
    let present = std::fs::symlink_metadata(checkout).is_ok();
    if present && admin.is_none() && !unregistered {
        return Err(UNREGISTERED_WORKTREE.to_string());
    }
    // Read before the checkout goes: the registration of another checkout is told from its
    // `.git`.
    let admin = match admin {
        Some(admin) => Some(admin),
        None => registration_by_id(repo_path, checkout)?,
    };
    if present {
        crate::core::fs_guard::guarded_remove_tree(root, checkout).map_err(|error| {
            if crate::core::fs_guard::is_link_refusal(&error) {
                format!("{} cannot be removed: {error}", checkout.display())
            } else {
                format!(
                    "{PARTLY_REMOVED}: {} could not be removed completely ({error})",
                    checkout.display()
                )
            }
        })?;
    }
    // The registration found before: what its `gitdir` says now does not matter.
    if let Some(admin) = admin {
        remove_admin_dir(&admin)
            .map_err(|error| format!("{PARTLY_REMOVED}: its directory is removed, but {error}"))?;
    }
    Ok(())
}

/// Remove Kronn's checkout `worktree_path` of `repo_path` and its registration, as
/// [`remove_discussion_worktree`] does, keeping every branch: a workflow run's forced cleanup.
pub fn remove_owned_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), String> {
    let (root, checkout) = owned_checkout(repo_path, worktree_path, true)?;
    remove_owned_checkout(repo_path, &root, &checkout, true)
}

/// The registration of Kronn's checkout `worktree_path`, gone from disk, removed by its own
/// id (KT-2084): a blanket `git worktree prune` would also drop a user's own worktree sitting
/// on an unmounted drive.
fn remove_registration_of_gone(repo_path: &Path, worktree_path: &Path) {
    let removed = owned_checkout(repo_path, worktree_path, true)
        .and_then(|(_, checkout)| remove_registration(repo_path, &checkout));
    if let Err(error) = removed {
        tracing::warn!(path = %worktree_path.display(), "registration kept: {error}");
    }
}

/// Removes the worktrees registered inside `parent`, deepest first, as Kronn removes its own
/// (KT-2084). A workflow step may add one under its run's worktree (`<run>/.kronn/pr-N`);
/// deleting the run's directory alone would leave its registration behind.
pub fn remove_nested_worktrees(repo_path: &Path, parent: &Path) -> Result<usize, String> {
    let Ok(root) = managed_root(repo_path) else {
        return Ok(0);
    };
    Ok(nested_worktrees(repo_path, parent)?
        .iter()
        .filter(
            |path| match remove_owned_checkout(repo_path, &root, path, false) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(path = %path.display(), "nested worktree kept: {error}");
                    false
                }
            },
        )
        .count())
}

/// The worktrees registered strictly inside `parent`, a checkout of `<repo>/.kronn/worktrees`,
/// deepest first and canonical. A path a registration names with a `..`, `.` or empty part, a
/// control character or a link below the repository is none of them (KT-2084). `Err` when the
/// registrations cannot be read: none can then be said absent.
pub fn nested_worktrees(repo_path: &Path, parent: &Path) -> Result<Vec<PathBuf>, String> {
    let Ok((_, parent)) = owned_checkout(repo_path, parent, false) else {
        return Ok(Vec::new());
    };
    let mut nested: Vec<PathBuf> = worktree_entries(repo_path)?
        .iter()
        .filter_map(plain_path)
        .filter_map(|path| owned_checkout(repo_path, &path, false).ok())
        .map(|(_, checkout)| checkout)
        .filter(|path| path.starts_with(&parent) && path != &parent)
        .collect();
    nested.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    nested.dedup();
    Ok(nested)
}

/// Drops the registrations of Kronn's worktrees whose checkout is gone (prunable); the
/// directory, if any, stays (KT-2084). Only a registration whose own `gitdir` names, exactly
/// as Kronn or git writes it, a gone checkout of `<repo>/.kronn/worktrees`, plainly spelled and
/// reached through no link: the registration removed is that entry itself, never one looked
/// up by a name its `gitdir` gives. A user's own worktree, on an unmounted drive or anywhere
/// else, is never touched. `Err`, and nothing dropped, when a registration cannot be read.
pub fn prune_kronn_worktrees(repo_path: &Path) -> Result<usize, String> {
    Ok(worktree_entries(repo_path)?
        .iter()
        .filter(|entry| entry.prunable)
        .filter_map(|entry| Some((entry.admin.as_deref()?, plain_path(entry)?)))
        .filter_map(|(admin, path)| {
            let (_, checkout) = owned_checkout(repo_path, &path, false).ok()?;
            Some((admin, checkout))
        })
        .filter(|(admin, checkout)| {
            crate::core::lane_git::registration_of(repo_path, checkout)
                .is_ok_and(|named| named.as_deref() == Some(*admin))
                && remove_admin_dir(admin).is_ok()
        })
        .count())
}

/// The agent configs [`copy_agent_configs`] puts into a new worktree.
const AGENT_CONFIG_FILES: [&[&str]; 4] = [
    &[".mcp.json"],
    &[".vibe", "config.toml"],
    &[".kiro", "settings", "mcp.json"],
    &[".gemini", "settings.json"],
];

/// The agents' project configs are gitignored, so a new worktree lacks them:
/// without `.mcp.json` a Claude agent gets no `--strict-mcp-config` and
/// inherits the host's personal MCP servers instead of the project's.
pub fn copy_agent_configs(repo_path: &Path, worktree_path: &Path) {
    for parts in AGENT_CONFIG_FILES {
        let relative: PathBuf = parts.iter().collect();
        let source = repo_path.join(&relative);
        if !source.is_file() {
            continue;
        }
        let target = worktree_path.join(&relative);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&source, &target) {
            Ok(_) => tracing::info!("Copied {} to worktree", relative.display()),
            Err(e) => tracing::warn!("Failed to copy {} to worktree: {}", relative.display(), e),
        }
    }
}

/// Largest agent config compared with the main checkout's: a bigger one is not Kronn's copy.
const AGENT_CONFIG_READ_CAP: u64 = 1024 * 1024;

/// The copies [`copy_agent_configs`] made that still equal the main
/// checkout's file: Kronn's own, not the work of what ran in the worktree.
pub fn unchanged_agent_configs(repo_path: &Path, worktree_path: &Path) -> Vec<PathBuf> {
    let Ok(root) = worktree_path.canonicalize() else {
        return Vec::new();
    };
    // A regular file under the cap, never through a link, never from a FIFO.
    let read = |root: &Path, relative: &Path| {
        crate::core::fs_guard::guarded_read_head(
            root,
            &root.join(relative),
            AGENT_CONFIG_READ_CAP + 1,
        )
        .ok()
        .flatten()
        .filter(|bytes| bytes.len() as u64 <= AGENT_CONFIG_READ_CAP)
    };
    AGENT_CONFIG_FILES
        .iter()
        .filter_map(|parts| {
            let relative: PathBuf = parts.iter().collect();
            let same = read(&root, &relative)? == read(repo_path, &relative)?;
            same.then_some(relative)
        })
        .collect()
}

/// What to do about the registration that holds the path or the branch of a reattach git
/// refused (KT-2084): read from the registrations' files, so it depends on neither git's
/// wording nor its version, and names that one registration. Empty when none holds them.
fn reattach_holder_advice(repo_path: &Path, worktree_path: &Path, branch: &str) -> String {
    let entries = match worktree_entries(repo_path) {
        Ok(entries) => entries,
        Err(error) => return format!(" {error}"),
    };
    let wanted = owned_checkout(repo_path, worktree_path, true)
        .ok()
        .map(|(_, checkout)| checkout);
    let holder = entries.iter().find_map(|entry| {
        let admin = entry.admin.as_deref()?;
        if entry.branch.as_deref() == Some(branch) {
            Some((admin, entry, format!("the branch {branch}")))
        } else if wanted.as_deref() == Some(entry.path.as_path()) {
            Some((admin, entry, "this path".to_string()))
        } else {
            None
        }
    });
    let Some((admin, entry, held)) = holder else {
        return String::new();
    };
    use crate::core::lane_git::{path_as_data, quoted_for_command};
    // A lane writes the path its registration names: in a command only when no shell reads it.
    let quoted = quoted_for_command(repo_path).zip(quoted_for_command(&entry.path));
    let removal = match quoted {
        Some((repo, path)) if entry.readable => format!(
            "run `git -C {repo} worktree remove {path}` (with `--force` when it holds changes \
             you do not need)"
        ),
        _ => "remove it with git's own worktree removal, from the project's folder, giving it \
              that path exactly as written above"
            .to_string(),
    };
    format!(
        " The registration {} holds {held}, and Kronn removes no registration whose worktree \
         may be in use. Its worktree is:{}If that worktree is no longer needed, {removal}, then \
         retry.",
        admin.display(),
        path_as_data(&entry.path)
    )
}

/// `command`, one that changes the owner's checkout (a checkout, a stash, a `worktree add`),
/// run to its end within five times the judgement's bound, its process group killed past it
/// (KT-2084): git reads every registration there, and a FIFO in one makes it wait forever.
fn bounded_change(
    mut command: std::process::Command,
    what: &str,
) -> Result<std::process::Output, String> {
    let timeout = crate::core::lane_git::snapshot::judge_timeout() * 5;
    match crate::core::cmd::run_bounded_output(&mut command, &[], timeout) {
        Ok(crate::core::cmd::BoundedOutput::Exited(output)) => Ok(output),
        Ok(crate::core::cmd::BoundedOutput::TimedOut) => Err(format!(
            "git {what} did not finish within Kronn's own {} s timeout, so Kronn stopped it",
            timeout.as_secs()
        )),
        Err(error) => Err(format!("git {what} could not be started: {error}")),
    }
}

/// Re-attach an existing branch to a new worktree path.
/// Used to migrate worktrees from /data/workspaces/ to .kronn/worktrees/.
pub fn reattach_worktree(
    repo_path: &Path,
    project_slug: &str,
    discussion_slug: &str,
    existing_branch: &str,
) -> Result<WorktreeInfo, String> {
    let project_slug = slugify(project_slug);
    let discussion_slug = slugify(discussion_slug);
    let dir_name = format!("{}--{}", project_slug, discussion_slug);
    let worktree_path = worktree_base_dir(repo_path).join(&dir_name);

    // Block if branch is checked out in the main repo (user is testing)
    crate::core::lane_git::check_branch_name(existing_branch)?;
    if let Some(existing) = branch_checked_out_at(repo_path, existing_branch)? {
        if is_main_checkout(&existing, repo_path) {
            return Err(format!(
                "Branch {} is currently checked out in the main repo. Please switch to another branch first.",
                existing_branch
            ));
        }
    }

    // If worktree already exists at new path, just return it
    if worktree_path.exists() {
        return Ok(WorktreeInfo {
            path: worktree_path.to_string_lossy().to_string(),
            branch: existing_branch.to_string(),
            is_main_repo: false,
        });
    }

    crate::core::fs_guard::guarded_create_dir_all(repo_path, &worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    ensure_managed_excludes(repo_path)?;

    crate::core::git_global_config::ensure_safe_directory(repo_path);
    crate::core::git_global_config::ensure_safe_directory(&worktree_path);

    // A registration left by a checkout of Kronn's that is gone holds the path and the branch:
    // only those go, each by its own id (KT-2084). Git's own prune would also act on a
    // registration a worker rewrote, or on a user's worktree on an unmounted drive.
    if let Err(error) = prune_kronn_worktrees(repo_path) {
        tracing::warn!(repo = %repo_path.display(), "no registration pruned: {error}");
    }

    // Attach existing branch to new worktree path (no -b, branch already exists)
    let mut add = crate::core::cmd::project_git_cmd();
    add.args(["worktree", "add"])
        .arg(&worktree_path)
        .arg(existing_branch)
        .current_dir(repo_path);
    let output = bounded_change(add, "worktree add")
        .map_err(|e| format!("Failed to execute git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Kronn prunes only its own registrations (KT-2084): one it did not make may hold the
        // path or the branch.
        let advice = reattach_holder_advice(repo_path, &worktree_path, existing_branch);
        return Err(format!("git worktree reattach failed: {stderr}{advice}"));
    }
    confirm_registration_id(repo_path, &worktree_path, None)?;

    tracing::info!(
        "Re-attached worktree at {} (branch: {})",
        worktree_path.display(),
        existing_branch
    );

    fix_worktree_paths(repo_path, &worktree_path);

    Ok(WorktreeInfo {
        path: worktree_path.to_string_lossy().to_string(),
        branch: existing_branch.to_string(),
        is_main_repo: false,
    })
}

/// Largest `HEAD` file read from a worktree's registration.
const HEAD_READ_CAP: u64 = 4096;

/// The branch the managed worktree `worktree_path` of `repo_path` is on, read from its own
/// registration (`<admin>/HEAD`, never through a link, a regular file under a size cap), so the
/// form `git worktree list` gives its path in does not matter. Only `ref: refs/heads/<name>`
/// names one; `None`: detached, or not a worktree Kronn manages. No git runs: the registration
/// is found from the owner's checkout, its `HEAD` read as a file.
fn find_branch_for_worktree(repo_path: &Path, worktree_path: &str) -> Option<String> {
    let checkout = Path::new(worktree_path);
    let pinned = crate::core::lane_git::managed_registration(checkout).ok()?;
    if !same_path(&pinned.repo, repo_path) {
        return None;
    }
    let head = crate::core::fs_guard::guarded_read_head(
        &pinned.git_dir,
        &pinned.git_dir.join("HEAD"),
        HEAD_READ_CAP,
    )
    .ok()??;
    if head.len() as u64 >= HEAD_READ_CAP {
        return None;
    }
    let target = std::str::from_utf8(&head)
        .ok()?
        .strip_prefix("ref: ")?
        .trim_end_matches('\n');
    // A reftable repository keeps HEAD in its tables, which Kronn does not read: this file is
    // then a stub, and no branch is found (kept, never guessed).
    let name = target
        .strip_prefix("refs/heads/")
        .filter(|name| *name != ".invalid")?;
    // A name git would take as a branch, never as an option (KT-2084).
    crate::core::lane_git::check_branch_name(name)
        .is_ok()
        .then(|| name.to_string())
}

/// Remove Kronn's worktree `worktree_path` of `repo_path` (a directory of
/// `<repo>/.kronn/worktrees`), and its branch when that is `delete_branch`.
///
/// Kronn removes the checkout itself (KT-2084): the directory Kronn named, never through a
/// link, then the one registration naming it exactly, neither found from the worktree's
/// `.git` or the `gitdir` file the agent may rewrite. A checkout no registration names goes
/// alone; several registrations keep it. Its submodules go with it: nothing in them is
/// judged here, the callers judge first.
///
/// The branch, read from the registration's `HEAD` before, goes only when it is
/// `delete_branch` and both removals succeeded.
///
/// The one thing this must never do is report success while the checkout is
/// still on disk. A worktree that silently survives its own reclamation is
/// exactly how the `.git/worktrees/*` count climbed unbounded until the sandbox
/// guard blocked every worker (KT-514). So the outcome is verified against the
/// filesystem before returning, and a residual checkout is a traced error rather
/// than a phantom success, with the branch left intact.
pub fn remove_discussion_worktree(
    repo_path: &Path,
    worktree_path: &str,
    delete_branch: Option<&str>,
) -> Result<(), String> {
    let (root, checkout) = owned_checkout(repo_path, Path::new(worktree_path), true)?;
    // Before the removal: the branch is read from the registration's `HEAD`.
    let branch_to_delete = delete_branch.filter(|expected| {
        find_branch_for_worktree(repo_path, worktree_path).as_deref() == Some(*expected)
    });
    remove_owned_checkout(repo_path, &root, &checkout, true)?;
    if std::fs::symlink_metadata(&checkout).is_ok() {
        return Err(format!(
            "worktree {worktree_path} could not be removed — leaving it rather than reporting a phantom reclaim"
        ));
    }
    if let Some(branch) = branch_to_delete {
        let mut command = delete_git(repo_path)?;
        command.args(["branch", "-D", "--end-of-options", branch]);
        let deleted = bounded(command, "branch -D").is_ok_and(|output| output.status.success());
        if deleted {
            tracing::info!("Deleted branch: {}", branch);
        } else {
            tracing::warn!("Branch {} of the removed worktree was not deleted", branch);
        }
    }
    tracing::info!("Removed worktree: {}", worktree_path);
    Ok(())
}

/// Git for a delete (KT-2084): lane_git's switches and pinning, since an agent may have
/// written a config in `dir`, and C messages, which Kronn parses.
fn delete_git(dir: &Path) -> Result<std::process::Command, String> {
    let mut command = crate::core::lane_git::git(dir)?;
    command.env("LC_ALL", "C");
    Ok(command)
}

/// `command` run to its end within the judgement's time bound (KT-2084): a timeout keeps
/// what it was judging, and says so.
fn bounded(command: std::process::Command, what: &str) -> Result<std::process::Output, String> {
    crate::core::lane_git::snapshot::bounded_output(command, what)
}

/// What removing a deleted discussion's worktree did with its branch (KT-2084).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletedDiscussionBranch {
    /// The checkout was on no branch (detached, or none found).
    NoBranch,
    /// Every commit on the branch is reachable from another ref: deleted.
    Deleted(String),
    /// A commit no other ref reaches, or no proof either way: kept.
    Kept { branch: String, reason: String },
}

/// Remove a deleted discussion's worktree (KT-2084). Its branch goes only when it is the
/// discussion's own (`expected_branch`, a `kronn/` branch), no commit is lost and
/// `branch_users` (branch → who names it) names none. Err keeps the worktree (or says what
/// of it is left) and its branch, with the reason.
pub fn remove_deleted_discussion_worktree(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: Option<&str>,
    branch_users: &std::collections::BTreeMap<String, String>,
) -> Result<DeletedDiscussionBranch, String> {
    let branch = find_branch_for_worktree(repo_path, worktree_path);
    remove_clean_discussion_worktree(repo_path, worktree_path).map_err(|reason| match &branch {
        Some(branch) if reason.starts_with(PARTLY_REMOVED) => {
            format!("{reason}; its branch {branch} is kept")
        }
        _ => reason,
    })?;
    let Some(branch) = branch else {
        return Ok(DeletedDiscussionBranch::NoBranch);
    };
    let own = expected_branch.filter(|expected| expected.starts_with("kronn/"));
    if own != Some(branch.as_str()) {
        let reason = match expected_branch {
            Some(expected) => format!(
                "the worktree was on it, not on the discussion's branch {expected}: neither is deleted"
            ),
            None => "the discussion names no branch of its own".to_string(),
        };
        return Ok(DeletedDiscussionBranch::Kept { branch, reason });
    }
    if let Some(user) = branch_users.get(&branch) {
        let reason = user.clone();
        return Ok(DeletedDiscussionBranch::Kept { branch, reason });
    }
    let reason = match commit_only_on_branch(repo_path, &branch) {
        Ok(None) => {
            let mut command = delete_git(repo_path)?;
            command.args(["branch", "-D", "--end-of-options", &branch]);
            let output = bounded(command, "branch -D")?;
            if output.status.success() {
                return Ok(DeletedDiscussionBranch::Deleted(branch));
            }
            format!(
                "git branch -D failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
        Ok(Some(commit)) => format!("commit {commit} is reachable from no other ref"),
        Err(reason) => reason,
    };
    Ok(DeletedDiscussionBranch::Kept { branch, reason })
}

/// Whether `checkout` is a worktree Kronn manages for `repo_path` (KT-2084): a registered
/// `<repo>/.kronn/worktrees/<name>` itself, pinned to that repository. Err: its registration
/// or its pointer cannot be trusted. No git runs in the worktree or its registration.
pub fn is_managed_discussion_worktree(repo_path: &Path, checkout: &Path) -> Result<bool, String> {
    let named = crate::core::lane_git::managed_worktree_repo(checkout);
    if !named.is_some_and(|repo| same_path(&repo, repo_path)) {
        return Ok(false);
    }
    let pinned = crate::core::lane_git::managed_registration(checkout)?;
    Ok(same_path(&pinned.repo, repo_path))
}

/// Kronn's own copy of the git state of a managed worktree and of each of its populated
/// submodules (KT-2084): every read-only git command of a judgement runs against it, never
/// against a configuration, `HEAD` or index the agent can write.
struct Judgement {
    worktree: Snapshot,
    /// `(path from the worktree, its copy)`, nested ones included, parents first.
    submodules: Vec<(String, Snapshot)>,
}

impl Judgement {
    fn of(checkout: &Path) -> Result<Self, String> {
        Self::with_submodules(Snapshot::of_worktree(checkout)?)
    }

    /// [`Judgement::of`] a worktree `repo_path` registers anywhere below its managed root.
    fn of_registered(repo_path: &Path, checkout: &Path) -> Result<Self, String> {
        Self::with_submodules(Snapshot::of_registered_worktree(repo_path, checkout)?)
    }

    fn with_submodules(worktree: Snapshot) -> Result<Self, String> {
        let submodules = populated_submodules(&worktree)?;
        Ok(Self {
            worktree,
            submodules,
        })
    }
}

/// Kronn's judgement is the verdict (KT-2084): a fresh look at the worktree, its submodules
/// and every submodule repository finds nothing to lose, then Kronn removes the checkout and
/// its registration itself. No git runs in the worktree or its submodules after that look.
fn remove_clean_discussion_worktree(repo_path: &Path, worktree_path: &str) -> Result<(), String> {
    let given = Path::new(worktree_path);
    let (root, checkout) = match owned_checkout(repo_path, given, true) {
        Ok(owned) => owned,
        Err(_) if std::fs::symlink_metadata(given).is_err() => return Ok(()),
        Err(_) => return Err(UNMANAGED_WORKTREE.to_string()),
    };
    if std::fs::symlink_metadata(&checkout).is_err() {
        return remove_registration(repo_path, &checkout).map(|_| ());
    }
    // Git follows the worktree's pointer to look at it: refused when it was moved.
    if !is_managed_discussion_worktree(repo_path, &checkout)? {
        return Err(UNMANAGED_WORKTREE.to_string());
    }
    let judged = Judgement::of(&checkout)?;
    let (loss, found) = forced_removal_check(repo_path, &judged, worktree_path, Saved::Nothing)?;
    drop(judged);
    if !found.files.nested.is_empty() {
        return Err(format!(
            "{} hold(s) a repository Kronn never removes by hand: move it out first",
            name_files(&found.files.nested)
        ));
    }
    if let Some(loss) = loss {
        return Err(loss);
    }
    remove_owned_checkout(repo_path, &root, &checkout, false)
}

/// What a stopped workflow run's checkout, or a worktree registered inside it, holds
/// (KT-2084), judged from Kronn's copy of its git state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCheckoutWork {
    /// Modified and untracked files, its submodules' included.
    Uncommitted(usize),
    /// Commits on a detached HEAD that no branch, tag or remote reaches.
    Detached,
    /// Anything else its removal keeps, worded: a lock, a repository kept inside it, a
    /// submodule's commits.
    Kept(String),
}

/// The first look at a run's `checkout` before its teardown (KT-2084): the verdict its removal
/// ([`remove_judged_run_checkout`]) gives, the worktrees registered inside it left to their
/// own look. No git runs in it or in its submodules.
pub fn run_checkout_work(
    repo_path: &Path,
    checkout: &Path,
) -> Result<Option<RunCheckoutWork>, String> {
    let (_, checkout) = owned_checkout(repo_path, checkout, false)?;
    let removed_first = nested_worktrees(repo_path, &checkout)?;
    let judged = Judgement::of_registered(repo_path, &checkout)?;
    let kept = kept_by_git(repo_path, &checkout, &judged, Removal::Run, &removed_first)?;
    Ok(if !kept.uncommitted.is_empty() {
        Some(RunCheckoutWork::Uncommitted(kept.uncommitted.len()))
    } else if kept.detached.is_some() {
        Some(RunCheckoutWork::Detached)
    } else {
        joined(kept.other).map(RunCheckoutWork::Kept)
    })
}

/// Which removal git's own check once guarded (KT-2084).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// A stopped workflow run's checkout, or a worktree registered inside it.
    Run,
    /// A worktree of Kronn's layout that nothing owns.
    Ownerless,
}

/// What keeps a checkout from a run's or an ownerless removal (KT-2084).
#[derive(Debug, Default)]
struct KeptByGit {
    /// Modified and untracked files, a run's submodules' included, by path from the checkout.
    uncommitted: Vec<String>,
    /// The newest commit on a detached HEAD that no branch, tag or remote reaches.
    detached: Option<String>,
    /// Anything else, worded.
    other: Vec<String>,
}

impl KeptByGit {
    /// Why the checkout `shown` is kept, `None` when nothing keeps it.
    fn describe(self, shown: &str) -> Option<String> {
        let mut lines = WorktreeFiles {
            uncommitted: self.uncommitted,
            ..WorktreeFiles::default()
        }
        .describe(shown);
        if let Some(commit) = self.detached {
            lines.push(format!(
                "{shown} has commits on a detached HEAD (newest {commit}) that no branch, tag or remote reaches: create a branch or discard them first"
            ));
        }
        lines.extend(self.other);
        joined(lines)
    }
}

/// Kronn's verdict on a run's or an ownerless `checkout` (canonical), judged from the copy
/// `judged` (KT-2084): what git's own removal without `--force` kept. Modified and untracked
/// files (links, OS junk and Kronn's own entries excepted), a detached HEAD's commits, a
/// repository kept inside it, a lock, submodule repositories with worktrees of their own or
/// commits no remote branch reaches; a run's submodules' files; any populated submodule of an
/// ownerless worktree. Ignored files go with it. `removed_first`: worktrees registered inside
/// it, removed before it and judged on their own.
fn kept_by_git(
    repo_path: &Path,
    checkout: &Path,
    judged: &Judgement,
    removal: Removal,
    removed_first: &[PathBuf],
) -> Result<KeptByGit, String> {
    let shown = checkout.display().to_string();
    let regenerable = regenerable_outputs();
    let files = worktree_files(Some(repo_path), &judged.worktree, &regenerable)?;
    let mut kept = KeptByGit {
        uncommitted: files.uncommitted,
        ..KeptByGit::default()
    };
    let mut skipped = files.ignored_dirs;
    match removal {
        Removal::Ownerless if !judged.submodules.is_empty() => {
            let paths: Vec<String> = judged.submodules.iter().map(|(p, _)| p.clone()).collect();
            kept.other.push(format!(
                "{shown} has populated submodules ({}): Kronn does not remove it; remove it by \
                 hand once nothing in it is needed",
                name_files(&paths)
            ));
        }
        Removal::Ownerless => {}
        Removal::Run => {
            for (relative, submodule) in &judged.submodules {
                let below = regenerable_below(&regenerable, relative);
                let found = worktree_files(None, submodule, &below)?;
                let below = |path: &String| format!("{relative}/{path}");
                kept.uncommitted.extend(found.uncommitted.iter().map(below));
                skipped.extend(found.ignored_dirs.iter().map(below));
            }
        }
    }
    kept.detached = commit_on_detached_head(&judged.worktree)?;
    let nested: Vec<String> = files
        .nested
        .into_iter()
        .filter(|entry| {
            let dir = checkout.join(entry.trim_end_matches('/'));
            holds_a_repository_besides(checkout, &dir, removed_first)
        })
        .collect();
    if !nested.is_empty() {
        kept.other.push(format!(
            "{} in {shown} hold(s) a repository: move it out first",
            name_files(&nested)
        ));
    }
    let admin = judged.worktree.source();
    if std::fs::symlink_metadata(admin.join("locked")).is_ok() {
        kept.other.push(format!(
            "{shown} is locked (`git worktree unlock` releases it)"
        ));
    }
    kept.other.extend(module_worktrees(admin)?);
    kept.other.extend(
        submodule_commits_off_remotes(repo_path, judged, SubmoduleCommits::OffRemotes)?
            .into_iter()
            .map(|line| format!("{line}: push it or discard it first")),
    );
    if let Some(reason) = embedded_repository(checkout, &skipped, removed_first) {
        kept.other.push(reason);
    }
    Ok(kept)
}

/// Why Kronn did not remove a checkout it judged (KT-2084).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotRemoved {
    /// Its judgement keeps it, or could not be made: nothing was removed.
    Kept(String),
    /// The removal stopped part way ([`PARTLY_REMOVED`]).
    Failed(String),
}

/// Removes a stopped run's checkout `checkout`, or a worktree registered inside it, with its
/// registration, once Kronn's judgement finds nothing in it to lose (KT-2084). The branch
/// stays. No git runs in the worktree or its submodules after the judgement.
pub fn remove_judged_run_checkout(repo_path: &Path, checkout: &Path) -> Result<(), NotRemoved> {
    let (root, checkout) = owned_checkout(repo_path, checkout, false).map_err(NotRemoved::Kept)?;
    if std::fs::symlink_metadata(&checkout).is_err() {
        return Ok(());
    }
    let judged = Judgement::of_registered(repo_path, &checkout).map_err(not_judged)?;
    remove_judged_checkout(repo_path, &root, &checkout, judged, Removal::Run)
}

fn not_judged(error: String) -> NotRemoved {
    NotRemoved::Kept(format!("a state git cannot read ({error})"))
}

/// Kronn's verdict on removing the judged `checkout` (canonical, below the canonical managed
/// `root`) with everything in it ([`kept_by_git`]), then its removal with the one
/// registration naming it. Worktrees registered inside it are gone already.
fn remove_judged_checkout(
    repo_path: &Path,
    root: &Path,
    checkout: &Path,
    judged: Judgement,
    removal: Removal,
) -> Result<(), NotRemoved> {
    let kept = kept_by_git(repo_path, checkout, &judged, removal, &[]).map_err(not_judged)?;
    drop(judged);
    if let Some(reason) = kept.describe(&checkout.display().to_string()) {
        return Err(NotRemoved::Kept(reason));
    }
    remove_owned_checkout(repo_path, root, checkout, false).map_err(|error| {
        if error.starts_with(PARTLY_REMOVED) {
            NotRemoved::Failed(error)
        } else {
            NotRemoved::Kept(error)
        }
    })
}

/// The submodule repositories of the registration `admin` that have worktrees of their own:
/// removing the registration would take what those worktrees need.
fn module_worktrees(admin: &Path) -> Result<Vec<String>, String> {
    let modules = admin.join("modules");
    let mut found = Vec::new();
    for git_dir in module_git_dirs(admin)? {
        let name = git_dir
            .strip_prefix(&modules)
            .unwrap_or(&git_dir)
            .display()
            .to_string();
        match crate::core::fs_guard::guarded_list(&git_dir, &git_dir.join("worktrees")) {
            Ok(None) => {}
            Ok(Some(entries)) if entries.is_empty() => {}
            Ok(Some(_)) => found.push(format!(
                "its submodule `{name}`, which has worktrees of its own"
            )),
            Err(error) => found.push(format!(
                "its submodule `{name}`, whose worktrees cannot be listed ({error})"
            )),
        }
    }
    Ok(found)
}

/// How deep [`embedded_repository`] looks, under the depth Kronn's removal walks
/// ([`crate::core::rooted_fd::MAX_DEPTH`]), and how many folders: past either it keeps.
const MAX_EMBEDDED_DEPTH: usize = 48;
const MAX_EMBEDDED_DIRS: usize = 100_000;

/// The deepest folder Kronn's removal takes, counted from the removed checkout
/// ([`crate::core::rooted_fd::MAX_DEPTH`] levels, the checkout itself included).
const REMOVAL_DEPTH: usize = crate::core::rooted_fd::MAX_DEPTH - 1;
/// How many entries, and how long, the look at the depth of an ignored folder spends: past
/// either it stops without keeping the checkout (no content of it is judged).
const IGNORED_DEPTH_ENTRIES: usize = 2_000_000;
const IGNORED_DEPTH_TIME: std::time::Duration = std::time::Duration::from_secs(60);

/// Why Kronn's removal of `checkout` would stop in its ignored folder `dir`, `depth` levels
/// below it: a folder deeper than [`REMOVAL_DEPTH`], or one that cannot be listed. Only the
/// folders are looked at, never what they hold.
fn ignored_folder_unremovable(checkout: &Path, dir: &Path, depth: usize) -> Option<String> {
    use crate::core::fs_guard::{guarded_walk, Visit, WalkBounds};
    let shown = |path: &Path| {
        path.strip_prefix(checkout)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let bounds = WalkBounds {
        max_entries: IGNORED_DEPTH_ENTRIES,
        deadline: std::time::Instant::now() + IGNORED_DEPTH_TIME,
        max_depth: crate::core::rooted_fd::MAX_DEPTH,
    };
    let mut too_deep = None;
    let walked = guarded_walk(checkout, dir, bounds, &mut |entry| {
        if entry.depth == 0 {
            return Visit::Enter;
        }
        if !entry.is_dir || entry.is_symlink {
            return Visit::Skip;
        }
        if depth + entry.depth > REMOVAL_DEPTH {
            too_deep = Some(entry.path.to_path_buf());
            return Visit::Stop;
        }
        Visit::Enter
    });
    if let Some(deep) = too_deep {
        return Some(format!(
            "its ignored folder `{}` goes more than {REMOVAL_DEPTH} levels deep (`{}`): Kronn's \
             removal stops there; remove it by hand",
            shown(dir),
            shown(&deep)
        ));
    }
    match walked {
        Ok(end) => {
            if let Some(why) = end.bound() {
                tracing::warn!(
                    "the depth of the ignored folder {} was not looked through ({why})",
                    dir.display()
                );
            }
            None
        }
        Err(error) => Some(format!(
            "its ignored folder `{}` cannot be listed ({error}): Kronn's removal would stop \
             there; remove it by hand",
            shown(dir)
        )),
    }
}

/// Why a repository kept inside `checkout` itself keeps it: a `.git` directory below its top
/// (a clone, a submodule never absorbed), found through no link, outside its ignored folders
/// `skipped` (by path from it; Kronn's `.kronn/` is never one) and the worktrees `besides`,
/// judged on their own. A folder that cannot be listed keeps it too, and so does a walk past
/// its bounds. An ignored folder is not searched, but one the removal would stop in keeps it
/// ([`ignored_folder_unremovable`]).
fn embedded_repository(checkout: &Path, skipped: &[String], besides: &[PathBuf]) -> Option<String> {
    let shown = |dir: &Path| {
        dir.strip_prefix(checkout)
            .unwrap_or(dir)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let skipped: std::collections::HashSet<PathBuf> = skipped
        .iter()
        .map(|dir| checkout.join(dir.trim_end_matches('/')))
        .collect();
    let mut pending = vec![(checkout.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = pending.pop() {
        visited += 1;
        if visited > MAX_EMBEDDED_DIRS {
            return Some(format!(
                "it holds more than {MAX_EMBEDDED_DIRS} folders outside its ignored ones: \
                 Kronn does not look further for a repository kept inside it"
            ));
        }
        let Some(entries) = listed(checkout, &dir) else {
            return Some(format!(
                "`{}` cannot be listed: Kronn cannot tell whether it holds a repository",
                shown(&dir)
            ));
        };
        for (name, kind) in entries {
            if !kind.is_dir() {
                continue;
            }
            if name == ".git" {
                if dir != checkout {
                    return Some(format!(
                        "a repository kept inside the worktree, at `{}`: move it out first",
                        shown(&dir)
                    ));
                }
                continue;
            }
            let child = dir.join(name);
            if besides.contains(&child) {
                continue;
            }
            if skipped.contains(&child) {
                if let Some(reason) = ignored_folder_unremovable(checkout, &child, depth + 1) {
                    return Some(reason);
                }
                continue;
            }
            if depth + 1 > MAX_EMBEDDED_DEPTH {
                return Some(format!(
                    "its folders go more than {MAX_EMBEDDED_DEPTH} levels deep (`{}`): Kronn \
                     does not look further for a repository kept inside it",
                    shown(&child)
                ));
            }
            pending.push((child, depth + 1));
        }
    }
    None
}

/// Why a delete leaves a directory of the worktree layout that git no longer lists on disk.
pub const UNREGISTERED_WORKTREE: &str =
    "git no longer lists it as a worktree (its registration was removed): it stays on disk; \
     remove it by hand once nothing in it is needed";

/// Whether `checkout` is a directory of `repo_path`'s worktree layout that git no longer lists
/// (its registration pruned, the directory left): Kronn neither judges nor removes it (KT-2084).
pub fn is_unregistered_managed_worktree(repo_path: &Path, checkout: &Path) -> bool {
    checkout
        .parent()
        .is_some_and(|parent| same_path(parent, &worktree_base_dir(repo_path)))
        && matches!(crate::core::lane_git::is_registered(checkout), Ok(false))
}

/// Why a delete leaves a worktree Kronn does not manage on disk (KT-2084).
pub const UNMANAGED_WORKTREE: &str =
    "Kronn does not manage this worktree (it is not one of the project's registered \
     .kronn/worktrees): it stays on disk";

use crate::core::lane_git::plain_relative;
use crate::core::lane_git::snapshot::Snapshot;

/// How deep [`populated_submodules`] looks before refusing to judge.
const MAX_SUBMODULE_DEPTH: usize = 6;

/// The populated submodules of the copy `parent`, nested ones included: `(path from its work
/// tree, the submodule's own copy)`, parents first, from the gitlinks of each copied index.
/// A submodule Kronn cannot judge from a copy (a link, a path outside the tree, a `.git` that
/// names another repository) refuses the whole judgement: git never runs in it.
fn populated_submodules(parent: &Snapshot) -> Result<Vec<(String, Snapshot)>, String> {
    let mut found = Vec::new();
    collect_submodules(parent, "", 0, &mut found)?;
    Ok(found)
}

fn collect_submodules(
    parent: &Snapshot,
    prefix: &str,
    depth: usize,
    found: &mut Vec<(String, Snapshot)>,
) -> Result<(), String> {
    for path in parent.gitlinks()? {
        let Some(submodule) = parent.of_submodule(&path)? else {
            continue;
        };
        if depth + 1 > MAX_SUBMODULE_DEPTH {
            return Err(format!(
                "submodules nested more than {MAX_SUBMODULE_DEPTH} levels deep"
            ));
        }
        let relative = if prefix.is_empty() {
            path
        } else {
            format!("{prefix}/{path}")
        };
        let mut nested = Vec::new();
        collect_submodules(&submodule, &relative, depth + 1, &mut nested)?;
        found.push((relative, submodule));
        found.extend(nested);
    }
    Ok(())
}

/// Most directories [`module_git_dirs`] visits, and how deep: past either it refuses.
const MAX_MODULE_DIRS: usize = 1024;
const MAX_MODULE_DEPTH: usize = 16;

/// The submodule repositories `--force` would delete with the worktree: every git
/// directory under its `modules` directory, a de-initialised one included. A link at
/// `<admin>/modules` or below refuses, and so does a walk past its bounds.
fn module_git_dirs(admin: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let modules = admin.join("modules");
    let is_dir = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    match std::fs::symlink_metadata(&modules) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(format!("{} cannot be read: {error}", modules.display())),
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "{} is a link or a file: Kronn does not judge its submodule repositories",
                modules.display()
            ))
        }
    }
    let mut pending = vec![(modules, 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = pending.pop() {
        visited += 1;
        if visited > MAX_MODULE_DIRS {
            return Err(format!(
                "{} holds more than {MAX_MODULE_DIRS} directories: Kronn does not judge its \
                 submodule repositories",
                admin.join("modules").display()
            ));
        }
        let names = crate::core::fs_guard::guarded_list(admin, &dir)
            .map_err(|error| format!("{} cannot be read: {error}", dir.display()))?
            .unwrap_or_default();
        for name in names {
            let path = dir.join(&name);
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "{} is a link: Kronn does not judge its submodule repositories",
                    path.display()
                ));
            }
            if !meta.is_dir() {
                continue;
            }
            if depth + 1 > MAX_MODULE_DEPTH {
                return Err(format!(
                    "{} is nested more than {MAX_MODULE_DEPTH} levels deep: Kronn does not \
                     judge its submodule repositories",
                    path.display()
                ));
            }
            let is_repository = std::fs::symlink_metadata(path.join("HEAD"))
                .is_ok_and(|meta| meta.is_file())
                && is_dir(&path.join("objects"));
            if is_repository {
                let nested = path.join("modules");
                match std::fs::symlink_metadata(&nested) {
                    Ok(meta) if meta.is_dir() => pending.push((nested, depth + 1)),
                    Ok(_) => {
                        return Err(format!(
                            "{} is a link or a file: Kronn does not judge its submodule \
                             repositories",
                            nested.display()
                        ))
                    }
                    Err(_) => {}
                }
                found.push(path);
            } else {
                pending.push((path, depth + 1));
            }
        }
    }
    Ok(found)
}

/// A commit of `snapshot`'s repository that `rule` counts and no `saved` commit reaches: what
/// deleting that repository may lose. Read from the copy's refs, `refs/stash` included.
fn commit_off_remotes(
    snapshot: &Snapshot,
    saved: &[String],
    rule: SubmoduleCommits,
) -> Result<Option<String>, String> {
    let mut command = snapshot.command();
    command.args(["rev-list", "-n", "1", "HEAD"]);
    match rule {
        // `--glob` matches `refs/stash` itself only through a pattern, and none is no error.
        SubmoduleCommits::OffRemotesAndTags => command.args([
            "--branches",
            "--glob=refs/stas[h]",
            "--not",
            "--remotes",
            "--tags",
        ]),
        SubmoduleCommits::OffRemotes => command.args(["--all", "--not", "--remotes"]),
    };
    command.args(saved);
    let output = bounded(command, "rev-list")?;
    if !output.status.success() {
        return Err(format!(
            "its commits cannot be read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!commit.is_empty()).then_some(commit))
}

/// Most tips of a submodule repository checked against its source; the rest count as unsaved.
const MAX_SOURCE_TIPS: usize = 64;

/// The tips of the submodule repository `submodule` (its HEAD and each local branch, read from
/// Kronn's copy) that the project checkout's own repository at the same path `relative`
/// already holds (KT-2084): a task's submodule is cloned from there, so a pin the project
/// holds is saved even when none of its branches reaches it. Only a source the owner itself
/// declares counts ([`owner_source_module`]), never one below `.kronn/`, where Kronn's
/// checkouts and the workers' lane targets live: otherwise empty, every tip unsaved, with
/// the reason the project's copy was not read (`Err` of the inner result).
fn tips_held_by_source(
    submodule: &Snapshot,
    repo_path: &Path,
    relative: &str,
) -> Result<Result<Vec<String>, String>, String> {
    let first = relative.split('/').next().unwrap_or_default();
    if first.eq_ignore_ascii_case(".kronn") {
        return Ok(Err(
            "it lies under .kronn, where Kronn reads no source".to_string()
        ));
    }
    let owner = crate::core::lane_git::without_verbatim_prefix(
        repo_path
            .canonicalize()
            .map_err(|e| format!("cannot resolve {}: {e}", repo_path.display()))?,
    );
    let source = match owner_source_module(&owner, relative)? {
        Ok(source) => source,
        Err(unread) => return Ok(Err(unread)),
    };
    let read = |args: &[&str]| -> Result<String, String> {
        let output = submodule.output(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    };
    let listed = [
        read(&["rev-parse", "-q", "--verify", "HEAD"])?,
        read(&["for-each-ref", "--format=%(objectname)", "refs/heads"])?,
    ]
    .concat();
    // HEAD first; an object id only, so nothing else reaches the source's git.
    let mut tips: Vec<&str> = Vec::new();
    for tip in listed.lines().map(str::trim) {
        let object_id =
            matches!(tip.len(), 40 | 64) && tip.bytes().all(|byte| byte.is_ascii_hexdigit());
        if object_id && !tips.contains(&tip) && tips.len() < MAX_SOURCE_TIPS {
            tips.push(tip);
        }
    }
    if tips.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    // Kronn's copy of the source: one git, bounded, answers a line per tip, in order.
    let asked: String = tips
        .iter()
        .map(|tip| format!("{tip}^{{commit}}\n"))
        .collect();
    let mut command = source.command();
    command.args(["cat-file", "--batch-check=%(objecttype)"]);
    let output = crate::core::lane_git::snapshot::bounded_output_with_input(
        command,
        asked.as_bytes(),
        "cat-file",
    )?;
    // Nothing proven held: every tip stays counted.
    if !output.status.success() {
        return Ok(Err("git could not read its objects".to_string()));
    }
    let answers = String::from_utf8_lossy(&output.stdout);
    Ok(Ok(tips
        .iter()
        .zip(answers.lines())
        .filter(|(_, answer)| *answer == "commit")
        .map(|(tip, _)| tip.to_string())
        .collect()))
}

/// Kronn's copy of the submodule `relative` of the owner's checkout `owner` (canonical), when
/// the owner declares it (KT-2084): a gitlink of the owner's own index, populated with a
/// `.git` file naming the owner's `<common>/modules/<path>`, vetted, no part a link. The inner
/// `Err` otherwise, saying which of these it is not, for the reason a checkout is kept: no git
/// runs in a repository found below the checkout by discovery.
fn owner_source_module(owner: &Path, relative: &str) -> Result<Result<Snapshot, String>, String> {
    let mut listed = delete_git(owner)?;
    listed.args([
        "ls-files",
        "--stage",
        "-z",
        "--",
        &format!(":(literal){relative}"),
    ]);
    let output = bounded(listed, "ls-files")?;
    if !output.status.success()
        || !crate::core::lane_git::snapshot::gitlinks_of(&output.stdout)?
            .iter()
            .any(|path| path == relative)
    {
        return Ok(Err(
            "the project does not list it as a submodule of its own (a submodule of a \
             submodule is one of that submodule)"
                .to_string(),
        ));
    }
    let Ok(source) = crate::core::lane_git::submodule_dir(owner, relative) else {
        return Ok(Err(
            "its folder is reached through a link or is not a folder".to_string(),
        ));
    };
    match std::fs::symlink_metadata(source.join(".git")) {
        Ok(meta) if meta.is_file() => {}
        Ok(meta) if meta.is_dir() => {
            let fix = match crate::core::lane_git::quoted_for_command(owner) {
                Some(owner) => format!("`git -C {owner} submodule absorbgitdirs`"),
                None => "git's `submodule absorbgitdirs`, run in the project's folder,".to_string(),
            };
            return Ok(Err(format!(
                "it keeps its repository in its own .git directory, which Kronn does not read: \
                 {fix} moves it under the project's .git"
            )));
        }
        Ok(_) => return Ok(Err("its .git is neither a file nor a directory".to_string())),
        Err(_) => return Ok(Err("it is not initialised there (no .git)".to_string())),
    }
    let common = crate::core::lane_git::common_dir_of(owner)?;
    let modules = common.join("modules");
    let outside = || {
        format!(
            "its .git names no repository under {} that Kronn reads",
            modules.display()
        )
    };
    let Ok(Some(git_dir)) = crate::core::lane_git::vetted_nested_git_dir(&source, &common) else {
        return Ok(Err(outside()));
    };
    if !git_dir.starts_with(&modules) {
        return Ok(Err(outside()));
    }
    Ok(Snapshot::of_module(&git_dir, &modules, None)
        .map_err(|error| format!("Kronn could not copy its git state ({error})")))
}

/// Up to five of `files`, for a message naming them.
pub fn name_files(files: &[String]) -> String {
    let named = files.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    if files.len() > 5 {
        format!("{named}, …")
    } else {
        named
    }
}

/// A detached-HEAD commit no branch, tag or remote reaches (KT-2084): removing the
/// worktree deletes the only reflog that remembers it. Read from Kronn's copy.
fn commit_on_detached_head(worktree: &Snapshot) -> Result<Option<String>, String> {
    let output = worktree.output(&[
        "rev-list",
        "-n",
        "1",
        "HEAD",
        "--not",
        "--branches",
        "--tags",
        "--remotes",
    ])?;
    if !output.status.success() {
        return Err(format!(
            "the commits of its HEAD cannot be read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!commit.is_empty()).then_some(commit))
}

/// A worktree's files as a delete judges them (KT-2084).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorktreeFiles {
    /// Modified tracked files, untracked files, dirty submodules: work to commit.
    pub uncommitted: Vec<String>,
    /// Ignored files (a `.env`, notes) and wholly ignored folders (a trailing `/`): work
    /// git never shows, to move away.
    pub ignored: Vec<String>,
    /// Untracked OS junk, links and Kronn's own entries, which git would still refuse.
    pub disposable: Vec<String>,
    /// Kronn's unchanged untracked agent-config copies, which git would refuse too.
    pub copies: Vec<String>,
    /// Kronn's copies of a TRACKED agent config, modified: git would refuse them.
    pub modified_copies: Vec<String>,
    /// Untracked or ignored directories of Kronn's own that hold a repository: never
    /// removed by hand.
    pub nested: Vec<String>,
    /// Every wholly ignored directory but Kronn's own `.kronn/` and a link (a trailing `/`),
    /// work or not: a repository inside one is no repository of the worktree's own.
    pub ignored_dirs: Vec<String>,
}

impl WorktreeFiles {
    /// What the work in it is and what to do with it, for a refusal naming `shown`.
    fn describe(&self, shown: &str) -> Vec<String> {
        let mut found = Vec::new();
        if !self.uncommitted.is_empty() {
            found.push(format!(
                "{} uncommitted file(s) in {shown} ({}): commit or discard them first",
                self.uncommitted.len(),
                name_files(&self.uncommitted)
            ));
        }
        if !self.ignored.is_empty() {
            let folders = self
                .ignored
                .iter()
                .filter(|path| path.ends_with('/'))
                .count();
            let files = self.ignored.len() - folders;
            let counted = match (files, folders) {
                (files, 0) => format!("{files} ignored file(s)"),
                (0, folders) => format!("{folders} ignored folder(s)"),
                (files, folders) => {
                    format!("{files} ignored file(s) and {folders} ignored folder(s)")
                }
            };
            found.push(format!(
                "{counted} in {shown} ({}): move or delete them first; if it is generated \
                 output, name it in `server.regenerable_outputs`",
                name_files(&self.ignored)
            ));
        }
        found
    }
}

/// What a delete never counts as work in an ignored path (KT-2084).
struct NotWork<'a> {
    checkout: &'a Path,
    regenerable: &'a [String],
    junk: &'a [String],
    /// Kronn's agent configs and copies, by path from the checkout.
    kronn_files: &'a [String],
    seeds: &'a [String],
}

impl NotWork<'_> {
    fn seeded(&self, path: &str) -> bool {
        self.seeds
            .iter()
            .any(|seed| path == seed || path.starts_with(&format!("{seed}/")))
    }

    /// Whether the ignored `path` (no trailing slash) is Kronn's or made again.
    fn ignored(&self, path: &str, is_dir: bool) -> bool {
        is_regenerable(path, self.regenerable)
            || self.seeded(path)
            || self.kronn_files.iter().any(|file| file == path)
            || (is_dir && self.holds_none(path))
    }

    /// A wholly ignored directory holds only links, Kronn's files, seeds, OS junk and what
    /// tools make again, at any depth. A name that is not UTF-8 is work.
    fn holds_none(&self, dir: &str) -> bool {
        let mut pending = vec![dir.to_string()];
        while let Some(relative) = pending.pop() {
            let Some(entries) = listed(self.checkout, &self.checkout.join(&relative)) else {
                return false;
            };
            for (name, kind) in entries {
                let Some(name) = name.to_str().map(str::to_string) else {
                    return false;
                };
                let path = format!("{relative}/{name}");
                if kind.is_symlink() || is_os_junk(&name, self.junk) || self.ignored(&path, false) {
                    continue;
                }
                if !kind.is_dir() {
                    return false;
                }
                pending.push(path);
            }
        }
        true
    }
}

/// The entries of `dir` below `root` and their types, listed through no link (KT-2084).
/// `None`: it cannot be listed.
fn listed(root: &Path, dir: &Path) -> Option<Vec<(std::ffi::OsString, std::fs::FileType)>> {
    let names = crate::core::fs_guard::guarded_list(root, dir).ok()??;
    names
        .into_iter()
        .map(|name| {
            let kind = std::fs::symlink_metadata(dir.join(&name)).ok()?.file_type();
            Some((name, kind))
        })
        .collect()
}

/// Whether `dir` (below `root`) holds a `.git` entry at any depth (a repository, a linked
/// worktree); one that cannot be read counts as holding one.
fn holds_a_repository(root: &Path, dir: &Path) -> bool {
    holds_a_repository_besides(root, dir, &[])
}

/// [`holds_a_repository`], the worktrees `besides` (judged on their own) left out.
fn holds_a_repository_besides(root: &Path, dir: &Path, besides: &[PathBuf]) -> bool {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if besides.contains(&dir) {
            continue;
        }
        let Some(entries) = listed(root, &dir) else {
            return true;
        };
        for (name, kind) in entries {
            if name == ".git" {
                return true;
            }
            if kind.is_dir() {
                pending.push(dir.join(name));
            }
        }
    }
    false
}

/// Kronn's own entries in `checkout`, by path from it: its unchanged agent-config copies,
/// every agent-config path with those copies, and its seeds (none in a submodule:
/// `repo_path` None).
fn kronn_entries(
    repo_path: Option<&Path>,
    checkout: &Path,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (copies, mut kronn_files): (Vec<String>, Vec<String>) = match repo_path {
        Some(repo) => (
            unchanged_agent_configs(repo, checkout)
                .iter()
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .collect(),
            AGENT_CONFIG_FILES
                .iter()
                .map(|parts| parts.join("/"))
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let seeds: Vec<String> = repo_path
        .map(|repo| seeded_paths(repo, checkout))
        .unwrap_or_default();
    kronn_files.extend(copies.iter().cloned());
    (copies, kronn_files, seeds)
}

/// Not work for a delete: OS junk, `.kronn/`, Kronn's agent-config copies and seeds, a link,
/// what tools make again (`regenerable`) when ignored (`repo_path` None: a submodule). Judged
/// from Kronn's copy `judged` of the git state.
fn worktree_files(
    repo_path: Option<&Path>,
    judged: &Snapshot,
    regenerable: &[String],
) -> Result<WorktreeFiles, String> {
    let checkout = judged
        .work_tree()
        .ok_or_else(|| format!("{} has no work tree", judged.source().display()))?;
    // A submodule's own files are judged from its own copy, never by git descending into a
    // repository whose configuration it would then read.
    let output = judged.output(&[
        "status",
        "--ignore-submodules=dirty",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignored=matching",
    ])?;
    if !output.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if std::str::from_utf8(&output.stdout).is_err() {
        return Err("git names a path that is not UTF-8: Kronn does not judge it".to_string());
    }
    let junk = os_junk_patterns();
    let (copies, kronn_files, seeds) = kronn_entries(repo_path, checkout);
    let not_work = NotWork {
        checkout,
        regenerable,
        junk: &junk,
        kronn_files: &kronn_files,
        seeds: &seeds,
    };
    let mut files = WorktreeFiles::default();
    for file in parse_porcelain_z(&output.stdout) {
        let path = file.path;
        if !plain_relative(&path) {
            return Err(format!(
                "git names {path:?}, which is not a plain path of the worktree: Kronn does not judge it"
            ));
        }
        let trimmed = path.trim_end_matches('/');
        let is_dir = path.ends_with('/');
        let kronn_own =
            trimmed == ".kronn" || path.starts_with(".kronn/") || is_os_junk(&path, &junk);
        // Removing a link loses nothing; what it points at stays.
        let link = std::fs::symlink_metadata(checkout.join(trimmed))
            .is_ok_and(|meta| meta.file_type().is_symlink());
        match file.status.as_str() {
            // `.kronn/` is ignored in every worktree; a PR checkout under it is a repository.
            "!!" if kronn_own && is_dir && !link => {
                if holds_a_repository(checkout, &checkout.join(trimmed)) {
                    files.nested.push(path);
                }
            }
            "!!" => {
                if is_dir && !link && !kronn_own {
                    files.ignored_dirs.push(path.clone());
                }
                if !(link || kronn_own || not_work.ignored(trimmed, is_dir)) {
                    files.ignored.push(path);
                }
            }
            "??" if copies.contains(&path) => files.copies.push(path),
            "??" if link => files.disposable.push(path),
            "??" if kronn_own
                || (is_dir && holds_only_kronn_files(checkout, &path, &copies, &junk)) =>
            {
                if is_dir && holds_a_repository(checkout, &checkout.join(trimmed)) {
                    files.nested.push(path);
                } else {
                    files.disposable.push(path);
                }
            }
            " M" if copies.contains(&path) => files.modified_copies.push(path),
            // A repository of its own (git lists it as one folder): no patch holds it.
            "??" if is_dir && !link && holds_a_repository(checkout, &checkout.join(trimmed)) => {
                files.nested.push(path.clone());
                files.uncommitted.push(path);
            }
            _ => files.uncommitted.push(path),
        }
    }
    Ok(files)
}

/// The file, in a managed worktree's registered git directory, naming the paths Kronn
/// seeded into it (KT-1438).
pub const LANE_SEEDS_MARKER_FILE: &str = "kronn-lane-seeds.json";

/// What [`LANE_SEEDS_MARKER_FILE`] holds: the source fingerprint of each seeded path.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LaneSeedsMarker {
    #[serde(default)]
    pub seeds: std::collections::BTreeMap<String, String>,
}

/// The paths Kronn seeded into the worktree `checkout` of `repo_path` (KT-1438): its
/// copies of the main checkout's generated assets. Empty when it has none.
fn seeded_paths(repo_path: &Path, checkout: &Path) -> Vec<String> {
    // Found from the owner's registrations, never by asking git through the worktree.
    let Some(git_dir) = checkout
        .canonicalize()
        .ok()
        .and_then(|checkout| crate::core::lane_git::registration_of(repo_path, &checkout).ok())
        .flatten()
    else {
        return Vec::new();
    };
    crate::core::fs_guard::guarded_read_head(
        &git_dir,
        &git_dir.join(LANE_SEEDS_MARKER_FILE),
        SEEDS_MARKER_READ_CAP + 1,
    )
    .ok()
    .flatten()
    .filter(|bytes| bytes.len() as u64 <= SEEDS_MARKER_READ_CAP)
    .and_then(|bytes| serde_json::from_slice::<LaneSeedsMarker>(&bytes).ok())
    .map(|marker| marker.seeds.into_keys().collect())
    .unwrap_or_default()
}

/// Largest seeds marker read: a list of paths and fingerprints.
const SEEDS_MARKER_READ_CAP: u64 = 4 * 1024 * 1024;

/// What deleting `checkout` would lose (KT-2084): files, detached commits and a submodule's
/// files, each named after `shown`. A submodule commit no remote reaches never refuses a
/// delete: see [`submodule_commits_off_remotes`].
pub fn unsaved_work(
    repo_path: &Path,
    checkout: &Path,
    shown: &str,
) -> Result<Option<String>, String> {
    unsaved_work_with(repo_path, checkout, shown, &regenerable_outputs())
}

/// [`unsaved_work`] with the regenerable outputs given.
fn unsaved_work_with(
    repo_path: &Path,
    checkout: &Path,
    shown: &str,
    regenerable: &[String],
) -> Result<Option<String>, String> {
    let judged = Judgement::of(checkout)?;
    let found = work_found(repo_path, &judged, shown, regenerable, Saved::Nothing)?;
    Ok(joined(found.lines))
}

fn joined(found: Vec<String>) -> Option<String> {
    (!found.is_empty()).then(|| found.join("; "))
}

/// What a release saves before it removes a task checkout (KT-2084).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// Nothing: every file counts (a discussion delete).
    Nothing,
    /// Its ignored files, its submodules' included, copied into the evidence.
    IgnoredFiles,
    /// Those, and its own modified and untracked files as a patch.
    Everything,
}

/// What a judgement of a checkout found: the work lines, its files and its submodules'.
struct WorkFound {
    lines: Vec<String>,
    files: WorktreeFiles,
    /// `(path from the checkout, its files)` of each populated submodule.
    submodules: Vec<(String, WorktreeFiles)>,
}

/// The work lines of `checkout`, and its files. What `saved` saves is not counted; a
/// submodule's modified and untracked files always are.
fn work_found(
    repo_path: &Path,
    judged: &Judgement,
    shown: &str,
    regenerable: &[String],
    saved: Saved,
) -> Result<WorkFound, String> {
    let files = worktree_files(Some(repo_path), &judged.worktree, regenerable)?;
    let mut lines = WorktreeFiles {
        uncommitted: match saved {
            Saved::Everything => Vec::new(),
            _ => files.uncommitted.clone(),
        },
        ignored: match saved {
            Saved::Nothing => files.ignored.clone(),
            _ => Vec::new(),
        },
        ..WorktreeFiles::default()
    }
    .describe(shown);
    if let Some(commit) = commit_on_detached_head(&judged.worktree)? {
        lines.push(format!(
            "{shown} has commits on a detached HEAD (newest {commit}) that no branch, tag or remote reaches: create a branch or discard them first"
        ));
    }
    let mut submodules = Vec::new();
    for (relative, submodule) in &judged.submodules {
        let shown = format!("the submodule {relative} of {shown}");
        let below = regenerable_below(regenerable, relative);
        let found = worktree_files(None, submodule, &below)?;
        lines.extend(
            WorktreeFiles {
                uncommitted: found.uncommitted.clone(),
                ignored: match saved {
                    Saved::Nothing => found.ignored.clone(),
                    _ => Vec::new(),
                },
                ..WorktreeFiles::default()
            }
            .describe(&shown),
        );
        submodules.push((relative.clone(), found));
    }
    Ok(WorkFound {
        lines,
        files,
        submodules,
    })
}

/// Which commits of a submodule repository its removal may lose (KT-2084).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmoduleCommits {
    /// HEAD, its branches and its stash, minus what remote branches and tags reach: a
    /// discussion's and a task's.
    OffRemotesAndTags,
    /// HEAD and every ref but a remote branch, minus what remote branches reach: a run's
    /// (KT-1809), whose stash and local tags count.
    OffRemotes,
}

/// The commits of the worktree's submodule repositories, de-initialised ones included,
/// that `rule` counts and the project checkout's own submodule does not hold (KT-2084):
/// removing them deletes those commits, and nothing proves them saved or lost without the
/// network.
fn submodule_commits_off_remotes(
    repo_path: &Path,
    judged: &Judgement,
    rule: SubmoduleCommits,
) -> Result<Vec<String>, String> {
    let reaches = match rule {
        SubmoduleCommits::OffRemotesAndTags => "no remote branch or tag reaches",
        SubmoduleCommits::OffRemotes => "no remote branch reaches",
    };
    let mut found: Vec<String> = Vec::new();
    for (relative, submodule) in &judged.submodules {
        // The reason says whether the project's own copy was read, and why not (KT-2084).
        let (saved, source) = match tips_held_by_source(submodule, repo_path, relative)? {
            Ok(saved) => (
                saved,
                "the project's own copy of that submodule does not hold it either".to_string(),
            ),
            Err(unread) => (
                Vec::new(),
                format!("the project's own copy of that submodule was not checked: {unread}"),
            ),
        };
        if let Some(commit) = commit_off_remotes(submodule, &saved, rule)? {
            found.push(format!(
                "the submodule {relative} holds commit {commit} that {reaches} ({source})"
            ));
        }
    }
    let admin = judged.worktree.source();
    let modules = admin.join("modules");
    for git_dir in module_git_dirs(admin)? {
        let module = Snapshot::of_module(&git_dir, &modules, None)?;
        if let Some(commit) = commit_off_remotes(&module, &[], rule)? {
            if !found.iter().any(|line| line.contains(&commit)) {
                found.push(format!(
                    "the submodule repository {} holds commit {commit} that {reaches}",
                    git_dir.display()
                ));
            }
        }
    }
    Ok(found)
}

/// What removing the task checkout `checkout` with everything in it would lose (KT-2084):
/// its work as [`unsaved_work`] judges it, a repository in Kronn's own directory, and
/// submodule commits no remote reaches, but what `saved` saves first (see
/// [`release_terminal_task_worktree`] and [`remove_cancelled_task_worktree`]).
pub fn forced_removal_loss(
    repo_path: &Path,
    checkout: &Path,
    shown: &str,
    saved: Saved,
) -> Result<Option<String>, String> {
    let judged = Judgement::of(checkout)?;
    forced_removal_check(repo_path, &judged, shown, saved).map(|(loss, _)| loss)
}

/// [`forced_removal_loss`] of the copy `judged`, and what it found.
fn forced_removal_check(
    repo_path: &Path,
    judged: &Judgement,
    shown: &str,
    saved: Saved,
) -> Result<(Option<String>, WorkFound), String> {
    let regenerable = regenerable_outputs();
    let mut found = work_found(repo_path, judged, shown, &regenerable, saved)?;
    if !found.files.nested.is_empty() {
        let line = format!(
            "{} in {shown} hold(s) a repository: move it out first",
            name_files(&found.files.nested)
        );
        found.lines.push(line);
    }
    found.lines.extend(
        submodule_commits_off_remotes(repo_path, judged, SubmoduleCommits::OffRemotesAndTags)?
            .into_iter()
            .map(|line| format!("{line}: push it or discard it first")),
    );
    let loss = joined(std::mem::take(&mut found.lines));
    Ok((loss, found))
}

/// An untracked or ignored directory of a worktree holds nothing but Kronn's
/// agent-config copies and OS junk, at any depth.
fn holds_only_kronn_files(worktree: &Path, dir: &str, copies: &[String], junk: &[String]) -> bool {
    let mut pending = vec![dir.trim_end_matches('/').to_string()];
    while let Some(relative) = pending.pop() {
        let Some(entries) = listed(worktree, &worktree.join(&relative)) else {
            return false;
        };
        for (name, kind) in entries {
            let path = format!("{relative}/{}", name.to_string_lossy());
            if kind.is_dir() {
                pending.push(path);
            } else if !(copies.contains(&path) || is_os_junk(&path, junk)) {
                return false;
            }
        }
    }
    true
}

/// A commit of `branch` that no other branch, tag or remote branch reaches, if
/// any (KT-2084): what deleting the branch, or forgetting it, would lose.
pub fn commit_unique_to_branch(repo_path: &Path, branch: &str) -> Result<Option<String>, String> {
    commit_only_on_branch(repo_path, branch)
}

/// A commit of `branch` that no other branch, tag or remote branch reaches, if any.
/// A stash, a detached HEAD or a backup ref goes away at will: none keeps a commit.
fn commit_only_on_branch(repo_path: &Path, branch: &str) -> Result<Option<String>, String> {
    reject_option_like_rev(branch)?;
    // `--exclude` before `--branches` takes the name without `refs/heads/`.
    let mut command = delete_git(repo_path)?;
    command
        .args([
            "rev-list",
            "-n",
            "1",
            &format!("refs/heads/{branch}"),
            "--not",
        ])
        .arg(format!("--exclude={branch}"))
        .args(["--branches", "--tags", "--remotes"]);
    let output = bounded(command, "rev-list")?;
    if !output.status.success() {
        return Err(format!(
            "the commits of {branch} cannot be read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!commit.is_empty()).then_some(commit))
}

// ── Task-execution worktree provisioning (KT-318) ─────────────────────────────
//
// The provisioning saga needs a stronger contract than `create_discussion_worktree`:
// a checkout pinned to an EXACT commit (not a mutable branch), a deterministic
// per-execution branch that never silently reuses a foreign/stale checkout, and a
// compensation that removes ONLY resources this attempt created — never work.

/// Read the current HEAD commit SHA of a git dir (repo or worktree).
fn git_head(dir: &Path) -> Result<String, String> {
    let out = crate::core::lane_git::git(dir)?
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("git rev-parse HEAD failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Resolve a local branch to its commit SHA, or `None` when it does not exist.
fn branch_commit(repo_path: &Path, branch: &str) -> Option<String> {
    let mut command = crate::core::cmd::git_cmd();
    command
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_path);
    let out = bounded(command, "rev-parse").ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// Refuse a revision git would read as an option.
///
/// `base_rev` arrives from the API and reaches git as a bare argument. On
/// `rev-parse` a leading `-` only fails, but the same string is destined for
/// commands that write, where an argument parsed as a flag acts instead of
/// naming a commit. One chokepoint, applied before any of them.
pub fn reject_option_like_rev(rev: &str) -> Result<(), String> {
    let trimmed = rev.trim();
    if trimmed.is_empty() {
        return Err("empty revision".into());
    }
    if trimmed.starts_with('-') {
        return Err(format!("refusing revision '{rev}': starts with '-'"));
    }
    Ok(())
}

/// Resolve a revision (branch, tag, sha) to a full commit SHA.
///
/// Used to PIN a mutable base branch to an exact commit before deriving a task
/// worktree, so a concurrent push to that branch cannot change what the worker
/// builds on. Returns an error the saga surfaces as a refusal (never a guess).
pub fn resolve_commit(repo_path: &Path, rev: &str) -> Result<String, String> {
    reject_option_like_rev(rev)?;
    // A worker's worktree is pinned and its pointer checked (KT-1745): git never
    // discovers the repository, and so never fetches lazily or runs a program,
    // from what the worker wrote. The owner's checkout keeps plain git.
    // Within a time bound (KT-2084), the revision never an option.
    let mut command = crate::core::lane_git::git_keeping_owner_hooks(repo_path)?;
    command.args([
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{rev}^{{commit}}"),
    ]);
    let out = bounded(command, "rev-parse").map_err(|e| format!("git rev-parse failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot resolve '{rev}' to a commit: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Resolve an integration target to a local branch name.
///
/// A task child may be pinned from any commit internally, but the parent side of
/// a two-phase integration must remain a movable, observable ref. Accepting a
/// tag or raw SHA here would make later drift checks keep resolving the same
/// immutable object while the checked-out parent branch moves underneath it.
pub fn resolve_local_branch(repo_path: &Path, rev: &str) -> Result<String, String> {
    reject_option_like_rev(rev)?;
    let trimmed = rev.trim();
    let branch = trimmed.strip_prefix("refs/heads/").unwrap_or(trimmed);
    let full_ref = format!("refs/heads/{branch}");
    let out = crate::core::cmd::git_cmd()
        .args(["show-ref", "--verify", "--quiet", &full_ref])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git show-ref failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "integration target '{rev}' is not a local branch; use a branch name, not a tag or SHA"
        ));
    }
    Ok(branch.to_string())
}

/// Every worktree that has the local branch `target` checked out, with the
/// resolved branch name. Integration needs exactly one of them.
pub fn target_branch_checkouts(
    repo_path: &Path,
    target: &str,
) -> Result<(String, Vec<PathBuf>), String> {
    let branch = resolve_local_branch(repo_path, target)?;
    let branch_field = format!("branch refs/heads/{branch}");
    // Within a time bound: it reads every registration (KT-2084).
    let mut list = crate::core::cmd::git_cmd();
    list.args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(repo_path);
    let output = bounded(list, "worktree list")
        .map_err(|error| format!("cannot list target worktrees: {error}"))?;
    if !output.status.success() {
        return Err("cannot list target worktrees".into());
    }
    let listing = std::str::from_utf8(&output.stdout)
        .map_err(|_| "target worktree paths are not valid UTF-8")?;
    let mut current_path = None;
    let mut matches = Vec::new();
    for field in listing.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if field == branch_field {
            if let Some(path) = current_path.take() {
                matches.push(path);
            }
        } else if field.is_empty() {
            current_path = None;
        }
    }
    Ok((branch, matches))
}

/// Resolve the checkout to mutate, not merely the repository containing its ref.
pub fn integration_target_worktree(repo_path: &Path, target: &str) -> Result<PathBuf, String> {
    let (branch, matches) = target_branch_checkouts(repo_path, target)?;
    let [path] = matches.as_slice() else {
        return Err(format!(
            "integration target '{branch}' must have exactly one checked-out worktree (found {})",
            matches.len()
        ));
    };
    let path = path
        .canonicalize()
        .map_err(|error| format!("integration target worktree is unavailable: {error}"))?;
    // The target checkout may be a Kronn-managed worktree: pinned there.
    let head = crate::core::lane_git::git_keeping_owner_hooks(&path)?
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .map_err(|error| format!("cannot verify target checkout branch: {error}"))?;
    if !head.status.success()
        || String::from_utf8_lossy(&head.stdout).trim() != format!("refs/heads/{branch}")
    {
        return Err("integration target checkout changed branch; refusing to mutate it".into());
    }
    let common_dir = |directory: &Path| -> Result<PathBuf, String> {
        let output = crate::core::lane_git::git_keeping_owner_hooks(directory)?
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .output()
            .map_err(|error| format!("cannot verify target repository: {error}"))?;
        if !output.status.success() {
            return Err("cannot verify target repository".into());
        }
        let value = std::str::from_utf8(&output.stdout)
            .map_err(|_| "target repository path is not valid UTF-8")?
            .trim_end_matches(['\r', '\n']);
        Path::new(value)
            .canonicalize()
            .map_err(|error| error.to_string())
    };
    if common_dir(repo_path)? != common_dir(&path)? {
        return Err("integration target checkout belongs to another repository".into());
    }
    Ok(path)
}

/// Outcome of building the integration candidate (phase 1 of `TwoPhaseFfOnly`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateOutcome {
    /// The child branch now descends from the pinned parent tip; the parent can be
    /// fast-forwarded onto `sha`.
    Built { sha: String },
    /// A hook the project's worker commit policy runs refused the merge commit
    /// (KT-1745), or could not be run safely. Nothing was committed and the merge
    /// was undone; this is not a conflict and no file is named.
    HookRefused { hook: String, output: String },
    /// The child cannot absorb the parent tip without a human decision. Nothing was
    /// left half-merged: the merge is aborted, so the worker keeps a usable worktree.
    Conflict { files: Vec<String> },
}

/// Write `refs/kronn-backup/<slug>` at `sha` and read it back.
///
/// This ref is what the parent branch comes back to if the apply goes wrong, so
/// writing it is not enough — an unverified backup is the same as none, and the
/// saga would arm an apply believing it had a way back.
pub fn write_backup_ref(repo_path: &Path, slug: &str, sha: &str) -> Result<String, String> {
    reject_option_like_rev(sha)?;
    if slug.is_empty() || slug.contains("..") || slug.contains(' ') || slug.starts_with('-') {
        return Err(format!("refusing backup slug '{slug}'"));
    }
    let full = format!("refs/kronn-backup/{slug}");
    let out = crate::core::cmd::git_cmd()
        .args(["update-ref", &full, sha])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git update-ref failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot write backup ref '{full}': {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let read_back = resolve_commit(repo_path, &full)?;
    if read_back != sha {
        return Err(format!(
            "backup ref '{full}' reads {read_back}, expected {sha}"
        ));
    }
    Ok(full)
}

/// Phase 1 of `TwoPhaseFfOnly`: bring the pinned parent tip INTO the child branch,
/// inside the child's own worktree.
///
/// The direction matters. Merging the parent into the child leaves the parent
/// untouched and puts any conflict where the worker can resolve it; the reverse
/// would resolve someone else's conflict inside the shared branch. Once this
/// succeeds the child descends from the parent tip, which is what lets phase 2 be
/// a fast-forward and never a merge.
pub fn build_candidate(worktree_path: &Path, base_sha: &str) -> Result<CandidateOutcome, String> {
    build_candidate_detailed(worktree_path, base_sha).map(|build| build.outcome)
}

/// [`build_candidate_detailed`] under the default worker commit hook policy
/// (no hook runs).
pub fn build_candidate_detailed(
    worktree_path: &Path,
    base_sha: &str,
) -> Result<CandidateBuild, String> {
    build_candidate_with_hooks(worktree_path, base_sha, WorkerCommitHooks::default())
}

/// A built candidate with what the merge settled or why it bounced (KT-1669).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateBuild {
    pub outcome: CandidateOutcome,
    /// Set when the merge settled a citation-only doc conflict itself.
    pub settled: Option<crate::core::docs_citations::merge::Settlement>,
    /// Set when the bounce is about doc citations: the sentence the worker
    /// reads, with the exact citations in it.
    pub detail: Option<String>,
}

impl CandidateBuild {
    fn of(outcome: CandidateOutcome) -> Self {
        Self {
            outcome,
            settled: None,
            detail: None,
        }
    }

    fn bounced_on_docs(files: Vec<String>, detail: String) -> Self {
        Self {
            outcome: CandidateOutcome::Conflict { files },
            settled: None,
            detail: Some(detail),
        }
    }
}

/// [`build_candidate`], with what the doc settling did. A conflict that is only
/// citation line anchors never reaches the worker: the target's side is taken,
/// the citations are re-anchored onto the merge, and every citation of those
/// docs must resolve, else the merge is undone and the bounce lists them.
///
/// KT-1745 — the merge runs in the worker's worktree but in Kronn's process, as
/// the host user: no git command here resolves a hook inside the worktree (a
/// husky `.husky/_` is worker-written). The merge commit's gating hook runs only
/// as `hooks` allows, and its refusal is [`CandidateOutcome::HookRefused`], never
/// an empty conflict.
pub fn build_candidate_with_hooks(
    worktree_path: &Path,
    base_sha: &str,
    hooks: WorkerCommitHooks,
) -> Result<CandidateBuild, String> {
    reject_option_like_rev(base_sha)?;
    // The merge stops before its commit so the doc citations the two sides
    // shifted are settled into it (KT-1321) and the commit is made once.
    // A fast-forward does run `post-merge`, hence the hooks override here too.
    let out = crate::core::lane_git::git(worktree_path)?
        .args(["merge", "--no-edit", "--no-commit", base_sha])
        .output()
        .map_err(|e| format!("git merge failed: {e}"))?;
    if !crate::core::docs_citations::merge::merge_in_progress(worktree_path) {
        // Fast-forwarded, already up to date, or refused before any merge state.
        if out.status.success() {
            return settle_branch_docs(worktree_path, base_sha, hooks);
        }
        // The merge was refused before it started (a dirty tree, an unrelated
        // history…): there are no conflicting files to name, so this is not a
        // `Conflict { files: [] }` the worker can only loop on — what git said
        // travels as the error.
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        tracing::warn!(
            "git merge refused in {}: {}",
            worktree_path.display(),
            said.trim()
        );
        return Err(format!(
            "git merge of {base_sha} was refused before it started: {}",
            crate::core::commit_hooks::first_lines(&said)
        ));
    }
    let conflicted_files = |path: &Path| {
        crate::core::lane_git::git(path)
            .ok()
            .and_then(|mut git| {
                git.args([
                    "diff",
                    "--ignore-submodules=dirty",
                    "--name-only",
                    "--diff-filter=U",
                    "--no-ext-diff",
                ])
                .output()
                .ok()
            })
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    // A conflict on nothing but citation line anchors is not a human decision.
    let resolved = crate::core::docs_citations::merge::resolve_anchor_only_conflicts(worktree_path)
        .unwrap_or_else(|error| {
            tracing::warn!("anchor-only doc conflicts left to the worker: {error}");
            Vec::new()
        });
    let files = conflicted_files(worktree_path);
    if !files.is_empty() {
        // Leave no half-merged tree behind: the worker must find its worktree as
        // it left it, not mid-conflict from a merge it never asked for.
        abort_merge(worktree_path);
        return Ok(CandidateBuild::of(CandidateOutcome::Conflict { files }));
    }
    use crate::core::docs_citations::merge::SettleError;
    let docs = match crate::core::docs_citations::merge::reanchor_and_stage(worktree_path, resolved)
    {
        Ok(docs) => Some(docs),
        Err(SettleError::Failed(error)) => {
            tracing::warn!("doc citations not re-anchored by the merge: {error}");
            None
        }
        // Unstaged work in a doc to rewrite, or a citation nobody can place
        // safely: a human decides, as for any other conflict.
        Err(
            SettleError::Refused {
                paths: files,
                message,
            }
            | SettleError::Unplaced {
                docs: files,
                message,
            },
        ) => {
            tracing::warn!("doc citations left to the worker: {files:?}");
            abort_merge(worktree_path);
            return Ok(CandidateBuild::bounced_on_docs(files, message));
        }
    };
    let settled = docs.as_ref().and_then(|docs| docs.settlement());
    // Conflict or not, the merge must not leave a citation unresolved that
    // resolved on the target; the worker gets the exact list otherwise.
    if let Some(bounce) = unresolved_bounce(worktree_path, base_sha) {
        abort_merge(worktree_path);
        return Ok(bounce);
    }
    // Repositories that enforce DCO reject an unsigned merge commit: the gating
    // hook runs under the project's policy (KT-1745), and the commit is signed
    // only where the project signs off, as the project's identity (KT-1750).
    if let Err(refusal) =
        crate::core::commit_hooks::run_gating_hook(worktree_path, hooks, "pre-merge-commit")
    {
        abort_merge(worktree_path);
        return Ok(CandidateBuild::of(CandidateOutcome::HookRefused {
            hook: refusal.hook,
            output: refusal.output,
        }));
    }
    let policy = crate::core::commit_policy::CommitPolicy::for_repo(worktree_path);
    let mut commit = crate::core::lane_git::git(worktree_path)?;
    commit.arg("commit");
    if policy.signoff {
        commit.arg("--signoff");
    }
    commit.args(["--no-gpg-sign", "--cleanup=strip"]);
    policy.apply_identity(&mut commit, false);
    match docs.as_ref().and_then(|docs| docs.summary()) {
        Some(note) => commit.args(["-m", &merge_subject(worktree_path, base_sha), "-m", &note]),
        None => commit.arg("--no-edit"),
    };
    let committed = commit
        .output()
        .map_err(|e| format!("git commit failed: {e}"))?;
    if !committed.status.success() {
        // Not a conflict: the tree merged cleanly, and no hook ran in this git
        // command. Say what git printed rather than hand the worker an empty
        // conflict it can only loop on.
        let stderr = String::from_utf8_lossy(&committed.stderr).into_owned();
        abort_merge(worktree_path);
        return Err(format!(
            "cannot commit the candidate merge: {}",
            crate::core::commit_hooks::first_lines(&stderr)
        ));
    }
    Ok(CandidateBuild {
        outcome: CandidateOutcome::Built {
            sha: git_head(worktree_path)?,
        },
        settled,
        detail: None,
    })
}

/// The candidate leaves a doc citation unresolved that resolved on the target:
/// the bounce the worker reads, with the exact citations. `None` when the docs
/// gate has nothing new to reject (a citation already broken on the target never
/// blocks a merge) or the comparison could not be made.
fn unresolved_bounce(worktree_path: &Path, target: &str) -> Option<CandidateBuild> {
    let newly = match crate::core::docs_citations::merge::newly_unresolved(worktree_path, target) {
        Ok(newly) => newly,
        Err(error) => {
            tracing::warn!("doc citations not compared with the target: {error}");
            return None;
        }
    };
    if newly.is_empty() {
        return None;
    }
    let mut files: Vec<String> = newly.iter().map(|(doc, _)| doc.clone()).collect();
    files.dedup();
    let listed: Vec<&str> = newly.iter().map(|(_, line)| line.as_str()).collect();
    tracing::warn!("candidate leaves unresolved citations: {files:?}");
    Some(CandidateBuild::bounced_on_docs(
        files,
        format!(
            "these citations resolved on the target and do not in the merged tree, after the \
             merge re-anchored what it safely could: {}. Point each at its new place by hand.",
            listed.join("; ")
        ),
    ))
}

/// The branch already descends from the target (fast-forwarded, up to date, or
/// merged by its worker): no merge commit carries the re-anchoring, so the
/// citations the branch's own code shift moved are re-anchored into a commit of
/// their own, under the same rules as a merge (KT-1669).
fn settle_branch_docs(
    worktree_path: &Path,
    base_sha: &str,
    hooks: WorkerCommitHooks,
) -> Result<CandidateBuild, String> {
    use crate::core::docs_citations::merge::{self, SettleError};
    let head = git_head(worktree_path)?;
    if head == base_sha {
        return Ok(CandidateBuild::of(CandidateOutcome::Built { sha: head }));
    }
    let docs = match merge::reanchor_branch_and_stage(worktree_path, base_sha) {
        Ok(docs) => Some(docs),
        Err(SettleError::Failed(error)) => {
            tracing::warn!("doc citations not re-anchored on the branch: {error}");
            None
        }
        Err(
            SettleError::Refused {
                paths: files,
                message,
            }
            | SettleError::Unplaced {
                docs: files,
                message,
            },
        ) => {
            tracing::warn!("doc citations left to the worker: {files:?}");
            return Ok(CandidateBuild::bounced_on_docs(files, message));
        }
    };
    if let Some(bounce) = unresolved_bounce(worktree_path, base_sha) {
        if let Some(docs) = &docs {
            if let Err(error) = merge::undo_staged(worktree_path, docs) {
                tracing::warn!("re-anchored docs not restored: {error}");
            }
        }
        return Ok(bounce);
    }
    let settled = docs.as_ref().and_then(|docs| docs.settlement());
    if let Some(note) = docs.as_ref().and_then(|docs| docs.summary()) {
        if let Err(refusal) =
            crate::core::commit_hooks::run_gating_hook(worktree_path, hooks, "pre-commit")
        {
            if let Some(docs) = &docs {
                if let Err(error) = merge::undo_staged(worktree_path, docs) {
                    tracing::warn!("re-anchored docs not restored: {error}");
                }
            }
            return Ok(CandidateBuild::of(CandidateOutcome::HookRefused {
                hook: refusal.hook,
                output: refusal.output,
            }));
        }
        let policy = crate::core::commit_policy::CommitPolicy::for_repo(worktree_path);
        let mut commit = crate::core::lane_git::git(worktree_path)?;
        commit.arg("commit");
        if policy.signoff {
            commit.arg("--signoff");
        }
        policy.apply_identity(&mut commit, false);
        let committed = commit
            .args([
                "--no-gpg-sign",
                "--cleanup=strip",
                "-m",
                "Re-anchor doc citations moved by the branch's code",
                "-m",
                &note,
            ])
            .output()
            .map_err(|e| format!("git commit failed: {e}"))?;
        if !committed.status.success() {
            return Err(format!(
                "cannot commit the re-anchored docs: {}",
                String::from_utf8_lossy(&committed.stderr).trim()
            ));
        }
    }
    Ok(CandidateBuild {
        outcome: CandidateOutcome::Built {
            sha: git_head(worktree_path)?,
        },
        settled,
        detail: None,
    })
}

/// Undo the merge in progress. Runs through the lane builder: `git merge --abort`
/// fires `reference-transaction`, and a hook of the worktree is the worker's.
fn abort_merge(worktree_path: &Path) {
    if let Ok(mut git) = crate::core::lane_git::git(worktree_path) {
        let _ = git.args(["merge", "--abort"]).output();
    }
}

/// The subject git prepared for the merge in progress.
fn merge_subject(worktree_path: &Path, base_sha: &str) -> String {
    let prepared = crate::core::lane_git::git(worktree_path)
        .ok()
        .and_then(|mut git| {
            git.args(["rev-parse", "--git-path", "MERGE_MSG"])
                .output()
                .ok()
        })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|path| worktree_path.join(path.trim()))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.lines().next().map(str::to_string))
        .filter(|subject| !subject.trim().is_empty());
    prepared.unwrap_or_else(|| format!("Merge commit '{base_sha}'"))
}

/// Phase 2 of `TwoPhaseFfOnly`: advance the parent onto the validated candidate.
///
/// `--ff-only` is the guarantee, not a preference: if the parent moved to anything
/// the candidate does not descend from, git refuses and nothing happens. A stale
/// candidate can therefore never be forced over work that landed meanwhile.
pub fn fast_forward_to(repo_path: &Path, candidate_sha: &str) -> Result<String, String> {
    reject_option_like_rev(candidate_sha)?;
    // The target checkout may itself be a Kronn-managed worktree (a branch a task
    // worker also writes): there it is the pinned, hook-less lane command; in the
    // owner's own checkout their hooks keep running.
    let out = crate::core::lane_git::git_keeping_owner_hooks(repo_path)?
        .args(["merge", "--ff-only", candidate_sha])
        .output()
        .map_err(|e| format!("git merge --ff-only failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot fast-forward onto {candidate_sha}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    git_head(repo_path)
}

/// How the integration target branch is held in the repository.
///
/// The saga used to require the checkout. A target checked out nowhere has no
/// working tree or index to protect, so it can be advanced by a ref
/// compare-and-swap; one checked out exactly once keeps today's guarded
/// fast-forward inside that checkout (moving a ref under a checkout would leave
/// its index and files stale). Several checkouts are still refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationTarget {
    /// Checked out in exactly one worktree (validated by `integration_target_worktree`).
    CheckedOut(PathBuf),
    /// A local branch that no worktree has checked out.
    NotCheckedOut { branch: String },
}

/// Resolve the shape of an integration target, refusing several checkouts.
pub fn integration_target(repo_path: &Path, target: &str) -> Result<IntegrationTarget, String> {
    let (branch, matches) = target_branch_checkouts(repo_path, target)?;
    if matches.is_empty() {
        return Ok(IntegrationTarget::NotCheckedOut { branch });
    }
    integration_target_worktree(repo_path, target).map(IntegrationTarget::CheckedOut)
}

/// The real tip of the target branch, read from its ref rather than from any
/// checkout's HEAD (identical for a checked-out branch, and the only source
/// when nothing has it checked out).
pub fn integration_target_tip(repo_path: &Path, target: &str) -> Result<String, String> {
    let branch = resolve_local_branch(repo_path, target)?;
    resolve_commit(repo_path, &format!("refs/heads/{branch}"))
}

/// Why a launch's integration target is refused before anything is created
/// (KT-1688). Integration advances the target branch: when a checkout holds it,
/// that checkout is written to, so it must be one Kronn owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationTargetRefusal {
    /// No `base_rev` was given and the room declares no integration branch.
    /// Defaulting to `main` would aim the integration at the owner's checkout.
    NoneDeclared,
    /// The branch is checked out in the project's own working checkout.
    OwnerCheckout { branch: String, path: PathBuf },
    /// The branch is checked out in a worktree that is under no Kronn-managed
    /// root and that Kronn has not recorded as a workspace.
    ForeignWorktree { branch: String, path: PathBuf },
    /// The branch is checked out in a Kronn-managed worktree a run or a live agent
    /// is using right now; advancing it would move files under that run. `holder`
    /// names the run. The next integration attempt checks again.
    BusyWorktree {
        branch: String,
        path: PathBuf,
        holder: String,
    },
}

impl IntegrationTargetRefusal {
    /// Stable machine code, the prefix of the refusal text.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoneDeclared => "integration_target_undeclared",
            Self::OwnerCheckout { .. } => "integration_target_owner_checkout",
            Self::ForeignWorktree { .. } => "integration_target_foreign_worktree",
            Self::BusyWorktree { .. } => "integration_target_busy_worktree",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::NoneDeclared => "no `base_rev` was given and the room declares no integration \
                branch; name the integration branch explicitly (a campaign room declares it), \
                Kronn never defaults to `main`"
                .to_string(),
            Self::OwnerCheckout { branch, path } => format!(
                "`{branch}` is checked out in the owner's working checkout ({}); integrating \
                 there would write into files the owner is working on and move `{branch}` past \
                 its review gate. Target an integration branch no checkout holds, or one held by \
                 a Kronn-owned worktree",
                path.display()
            ),
            Self::ForeignWorktree { branch, path } => format!(
                "`{branch}` is checked out in {}, a worktree Kronn does not own (Kronn-owned \
                 worktrees live under <repo>/.kronn/worktrees, ~/.kronn/autocode-worktrees or a \
                 managed root listed in Settings > Workers or in the project's workspace \
                 settings, or are recorded as a Kronn workspace); integration never writes \
                 into it",
                path.display()
            ),
            Self::BusyWorktree {
                branch,
                path,
                holder,
            } => format!(
                "`{branch}` is checked out in {}, a Kronn-managed worktree in use right now \
                 ({holder}); integrating would move its files under that run. Nothing was \
                 changed: integrate again once it has finished",
                path.display()
            ),
        }
    }

    /// The refusal as shown to the caller: `<code>: <detail>`.
    pub fn message(&self) -> String {
        format!("{}: {}", self.code(), self.detail())
    }
}

/// Directory, under Kronn's home (`~/.kronn`), where autoCode creates its worktrees.
const AUTOCODE_WORKTREES_DIR: &str = "autocode-worktrees";

/// Managed worktree roots of Settings > Workers, published by `config::save` and at
/// boot (the same way the worker shell limits are), so the launch, status and
/// integration paths read them without each carrying the config.
static GLOBAL_MANAGED_ROOTS: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());

/// Publish the owner's Settings > Workers managed roots to this process.
pub fn set_global_managed_worktree_roots(workers: &crate::models::WorkersConfig) {
    let roots = crate::models::effective_managed_worktree_roots(&workers.managed_worktree_roots);
    *GLOBAL_MANAGED_ROOTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = roots;
}

fn global_managed_worktree_roots() -> Vec<PathBuf> {
    GLOBAL_MANAGED_ROOTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The directories whose worktrees are Kronn's own, for one repository: its lane
/// root (`<repo>/.kronn/worktrees`), autoCode's root under `~/.kronn`, and the
/// absolute roots the owner configured (global and per project).
pub fn managed_worktree_roots(
    repo_path: &Path,
    kronn_home: Option<&Path>,
    extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut roots = vec![worktree_base_dir(repo_path)];
    if let Some(home) = kronn_home {
        roots.push(home.join(AUTOCODE_WORKTREES_DIR));
    }
    roots.extend(extra.iter().filter(|root| root.is_absolute()).cloned());
    roots
}

/// [`managed_worktree_roots`] for this machine: `~/.kronn`, the global roots of
/// Settings > Workers and the project's own `project_roots`.
fn host_managed_worktree_roots(repo_path: &Path, project_roots: &[PathBuf]) -> Vec<PathBuf> {
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".kronn"));
    let mut extra = global_managed_worktree_roots();
    extra.extend(project_roots.iter().cloned());
    managed_worktree_roots(repo_path, home.as_deref(), &extra)
}

/// Refuse an integration target whose checkout is not Kronn's. A branch checked
/// out nowhere is advanced by a ref compare-and-swap that touches no checkout;
/// one checked out in a Kronn-managed worktree is advanced inside it.
///
/// A checkout is Kronn's when it sits under a managed root (task lanes, autoCode
/// worktrees, the roots of Settings > Workers and the project's `project_roots`)
/// or when `registered` says Kronn recorded it as a workspace (workflow runs,
/// managed discussion workspaces). The owner's own checkout is refused first,
/// whatever the registry says.
pub fn integration_target_ownership(
    repo_path: &Path,
    branch: &str,
    project_roots: &[PathBuf],
    registered: &dyn Fn(&Path) -> bool,
) -> Result<Result<(), IntegrationTargetRefusal>, String> {
    #[cfg(any(test, feature = "test-support"))]
    if !ownership_seam::enforced(repo_path) {
        return Ok(Ok(()));
    }
    integration_target_ownership_in(
        repo_path,
        branch,
        &host_managed_worktree_roots(repo_path, project_roots),
        registered,
    )
}

/// [`integration_target_ownership`] against explicit managed roots.
pub fn integration_target_ownership_in(
    repo_path: &Path,
    branch: &str,
    managed_roots: &[PathBuf],
    registered: &dyn Fn(&Path) -> bool,
) -> Result<Result<(), IntegrationTargetRefusal>, String> {
    let (branch, checkouts) = target_branch_checkouts(repo_path, branch)?;
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let owner = canonical(repo_path);
    let roots: Vec<PathBuf> = managed_roots.iter().map(|root| canonical(root)).collect();
    for path in checkouts {
        let resolved = canonical(&path);
        if resolved == owner {
            return Ok(Err(IntegrationTargetRefusal::OwnerCheckout {
                branch,
                path,
            }));
        }
        let managed = roots.iter().any(|root| resolved.starts_with(root))
            || registered(&resolved)
            || registered(&path);
        if !managed {
            return Ok(Err(IntegrationTargetRefusal::ForeignWorktree {
                branch,
                path,
            }));
        }
    }
    Ok(Ok(()))
}

/// Refuse to advance a target held by a Kronn-managed worktree that a run or a
/// live agent is using right now (KT-1748). `holder` answers, for one checkout
/// path, who is using it (`None`: idle). A target no worktree holds is advanced by
/// a ref compare-and-swap that touches no checkout, so it is never busy. Nothing
/// is changed on refusal: the next integration attempt asks again.
pub fn integration_target_idle(
    repo_path: &Path,
    target: &str,
    holder: &dyn Fn(&Path) -> Option<String>,
) -> Result<Result<(), IntegrationTargetRefusal>, String> {
    let (branch, checkouts) = target_branch_checkouts(repo_path, target)?;
    for path in checkouts {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.clone());
        if let Some(holder) = holder(&resolved).or_else(|| holder(&path)) {
            return Ok(Err(IntegrationTargetRefusal::BusyWorktree {
                branch,
                path,
                holder,
            }));
        }
    }
    Ok(Ok(()))
}

/// Test seam. The lib's launch tests drive whole sagas against throwaway repos
/// whose own working checkout holds `main`, which is exactly what production
/// refuses as an integration target. Those repos are exempt; a test that
/// exercises the refusal opts its repo in with [`ownership_seam::enforce_for`].
/// Keyed by repository path, so concurrent tests never see each other's choice.
fn apply_config(config: &crate::models::AppConfig) {
    set_disk_thresholds(
        config.server.disk_warning_gib,
        config.server.disk_critical_gib,
    );
    // KT-1750 — Settings > Workers: the OS junk files that never count as work.
    set_os_junk_patterns(&config.workers.os_junk_patterns);
    // KT-2084 — what a discussion delete never counts as work.
    set_regenerable_outputs(&config.server.regenerable_outputs);
    // KT-1748 — Settings > Workers: the extra roots of Kronn-managed worktrees.
    set_global_managed_worktree_roots(&config.workers);
    // KT-1718 — Settings > Workers: the shell limits of a Claude task worker.
    crate::core::task_worker_env::set_bash_timeouts(&config.workers);
    // KT-1750 — Settings > Workers: the commit identity and sign-off projects inherit.
    crate::core::commit_policy::set_global_settings(&config.workers.commit);
    // KT-1742 — Settings > Workers: how every project's lane is warmed.
    crate::core::task_worker_lane_settings::set_global(&config.workers.lane_warm);
}
inventory::submit! { crate::core::uplink::ConfigApplied(apply_config) }

#[cfg(any(test, feature = "test-support"))]
pub mod ownership_seam {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static ENFORCED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    fn key(repo: &Path) -> PathBuf {
        repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf())
    }

    pub fn enforce_for(repo: &Path) {
        ENFORCED.lock().unwrap().push(key(repo));
    }

    pub(super) fn enforced(repo: &Path) -> bool {
        ENFORCED.lock().unwrap().contains(&key(repo))
    }
}

/// Who holds `branch` and where, for the status a principal reads before any
/// integration. Never fails: what cannot be read is `Unresolved`.
pub fn describe_integration_target(
    repo_path: &Path,
    branch: &str,
    project_roots: &[PathBuf],
    registered: &dyn Fn(&Path) -> bool,
) -> crate::models::TaskExecutionIntegrationTarget {
    use crate::models::IntegrationTargetState as State;
    let unresolved = || crate::models::TaskExecutionIntegrationTarget {
        branch: branch.to_string(),
        worktree: None,
        state: State::Unresolved,
    };
    let Ok((resolved, checkouts)) = target_branch_checkouts(repo_path, branch) else {
        return unresolved();
    };
    let Some(first) = checkouts.first() else {
        return crate::models::TaskExecutionIntegrationTarget {
            branch: resolved,
            worktree: None,
            state: State::NotCheckedOut,
        };
    };
    // A refused checkout is the one to show: it is the path a wrong target
    // would have written into.
    let (state, path) =
        match integration_target_ownership(repo_path, branch, project_roots, registered) {
            Ok(Ok(())) => (State::KronnWorktree, first.clone()),
            Ok(Err(IntegrationTargetRefusal::OwnerCheckout { path, .. })) => {
                (State::OwnerCheckout, path)
            }
            Ok(Err(IntegrationTargetRefusal::ForeignWorktree { path, .. })) => {
                (State::ForeignWorktree, path)
            }
            Ok(Err(
                IntegrationTargetRefusal::NoneDeclared
                | IntegrationTargetRefusal::BusyWorktree { .. },
            ))
            | Err(_) => return unresolved(),
        };
    crate::models::TaskExecutionIntegrationTarget {
        branch: resolved,
        worktree: Some(path.display().to_string()),
        state,
    }
}

/// Uncommitted files that would be endangered by advancing the target. A target
/// no worktree has checked out has none.
pub fn integration_target_dirty_files(
    target: &IntegrationTarget,
) -> Result<Vec<DirtyFile>, String> {
    match target {
        IntegrationTarget::CheckedOut(path) => worktree_dirty_files(path),
        IntegrationTarget::NotCheckedOut { .. } => Ok(Vec::new()),
    }
}

/// Advance a target branch that no worktree has checked out onto `candidate_sha`.
///
/// `git update-ref <ref> <new> <old>` is the compare-and-swap: git refuses when
/// the ref no longer holds `expected_tip`, so a branch that moved since the
/// review is never overwritten. The candidate must descend from the expected
/// tip, which keeps the move a fast-forward. No working tree or index is read
/// or written.
fn update_ref_target_to(
    repo_path: &Path,
    branch: &str,
    expected_tip: &str,
    candidate_sha: &str,
) -> Result<String, String> {
    reject_option_like_rev(expected_tip)?;
    reject_option_like_rev(candidate_sha)?;
    let full_ref = format!("refs/heads/{branch}");
    let tip = resolve_commit(repo_path, &full_ref)?;
    if tip != expected_tip {
        return Err(format!(
            "integration target '{branch}' is at {tip}, expected {expected_tip}"
        ));
    }
    if !is_ancestor(repo_path, expected_tip, candidate_sha)? {
        return Err(format!(
            "candidate {candidate_sha} does not descend from {expected_tip}; refusing a non fast-forward"
        ));
    }
    let reason = format!("kronn: integrate {candidate_sha} (fast-forward)");
    let out = crate::core::cmd::git_cmd()
        .args([
            "update-ref",
            "-m",
            &reason,
            &full_ref,
            candidate_sha,
            expected_tip,
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git update-ref failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot advance '{branch}' onto {candidate_sha}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let read_back = resolve_commit(repo_path, &full_ref)?;
    if read_back != candidate_sha {
        return Err(format!(
            "integration target '{branch}' reads {read_back} after the update, expected {candidate_sha}"
        ));
    }
    Ok(read_back)
}

pub fn fast_forward_target_to(
    repo_path: &Path,
    target: &str,
    expected_tip: &str,
    candidate_sha: &str,
) -> Result<String, String> {
    // The shape is read here, at the moment of the move: a target that became
    // checked out since an earlier phase takes the checkout path below.
    match integration_target(repo_path, target)? {
        IntegrationTarget::NotCheckedOut { branch } => {
            update_ref_target_to(repo_path, &branch, expected_tip, candidate_sha)
        }
        IntegrationTarget::CheckedOut(checkout) => {
            verify_worktree_head(&checkout, expected_tip)?;
            if !worktree_dirty_files(&checkout)?.is_empty() {
                return Err("integration target worktree became dirty before fast-forward".into());
            }
            fast_forward_to(&checkout, candidate_sha)
        }
    }
}

/// Whether `ancestor` is reachable from `descendant` (`git merge-base --is-ancestor`).
pub fn is_ancestor(repo_path: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    reject_option_like_rev(ancestor)?;
    reject_option_like_rev(descendant)?;
    let out = crate::core::lane_git::git_keeping_owner_hooks(repo_path)?
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|e| format!("git merge-base failed: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "cannot compare {ancestor} with {descendant}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// Confirm a worktree's HEAD is exactly `expected_sha`.
///
/// Defense-in-depth after `create_task_worktree` (a ref that moved between pin and
/// checkout) and the gate the saga re-checks before pinning `workspace_id`.
pub fn verify_worktree_head(worktree_path: &Path, expected_sha: &str) -> Result<(), String> {
    let head = git_head(worktree_path)?;
    if head != expected_sha {
        return Err(format!(
            "worktree HEAD {head} does not match pinned base {expected_sha} at {}",
            worktree_path.display()
        ));
    }
    Ok(())
}

fn task_submodule_head(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    // The closest repository from `path`, pinned when it is (or is inside) a
    // managed worktree: a submodule is its own repository, an empty placeholder
    // is the parent worktree's directory.
    let top_level = crate::core::lane_git::git_keeping_owner_hooks(path)?
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("cannot inspect task submodule {}: {error}", path.display()))?;
    let is_root = top_level.status.success()
        && Path::new(String::from_utf8_lossy(&top_level.stdout).trim())
            .canonicalize()
            .ok()
            == path.canonicalize().ok();
    if is_root {
        return git_head(path).map(Some);
    }
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("cannot inspect task submodule {}: {error}", path.display()))?;
    if entries.next().is_none() {
        std::fs::remove_dir(path).map_err(|error| {
            format!(
                "cannot remove empty task submodule placeholder {}: {error}",
                path.display()
            )
        })?;
        return Ok(None);
    }
    Err(format!(
        "task submodule {} exists but is not an isolated Git checkout",
        path.display()
    ))
}

/// Materialize every submodule required by a task checkout from the source
/// checkout, without consulting a remote.
///
/// A Git worktree copies gitlinks but not their working directories.  Running
/// `git submodule update --init` here would fill them by cloning each configured
/// URL, which can turn task provisioning into an implicit network operation.
/// Instead, each already-materialized source submodule is cloned directly from
/// its local Git directory with `--no-local`, then checked out at the gitlink's
/// exact SHA. `--no-local` prevents Git from sharing hard-linked object files
/// or a mutable checkout between concurrent task worktrees.
pub fn materialize_task_submodules(source_root: &Path, worktree_root: &Path) -> Result<(), String> {
    let output = crate::core::cmd::git_cmd()
        .args(["ls-files", "--stage", "-z"])
        .current_dir(source_root)
        .output()
        .map_err(|error| {
            format!(
                "cannot inspect source submodules at {}: {error}",
                source_root.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect source submodules at {}: {}",
            source_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    for entry in output.stdout.split(|byte| *byte == b'\0') {
        if entry.is_empty() {
            continue;
        }
        let entry = std::str::from_utf8(entry).map_err(|_| {
            format!(
                "source submodule path at {} is not valid UTF-8",
                source_root.display()
            )
        })?;
        let (metadata, relative_path) = entry.split_once('\t').ok_or_else(|| {
            format!(
                "cannot parse source Git index entry at {}",
                source_root.display()
            )
        })?;
        let mut fields = metadata.split_whitespace();
        if fields.next() != Some("160000") {
            continue;
        }
        let expected_sha = fields.next().ok_or_else(|| {
            format!(
                "cannot parse source submodule SHA at {}",
                source_root.display()
            )
        })?;
        if fields.next() != Some("0") || fields.next().is_some() {
            return Err(format!(
                "cannot parse source submodule index entry at {}",
                source_root.display()
            ));
        }
        let relative_path = Path::new(relative_path);
        if relative_path.as_os_str().is_empty()
            || relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!(
                "source submodule path {:?} is unsafe",
                relative_path.display()
            ));
        }

        let source_submodule = source_root.join(relative_path);
        let target_submodule = worktree_root.join(relative_path);
        let source_head = git_head(&source_submodule).map_err(|error| {
            format!(
                "required source submodule {} is not materialized at pinned {expected_sha}: {error}",
                source_submodule.display()
            )
        })?;
        if source_head != expected_sha {
            return Err(format!(
                "required source submodule {} is at {source_head}, not pinned {expected_sha}",
                source_submodule.display()
            ));
        }
        if let Some(target_head) = task_submodule_head(&target_submodule)? {
            if target_head == expected_sha {
                materialize_task_submodules(&source_submodule, &target_submodule)?;
                continue;
            }
            return Err(format!(
                "task submodule {} is at {target_head}, not pinned {expected_sha}",
                target_submodule.display()
            ));
        }

        let source_git_dir = crate::core::cmd::git_cmd()
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(&source_submodule)
            .output()
            .map_err(|error| {
                format!(
                    "cannot locate source submodule Git directory {}: {error}",
                    source_submodule.display()
                )
            })?;
        if !source_git_dir.status.success() {
            return Err(format!(
                "cannot locate source submodule Git directory {}: {}",
                source_submodule.display(),
                String::from_utf8_lossy(&source_git_dir.stderr).trim()
            ));
        }
        let source_git_dir = String::from_utf8_lossy(&source_git_dir.stdout)
            .trim()
            .to_string();
        if source_git_dir.is_empty() {
            return Err(format!(
                "source submodule {} returned an empty Git directory",
                source_submodule.display()
            ));
        }
        let parent = target_submodule.parent().ok_or_else(|| {
            format!(
                "task submodule {} has no parent",
                target_submodule.display()
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create task submodule parent {}: {error}",
                parent.display()
            )
        })?;

        let clone = crate::core::cmd::git_cmd()
            .args(["clone", "--no-local", "--no-checkout", &source_git_dir])
            .arg(&target_submodule)
            .current_dir(worktree_root)
            .output()
            .map_err(|error| {
                format!(
                    "cannot copy required source submodule {}: {error}",
                    source_submodule.display()
                )
            })?;
        if !clone.status.success() {
            return Err(format!(
                "cannot copy required source submodule {} without network access: {}",
                source_submodule.display(),
                String::from_utf8_lossy(&clone.stderr).trim()
            ));
        }
        let checkout = crate::core::cmd::git_cmd()
            .args(["checkout", "--detach", expected_sha])
            .current_dir(&target_submodule)
            .output()
            .map_err(|error| {
                format!(
                    "cannot check out task submodule {} at {expected_sha}: {error}",
                    target_submodule.display()
                )
            })?;
        if !checkout.status.success() {
            return Err(format!(
                "cannot check out task submodule {} at {expected_sha}: {}",
                target_submodule.display(),
                String::from_utf8_lossy(&checkout.stderr).trim()
            ));
        }
        materialize_task_submodules(&source_submodule, &target_submodule)?;
    }
    Ok(())
}

/// The deterministic `(worktree path, branch)` for a task execution — the single
/// source of truth shared by [`create_task_worktree`] (which materializes it) and
/// the KT-318 provisioning saga (which records this exact path as the managed
/// `discussion_workspace` intent BEFORE the physical checkout). Recording the
/// intent first (ADR §4bis) means a crash mid-provision resumes from the row's own
/// path, and computing it in one place removes any risk of the intent and the
/// checkout drifting apart. Path is `<repo>/.kronn/worktrees/task-<ref>-<exec>`;
/// branch is `kronn/task/<ref>-<exec>` (ADR §4, domain-named — never the ticket).
pub fn task_worktree_layout(
    repo_path: &Path,
    task_ref: &str,
    exec_short: &str,
) -> Result<(PathBuf, String), String> {
    let ref_slug = slugify(task_ref);
    let exec_slug = slugify(exec_short);
    if ref_slug.is_empty() || exec_slug.is_empty() {
        return Err(format!(
            "task worktree needs a non-empty task reference and execution id (got {task_ref:?}, {exec_short:?})"
        ));
    }
    let branch = format!("kronn/task/{ref_slug}-{exec_slug}");
    let dir_name = format!("task-{ref_slug}-{exec_slug}");
    Ok((worktree_base_dir(repo_path).join(&dir_name), branch))
}

#[cfg(windows)]
fn assert_no_reparse_component(root: &Path, path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

    let rel = path
        .strip_prefix(root)
        .map_err(|_| format!("{} escapes the root {}", path.display(), root.display()))?;
    let mut current = root.to_path_buf();
    for component in rel.components() {
        current.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(format!(
                    "{} is a Windows reparse point — refusing task worktree access",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn assert_no_reparse_component(_root: &Path, _path: &Path) -> Result<(), String> {
    Ok(())
}

/// Destructive task-worktree operations are valid only for a direct child of
/// `<repo>/.kronn/worktrees`. Branch/HEAD/propreté prove Git ownership, but do
/// not prove filesystem ownership: without this guard a forged workspace row
/// could point cleanup at a clean external checkout with matching refs.
fn assert_managed_task_worktree_path(repo_path: &Path, worktree_path: &Path) -> Result<(), String> {
    let managed_root = worktree_base_dir(repo_path);
    let relative = worktree_path.strip_prefix(&managed_root).map_err(|_| {
        format!(
            "task worktree {} is outside managed root {} — refusing access",
            worktree_path.display(),
            managed_root.display()
        )
    })?;
    let mut components = relative.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(format!(
            "task worktree {} is not a direct managed checkout under {}",
            worktree_path.display(),
            managed_root.display()
        ));
    }
    crate::core::fs_guard::assert_contained_no_symlink(repo_path, worktree_path)?;
    assert_no_reparse_component(repo_path, worktree_path)?;

    if worktree_path.exists() && managed_root.exists() {
        let canonical_root = managed_root
            .canonicalize()
            .map_err(|error| format!("cannot resolve managed worktree root: {error}"))?;
        let canonical_worktree = worktree_path
            .canonicalize()
            .map_err(|error| format!("cannot resolve task worktree: {error}"))?;
        if !canonical_worktree.starts_with(&canonical_root) {
            return Err(format!(
                "task worktree {} resolves outside managed root {} — refusing access",
                worktree_path.display(),
                managed_root.display()
            ));
        }
    }
    Ok(())
}

/// Create a sibling worktree for a task execution, pinned to an exact commit.
///
/// Contract for KT-318 provisioning (differs from `create_discussion_worktree`):
/// - `base_sha` is a resolved commit SHA, never a mutable branch, so a concurrent
///   push cannot change what the worker builds on (pin via `resolve_commit`).
/// - branch + path are deterministic from the task reference + a short execution
///   id (`kronn/task/<ref>-<exec>`), collision-free per execution.
/// - it NEVER silently reuses an existing branch/path: a pre-existing one means a
///   concurrent launch or a stale attempt, so it fails closed and lets the saga
///   (which owns idempotent reuse via the managed `discussion_workspace` row)
///   decide — a foreign checkout is never handed back as isolated.
/// - after creation, HEAD is verified to equal `base_sha`; on mismatch the
///   half-created resource is undone so no orphan is left.
pub fn create_task_worktree(
    repo_path: &Path,
    task_ref: &str,
    exec_short: &str,
    base_sha: &str,
) -> Result<WorktreeInfo, String> {
    let (worktree_path, branch) = task_worktree_layout(repo_path, task_ref, exec_short)?;
    assert_managed_task_worktree_path(repo_path, &worktree_path)?;

    // KT-373 — refuse before creating anything. A worktree provisioned onto a
    // full disk fails later, deeper, and leaves artefacts behind; refusing here
    // means there is nothing to compensate. Measured on the repo, because that
    // is the filesystem the worktree and its `target/` will land on.
    let (warning_gib, critical_gib) = configured_disk_thresholds();
    ensure_disk_headroom(repo_path, warning_gib, critical_gib)?;

    // Fail closed on any collision — never reuse a foreign or stale checkout.
    if let Some(existing) = branch_checked_out_at(repo_path, &branch)? {
        return Err(format!(
            "task branch {branch} is already checked out at {} — refusing to reuse it",
            existing.path.display()
        ));
    }
    if branch_commit(repo_path, &branch).is_some() {
        return Err(format!(
            "task branch {branch} already exists — refusing to reuse it"
        ));
    }
    if worktree_path.exists() {
        return Err(format!(
            "task worktree path {} already exists — refusing to reuse it",
            worktree_path.display()
        ));
    }

    crate::core::fs_guard::guarded_create_dir_all(repo_path, &worktree_base_dir(repo_path))?;
    assert_managed_task_worktree_path(repo_path, &worktree_path)?;
    ensure_managed_excludes(repo_path)?;
    crate::core::git_global_config::ensure_safe_directory(repo_path);
    crate::core::git_global_config::ensure_safe_directory(&worktree_path);

    // Create from the EXACT pinned commit (a SHA, not a branch name).
    let output = crate::core::cmd::project_git_cmd()
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_path)
        .arg(base_sha)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree add: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    // Rewrite the gitdir to the portable RELATIVE form (KT-331 fixed the off-by-one
    // depth). Git's native absolute gitdir only works for a worker that shares the
    // creator's filesystem view; when the backend runs in Docker it writes the
    // container path (`/host-home/...`), unusable by a host CLI worker. The relative
    // gitdir at the correct depth resolves from both views — the Git-side prerequisite
    // for handing a task worktree to a CLI worker (KT-328). Applied before the HEAD
    // check below so verification exercises the same gitdir the worker will use.
    confirm_registration_id(repo_path, &worktree_path, Some(&branch))?;
    fix_worktree_paths(repo_path, &worktree_path);

    // Defense-in-depth: confirm HEAD landed on the pinned base. On mismatch,
    // undo our own just-created (unique-branch) resource so no orphan remains.
    if let Err(e) = verify_worktree_head(&worktree_path, base_sha) {
        let _ =
            remove_discussion_worktree(repo_path, &worktree_path.to_string_lossy(), Some(&branch));
        return Err(e);
    }
    if let Err(e) = materialize_task_submodules(repo_path, &worktree_path) {
        let _ =
            remove_discussion_worktree(repo_path, &worktree_path.to_string_lossy(), Some(&branch));
        return Err(e);
    }

    tracing::info!(
        "Created task worktree at {} (branch {}, pinned {})",
        worktree_path.display(),
        branch,
        base_sha
    );

    Ok(WorktreeInfo {
        path: worktree_path.to_string_lossy().to_string(),
        branch,
        is_main_repo: false,
    })
}

/// Ownership-aware compensation for a task worktree created by `create_task_worktree`.
///
/// Removes ONLY what this attempt created, after proving ownership: the checkout
/// must still be on `expected_branch`, its HEAD still at `expected_base_sha`
/// (nothing committed), and its tree clean. Any mismatch — a reused checkout, a
/// checkout with work, or uncommitted changes — is refused so the caller can keep
/// the execution `Provisioning`/`Blocked` and resumable rather than destroy work
/// or an unrelated checkout. Used on the provisioning-failure path only; post-work
/// teardown belongs to KT-320. Idempotent: a directory already gone is a success.
pub fn remove_task_worktree(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
    expected_base_sha: &str,
) -> Result<(), String> {
    let wt = Path::new(worktree_path);
    assert_managed_task_worktree_path(repo_path, wt)?;
    if !wt.exists() {
        // Directory already gone (a prior partial cleanup). Drop the branch only
        // if it is still exactly at the pinned base — never delete landed work.
        return match branch_commit(repo_path, expected_branch) {
            None => {
                remove_registration_of_gone(repo_path, wt);
                Ok(())
            }
            Some(sha) if sha == expected_base_sha => {
                remove_registration_of_gone(repo_path, wt);
                let mut command = crate::core::cmd::git_cmd();
                command
                    .args(["branch", "-D", "--end-of-options", expected_branch])
                    .current_dir(repo_path);
                let _ = bounded(command, "branch -D");
                Ok(())
            }
            Some(sha) => Err(format!(
                "task branch {expected_branch} advanced to {sha} (pinned {expected_base_sha}) but its worktree is gone — leaving it for manual/KT-320 cleanup"
            )),
        };
    }

    // Prove ownership before touching anything.
    let actual_branch = find_branch_for_worktree(repo_path, worktree_path).ok_or_else(|| {
        branch_unknown(
            worktree_path,
            format!(
                "cannot determine branch for worktree {worktree_path} — refusing blind removal"
            ),
        )
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — refusing to remove a checkout we do not own"
        ));
    }
    let judged = Snapshot::of_worktree(wt)?;
    let head = judged_head(&judged)?;
    if head != expected_base_sha {
        return Err(format!(
            "worktree {worktree_path} HEAD advanced to {head} (pinned {expected_base_sha}) — refusing to remove a checkout with work"
        ));
    }
    if !judged_dirty_files(&judged)?.is_empty() {
        return Err(format!(
            "worktree {worktree_path} has uncommitted changes — refusing to remove; leave the execution resumable"
        ));
    }
    drop(judged);

    // Ownership proven and tree clean → safe to remove worktree + its branch.
    remove_discussion_worktree(repo_path, worktree_path, Some(expected_branch))
}

/// Remove the clean checkout of a task while preserving its branch for inspection: a
/// cancelled, failed or finished execution's, a closed task's, an orphaned one. Ownership is
/// proved by the exact branch and a modified or untracked file keeps it; unlike provisioning
/// compensation, the branch may legitimately have advanced beyond its base commit. Its
/// ignored files are copied into `evidence_dir` first, as [`release_terminal_task_worktree`]
/// does (KT-2084).
pub fn remove_cancelled_task_worktree(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
    evidence_dir: &Path,
) -> Result<TerminalWorktreeRelease, String> {
    release_task_checkout(
        repo_path,
        worktree_path,
        expected_branch,
        evidence_dir,
        RELEASED_WORKTREE_PATCH_CAP_BYTES,
        TaskRelease::Clean,
    )
}

/// How a task checkout is released (KT-2084).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskRelease {
    /// Only once clean: a modified or untracked file keeps it.
    Clean,
    /// A terminal execution's: its modified and untracked files are saved as a patch.
    Terminal,
}

/// Why Kronn does not know the branch of the managed worktree `worktree_path`: a reftable
/// repository keeps it in tables Kronn does not read yet (KT-2101).
fn branch_unknown(worktree_path: &str, unknown: String) -> String {
    let checkout = Path::new(worktree_path);
    let reftable = crate::core::lane_git::managed_registration(checkout)
        .ok()
        .and_then(|pinned| {
            crate::core::fs_guard::guarded_read_head(
                &pinned.git_dir,
                &pinned.git_dir.join("HEAD"),
                HEAD_READ_CAP,
            )
            .ok()
            .flatten()
        })
        .is_some_and(|head| head == crate::core::lane_git::snapshot::REFTABLE_STUB);
    if reftable {
        crate::core::lane_git::snapshot::reftable_refusal(checkout)
    } else {
        unknown
    }
}

/// What [`release_terminal_task_worktree`] did with the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalWorktreeRelease {
    /// The checkout was already gone; only the admin entry was pruned.
    AlreadyGone,
    /// The checkout was removed. `patch` is the saved uncommitted diff, when
    /// there was one: the working-tree patch, or the staged one when that is
    /// all there was (both sit in the evidence directory). `ignored_copied` is the
    /// folder its ignored files were copied to, when it held any (KT-2084).
    Removed {
        patch: Option<PathBuf>,
        ignored_copied: Option<PathBuf>,
    },
}

/// The folder of the evidence directory that holds a released checkout's ignored files, at
/// their path from the checkout (KT-2084).
pub const RELEASED_WORKTREE_IGNORED_DIR: &str = "ignored";

/// Most ignored files a release copies; above this the checkout is kept.
const MAX_RELEASED_IGNORED_FILES: usize = 20_000;

/// Name of the working-tree patch (index -> files on disk, untracked files
/// included) kept as evidence of a released worktree. With nothing staged it is
/// the whole uncommitted diff against `HEAD`.
pub const RELEASED_WORKTREE_PATCH: &str = "uncommitted-changes.patch";

/// Name of the staged patch (`HEAD` -> the worktree's own index), saved next to
/// [`RELEASED_WORKTREE_PATCH`] only when the index differs from `HEAD`.
pub const RELEASED_WORKTREE_STAGED_PATCH: &str = "staged-changes.patch";

/// The most uncommitted state a release will turn into evidence. Untracked
/// build output that `.gitignore` does not cover would otherwise become a
/// multi-GB patch (and as many loose objects in the shared repository): above
/// this the checkout is kept and the reason recorded (KT-1397).
pub const RELEASED_WORKTREE_PATCH_CAP_BYTES: u64 = 256 * 1024 * 1024;

/// `git <args>` against Kronn's copy `judged`, reading and writing the scratch `index`.
fn git_in_scratch_index(judged: &Snapshot, index: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    git_in_scratch_index_fed(judged, index, args, &[])
}

/// [`git_in_scratch_index`] with `input` on the command's standard input.
fn git_in_scratch_index_fed(
    judged: &Snapshot,
    index: &Path,
    args: &[&str],
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let mut command = judged.command();
    command.args(args).env("GIT_INDEX_FILE", index);
    let output = crate::core::lane_git::snapshot::bounded_output_with_input(
        command,
        input,
        args.first().copied().unwrap_or_default(),
    )?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// `Some(bytes)` once the content of the modified and untracked (not ignored)
/// files of the worktree `judged` is a copy of is known to exceed `cap`. Measured
/// from file sizes before anything is staged, so an oversized tree is refused
/// without copying a byte.
fn uncommitted_content_over(judged: &Snapshot, cap: u64) -> Result<Option<u64>, String> {
    let wt = judged
        .work_tree()
        .ok_or_else(|| format!("{} has no work tree", judged.source().display()))?;
    let output = judged.output(&[
        "ls-files",
        "-z",
        "--modified",
        "--others",
        "--exclude-standard",
    ])?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut total = 0u64;
    for name in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        if let Ok(meta) = std::fs::symlink_metadata(wt.join(name)) {
            if meta.is_file() {
                total = total.saturating_add(meta.len());
                if total > cap {
                    return Ok(Some(total));
                }
            }
        }
    }
    Ok(None)
}

/// Stream `git <args>` (a tree-to-tree `diff`, so neither index is read) to `out`
/// under `cap` bytes. `Ok(None)` is an empty diff and leaves no file; past the cap
/// nothing is left behind and the result is an error. `Ok(Some(n))` is the size
/// written.
fn write_capped_git_patch(
    judged: &Snapshot,
    args: &[&str],
    out: &Path,
    cap: u64,
) -> Result<Option<u64>, String> {
    use std::io::Read;
    let mut command = judged.command();
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::core::cmd::own_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("git {} failed: {e}", args.join(" ")))?;
    let timeout = crate::core::lane_git::snapshot::judge_timeout();
    let deadline = crate::core::cmd::Deadline::arm(&child, timeout);
    let mut stdout = child.stdout.take().ok_or("git diff has no output")?;
    let staging = out.with_extension("patch.tmp");
    let copied = std::fs::File::create(&staging)
        .and_then(|mut file| std::io::copy(&mut stdout.by_ref().take(cap + 1), &mut file));
    let abandon = |child: &mut std::process::Child| {
        crate::core::cmd::kill_process_group(child);
        let _ = std::fs::remove_file(&staging);
    };
    let timed_out = || {
        format!(
            "git {} did not finish within its {} s timeout",
            args.first().copied().unwrap_or_default(),
            timeout.as_secs_f32()
        )
    };
    let copied = match copied {
        Ok(copied) => copied,
        Err(e) => {
            abandon(&mut child);
            return Err(format!(
                "cannot write the patch to {}: {e}",
                staging.display()
            ));
        }
    };
    if copied > cap {
        abandon(&mut child);
        return Err(format!("the patch exceeds the {cap}-byte evidence cap"));
    }
    let status = child.wait().map_err(|e| format!("git diff failed: {e}"))?;
    if deadline.fired() {
        let _ = std::fs::remove_file(&staging);
        return Err(timed_out());
    }
    drop(deadline);
    if !status.success() {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("git {} failed", args.join(" ")));
    }
    if copied == 0 {
        let _ = std::fs::remove_file(&staging);
        return Ok(None);
    }
    std::fs::rename(&staging, out).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        format!("cannot save the patch to {}: {e}", out.display())
    })?;
    Ok(Some(copied))
}

/// Which patches a release saved. `staged` is `HEAD` -> the worktree's own index;
/// `working` is that index -> the files on disk (tracked edits and untracked,
/// not ignored, files). Reapplied in that order (`git apply --index` the first,
/// `git apply` the second) on the kept branch they rebuild the index and the
/// working tree exactly.
#[derive(Debug)]
struct SavedPatches {
    staged: bool,
    working: bool,
}

/// Save the whole uncommitted state of the worktree `judged` is a copy of as the two
/// patches described on [`SavedPatches`], within one `cap`. Kronn's copy of its index is
/// copied again as a scratch index in the copy's own folder: the worktree's index is never
/// read by git nor opened for writing, so the checkout is exactly as it was when this
/// returns. An index that cannot be written as a tree (unmerged paths) is an error: the
/// checkout is kept rather than lose what one patch cannot hold.
///
/// No git starts in a submodule (KT-2084): `add -A` would run `git status` in each populated
/// one, with that repository's configuration. Each submodule is recorded at the commit
/// Kronn's copy of its repository has checked out (`update-index`), and `add` is told to
/// leave every submodule path out.
fn write_uncommitted_patches(
    judgement: &Judgement,
    evidence_dir: &Path,
    cap: u64,
) -> Result<SavedPatches, String> {
    let judged = &judgement.worktree;
    let scratch_index = judged.dir().join("patch-index");
    std::fs::copy(judged.dir().join("index"), &scratch_index).map_err(|e| {
        format!(
            "cannot copy Kronn's copy of the index of {}: {e}",
            judged.source().display()
        )
    })?;
    let scratch_index = scratch_index.as_path();
    let staged_tree = String::from_utf8_lossy(&git_in_scratch_index(
        judged,
        scratch_index,
        &["write-tree"],
    )?)
    .trim()
    .to_string();
    let (index_info, left_out) = submodule_index_lines(judgement, scratch_index)?;
    if !index_info.is_empty() {
        git_in_scratch_index_fed(
            judged,
            scratch_index,
            &["update-index", "-z", "--index-info"],
            &index_info,
        )?;
    }
    let mut pathspecs = b":(top)\0".to_vec();
    for path in &left_out {
        pathspecs.extend_from_slice(format!(":(top,exclude,literal){path}").as_bytes());
        pathspecs.push(0);
    }
    git_in_scratch_index_fed(
        judged,
        scratch_index,
        &["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"],
        &pathspecs,
    )?;
    let working_tree = String::from_utf8_lossy(&git_in_scratch_index(
        judged,
        scratch_index,
        &["write-tree"],
    )?)
    .trim()
    .to_string();
    let staged_out = evidence_dir.join(RELEASED_WORKTREE_STAGED_PATCH);
    let working_out = evidence_dir.join(RELEASED_WORKTREE_PATCH);
    let diff = |from: &str, to: &str| {
        vec![
            "diff".to_string(),
            "--ignore-submodules=dirty".to_string(),
            "--binary".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            "--end-of-options".to_string(),
            from.to_string(),
            to.to_string(),
        ]
    };
    let saved = (|| {
        let args = diff("HEAD", &staged_tree);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let staged = write_capped_git_patch(judged, &args, &staged_out, cap)?;
        let args = diff(&staged_tree, &working_tree);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let working = write_capped_git_patch(
            judged,
            &args,
            &working_out,
            cap.saturating_sub(staged.unwrap_or(0)),
        )?;
        Ok(SavedPatches {
            staged: staged.is_some(),
            working: working.is_some(),
        })
    })();
    if saved.is_err() {
        // Half the evidence is worse than none: the checkout stays.
        let _ = std::fs::remove_file(&staged_out);
        let _ = std::fs::remove_file(&working_out);
    }
    saved
}

/// What [`write_uncommitted_patches`] records for each submodule of the scratch `index`
/// (KT-2084): `update-index -z --index-info` input and the submodule paths `add` leaves out.
/// A populated submodule is set to the commit Kronn's copy of its repository has checked out,
/// an unpopulated one stays as it is, and one whose folder is gone or is no longer a folder is
/// dropped (what stands there instead is then added like any file).
fn submodule_index_lines(
    judgement: &Judgement,
    index: &Path,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let judged = &judgement.worktree;
    let checkout = judged
        .work_tree()
        .ok_or_else(|| format!("{} has no work tree", judged.source().display()))?;
    let listed = git_in_scratch_index(judged, index, &["ls-files", "--stage", "-z"])?;
    let mut lines = Vec::new();
    let mut left_out = Vec::new();
    for entry in listed.split(|byte| *byte == 0) {
        let Some(rest) = entry.strip_prefix(b"160000 ") else {
            continue;
        };
        let Some(tab) = rest.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let (meta, path) = (&rest[..tab], &rest[tab + 1..]);
        let (Ok(meta), Ok(path)) = (std::str::from_utf8(meta), std::str::from_utf8(path)) else {
            return Err(format!(
                "a submodule entry is not UTF-8 ({}): Kronn does not save it",
                String::from_utf8_lossy(entry)
            ));
        };
        let oid = meta.split(' ').next().unwrap_or_default();
        if !plain_relative(path) || oid.is_empty() {
            return Err(format!(
                "the submodule entry {path:?} is not a plain path of the worktree"
            ));
        }
        let line = match std::fs::symlink_metadata(checkout.join(path)) {
            Ok(meta) if meta.is_dir() => {
                left_out.push(path.to_string());
                let populated = judgement
                    .submodules
                    .iter()
                    .find(|(relative, _)| relative == path);
                match populated {
                    Some((_, submodule)) => {
                        format!("160000 {}\t{path}", checked_out_commit(submodule, path)?)
                    }
                    None => continue,
                }
            }
            Ok(_) => format!("0 {oid}\t{path}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                format!("0 {oid}\t{path}")
            }
            Err(error) => return Err(format!("the submodule {path} cannot be read: {error}")),
        };
        lines.extend_from_slice(line.as_bytes());
        lines.push(0);
    }
    Ok((lines, left_out))
}

/// The commit Kronn's copy `submodule` of the submodule at `path` has checked out.
fn checked_out_commit(submodule: &Snapshot, path: &str) -> Result<String, String> {
    let output = submodule.output(&["rev-parse", "--verify", "-q", "HEAD^{commit}"])?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || commit.is_empty() {
        return Err(format!(
            "the commit the submodule {path} has checked out cannot be read"
        ));
    }
    Ok(commit)
}

/// Whether anything in the checkout `wt` was written after `cutoff`: the newest
/// mtime of any file or directory in it, except the Git admin entry and without
/// following symlinks (the lane build dir lives outside the checkout). Stops at
/// the first write newer than `cutoff`. Err when the tree cannot be read: an
/// activity that cannot be established is never idle (KT-1397).
pub fn checkout_written_since(
    wt: &Path,
    cutoff: std::time::SystemTime,
) -> Result<Option<PathBuf>, String> {
    let mut found = None;
    walk_checkout_writes(wt, |path, written| {
        if written > cutoff {
            found = Some(path.to_path_buf());
        }
        found.is_none()
    })?;
    Ok(found)
}

/// The newest write in the checkout `wt` after `cutoff`, over the whole tree
/// (KT-2084): when a window that restarts at each write ends.
pub fn checkout_newest_write_since(
    wt: &Path,
    cutoff: std::time::SystemTime,
) -> Result<Option<std::time::SystemTime>, String> {
    let mut newest: Option<std::time::SystemTime> = None;
    walk_checkout_writes(wt, |_, written| {
        if written > cutoff && newest.is_none_or(|known| written > known) {
            newest = Some(written);
        }
        true
    })?;
    Ok(newest)
}

/// The most entries a checkout's write walk looks at, and for how long: past either, its
/// activity is unknown, never idle.
const CHECKOUT_WALK_ENTRIES: usize = 1_000_000;
const CHECKOUT_WALK_TIME: std::time::Duration = std::time::Duration::from_secs(30);

/// Visit the mtime of every file and directory of `wt` but the Git admin entry, until
/// `visit` returns false. Rooted (KT-2084): a folder is listed through its own handle, so one
/// swapped for a link is never followed; `Err` past [`CHECKOUT_WALK_ENTRIES`] or
/// [`CHECKOUT_WALK_TIME`].
fn walk_checkout_writes(
    wt: &Path,
    mut visit: impl FnMut(&Path, std::time::SystemTime) -> bool,
) -> Result<(), String> {
    use crate::core::fs_guard::{guarded_walk, Visit, WalkBounds};
    let bounds = WalkBounds::within(CHECKOUT_WALK_ENTRIES, CHECKOUT_WALK_TIME);
    // Opened from its parent: a checkout that is a link is never followed (KT-2084).
    let parent = wt.parent().unwrap_or(wt);
    let end = guarded_walk(parent, wt, bounds, &mut |entry| {
        if entry.depth == 1 && entry.path.file_name() == Some(std::ffi::OsStr::new(".git")) {
            Visit::Skip
        } else if visit(entry.path, entry.modified) {
            Visit::Enter
        } else {
            Visit::Stop
        }
    })
    .map_err(|e| format!("cannot read {}: {e}", wt.display()))?;
    match end.bound() {
        Some(why) => Err(format!(
            "the writes in {} cannot all be read ({why})",
            wt.display()
        )),
        None => Ok(()),
    }
}

/// Release the checkout of a task execution that has reached a terminal status
/// (KT-1397): the branch is kept, the uncommitted changes are saved as a patch
/// under `evidence_dir` first, its ignored files that a delete counts as work are
/// copied to [`RELEASED_WORKTREE_IGNORED_DIR`] there (KT-2084), both within the
/// cap, and the checkout is then removed.
///
/// The caller proves the execution is terminal and the workspace is Kronn's
/// (`managed`); this proves the rest against Git: the path is in the managed
/// layout and the checkout is on exactly `expected_branch`. Unlike
/// [`remove_cancelled_task_worktree`] a dirty tree is not a reason to keep the
/// directory, but a patch that cannot be written, or that would exceed
/// [`RELEASED_WORKTREE_PATCH_CAP_BYTES`], is: nothing is removed unless the
/// evidence is on disk.
pub fn release_terminal_task_worktree(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
    evidence_dir: &Path,
) -> Result<TerminalWorktreeRelease, String> {
    release_terminal_task_worktree_capped(
        repo_path,
        worktree_path,
        expected_branch,
        evidence_dir,
        RELEASED_WORKTREE_PATCH_CAP_BYTES,
    )
}

/// [`release_terminal_task_worktree`] with the evidence cap injected.
pub fn release_terminal_task_worktree_capped(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
    evidence_dir: &Path,
    patch_cap_bytes: u64,
) -> Result<TerminalWorktreeRelease, String> {
    release_task_checkout(
        repo_path,
        worktree_path,
        expected_branch,
        evidence_dir,
        patch_cap_bytes,
        TaskRelease::Terminal,
    )
}

/// Every release of a task checkout (KT-2084), judged once: its ignored files, its
/// submodules' included, are copied into `evidence_dir`, and for a [`TaskRelease::Terminal`]
/// one its modified and untracked files are saved as a patch, all within `patch_cap_bytes`,
/// before it is removed with its registration. Its branch stays.
fn release_task_checkout(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
    evidence_dir: &Path,
    patch_cap_bytes: u64,
    release: TaskRelease,
) -> Result<TerminalWorktreeRelease, String> {
    let label = match release {
        TaskRelease::Clean => "cancelled",
        TaskRelease::Terminal => "terminal",
    };
    reject_option_like_rev(expected_branch)?;
    let wt = Path::new(worktree_path);
    assert_managed_task_worktree_path(repo_path, wt)?;
    if !wt.exists() {
        remove_registration_of_gone(repo_path, wt);
        return Ok(TerminalWorktreeRelease::AlreadyGone);
    }
    let actual_branch = find_branch_for_worktree(repo_path, worktree_path).ok_or_else(|| {
        branch_unknown(
            worktree_path,
            format!("cannot determine branch for {label} worktree {worktree_path} — preserving it"),
        )
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "{label} worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — preserving it"
        ));
    }
    let judged = Judgement::of(wt)?;
    let dirty = !judged_dirty_files(&judged.worktree)?.is_empty();
    let saved = match release {
        TaskRelease::Clean if dirty => {
            return Err(format!(
                "{label} worktree {worktree_path} has uncommitted changes — preserving it"
            ))
        }
        TaskRelease::Clean => Saved::IgnoredFiles,
        TaskRelease::Terminal => Saved::Everything,
    };
    // The patch saves modified and untracked files, the copy the ignored ones; the removal
    // takes the rest (KT-2084).
    let (loss, found) = forced_removal_check(repo_path, &judged, worktree_path, saved)?;
    if let Some(loss) = loss {
        return Err(format!("{loss} — preserving it"));
    }
    let ignored = ignored_files_to_copy(repo_path, wt, &found).map_err(|e| {
        format!("the ignored files of {worktree_path} cannot be read: {e} — preserving it")
    })?;
    let ignored_bytes = ignored
        .iter()
        .fold(0u64, |total, (_, size)| total.saturating_add(*size));
    if ignored_bytes > patch_cap_bytes {
        return Err(format!(
            "the ignored files of {worktree_path} hold {ignored_bytes} bytes, over the \
             {patch_cap_bytes}-byte evidence cap — preserving it"
        ));
    }
    let patch_cap_bytes = patch_cap_bytes - ignored_bytes;
    // A previous attempt's copy must not pass for this release's evidence.
    let copied = evidence_dir.join(RELEASED_WORKTREE_IGNORED_DIR);
    if std::fs::symlink_metadata(evidence_dir).is_ok_and(|meta| meta.is_dir()) {
        crate::core::fs_guard::guarded_remove_tree(evidence_dir, &copied).map_err(|e| {
            format!(
                "cannot clear {} before the release of {worktree_path}: {e} — preserving it",
                copied.display()
            )
        })?;
    }
    if !ignored.is_empty() {
        std::fs::create_dir_all(evidence_dir).map_err(|e| {
            format!(
                "cannot create {} for the ignored files of {worktree_path}: {e} — preserving it",
                evidence_dir.display()
            )
        })?;
    }
    let mut patch = None;
    if dirty {
        if let Some(bytes) = uncommitted_content_over(&judged.worktree, patch_cap_bytes)? {
            let left = if ignored_bytes > 0 {
                " its ignored files leave"
            } else {
                ""
            };
            return Err(format!(
                "the uncommitted files of {worktree_path} hold {bytes}+ bytes, over the \
                 {patch_cap_bytes}-byte evidence cap{left} — preserving it"
            ));
        }
        std::fs::create_dir_all(evidence_dir).map_err(|e| {
            format!(
                "cannot create {} for the uncommitted changes of {worktree_path}: {e} — preserving it",
                evidence_dir.display()
            )
        })?;
        // A previous attempt's leftovers must not pass for this release's evidence.
        for stale in [RELEASED_WORKTREE_PATCH, RELEASED_WORKTREE_STAGED_PATCH] {
            let _ = std::fs::remove_file(evidence_dir.join(stale));
        }
        let saved = write_uncommitted_patches(&judged, evidence_dir, patch_cap_bytes);
        let saved = saved.map_err(|e| {
            format!("cannot save the uncommitted changes of {worktree_path}: {e} — preserving it")
        })?;
        if saved.working {
            patch = Some(evidence_dir.join(RELEASED_WORKTREE_PATCH));
        } else if saved.staged {
            patch = Some(evidence_dir.join(RELEASED_WORKTREE_STAGED_PATCH));
        }
    }
    let ignored_copied = match copy_ignored_files(wt, &ignored, evidence_dir, ignored_bytes) {
        Ok(copied) => copied,
        Err(e) => {
            // Half the evidence is worse than none: the checkout stays.
            let _ = crate::core::fs_guard::guarded_remove_tree(evidence_dir, &copied);
            for saved in [RELEASED_WORKTREE_PATCH, RELEASED_WORKTREE_STAGED_PATCH] {
                let _ = std::fs::remove_file(evidence_dir.join(saved));
            }
            return Err(format!(
                "cannot save the ignored files of {worktree_path}: {e} — preserving it"
            ));
        }
    };
    if let Some(copied) = &ignored_copied {
        tracing::info!(
            "{} ignored file(s) of {worktree_path} copied to {} before its release",
            ignored.len(),
            copied.display()
        );
    }
    drop(judged);
    remove_discussion_worktree(repo_path, worktree_path, None)?;
    Ok(TerminalWorktreeRelease::Removed {
        patch,
        ignored_copied,
    })
}

/// The ignored files a release copies (KT-2084), its submodules' included, the latter under
/// their submodule's path: each ignored file `found` counts as work, and each file below
/// such an ignored folder that a delete counts as work. Links, OS junk, Kronn's files, seeds
/// and regenerable output are left out; a name that is not UTF-8 refuses. `(path from the
/// checkout, size)`.
fn ignored_files_to_copy(
    repo_path: &Path,
    checkout: &Path,
    found: &WorkFound,
) -> Result<Vec<(String, u64)>, String> {
    let regenerable = regenerable_outputs();
    let mut files = Vec::new();
    let top = IgnoredIn {
        repo_path: Some(repo_path),
        checkout,
        prefix: String::new(),
        regenerable: &regenerable,
    };
    top.collect(&found.files.ignored, &mut files)?;
    for (relative, submodule) in &found.submodules {
        let below = regenerable_below(&regenerable, relative);
        let sub = IgnoredIn {
            repo_path: None,
            checkout: &checkout.join(relative),
            prefix: format!("{relative}/"),
            regenerable: &below,
        };
        sub.collect(&submodule.ignored, &mut files)?;
    }
    Ok(files)
}

/// The ignored files of one work tree, the checkout's or a submodule's at `prefix`.
struct IgnoredIn<'a> {
    /// `None`: a submodule, which holds none of Kronn's files.
    repo_path: Option<&'a Path>,
    checkout: &'a Path,
    prefix: String,
    regenerable: &'a [String],
}

impl IgnoredIn<'_> {
    /// Adds the files of `entries` (ignored entries of this work tree) to `files`, by path
    /// from the checkout.
    fn collect(&self, entries: &[String], files: &mut Vec<(String, u64)>) -> Result<(), String> {
        let checkout = self.checkout;
        let junk = os_junk_patterns();
        let (_, kronn_files, seeds) = kronn_entries(self.repo_path, checkout);
        let not_work = NotWork {
            checkout,
            regenerable: self.regenerable,
            junk: &junk,
            kronn_files: &kronn_files,
            seeds: &seeds,
        };
        let mut pending = Vec::new();
        for entry in entries {
            if !plain_relative(entry) {
                return Err(format!("{entry:?} is not a plain path of the worktree"));
            }
            pending.push(entry.trim_end_matches('/').to_string());
        }
        while let Some(relative) = pending.pop() {
            let path = checkout.join(&relative);
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}{relative}: {e}", self.prefix)),
            };
            if meta.is_file() {
                if files.len() == MAX_RELEASED_IGNORED_FILES {
                    return Err(format!(
                        "more than {MAX_RELEASED_IGNORED_FILES} ignored files"
                    ));
                }
                files.push((format!("{}{relative}", self.prefix), meta.len()));
                continue;
            }
            // A link is never followed, and removing it loses nothing.
            if !meta.is_dir() {
                continue;
            }
            let names = crate::core::fs_guard::guarded_list(checkout, &path)
                .map_err(|e| format!("{}{relative}: {e}", self.prefix))?
                .unwrap_or_default();
            for name in names {
                let name = name.into_string().map_err(|name| {
                    format!("{}{relative}/{name:?} is not a UTF-8 name", self.prefix)
                })?;
                let child = format!("{relative}/{name}");
                if !(is_os_junk(&name, &junk) || not_work.ignored(&child, false)) {
                    pending.push(child);
                }
            }
        }
        Ok(())
    }
}

/// Copy `files` of `checkout` into the evidence folder [`RELEASED_WORKTREE_IGNORED_DIR`] of
/// `evidence_dir`, at their path, within `budget` bytes: read without following a link,
/// written without replacing anything. A file with another name (a hard link, maybe to a file
/// elsewhere) is not copied and keeps the checkout. `None` when there was nothing to copy.
fn copy_ignored_files(
    checkout: &Path,
    files: &[(String, u64)],
    evidence_dir: &Path,
    budget: u64,
) -> Result<Option<PathBuf>, String> {
    let copied = evidence_dir.join(RELEASED_WORKTREE_IGNORED_DIR);
    let mut left = budget;
    let mut any = false;
    for (relative, _) in files {
        let read = crate::core::fs_guard::guarded_read_head_one_name(
            checkout,
            &checkout.join(relative),
            left.saturating_add(1),
        )
        .map_err(|e| format!("{relative}: {e}"))?;
        // Gone since it was listed: nothing of it is lost.
        let Some(bytes) = read else {
            continue;
        };
        let size = bytes.len() as u64;
        if size > left {
            return Err(format!("{relative} grew past the evidence cap"));
        }
        left -= size;
        if !crate::core::fs_guard::guarded_write_new(evidence_dir, &copied.join(relative), &bytes)?
        {
            return Err(format!("{} already holds {relative}", copied.display()));
        }
        any = true;
    }
    Ok(any.then_some(copied))
}

/// Remove a task worktree AFTER its candidate was integrated successfully.
/// Unlike provisioning compensation, the branch is expected to have advanced:
/// ownership is proven by the exact managed branch, a clean tree and
/// `HEAD == integrated_sha`. Any divergence is preserved for manual inspection.
/// Idempotent when a previous cleanup already removed both checkout and branch.
pub fn remove_integrated_task_worktree(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
    integrated_sha: &str,
) -> Result<(), String> {
    reject_option_like_rev(expected_branch)?;
    reject_option_like_rev(integrated_sha)?;
    let wt = Path::new(worktree_path);
    assert_managed_task_worktree_path(repo_path, wt)?;
    if !wt.exists() {
        return match branch_commit(repo_path, expected_branch) {
            None => {
                remove_registration_of_gone(repo_path, wt);
                Ok(())
            }
            Some(sha) if sha == integrated_sha => {
                remove_registration_of_gone(repo_path, wt);
                let mut command = crate::core::cmd::git_cmd();
                command
                    .args(["branch", "-D", "--end-of-options", expected_branch])
                    .current_dir(repo_path);
                let output = bounded(command, "branch -D")
                    .map_err(|e| format!("failed to delete integrated task branch: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "failed to delete integrated task branch {expected_branch}: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                Ok(())
            }
            Some(sha) => Err(format!(
                "integrated task branch {expected_branch} diverged to {sha} (expected {integrated_sha}) — preserving it"
            )),
        };
    }

    let actual_branch = find_branch_for_worktree(repo_path, worktree_path).ok_or_else(|| {
        branch_unknown(
            worktree_path,
            format!(
                "cannot determine branch for integrated worktree {worktree_path} — preserving it"
            ),
        )
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "integrated worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — preserving it"
        ));
    }
    let judged = Snapshot::of_worktree(wt)?;
    let head = judged_head(&judged)?;
    if head != integrated_sha {
        return Err(format!(
            "integrated worktree {worktree_path} diverged to {head} (expected {integrated_sha}) — preserving it"
        ));
    }
    if !judged_dirty_files(&judged)?.is_empty() {
        return Err(format!(
            "integrated worktree {worktree_path} has uncommitted changes — preserving it"
        ));
    }
    drop(judged);
    remove_discussion_worktree(repo_path, worktree_path, Some(expected_branch))
}

/// Validate that a worktree path still exists on disk.
pub fn validate_worktree(worktree_path: &str) -> bool {
    Path::new(worktree_path).exists()
}

// ── Test mode helpers ────────────────────────────────────────────────────────
//
// These wrap plain `git` calls used by the `test-mode/enter` + `exit`
// endpoints (see `api::disc_git`). They stay here (rather than in
// `api::git_ops`) because they operate on the same repo / worktree layout
// the rest of this module owns, and share the same sync `Command` wrappers.

/// Short description of a file that `git status --porcelain` considers
/// uncommitted in a given tree. Used to report dirty state back to the UI
/// so the user can see exactly what is at risk before confirming an action.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirtyFile {
    pub path: String,
    /// Porcelain v1 status code — two chars, index + worktree. `??` means
    /// untracked. We surface as-is; the UI translates to human text.
    pub status: String,
}

/// One committed path in the diff between an execution's pinned base and the
/// worker HEAD. Renames are deliberately exposed as one deletion plus one
/// addition: the delivery contract has only added/modified/deleted kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedFileChange {
    pub path: String,
    pub kind: char,
}

/// Current state of the repository at `repo_path` — what the user will see
/// if they `cd` into it right now. Used by `test-mode/enter` to decide
/// whether to block, stash, or proceed cleanly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MainRepoState {
    /// Current branch, e.g. `main` or `feature/x`. Empty string if detached HEAD.
    pub current_branch: String,
    /// True when HEAD is not pointing at a named branch (e.g. after a manual
    /// `git checkout <sha>`). We don't block, but the UI warns.
    pub is_detached: bool,
    /// Uncommitted + untracked files, porcelain output.
    pub dirty_files: Vec<DirtyFile>,
}

/// Parse `git status --porcelain=v1 -z` output into a list of dirty files: NUL-ended
/// `XY path` entries, names unquoted. A rename or a copy (`R`/`C`) is followed by its
/// source path, which is skipped: the target is the file the user needs to see.
fn parse_porcelain_z(stdout: &[u8]) -> Vec<DirtyFile> {
    let mut fields = stdout.split(|byte| *byte == 0);
    let mut files = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }
        if matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C') {
            fields.next();
        }
        files.push(DirtyFile {
            path: String::from_utf8_lossy(&field[3..]).into_owned(),
            status: String::from_utf8_lossy(&field[..2]).into_owned(),
        });
    }
    files
}

/// Check whether the given worktree has uncommitted changes (modified,
/// staged, or untracked).
///
/// Used as preflight #1 on `test-mode/enter`: if an agent left the worktree
/// dirty, unlocking it would lose the changes. We surface the list so the UI
/// can block with a "commit first" CTA rather than a blind refusal.
pub fn worktree_dirty_files(worktree_path: &Path) -> Result<Vec<DirtyFile>, String> {
    let mut command = crate::core::lane_git::git(worktree_path)?;
    command.args(DIRTY_STATUS);
    // Within git's time bound (KT-2084): a FIFO in the worktree never holds the caller.
    let output =
        bounded(command, "status").map_err(|e| format!("git status failed in worktree: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git status failed in worktree ({}): {}",
            worktree_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(dirty_files_of(worktree_path, &output.stdout))
}

/// `--ignore-submodules=dirty` on the command line: a `.gitmodules` the worker
/// wrote (`ignore = none`) must not make git run `status` inside a submodule.
const DIRTY_STATUS: [&str; 4] = [
    "status",
    "--ignore-submodules=dirty",
    "--porcelain=v1",
    "-z",
];

/// [`worktree_dirty_files`] judged from Kronn's copy `judged` of the worktree's git state, for
/// a removal or a workspace declaration (KT-2084).
pub fn judged_dirty_files(judged: &Snapshot) -> Result<Vec<DirtyFile>, String> {
    let worktree_path = judged
        .work_tree()
        .ok_or_else(|| format!("{} has no work tree", judged.source().display()))?;
    let output = judged.output(&DIRTY_STATUS)?;
    if !output.status.success() {
        return Err(format!(
            "git status failed in worktree ({}): {}",
            worktree_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(dirty_files_of(worktree_path, &output.stdout))
}

/// The HEAD commit of Kronn's copy `judged`.
fn judged_head(judged: &Snapshot) -> Result<String, String> {
    let output = judged.output(&["rev-parse", "HEAD"])?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed in {}: {}",
            judged.work_tree().unwrap_or(judged.source()).display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The dirty files of `status --porcelain=v1 -z` output in `worktree_path`. An untracked
/// `.DS_Store` is the file manager's, not work: a checkout made before the local exclude
/// carried it must not stay "dirty" for it (KT-1750).
fn dirty_files_of(worktree_path: &Path, stdout: &[u8]) -> Vec<DirtyFile> {
    let junk = os_junk_patterns();
    parse_porcelain_z(stdout)
        .into_iter()
        .filter(|file| {
            !(file.status == "??"
                && (is_os_junk(&file.path, &junk)
                    || (file.path.ends_with('/')
                        && holds_only_os_junk(
                            worktree_path,
                            &worktree_path.join(&file.path),
                            &junk,
                        ))))
        })
        .collect()
}

/// The most entries, and the longest time, an untracked folder is looked into for OS junk:
/// past either it is work.
const JUNK_WALK_ENTRIES: usize = 10_000;
const JUNK_WALK_TIME: std::time::Duration = std::time::Duration::from_secs(5);

/// An untracked directory is listed by git as one entry (`?? sub/`): it is junk
/// when nothing in it, at any depth, is anything else. A nested repository (a `.git` of any
/// kind) is somebody's work. Walked rooted at `root` (KT-2084): a folder swapped for a link is
/// never followed, and a walk that cannot finish within its bounds finds work.
fn holds_only_os_junk(root: &Path, dir: &Path, patterns: &[String]) -> bool {
    use crate::core::fs_guard::{guarded_walk, Visit, WalkBounds, WalkEnd};
    let bounds = WalkBounds::within(JUNK_WALK_ENTRIES, JUNK_WALK_TIME);
    let mut only_junk = true;
    let walked = guarded_walk(root, dir, bounds, &mut |entry| {
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if entry.depth == 0 || (entry.is_dir && name != ".git") {
            return Visit::Enter;
        }
        if name != ".git" && is_os_junk(&name, patterns) {
            Visit::Skip
        } else {
            only_junk = false;
            Visit::Stop
        }
    });
    matches!(walked, Ok(WalkEnd::Done)) && only_junk
}

/// Return the exact committed file changes between two revisions.
///
/// `-z` keeps paths with spaces/newlines unambiguous and `--no-renames` maps
/// Git's richer rename model onto the delivery manifest's closed A/M/D model.
pub fn committed_file_changes(
    worktree_path: &Path,
    base_rev: &str,
    head_rev: &str,
) -> Result<Vec<CommittedFileChange>, String> {
    let output = crate::core::lane_git::git(worktree_path)?
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-status",
            "--no-renames",
            "-z",
            "--end-of-options",
            base_rev,
            head_rev,
            "--",
        ])
        .output()
        .map_err(|e| format!("git diff failed in worktree: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff failed in worktree ({}): {}",
            worktree_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() % 2 != 0 {
        return Err("git diff returned an incomplete name-status record".into());
    }
    fields
        .as_chunks::<2>()
        .0
        .iter()
        .map(|record| {
            let status = std::str::from_utf8(record[0])
                .map_err(|error| format!("git diff status is not UTF-8: {error}"))?;
            let kind = status
                .chars()
                .next()
                .filter(|kind| matches!(kind, 'A' | 'M' | 'D' | 'T'))
                .ok_or_else(|| format!("unsupported git diff status `{status}`"))?;
            let path = std::str::from_utf8(record[1])
                .map_err(|error| format!("git diff path is not UTF-8: {error}"))?
                .to_string();
            Ok(CommittedFileChange {
                path,
                kind: if kind == 'T' { 'M' } else { kind },
            })
        })
        .collect()
}

/// `GIT_GRAFT_FILE` value that reads as "no grafts": `<git-dir>/info/grafts`
/// is shared by every worktree of the repository, so a worker can write it to
/// give a commit other parents and hide the commits behind it from a log.
pub const NO_GRAFTS_ENV: (&str, &str) = (
    "GIT_GRAFT_FILE",
    if cfg!(windows) { "NUL" } else { "/dev/null" },
);

/// The `(sha, full message)` records of `git log -z --format=%H%n%B`.
pub fn parse_commit_log(stdout: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(stdout)
        .split('\0')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let record = record.trim_start();
            let (sha, message) = record.split_once('\n').unwrap_or((record, ""));
            (sha.trim().to_string(), message.to_string())
        })
        .collect()
}

/// Raw bytes of `path` as committed at `rev`.
pub fn file_at_revision(worktree_path: &Path, rev: &str, path: &str) -> Result<Vec<u8>, String> {
    reject_option_like_rev(rev)?;
    let output = crate::core::lane_git::git(worktree_path)?
        .args(["cat-file", "blob", &format!("{rev}:{path}")])
        .output()
        .map_err(|e| format!("git cat-file failed in worktree: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read `{path}` at {rev}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// The `Name <email>` that `git commit -s` signs with in this worktree.
pub fn committer_identity(worktree_path: &Path) -> Result<String, String> {
    let output = crate::core::lane_git::git(worktree_path)?
        .args(["var", "GIT_COMMITTER_IDENT"])
        .output()
        .map_err(|e| format!("git var failed in worktree: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git has no committer identity in this worktree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let ident = String::from_utf8_lossy(&output.stdout);
    let end = ident
        .rfind('>')
        .ok_or_else(|| format!("unexpected git committer identity `{}`", ident.trim()))?;
    Ok(ident[..=end].trim().to_string())
}

/// Snapshot the main repo's state (current branch + dirty files).
///
/// Used as preflight #2 + #3 on `test-mode/enter`. Returns an empty
/// `current_branch` and `is_detached = true` when HEAD is not on a named
/// branch — the UI warns but does not block.
pub fn main_repo_state(repo_path: &Path) -> Result<MainRepoState, String> {
    // Branch / detached HEAD detection. `symbolic-ref -q HEAD` returns
    // "refs/heads/<name>" on a branch and fails (exit 1) on detached HEAD —
    // that's how we distinguish the two states without parsing `git status`.
    // Each within git's time bound (KT-2084).
    let mut sym = crate::core::cmd::git_cmd();
    sym.args(["symbolic-ref", "-q", "--short", "HEAD"])
        .current_dir(repo_path);
    let sym =
        bounded(sym, "symbolic-ref").map_err(|e| format!("git symbolic-ref failed: {}", e))?;
    let (current_branch, is_detached) = if sym.status.success() {
        (
            String::from_utf8_lossy(&sym.stdout).trim().to_string(),
            false,
        )
    } else {
        (String::new(), true)
    };

    let mut status = crate::core::cmd::git_cmd();
    status
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(repo_path);
    let status = bounded(status, "status").map_err(|e| format!("git status failed: {}", e))?;
    if !status.status.success() {
        return Err(format!(
            "git status failed in repo ({}): {}",
            repo_path.display(),
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    let dirty_files = parse_porcelain_z(&status.stdout);
    Ok(MainRepoState {
        current_branch,
        is_detached,
        dirty_files,
    })
}

/// `git checkout <branch>` in the main repo.
///
/// We run plain `checkout` (not `-f`) — the caller guarantees the repo is
/// clean (via `main_repo_state`) or has stashed beforehand. If checkout
/// fails (conflict, unknown branch, etc.), we return the stderr verbatim so
/// the caller can surface a precise error to the user instead of a
/// generic 500.
pub fn checkout_branch(repo_path: &Path, branch: &str) -> Result<(), String> {
    // A branch name, never an option nor a path (KT-2084).
    crate::core::lane_git::check_branch_name(branch)?;
    let mut checkout = crate::core::cmd::git_cmd();
    checkout
        .args(["checkout", branch, "--"])
        .current_dir(repo_path);
    let output =
        bounded_change(checkout, "checkout").map_err(|e| format!("git checkout failed: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git checkout {} failed: {}",
            branch,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// `git stash push -u -m <message>`. Includes untracked files so nothing is
/// left behind when the user enters test mode.
///
/// Returns `true` if something was actually stashed (there were changes to
/// stash), `false` if the working tree was already clean — mirrors git's
/// own behavior where `stash push` on a clean tree succeeds but does
/// nothing. The caller stores the message only when `true` so a later
/// `stash_pop_by_message` can find the exact stash even if the user stashed
/// manually in between.
pub fn stash_push(repo_path: &Path, message: &str) -> Result<bool, String> {
    let mut push = crate::core::cmd::git_cmd();
    push.args(["stash", "push", "-u", "-m", message])
        .current_dir(repo_path);
    let output =
        bounded_change(push, "stash push").map_err(|e| format!("git stash push failed: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git stash push failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // git prints "No local changes to save" (on stdout) when the tree is
    // already clean. Anything else (e.g. "Saved working directory...")
    // means a stash was created.
    Ok(!stdout.contains("No local changes to save"))
}

/// Pop the stash whose message matches `message` (exact match).
///
/// Used by `test-mode/exit` to restore the user's pre-test state. If the
/// stash has disappeared (user popped it manually, cleared stashes, etc.)
/// we report a clear error — we do NOT guess which stash to pop, it's
/// safer to tell the user than to restore the wrong thing.
pub fn stash_pop_by_message(repo_path: &Path, message: &str) -> Result<(), String> {
    // Find the stash ref matching the message. `git stash list` output
    // looks like: `stash@{0}: On main: <message>`.
    let mut list = crate::core::cmd::git_cmd();
    list.args(["stash", "list"]).current_dir(repo_path);
    let list = bounded(list, "stash list").map_err(|e| format!("git stash list failed: {}", e))?;
    if !list.status.success() {
        return Err("git stash list failed".into());
    }
    let list_str = String::from_utf8_lossy(&list.stdout);
    let stash_ref = list_str
        .lines()
        .find(|l| l.contains(message))
        .and_then(|l| l.split(':').next())
        .ok_or_else(|| {
            format!(
                "stash '{}' not found — was it dropped manually? \
             Run `git stash list` to inspect, then `git stash pop <ref>`.",
                message
            )
        })?;

    let mut pop = crate::core::cmd::git_cmd();
    pop.args(["stash", "pop", stash_ref]).current_dir(repo_path);
    let output =
        bounded_change(pop, "stash pop").map_err(|e| format!("git stash pop failed: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git stash pop {} failed (conflicts?): {}",
            stash_ref,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── KT-373 — cleanup: every refusal gets its own proof ───────────────

    #[test]
    fn a_terminal_worktree_gives_its_target_back() {
        let repo = make_test_repo("clean-terminal");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0001");
        let target = managed_target(repo.path(), "KT-9-aaaa0001", 128);
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(worktree.join("src/main.rs"), b"fn main() {}").unwrap();

        let report =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
                .expect("a finished execution releases its artefacts");

        assert!(!target.exists(), "the target is gone");
        assert!(
            report.bytes_reclaimed >= 128,
            "the audit trail carries a figure"
        );
        // Cleanup removes what a compiler can rebuild, and nothing else.
        assert!(
            worktree.join("src/main.rs").exists(),
            "sources are never touched"
        );
        assert!(worktree.exists(), "the worktree itself survives");
    }

    #[test]
    fn an_active_execution_keeps_its_target() {
        let repo = make_test_repo("clean-active");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0002");
        let target = managed_target(repo.path(), "KT-9-aaaa0002", 16);

        let error = clean_worktree_build_artifacts(
            repo.path(),
            &worktree,
            ExecutionLiveness::Active("its execution is still in flight".into()),
        )
        .expect_err("work in flight is never cleaned");

        assert!(error.contains("still in flight"), "got: {error}");
        assert!(target.exists(), "nothing was removed");
    }

    #[test]
    fn an_unknown_execution_state_refuses_rather_than_guesses() {
        // The 2026-08-21 shape: nothing says this worktree is busy, and that is
        // exactly not the same as something saying it is finished.
        let repo = make_test_repo("clean-unknown");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0003");
        let target = managed_target(repo.path(), "KT-9-aaaa0003", 16);

        let error = clean_worktree_build_artifacts(
            repo.path(),
            &worktree,
            ExecutionLiveness::Unknown(
                "no durable execution state says this worktree is finished".into(),
            ),
        )
        .expect_err("silence is not permission");

        assert!(error.contains("no durable execution state"), "got: {error}");
        assert!(target.exists(), "nothing was removed");
    }

    #[test]
    fn a_path_outside_the_managed_root_is_refused_before_it_is_read() {
        let repo = make_test_repo("clean-outside");
        let outside = repo.path().join("some-other-checkout");
        fs::create_dir_all(outside.join("target/debug")).unwrap();
        fs::write(outside.join("target/debug/artifact"), b"x").unwrap();

        let error =
            clean_worktree_build_artifacts(repo.path(), &outside, ExecutionLiveness::Terminal)
                .expect_err("ownership decides, not the caller's confidence");

        assert!(error.contains("outside managed root"), "got: {error}");
        assert!(outside.join("target/debug/artifact").exists());
    }

    #[test]
    fn a_worktree_with_nothing_to_reclaim_succeeds_at_zero() {
        // The caller asked for the artefacts to be gone. They are.
        let repo = make_test_repo("clean-empty");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0004");
        fs::create_dir_all(&worktree).unwrap();

        let report =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
                .expect("absence is not a failure");

        assert_eq!(report.bytes_reclaimed, 0);
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn a_symlinked_target_is_refused_instead_of_followed() {
        let repo = make_test_repo("clean-symlink");
        let elsewhere = repo.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("precious"), b"not ours").unwrap();
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0005");
        fs::create_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(&elsewhere, worktree.join("target")).unwrap();

        let error =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
                .expect_err("a symlink is never followed into someone else's storage");

        assert!(error.contains("symlink"), "got: {error}");
        assert!(
            elsewhere.join("precious").exists(),
            "the link target must be untouched",
        );
    }

    #[test]
    fn a_running_cargo_build_blocks_the_cleanup_it_would_otherwise_allow() {
        // Cargo holds this lock for the duration of a build, so taking it is a
        // direct answer to "is a build running" — unlike a process scan, which
        // the incident showed to be unreliable in the other direction.
        use fs2::FileExt;
        let repo = make_test_repo("clean-building");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0006");
        let target = managed_target(repo.path(), "KT-9-aaaa0006", 16);
        let lock_path = target.join("debug/.cargo-lock");
        fs::write(&lock_path, b"").unwrap();
        let held = std::fs::File::open(&lock_path).unwrap();
        held.lock_exclusive().unwrap();

        let error =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
                .expect_err("a live build wins over a stale terminal state");

        assert!(error.contains("build lock"), "got: {error}");
        assert!(
            target.exists(),
            "nothing was removed while cargo was working"
        );
        fs2::FileExt::unlock(&held).unwrap();

        // Once the build releases it, the same call proceeds.
        clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
            .expect("the refusal was about the lock, not about the worktree");
        assert!(!target.exists());
    }

    #[cfg(unix)] // creates symlinks through std::os::unix; asserts POSIX mode bits, which Windows does not have
    #[test]
    fn a_target_we_cannot_even_look_at_is_refused_not_reported_as_absent() {
        // "I could not look" must never be reported as "nothing to do". Absence
        // is the ONLY error that justifies a success at zero; a permission or
        // I/O failure means we do not know what is there, and answering `Ok(0)`
        // would tell the caller the artefacts are gone when they may not be.
        use std::os::unix::fs::PermissionsExt;
        let repo = make_test_repo("clean-unreadable");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0009");
        managed_target(repo.path(), "KT-9-aaaa0009", 16);
        let original = fs::metadata(&worktree).unwrap().permissions();
        // No execute bit: the directory cannot be traversed, so stat'ing its
        // child fails with a permission error rather than NotFound.
        fs::set_permissions(&worktree, fs::Permissions::from_mode(0o600)).unwrap();

        let outcome =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal);

        fs::set_permissions(&worktree, original).unwrap();

        // Running as root defeats the permission bit; assert only when the OS
        // actually refused.
        if let Err(reason) = outcome {
            assert!(reason.contains("cannot read"), "got: {reason}");
            assert!(reason.contains("Nothing was attempted"), "got: {reason}");
        }
    }

    #[cfg(unix)] // creates symlinks through std::os::unix; asserts POSIX mode bits, which Windows does not have
    #[test]
    fn a_deletion_that_cannot_complete_stops_on_that_target_and_says_so() {
        // DoD-7: a permission failure must produce an actionable diagnostic and
        // stop, never a broader or more forceful retry. The worktree directory
        // is made non-writable, so removing `target/` from it is refused by the
        // OS rather than by us — which is the case we cannot control.
        use std::os::unix::fs::PermissionsExt;
        let repo = make_test_repo("clean-denied");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0007");
        let target = managed_target(repo.path(), "KT-9-aaaa0007", 16);
        let original = fs::metadata(&worktree).unwrap().permissions();
        fs::set_permissions(&worktree, fs::Permissions::from_mode(0o500)).unwrap();

        let outcome =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal);

        // Restore before asserting, so a failed assertion cannot leave an
        // undeletable directory behind for the next run.
        fs::set_permissions(&worktree, original).unwrap();

        // Running as root defeats the permission bit entirely; the assertion is
        // only meaningful when the OS actually refused.
        if let Err(reason) = outcome {
            assert!(reason.contains("failed to clean"), "got: {reason}");
            assert!(
                reason.contains("left as it stands"),
                "the diagnostic must say nothing further was attempted: {reason}",
            );
            assert!(
                target.exists(),
                "a refused deletion leaves the target intact"
            );

            // Reprise: with the obstacle gone, the same call completes. A
            // failure must not poison the target for later attempts.
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
                .expect("the refusal was about permissions, not about this worktree");
            assert!(!target.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_reparse_point_target_is_refused_instead_of_followed() {
        // The Unix twin of this test covers symlinks; on Windows the same
        // shape arrives as a directory symlink / reparse point, and
        // `file_type().is_symlink()` reports it. Creating one may require
        // developer mode or elevation, so a refusal to create is skipped
        // rather than failed — the assertion is about our behaviour, not the
        // host's privileges.
        let repo = make_test_repo("clean-reparse");
        let elsewhere = repo.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("precious"), b"not ours").unwrap();
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0008");
        fs::create_dir_all(&worktree).unwrap();
        if std::os::windows::fs::symlink_dir(&elsewhere, worktree.join("target")).is_err() {
            return;
        }

        let error =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal)
                .expect_err("a reparse point is never followed into someone else's storage");

        assert!(error.contains("symlink"), "got: {error}");
        assert!(elsewhere.join("precious").exists());
    }

    // ── KT-373 — read-only inventory of build artefacts ──────────────────

    /// Lay out a managed worktree with a `target/` holding one file.
    fn managed_target(repo: &Path, name: &str, bytes: usize) -> PathBuf {
        let worktree = repo.join(".kronn/worktrees").join(name);
        let target = worktree.join("target/debug");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("artifact.rlib"), vec![b'x'; bytes]).unwrap();
        worktree.join("target")
    }

    #[test]
    fn the_scan_reports_every_managed_target_in_a_stable_order() {
        // DoD-3 asks for a DETERMINISTIC dry-run: what a human is shown has to
        // be what a later pass acts on. Filesystem enumeration order is not
        // stable, and mtime granularity is a second on some filesystems, so the
        // property pinned here is the one actually guaranteed — same input,
        // same order, with the path breaking any tie.
        let repo = make_test_repo("scan-order");
        let first = managed_target(repo.path(), "KT-1-aaaa1111", 16);
        let second = managed_target(repo.path(), "KT-2-bbbb2222", 32);

        let found = scan_build_artifacts(repo.path());
        assert_eq!(found.len(), 2, "both managed targets are inventoried");

        let again = scan_build_artifacts(repo.path());
        assert_eq!(
            found.iter().map(|t| &t.target_path).collect::<Vec<_>>(),
            again.iter().map(|t| &t.target_path).collect::<Vec<_>>(),
            "two scans of an unchanged tree must agree",
        );

        let listed: Vec<&PathBuf> = found.iter().map(|t| &t.target_path).collect();
        assert!(listed.contains(&&first) && listed.contains(&&second));
        let measured = found.iter().find(|t| t.target_path == second).unwrap();
        assert!(
            measured.bytes >= 32,
            "sizes are measured, got {}",
            measured.bytes
        );
        assert!(!measured.size_is_partial, "a tiny tree is measured exactly");
    }

    #[test]
    fn the_scan_never_leaves_the_managed_root() {
        // A `target/` next to the repo, outside `.kronn/worktrees`, belongs to
        // whoever put it there. The inventory is built from Kronn's ownership,
        // not from a filesystem glob.
        let repo = make_test_repo("scan-outside");
        fs::create_dir_all(repo.path().join("target/debug")).unwrap();
        fs::write(repo.path().join("target/debug/main"), b"x").unwrap();
        managed_target(repo.path(), "KT-3-cccc3333", 8);

        let found = scan_build_artifacts(repo.path());

        let reclaimable: Vec<&BuildArtifactTarget> =
            found.iter().filter(|t| t.refusal.is_none()).collect();
        assert_eq!(reclaimable.len(), 1);
        assert!(
            reclaimable[0]
                .target_path
                .starts_with(repo.path().join(".kronn/worktrees")),
            "only managed checkouts are reclaimable, got {:?}",
            reclaimable[0].target_path,
        );
    }

    #[cfg(unix)] // creates symlinks through std::os::unix; reads unix inode metadata
    #[test]
    fn a_symlinked_target_is_listed_as_refused_rather_than_hidden() {
        // Following it would offer to delete storage this repository does not
        // own — but omitting it entirely is worse: someone hunting for space
        // concludes the tool found nothing and goes deleting by hand, which is
        // the 2026-08-21 sequence. What we refuse to touch is what a human most
        // needs to see.
        let repo = make_test_repo("scan-symlink");
        let elsewhere = repo.path().join("elsewhere");
        fs::create_dir_all(elsewhere.join("debug")).unwrap();
        fs::write(elsewhere.join("debug/huge.rlib"), vec![b'x'; 64]).unwrap();
        let worktree = repo.path().join(".kronn/worktrees/KT-4-dddd4444");
        fs::create_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(&elsewhere, worktree.join("target")).unwrap();

        let found = scan_build_artifacts(repo.path());

        assert_eq!(found.len(), 1, "the entry is reported, not dropped");
        let reason = found[0]
            .refusal
            .as_deref()
            .expect("it must carry its reason");
        assert!(reason.contains("symlink"), "got: {reason}");
        assert_eq!(
            found[0].bytes, 0,
            "no size is claimed for what we would not remove"
        );
    }

    #[test]
    fn a_target_that_is_a_file_is_reported_as_refused() {
        let repo = make_test_repo("scan-not-dir");
        let worktree = repo.path().join(".kronn/worktrees/KT-7-gggg7777");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join("target"), b"not a directory").unwrap();

        let found = scan_build_artifacts(repo.path());

        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .refusal
                .as_deref()
                .unwrap()
                .contains("not a directory"),
            "got: {:?}",
            found[0].refusal,
        );
    }

    #[test]
    fn a_worktree_without_a_target_is_simply_absent() {
        let repo = make_test_repo("scan-empty");
        fs::create_dir_all(repo.path().join(".kronn/worktrees/KT-5-eeee5555/src")).unwrap();

        assert!(
            scan_build_artifacts(repo.path()).is_empty(),
            "nothing to reclaim is not the same as something of size zero",
        );
    }

    #[test]
    fn a_repo_that_never_provisioned_scans_to_nothing() {
        let repo = make_test_repo("scan-none");
        assert!(scan_build_artifacts(repo.path()).is_empty());
    }

    #[test]
    fn an_oversized_tree_reports_a_floor_rather_than_stalling() {
        // The incident's `target/` held 1.69 million files and took 44 minutes
        // to walk, on a machine that was already unusable. A partial answer now
        // is worth more than an exact one after the disk fills.
        let repo = make_test_repo("scan-budget");
        let target = repo
            .path()
            .join(".kronn/worktrees/KT-6-ffff6666/target/debug");
        fs::create_dir_all(&target).unwrap();
        for i in 0..(scan_entry_budget() + 50) {
            fs::write(target.join(format!("dep-{i}.rmeta")), b"x").unwrap();
        }

        let found = scan_build_artifacts(repo.path());

        assert_eq!(found.len(), 1);
        assert!(
            found[0].size_is_partial,
            "the walk must admit it stopped rather than imply an exact size",
        );
    }

    // ── KT-373 — disk as a provisioning precondition ─────────────────────

    #[test]
    fn a_full_disk_refuses_provisioning_and_names_the_setting() {
        // The thresholds are in GiB and the real filesystem has some free
        // space, so a critical threshold above whatever is actually available
        // reproduces the full-disk verdict without needing a full disk.
        let here = std::env::temp_dir();
        let available_gib = fs2::available_space(&here).unwrap() / BYTES_PER_GIB;

        let verdict = disk_headroom(&here, available_gib + 100, available_gib + 50);
        assert!(
            matches!(verdict, DiskHeadroom::Critical { .. }),
            "below the critical mark the answer is a refusal, got {verdict:?}",
        );

        let error = ensure_disk_headroom(&here, available_gib + 100, available_gib + 50)
            .expect_err("provisioning must be refused");
        // Whoever reads this is on a machine that is about to stop working.
        // The message has to carry both the number and the knob.
        assert!(error.contains("refusing to provision"), "got: {error}");
        assert!(error.contains("disk_critical_gib"), "got: {error}");
    }

    #[test]
    fn a_full_disk_refuses_a_build_with_the_same_configured_knob() {
        let here = std::env::temp_dir();
        let available_gib = fs2::available_space(&here).unwrap() / BYTES_PER_GIB;

        let error = ensure_disk_headroom_for(
            &here,
            available_gib + 100,
            available_gib + 50,
            "run a build or validation",
        )
        .expect_err("a build must be refused below the critical threshold");

        assert!(
            error.contains("refusing to run a build or validation"),
            "got: {error}"
        );
        assert!(error.contains("disk_critical_gib"), "got: {error}");
    }

    #[test]
    fn build_headroom_measures_a_fresh_target_parent_without_creating_output() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("target with spaces");
        ensure_build_disk_headroom(&absent).unwrap();
        assert!(
            !absent.exists(),
            "the guard must leave first-build target creation to Cargo"
        );
    }

    #[test]
    fn build_headroom_refuses_a_fresh_target_below_a_missing_parent() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("missing-parent").join("target");

        let error = ensure_build_disk_headroom(&target).unwrap_err();
        assert!(error.contains("parent"), "got: {error}");
        assert!(
            !target.exists(),
            "the guard must not create arbitrary parents"
        );
    }

    #[test]
    fn build_headroom_refuses_a_target_symlink() {
        let root = tempfile::tempdir().unwrap();
        let real_target = root.path().join("real-target");
        std::fs::create_dir(&real_target).unwrap();
        let target = root.path().join("target-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_target, &target).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real_target, &target).unwrap();

        let error = ensure_build_disk_headroom(&target).unwrap_err();
        assert!(error.contains("is a symlink"), "got: {error}");
    }

    #[test]
    fn build_headroom_refuses_a_file_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target-file");
        std::fs::write(&target, "not a directory").unwrap();

        let error = ensure_build_disk_headroom(&target).unwrap_err();
        assert!(error.contains("not a directory"), "got: {error}");
    }

    #[test]
    fn a_tight_disk_warns_without_blocking_the_work() {
        let here = std::env::temp_dir();
        let available_gib = fs2::available_space(&here).unwrap() / BYTES_PER_GIB;

        let verdict = disk_headroom(&here, available_gib + 50, 0);
        assert!(
            matches!(verdict, DiskHeadroom::Low { .. }),
            "between the two marks the answer is a warning, got {verdict:?}",
        );
        assert!(
            ensure_disk_headroom(&here, available_gib + 50, 0).is_ok(),
            "a warning must not refuse: the work still fits",
        );
    }

    #[test]
    fn a_roomy_disk_says_nothing() {
        let here = std::env::temp_dir();
        assert_eq!(disk_headroom(&here, 0, 0), DiskHeadroom::Ok);
    }

    #[test]
    fn a_warning_below_the_critical_mark_cannot_silently_disarm_the_refusal() {
        // A config where warning < critical is contradictory. Honouring it
        // literally would let a low warning imply "merely warn" on a disk that
        // is in fact below the refusal line — a misconfiguration that reads as
        // safe. The critical mark wins.
        let here = std::env::temp_dir();
        let available_gib = fs2::available_space(&here).unwrap() / BYTES_PER_GIB;

        let verdict = disk_headroom(&here, 1, available_gib + 50);
        assert!(
            matches!(verdict, DiskHeadroom::Critical { .. }),
            "the critical threshold must still refuse, got {verdict:?}",
        );
    }

    #[test]
    fn an_unreadable_filesystem_lets_the_work_through() {
        // Deliberately the opposite of fail-closed. This guard exists to stop a
        // disk from filling, not to become a new way for provisioning to fail:
        // a path we cannot measure is not evidence of a problem, and refusing
        // on no evidence would break every user to protect none. Inject the
        // failed probe directly: Windows reports its containing volume for a
        // missing child path, whereas Unix rejects that same path.
        assert_eq!(
            classify_disk_headroom(None, u64::MAX, u64::MAX),
            DiskHeadroom::Ok
        );
    }

    #[test]
    fn configured_thresholds_reach_the_provisioning_path() {
        // The guard reads atomics rather than a threaded-through parameter, so
        // the wiring itself is worth one assertion: what the config publishes is
        // what provisioning enforces.
        let (warning_before, critical_before) = configured_disk_thresholds();
        set_disk_thresholds(42, 7);
        assert_eq!(configured_disk_thresholds(), (42, 7));
        set_disk_thresholds(warning_before, critical_before);
    }

    /// Create a temporary git repo for testing.
    fn current_branch(repo_path: &Path) -> Option<String> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(repo_path)
            .output()
            .ok()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }

    fn make_test_repo(name: &str) -> tempfile::TempDir {
        let dir = tempfile::Builder::new()
            .prefix(&format!("kronn-wt-{}", name))
            .tempdir()
            .unwrap();
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Windows runners convert line endings by default: a reapplied patch
        // would then differ from the checkout it was taken from.
        std::process::Command::new("git")
            .args(["config", "core.autocrlf", "false"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Need at least one commit for worktrees to work
        fs::write(dir.path().join("README.md"), "# test").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    /// A repo carrying one committed, materialized submodule — the shape KT-478
    /// gives every task worktree, and the shape `git worktree remove` refuses
    /// without `--force`. The returned upstream `TempDir` must outlive the
    /// `submodule add`; it is handed back so the caller keeps it alive.
    fn make_test_repo_with_submodule(name: &str) -> (tempfile::TempDir, tempfile::TempDir) {
        let upstream = tempfile::Builder::new()
            .prefix(&format!("kronn-wt-{}-sub", name))
            .tempdir()
            .unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(upstream.path())
                .output()
                .unwrap();
        }
        fs::write(upstream.path().join("lib.txt"), "shared").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "sub init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(upstream.path())
                .output()
                .unwrap();
        }

        let repo = make_test_repo(name);
        // Local-file submodules require the explicit protocol opt-in (git CVE
        // hardening); the same flag Kronn uses on real checkouts.
        let add = std::process::Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                &upstream.path().to_string_lossy(),
                "sub",
            ])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            add.status.success(),
            "submodule add failed: {}",
            String::from_utf8_lossy(&add.stderr)
        );
        std::process::Command::new("git")
            .args(["commit", "-m", "add submodule"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        (repo, upstream)
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("My Project"), "my-project");
        assert_eq!(slugify("Fix bug #123"), "fix-bug-123");
        assert_eq!(slugify("  spaces  and---dashes  "), "spaces-and-dashes");
        assert_eq!(slugify("UPPER_case"), "upper-case");
    }

    #[test]
    fn test_slugify_already_slugified() {
        assert_eq!(slugify("my-branch"), "my-branch");
        assert_eq!(slugify("feat-add-thing"), "feat-add-thing");
    }

    #[test]
    fn test_slugify_special_chars() {
        // Slashes, @, ! become dashes, then consecutive dashes collapse
        assert_eq!(slugify("feat/add-@thing!"), "feat-add-thing");
    }

    #[test]
    fn test_slugify_unicode() {
        // Non-alphanumeric unicode chars (accented letters are alphanumeric in Rust)
        // 'é' is alphanumeric → kept as-is; let's verify it doesn't panic
        let result = slugify("café");
        // "café" lowercased is "café", all chars alphanumeric → no dashes → "café"
        assert_eq!(result, "café");

        // Non-alphanumeric unicode punctuation gets replaced
        let result2 = slugify("hello•world");
        assert_eq!(result2, "hello-world");
    }

    #[test]
    fn test_slugify_empty_string() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn test_slugify_caps_at_max_len() {
        // A 200-char input must be capped to MAX_SLUG_LEN (60).
        let long = "a".repeat(200);
        let result = slugify(&long);
        assert!(
            result.len() <= MAX_SLUG_LEN,
            "slug must be <= MAX_SLUG_LEN, got {}",
            result.len()
        );
        assert_eq!(result.len(), MAX_SLUG_LEN);
    }

    #[test]
    fn test_slugify_truncation_does_not_leave_trailing_dash() {
        // 30 chars + dash + 30 chars = 61 chars → truncation lands on the dash boundary
        let s = format!("{}-{}", "a".repeat(30), "b".repeat(30));
        let result = slugify(&s);
        assert!(result.len() <= MAX_SLUG_LEN);
        assert!(
            !result.ends_with('-'),
            "truncated slug must not end with a dash, got {:?}",
            result
        );
    }

    #[test]
    fn test_truncate_slug_unicode_safe() {
        // Truncation must operate on chars, not bytes, to avoid panicking
        // mid-codepoint with unicode slugs (e.g. lots of "é").
        let s = "é".repeat(200);
        let result = truncate_slug(&s);
        assert!(result.chars().count() <= MAX_SLUG_LEN);
    }

    #[test]
    fn test_long_path_noop_on_unix() {
        // long_path is a no-op on non-Windows. Confirm it returns the same path.
        let p = PathBuf::from("/home/user/project");
        assert_eq!(long_path(&p), p);
    }

    #[test]
    fn test_worktree_base_dir() {
        let repo = PathBuf::from("/home/user/project");
        let base = worktree_base_dir(&repo);
        assert_eq!(base, PathBuf::from("/home/user/project/.kronn/worktrees"));
    }

    #[test]
    fn test_validate_worktree_nonexistent() {
        assert!(!validate_worktree("/nonexistent/path/that/does/not/exist"));
    }

    #[test]
    fn a_folder_that_is_no_checkout_lists_no_worktree() {
        // An error, never an empty list.
        let dir = tempfile::tempdir().unwrap();
        assert!(registered_worktree_paths(dir.path()).is_err());
    }

    // ── Worktree lifecycle tests ─────────────────────────────────────────────

    #[test]
    fn test_create_discussion_worktree_creates_branch_and_dir() {
        let repo = make_test_repo("create");
        let result = create_discussion_worktree(repo.path(), "myproject", "fix-bug", "main");
        assert!(
            result.is_ok(),
            "create_discussion_worktree failed: {:?}",
            result.err()
        );
        let info = result.unwrap();
        assert_eq!(info.branch, "kronn/fix-bug");
        assert!(!info.is_main_repo);
        assert!(
            Path::new(&info.path).exists(),
            "Worktree directory should exist"
        );
        assert!(
            Path::new(&info.path).join(".git").exists(),
            "Worktree .git file should exist"
        );
    }

    #[test]
    fn test_create_worktree_in_kronn_worktrees_dir() {
        let repo = make_test_repo("basedir");
        let result = create_discussion_worktree(repo.path(), "proj", "feat", "main").unwrap();
        let expected_base = repo.path().join(".kronn/worktrees");
        assert!(result
            .path
            .starts_with(&expected_base.to_string_lossy().to_string()));
    }

    #[test]
    fn test_fix_worktree_paths_writes_relative() {
        let repo = make_test_repo("relpath");
        let info = create_discussion_worktree(repo.path(), "proj", "test-rel", "main").unwrap();
        let wt_path = Path::new(&info.path);
        let wt_name = wt_path.file_name().unwrap().to_string_lossy();

        // Forward .git file must be the 3-level relative form for a .kronn/worktrees/<name>
        // checkout. Assert EXACT content: `../../../.git/...` CONTAINS `../../.git/...`
        // as a substring, so the old `contains(..)` check passed for the buggy 2-level
        // form too — that is precisely what let the off-by-one ship (KT-331).
        let dot_git_content = fs::read_to_string(wt_path.join(".git")).unwrap();
        assert_eq!(
            dot_git_content,
            format!("gitdir: ../../../.git/worktrees/{}", wt_name),
            "forward gitdir must be the 3-level relative form, got: {}",
            dot_git_content
        );

        // Reverse gitdir points back from the repo root to the worktree's .git.
        let gitdir_content = fs::read_to_string(
            repo.path()
                .join(".git")
                .join("worktrees")
                .join(wt_name.as_ref())
                .join("gitdir"),
        )
        .unwrap();
        assert_eq!(
            gitdir_content,
            format!("../../../.kronn/worktrees/{}/.git\n", wt_name),
            "back-reference must climb to the repo root (else git marks it prunable), got: {}",
            gitdir_content
        );
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn fix_worktree_paths_never_writes_through_a_link_at_either_name() {
        // KT-1595: the two reference files are written through the rooted
        // primitive, so a link at either name is refused (the fix is skipped).
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt = repo.join(".kronn/worktrees/feat");
        let admin = repo.join(".git/worktrees/feat");
        fs::create_dir_all(&wt).unwrap();
        fs::create_dir_all(&admin).unwrap();
        let external = tmp.path().join("external");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("dot-git"), "OUTSIDE-1").unwrap();
        fs::write(external.join("gitdir"), "OUTSIDE-2").unwrap();
        std::os::unix::fs::symlink(external.join("dot-git"), wt.join(".git")).unwrap();
        std::os::unix::fs::symlink(external.join("gitdir"), admin.join("gitdir")).unwrap();

        fix_worktree_paths(&repo, &wt);

        assert_eq!(
            fs::read_to_string(external.join("dot-git")).unwrap(),
            "OUTSIDE-1"
        );
        assert_eq!(
            fs::read_to_string(external.join("gitdir")).unwrap(),
            "OUTSIDE-2"
        );
        assert!(fs::symlink_metadata(wt.join(".git")).unwrap().is_symlink());
        assert!(fs::symlink_metadata(admin.join("gitdir"))
            .unwrap()
            .is_symlink());
        // Regular files are rewritten as before.
        fs::remove_file(wt.join(".git")).unwrap();
        fs::remove_file(admin.join("gitdir")).unwrap();
        fs::write(wt.join(".git"), "stale").unwrap();
        fs::write(admin.join("gitdir"), "stale").unwrap();
        fix_worktree_paths(&repo, &wt);
        assert_eq!(
            fs::read_to_string(wt.join(".git")).unwrap(),
            "gitdir: ../../../.git/worktrees/feat"
        );
        assert_eq!(
            fs::read_to_string(admin.join("gitdir")).unwrap(),
            "../../../.kronn/worktrees/feat/.git\n"
        );
    }

    #[test]
    fn git_commands_run_inside_a_discussion_worktree() {
        // Regression for KT-331: the gitdir rewrite must let real git commands run
        // from INSIDE the worktree, not merely leave a `.git` file present. The prior
        // tests only checked the file existed / a substring of its content, so the
        // off-by-one depth (`../../.git` instead of `../../../.git`) shipped silently.
        let repo = make_test_repo("git-in-wt");
        let info = create_discussion_worktree(repo.path(), "proj", "run-git", "main").unwrap();
        let wt = Path::new(&info.path);

        for args in [&["rev-parse", "HEAD"][..], &["status", "--short"][..]] {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(wt)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} inside the discussion worktree failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        // The repo side must resolve the worktree via the back-reference to its REAL
        // path — not a malformed one nested under .git/worktrees that git marks
        // `prunable` (the second gitdir defect KT-331 fixes). Match on the unique dir
        // name to stay robust to the /tmp <-> /private/tmp symlink on macOS.
        let list = std::process::Command::new("git")
            .args(["worktree", "list"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(list.status.success());
        let listing = String::from_utf8_lossy(&list.stdout);
        let our_line = listing
            .lines()
            .find(|l| l.contains("kronn/run-git"))
            .unwrap_or_else(|| panic!("git worktree list missing our worktree:\n{}", listing));
        assert!(
            !our_line.contains("prunable"),
            "back-reference is malformed (worktree marked prunable): {}",
            our_line
        );
        assert!(
            !our_line.contains(".git/worktrees/proj--run-git/.kronn"),
            "back-reference resolved under .git/worktrees instead of the repo root: {}",
            our_line
        );
    }

    #[test]
    fn task_worktree_gitdir_is_portable_relative() {
        // KT-331: create_task_worktree now adopts the corrected relative rewrite instead
        // of the native absolute gitdir, so the checkout is portable host<->container.
        // It must write the 3-level relative form AND stay usable by git run inside it
        // (verify_worktree_head already runs `git rev-parse HEAD` inside during creation,
        // so a wrong depth would have failed create_task_worktree outright).
        let repo = make_test_repo("task-portable");
        let head = git_head(repo.path()).unwrap();
        let info = create_task_worktree(repo.path(), "KT-999", "abcd1234", &head).unwrap();
        let wt = Path::new(&info.path);
        let wt_name = wt.file_name().unwrap().to_string_lossy();

        let dot_git = fs::read_to_string(wt.join(".git")).unwrap();
        assert_eq!(
            dot_git,
            format!("gitdir: ../../../.git/worktrees/{}", wt_name),
            "task worktree gitdir must be the portable 3-level relative form, got: {}",
            dot_git
        );

        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(wt)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git rev-parse inside the task worktree failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn test_remove_worktree_cleans_up() {
        let repo = make_test_repo("remove");
        let info = create_discussion_worktree(repo.path(), "proj", "to-remove", "main").unwrap();
        let wt_path = info.path.clone();
        assert!(Path::new(&wt_path).exists());

        let result = remove_discussion_worktree(repo.path(), &wt_path, None);
        assert!(result.is_ok());
        assert!(
            !Path::new(&wt_path).exists(),
            "Worktree directory should be removed"
        );
    }

    #[test]
    fn test_remove_worktree_keeps_branch_when_requested() {
        let repo = make_test_repo("keep-branch");
        let info = create_discussion_worktree(repo.path(), "proj", "keep-me", "main").unwrap();

        remove_discussion_worktree(repo.path(), &info.path, None).unwrap();

        // Branch should still exist
        let output = std::process::Command::new("git")
            .args(["branch", "--list", &info.branch])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(
            branches.contains("kronn/keep-me"),
            "Branch should still exist after remove with delete_branch=false"
        );
    }

    #[test]
    fn test_remove_worktree_deletes_branch_when_requested() {
        let repo = make_test_repo("del-branch");
        let info = create_discussion_worktree(repo.path(), "proj", "delete-me", "main").unwrap();

        remove_discussion_worktree(repo.path(), &info.path, Some(&info.branch)).unwrap();

        let output = std::process::Command::new("git")
            .args(["branch", "--list", &info.branch])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(
            !branches.contains("kronn/delete-me"),
            "Branch should be deleted"
        );
    }

    /// KT-2084 — the branch of a deleted discussion goes only when another ref
    /// reaches every commit on it. The other ref here is not `HEAD`, so a
    /// `--exclude` that missed the branch itself, or excluded too much, fails.
    #[test]
    fn a_deleted_discussion_branch_goes_only_when_another_ref_holds_its_commits() {
        let repo = make_test_repo("deleted-disc-branch");
        let update_ref = |name: &str, sha: &str| {
            let out = std::process::Command::new("git")
                .args(["update-ref", name, sha])
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(out.status.success());
        };

        let pushed = create_discussion_worktree(repo.path(), "proj", "pushed", "main").unwrap();
        let sha = commit_file(Path::new(&pushed.path), "a.txt", "a", "pushed work");
        update_ref("refs/remotes/origin/kronn/pushed", &sha);
        assert_eq!(
            remove_deleted(repo.path(), &pushed.path).unwrap(),
            DeletedDiscussionBranch::Deleted(pushed.branch.clone())
        );
        assert!(!Path::new(&pushed.path).exists());
        assert!(branch_commit(repo.path(), &pushed.branch).is_none());

        let local = create_discussion_worktree(repo.path(), "proj", "local", "main").unwrap();
        let only_here = commit_file(Path::new(&local.path), "b.txt", "b", "local work");
        let outcome = remove_deleted(repo.path(), &local.path).unwrap();
        assert!(
            matches!(&outcome, DeletedDiscussionBranch::Kept { branch, reason }
                if branch == &local.branch && reason.contains(&only_here)),
            "{outcome:?}"
        );
        assert!(!Path::new(&local.path).exists(), "the checkout still goes");
        assert_eq!(branch_commit(repo.path(), &local.branch), Some(only_here));

        let other = create_discussion_worktree(repo.path(), "proj", "other", "main").unwrap();
        let sha = commit_file(Path::new(&other.path), "c.txt", "c", "kept elsewhere");
        update_ref("refs/heads/backup/other", &sha);
        assert_eq!(
            remove_deleted(repo.path(), &other.path).unwrap(),
            DeletedDiscussionBranch::Deleted(other.branch.clone()),
            "another local branch holds it"
        );

        // Neither a stash (dropped at will), a backup ref nor a detached HEAD keeps
        // a commit: the main checkout's HEAD comes last, it moves for good.
        let stashed = create_discussion_worktree(repo.path(), "proj", "stashed", "main").unwrap();
        let sha = commit_file(Path::new(&stashed.path), "d.txt", "d", "stashed work");
        update_ref("refs/stash", &sha);
        update_ref("refs/kronn-backup/stashed", &sha);
        let outcome = remove_deleted(repo.path(), &stashed.path).unwrap();
        assert!(
            matches!(&outcome, DeletedDiscussionBranch::Kept { .. }),
            "{outcome:?}"
        );
        assert_eq!(branch_commit(repo.path(), &stashed.branch), Some(sha));

        let detached = create_discussion_worktree(repo.path(), "proj", "detached", "main").unwrap();
        let sha = commit_file(Path::new(&detached.path), "e.txt", "e", "seen detached");
        let out = std::process::Command::new("git")
            .args(["checkout", "-q", "--detach", &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let outcome = remove_deleted(repo.path(), &detached.path).unwrap();
        assert!(
            matches!(&outcome, DeletedDiscussionBranch::Kept { .. }),
            "{outcome:?}"
        );
        assert_eq!(branch_commit(repo.path(), &detached.branch), Some(sha));
    }

    /// KT-2084 — a file written after the handler's look is seen by Kronn's own look right
    /// before the removal: the worktree stays, the file with it.
    #[test]
    fn a_deleted_discussion_worktree_written_after_the_check_is_kept() {
        let repo = make_test_repo("deleted-disc-late-write");
        let info = create_discussion_worktree(repo.path(), "proj", "late", "main").unwrap();
        fs::write(Path::new(&info.path).join("late.txt"), "late").unwrap();

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(
            fs::read_to_string(Path::new(&info.path).join("late.txt")).unwrap(),
            "late"
        );
        assert!(branch_commit(repo.path(), &info.branch).is_some());
    }

    /// A populated submodule makes git refuse any removal without `--force`:
    /// clean, the worktree still goes.
    #[test]
    fn a_deleted_discussion_worktree_with_a_clean_submodule_is_removed() {
        let (repo, _upstream) = make_test_repo_with_submodule("deleted-disc-submodule");
        let info = create_discussion_worktree(repo.path(), "proj", "submod", "main").unwrap();
        let init = std::process::Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
            ])
            .current_dir(&info.path)
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        assert!(Path::new(&info.path).join("sub/lib.txt").exists());

        remove_deleted(repo.path(), &info.path).unwrap();

        assert!(!Path::new(&info.path).exists());
    }

    /// A discussion worktree of a repository with a submodule, populated.
    fn discussion_worktree_with_submodule(name: &str) -> (tempfile::TempDir, PathBuf) {
        let (repo, upstream) = make_test_repo_with_submodule(name);
        // The upstream is only needed to populate: keep it until then.
        let info = create_discussion_worktree(repo.path(), "proj", name, "main").unwrap();
        let checkout = PathBuf::from(&info.path);
        let init = std::process::Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
            ])
            .current_dir(&checkout)
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        drop(upstream);
        (repo, checkout)
    }

    fn git_dir_of(dir: &Path) -> PathBuf {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(dir)
            .output()
            .unwrap();
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
    }

    /// KT-2084 — `--force` deletes the worktree's submodule repositories: a
    /// commit none of the submodule's remotes has keeps the worktree.
    #[test]
    fn a_submodule_commit_missing_from_its_remote_keeps_the_deleted_discussion_worktree() {
        let (repo, checkout) = discussion_worktree_with_submodule("deleted-disc-sub-commit");
        let sub = checkout.join("sub");
        fs::write(sub.join("lib.txt"), "local only").unwrap();
        let identity = ["-c", "user.email=t@t.com", "-c", "user.name=T"];
        git(
            &sub,
            &[&identity[..], &["commit", "-qam", "local"]].concat(),
        );
        let commit = head_sha(&sub);
        // The gitlink is committed too: the worktree itself is clean.
        git(
            &checkout,
            &[&identity[..], &["commit", "-qam", "bump"]].concat(),
        );

        let outcome = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            outcome.as_ref().is_err_and(|error| error.contains(&commit)),
            "{outcome:?}"
        );
        assert_eq!(head_sha(&sub), commit, "the commit is intact");
    }

    #[test]
    fn a_submodule_holding_an_ignored_file_keeps_the_deleted_discussion_worktree() {
        let (repo, checkout) = discussion_worktree_with_submodule("deleted-disc-sub-env");
        let sub = checkout.join("sub");
        let info = git_dir_of(&sub).join("info");
        fs::create_dir_all(&info).unwrap();
        fs::write(info.join("exclude"), ".env\n").unwrap();
        fs::write(sub.join(".env"), "TOKEN=1").unwrap();

        let outcome = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            outcome
                .as_ref()
                .is_err_and(|error| error.contains(".env") && error.contains("sub")),
            "{outcome:?}"
        );
        assert_eq!(fs::read_to_string(sub.join(".env")).unwrap(), "TOKEN=1");
    }

    /// KT-2084 — when the delete keeps the worktree, Kronn's untracked agent-config copy
    /// stays with it: a same-slug reuse runs with it.
    #[test]
    fn a_kept_discussion_worktree_keeps_its_agent_config_copy() {
        let repo = make_test_repo("deleted-disc-copy-kept");
        fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
        let info = create_discussion_worktree(repo.path(), "proj", "copy-kept", "main").unwrap();
        let checkout = Path::new(&info.path);
        assert!(checkout.join(".mcp.json").is_file());
        fs::write(checkout.join("late.txt"), "late").unwrap();

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(
            outcome
                .as_ref()
                .is_err_and(|error| !error.trim().is_empty()),
            "{outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(checkout.join(".mcp.json")).unwrap(),
            "{}"
        );
        assert!(checkout.join("late.txt").exists());
    }

    /// KT-2084 — untracked OS junk and an untracked `.kronn/` are not work: they go with
    /// the worktree.
    #[test]
    fn untracked_os_junk_never_keeps_a_deleted_discussion_worktree() {
        let repo = make_test_repo("deleted-disc-junk");
        let info = create_discussion_worktree(repo.path(), "proj", "junk", "main").unwrap();
        let checkout = Path::new(&info.path);
        // A repository whose exclude file names none of them (made before KT-1750).
        let exclude = repo.path().join(".git/info/exclude");
        let kept: String = fs::read_to_string(&exclude)
            .unwrap()
            .lines()
            .filter(|line| line.contains("worktrees"))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(&exclude, kept).unwrap();
        fs::write(checkout.join(".DS_Store"), "junk").unwrap();
        fs::create_dir_all(checkout.join(".kronn")).unwrap();
        fs::write(checkout.join(".kronn/state.json"), "{}").unwrap();

        remove_deleted(repo.path(), &info.path).unwrap();

        assert!(!checkout.exists());
    }

    /// KT-2084 — the delete runs git where an agent wrote: a `core.fsmonitor` it
    /// configured never runs.
    #[cfg(unix)] // the fsmonitor hook is a #!/bin/sh script, executable through unix mode bits
    #[test]
    fn deleting_a_discussion_worktree_never_runs_its_configured_fsmonitor() {
        let repo = make_test_repo("deleted-disc-fsmonitor");
        let info = create_discussion_worktree(repo.path(), "proj", "fsmonitor", "main").unwrap();
        let checkout = Path::new(&info.path);
        let (_hooks, hook, ran) = fsmonitor_hook("worktree");
        git(
            checkout,
            &["config", "core.fsmonitor", &hook.to_string_lossy()],
        );

        let _ = remove_deleted(repo.path(), &info.path);

        assert!(!ran.exists(), "the worktree's fsmonitor ran");
    }

    #[cfg(unix)] // the fsmonitor hook is a #!/bin/sh script, executable through unix mode bits
    #[test]
    fn deleting_a_discussion_worktree_never_runs_a_submodule_fsmonitor() {
        let (repo, checkout) = discussion_worktree_with_submodule("deleted-disc-sub-fsmonitor");
        let (_hooks, hook, ran) = fsmonitor_hook("submodule");
        git(
            &checkout.join("sub"),
            &["config", "core.fsmonitor", &hook.to_string_lossy()],
        );

        let _ = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(!ran.exists(), "the submodule's fsmonitor ran");
    }

    /// An fsmonitor hook that leaves a mark, in a directory of its own.
    #[cfg(unix)]
    fn fsmonitor_hook(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .prefix(&format!("kronn-fsmonitor-{name}"))
            .tempdir()
            .unwrap();
        let ran = dir.path().join("ran");
        let hook = dir.path().join("hook.sh");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, hook, ran)
    }

    fn regenerable_defaults() -> Vec<String> {
        regenerable_outputs_with(&[])
    }

    /// KT-2084 — what a discussion delete counts as work: a gitignored `.env`, a
    /// dirty submodule; never Kronn's agent-config copies, ignored or not.
    #[test]
    fn a_delete_counts_ignored_files_and_dirty_submodules_as_work() {
        let uncommitted = |repo: &Path, checkout: &Path| {
            let judged = Snapshot::of_worktree(checkout).unwrap();
            let files = worktree_files(Some(repo), &judged, &regenerable_defaults()).unwrap();
            [files.uncommitted, files.ignored].concat()
        };
        let (repo, _upstream) = make_test_repo_with_submodule("disc-ignored");
        fs::write(
            repo.path().join(".gitignore"),
            ".env\n.mcp.json\n.gemini/\n",
        )
        .unwrap();
        for args in [vec!["add", ".gitignore"], vec!["commit", "-m", "ignore"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(repo.path())
                .output()
                .unwrap();
        }
        fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
        fs::create_dir_all(repo.path().join(".gemini")).unwrap();
        fs::write(repo.path().join(".gemini/settings.json"), "{}").unwrap();
        let info = create_discussion_worktree(repo.path(), "proj", "ignored", "main").unwrap();
        let checkout = Path::new(&info.path);
        assert!(checkout.join(".gemini/settings.json").is_file());
        assert_eq!(
            uncommitted(repo.path(), checkout),
            Vec::<String>::new(),
            "Kronn's copies are not work"
        );

        fs::write(checkout.join(".env"), "TOKEN=1").unwrap();
        assert_eq!(uncommitted(repo.path(), checkout), vec![".env".to_string()]);
        fs::remove_file(checkout.join(".env")).unwrap();

        let init = std::process::Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
            ])
            .current_dir(checkout)
            .output()
            .unwrap();
        assert!(init.status.success());
        fs::write(checkout.join("sub/lib.txt"), "edited in the submodule").unwrap();
        // The submodule's files are judged by Kronn's own command in it, not by git
        // descending into it from the worktree.
        assert_eq!(uncommitted(repo.path(), checkout), Vec::<String>::new());
        let work = unsaved_work_with(repo.path(), checkout, "wt", &regenerable_defaults())
            .unwrap()
            .unwrap_or_default();
        assert!(
            work.contains("the submodule sub of wt") && work.contains("lib.txt"),
            "{work}"
        );
    }

    /// KT-2084 — `server.regenerable_outputs` adds names to the built-in list, trimmed, with
    /// no process-wide state touched.
    #[test]
    fn a_configured_regenerable_directory_is_not_work() {
        let repo = make_test_repo("disc-regenerable");
        fs::write(repo.path().join(".gitignore"), "generated/\n").unwrap();
        git(repo.path(), &["add", ".gitignore"]);
        git(repo.path(), &["commit", "-qm", "ignore"]);
        let info = create_discussion_worktree(repo.path(), "proj", "regenerable", "main").unwrap();
        let checkout = Path::new(&info.path);
        fs::create_dir_all(checkout.join("generated/deep")).unwrap();
        fs::write(checkout.join("generated/deep/out.js"), "built").unwrap();
        let refused = unsaved_work_with(repo.path(), checkout, "wt", &regenerable_defaults());
        assert!(
            refused
                .as_ref()
                .is_ok_and(|work| work.as_ref().is_some_and(|w| w.contains("generated/"))),
            "{refused:?}"
        );

        let configured = regenerable_outputs_with(&[" generated/ ".to_string(), String::new()]);
        assert!(
            configured.contains(&"generated".to_string()),
            "{configured:?}"
        );
        assert!(configured.contains(&"node_modules".to_string()));
        assert!(!configured.contains(&String::new()));
        assert_eq!(
            unsaved_work_with(repo.path(), checkout, "wt", &configured),
            Ok(None)
        );
    }

    /// The delete of a discussion whose stored branch is the one its worktree is on.
    fn remove_deleted(repo: &Path, path: &str) -> Result<DeletedDiscussionBranch, String> {
        let own = find_branch_for_worktree(repo, path);
        remove_deleted_discussion_worktree(repo, path, own.as_deref(), &Default::default())
    }

    /// `patterns` committed to `repo` as its `.gitignore`.
    fn commit_ignore(repo: &Path, patterns: &str) {
        fs::write(repo.join(".gitignore"), patterns).unwrap();
        git(repo, &["add", ".gitignore"]);
        git(repo, &["commit", "-qm", "ignore"]);
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn work_in(repo: &Path, checkout: &Path, regenerable: &[String]) -> String {
        unsaved_work_with(repo, checkout, "wt", regenerable)
            .unwrap()
            .unwrap_or_default()
    }

    /// KT-2084 — what tools write again (a TypeScript build cache, a test report, an agent
    /// CLI's write log) is not work; a `.env` beside it, or a whole `.claude/`, still is.
    #[test]
    fn routine_tool_output_is_not_work() {
        let repo = make_test_repo("disc-tool-output");
        commit_ignore(
            repo.path(),
            "*.tsbuildinfo\ntest-results/\n.claude/\n.env\n",
        );
        let info = create_discussion_worktree(repo.path(), "proj", "tool-output", "main").unwrap();
        let checkout = Path::new(&info.path);
        for file in [
            "frontend/tsconfig.tsbuildinfo",
            "frontend/test-results/run/trace.zip",
            ".claude/.cc-writes/1",
            "backend/.claude/.cc-writes/2",
        ] {
            write_file(checkout, file, "made again");
        }
        let defaults = regenerable_defaults();
        assert_eq!(work_in(repo.path(), checkout, &defaults), "");

        write_file(checkout, ".claude/settings.local.json", "{}");
        let work = work_in(repo.path(), checkout, &defaults);
        assert!(
            work.contains("1 ignored folder(s)") && work.contains(".claude/"),
            "{work}"
        );
        fs::remove_file(checkout.join(".claude/settings.local.json")).unwrap();

        write_file(checkout, "frontend/.env", "TOKEN=1");
        let work = work_in(repo.path(), checkout, &defaults);
        assert!(work.contains("frontend/.env"), "{work}");
    }

    /// KT-2084 — a configured glob matches an ignored file's name at any depth.
    #[test]
    fn a_configured_regenerable_glob_is_not_work() {
        let repo = make_test_repo("disc-regenerable-glob");
        commit_ignore(repo.path(), "*.gen.ts\n");
        let info = create_discussion_worktree(repo.path(), "proj", "glob", "main").unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, "src/api/client.gen.ts", "generated");
        let work = work_in(repo.path(), checkout, &regenerable_defaults());
        assert!(work.contains("src/api/client.gen.ts"), "{work}");

        let mut configured = regenerable_defaults();
        configured.push("*.gen.ts".to_string());
        assert_eq!(work_in(repo.path(), checkout, &configured), "");
    }

    /// KT-2084 — git quotes a name holding a space or a non-ASCII letter unless asked for
    /// NUL-separated output: the delete reads each name as git names it.
    #[test]
    fn names_with_spaces_or_accents_are_read_as_git_names_them() {
        let repo = make_test_repo("disc-quoted-names");
        commit_ignore(repo.path(), "node_modules/\n");
        let info = create_discussion_worktree(repo.path(), "proj", "quoted", "main").unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, "my app/node_modules/pkg/index.js", "installed");
        write_file(checkout, "café/node_modules/pkg/index.js", "installed");
        assert_eq!(work_in(repo.path(), checkout, &regenerable_defaults()), "");

        write_file(checkout, "notes é.md", "mine");
        let work = work_in(repo.path(), checkout, &regenerable_defaults());
        assert!(work.contains("(notes é.md)"), "{work}");
    }

    /// KT-2084 — a name is read whole, whatever it holds.
    #[test]
    fn a_name_holding_an_arrow_or_a_quote_is_read_whole() {
        let repo = make_test_repo("disc-arrow-name");
        let info = create_discussion_worktree(repo.path(), "proj", "arrow", "main").unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, "x -> .kronn/notes.md", "mine");
        write_file(checkout, "say \"hi\".md", "mine");
        let work = work_in(repo.path(), checkout, &regenerable_defaults());
        assert!(
            work.contains("2 uncommitted file(s)")
                && work.contains("x -> .kronn/notes.md")
                && work.contains("say \"hi\".md"),
            "{work}"
        );
    }

    /// KT-2084 — seeds under a wholly ignored directory are Kronn's copies; anything
    /// else in that directory is still work.
    #[test]
    fn seeds_under_a_wholly_ignored_directory_are_not_work() {
        let repo = make_test_repo("disc-seeds-parent");
        commit_ignore(repo.path(), "public/\n");
        let info = create_discussion_worktree(repo.path(), "proj", "seeds", "main").unwrap();
        let checkout = Path::new(&info.path);
        let admin = registered_worktree_git_dir(repo.path(), checkout).unwrap();
        fs::write(
            admin.join(LANE_SEEDS_MARKER_FILE),
            r#"{"seeds":{"public/build":"fingerprint"}}"#,
        )
        .unwrap();
        write_file(checkout, "public/build/app.js", "seeded");
        assert_eq!(work_in(repo.path(), checkout, &regenerable_defaults()), "");

        write_file(checkout, "public/notes.txt", "mine");
        let work = work_in(repo.path(), checkout, &regenerable_defaults());
        assert!(work.contains("public/"), "{work}");
    }

    /// KT-2084 — `--force` keeps nothing of a submodule's repository: a pin that only a
    /// tag reaches is safe on the remote, and the worktree goes.
    #[test]
    fn a_submodule_pinned_to_a_tag_only_commit_is_removed_with_its_worktree() {
        let (repo, upstream) = make_test_repo_with_submodule("deleted-disc-sub-tag");
        let up = upstream.path();
        git(up, &["checkout", "-q", "--detach"]);
        write_file(up, "release.txt", "v1");
        git(up, &["add", "release.txt"]);
        git(up, &["commit", "-qm", "release"]);
        git(up, &["tag", "v1"]);
        let pinned = head_sha(up);
        git(up, &["checkout", "-q", "main"]);
        let sub = repo.path().join("sub");
        git(&sub, &["fetch", "-q", "origin", "--tags"]);
        git(&sub, &["checkout", "-q", &pinned]);
        git(repo.path(), &["commit", "-qam", "pin v1"]);
        let info = create_discussion_worktree(repo.path(), "proj", "sub-tag", "main").unwrap();
        git(
            Path::new(&info.path),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "-q",
            ],
        );
        assert_eq!(head_sha(&Path::new(&info.path).join("sub")), pinned);
        drop(upstream);

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(!Path::new(&info.path).exists());
    }

    /// KT-2084 — a tracked agent config Kronn rewrote in the worktree (`.mcp.json` committed
    /// by the project) never keeps it; Kronn's copy stays when other work keeps it.
    #[test]
    fn a_tracked_agent_config_copy_never_keeps_a_deleted_discussion_worktree() {
        let repo = make_test_repo("deleted-disc-tracked-copy");
        fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
        git(repo.path(), &["add", ".mcp.json"]);
        git(repo.path(), &["commit", "-qm", "mcp"]);
        fs::write(repo.path().join(".mcp.json"), r#"{"local":1}"#).unwrap();
        let kept = create_discussion_worktree(repo.path(), "proj", "copy-kept", "main").unwrap();
        let kept_checkout = Path::new(&kept.path);
        assert_eq!(
            fs::read_to_string(kept_checkout.join(".mcp.json")).unwrap(),
            r#"{"local":1}"#
        );
        fs::write(kept_checkout.join("late.txt"), "late").unwrap();

        let outcome = remove_deleted(repo.path(), &kept.path);

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(
            fs::read_to_string(kept_checkout.join(".mcp.json")).unwrap(),
            r#"{"local":1}"#,
            "Kronn's copy stays"
        );

        let info = create_discussion_worktree(repo.path(), "proj", "copy-gone", "main").unwrap();
        let outcome = remove_deleted(repo.path(), &info.path);
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(!Path::new(&info.path).exists());
    }

    /// KT-2084 — a repository inside Kronn's own untracked directory is never removed by
    /// hand: the worktree stays and the reason names it.
    #[test]
    fn a_repository_inside_an_untracked_kronn_directory_keeps_the_worktree() {
        let repo = make_test_repo("deleted-disc-nested-repo");
        let info = create_discussion_worktree(repo.path(), "proj", "nested", "main").unwrap();
        let checkout = Path::new(&info.path);
        let exclude = repo.path().join(".git/info/exclude");
        let kept: String = fs::read_to_string(&exclude)
            .unwrap()
            .lines()
            .filter(|line| line.contains("worktrees"))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(&exclude, kept).unwrap();
        fs::create_dir_all(checkout.join(".kronn/data")).unwrap();
        git(&checkout.join(".kronn/data"), &["init", "-q", "mirror"]);
        write_file(checkout, ".kronn/data/mirror/notes.txt", "mine");

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(
            outcome
                .as_ref()
                .is_err_and(|error| error.contains(".kronn/data/mirror")),
            "{outcome:?}"
        );
        assert!(checkout.join(".kronn/data/mirror/.git").is_dir());
        assert_eq!(
            fs::read_to_string(checkout.join(".kronn/data/mirror/notes.txt")).unwrap(),
            "mine"
        );
    }

    // ── KT-2084 — the delete path stays inside the worktree ──

    /// An outside directory holding one file, to prove it survives.
    fn outside_file() -> (tempfile::TempDir, PathBuf) {
        let outside = tempfile::Builder::new()
            .prefix("kronn-wt-outside-")
            .tempdir()
            .unwrap();
        let file = outside.path().join("kept.txt");
        fs::write(&file, "outside").unwrap();
        (outside, file)
    }

    /// A submodule repository whose configuration Kronn does not trust keeps the worktree,
    /// and no git command runs inside it.
    #[test]
    fn an_untrusted_submodule_repository_keeps_the_worktree_and_runs_nothing_in_it() {
        let (repo, checkout) = discussion_worktree_with_submodule("deleted-disc-planted-sub");
        let sub = checkout.join("sub");
        fs::remove_file(sub.join(".git")).unwrap();
        git(&sub, &["init", "-q", "-b", "main"]);
        let identity = ["-c", "user.email=t@t.com", "-c", "user.name=T"];
        git(&sub, &["add", "lib.txt"]);
        git(
            &sub,
            &[&identity[..], &["commit", "-qm", "planted"]].concat(),
        );
        let (elsewhere, _) = outside_file();
        git(
            &sub,
            &[
                "config",
                "core.worktree",
                &elsewhere.path().to_string_lossy(),
            ],
        );
        fs::write(sub.join(".git/info/attributes"), "* filter=lfs\n").unwrap();
        let index = sub.join(".git/index");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        fs::File::options()
            .append(true)
            .open(&index)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let outcome = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(outcome.is_err(), "{outcome:?}");
        assert!(checkout.exists());
        assert_eq!(
            fs::metadata(&index).unwrap().modified().unwrap(),
            old,
            "a git command ran inside the planted repository"
        );
    }

    /// The owner's filter naming a program by a path relative to the worktree never runs
    /// during the removal itself.
    #[cfg(unix)] // plants #!/bin/sh programs, executable through unix mode bits
    #[test]
    fn removing_a_discussion_worktree_runs_no_program_it_holds() {
        use std::os::unix::fs::PermissionsExt;
        let repo = make_test_repo("deleted-disc-relative-filter");
        fs::write(repo.path().join(".gitattributes"), "README.md filter=kt\n").unwrap();
        git(repo.path(), &["add", ".gitattributes"]);
        git(repo.path(), &["commit", "-qm", "attributes"]);
        git(repo.path(), &["config", "filter.kt.clean", "./tool.sh"]);
        let info = create_discussion_worktree(repo.path(), "proj", "rel-filter", "main").unwrap();
        let checkout = Path::new(&info.path);
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        let tool = checkout.join("tool.sh");
        fs::write(
            &tool,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", ran.display()),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        fs::File::options()
            .append(true)
            .open(checkout.join("README.md"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();

        let _ = remove_deleted(repo.path(), &info.path);

        assert!(!ran.exists(), "a program in the worktree ran");
    }

    #[test]
    fn only_a_plain_relative_path_is_judged() {
        for plain in ["a", "a/b", "dir/", ".kronn/x", "caf\u{e9} \"q\" -> b"] {
            assert!(plain_relative(plain), "{plain}");
        }
        for odd in [
            "", "/", "/abs", "a//b", "./a", "a/./b", "../a", "a/..", "a/../b",
        ] {
            assert!(!plain_relative(odd), "{odd}");
        }
    }

    /// A path that only ends like a registered worktree's is not that worktree.
    #[test]
    fn a_worktree_path_sharing_a_suffix_is_not_matched() {
        let repo = make_test_repo("branch-suffix");
        let info = create_discussion_worktree(repo.path(), "proj", "suffix", "main").unwrap();
        let name = Path::new(&info.path).file_name().unwrap().to_string_lossy();
        let elsewhere = format!("/elsewhere/{name}");
        assert_eq!(find_branch_for_worktree(repo.path(), &elsewhere), None);
        assert_eq!(
            find_branch_for_worktree(repo.path(), &info.path),
            Some(info.branch.clone())
        );
    }

    /// KT-2084 — the branch is read from the worktree's own registration: Kronn's relative
    /// `gitdir` finds it, whatever form `git worktree list` gives the path in.
    #[test]
    fn the_branch_of_a_worktree_registered_with_a_relative_gitdir_is_found() {
        let repo = make_test_repo("branch-relative-gitdir");
        let info = create_discussion_worktree(repo.path(), "proj", "relative", "main").unwrap();
        let admin = registered_worktree_git_dir(repo.path(), Path::new(&info.path)).unwrap();
        let gitdir = fs::read_to_string(admin.join("gitdir")).unwrap();
        assert!(Path::new(gitdir.trim()).is_relative(), "{gitdir}");

        assert_eq!(
            find_branch_for_worktree(repo.path(), &info.path),
            Some(info.branch.clone())
        );
        git(Path::new(&info.path), &["checkout", "-q", "--detach"]);
        assert_eq!(find_branch_for_worktree(repo.path(), &info.path), None);
    }

    /// KT-2084 — Kronn's `.kronn/` is ignored in every worktree; a linked worktree inside it
    /// (a PR checkout) is still a repository the delete never takes.
    #[test]
    fn a_worktree_inside_the_ignored_kronn_directory_keeps_the_worktree() {
        let repo = make_test_repo("deleted-disc-ignored-nested");
        let info =
            create_discussion_worktree(repo.path(), "proj", "ignored-nested", "main").unwrap();
        let checkout = Path::new(&info.path);
        let exclude = fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert!(
            exclude.lines().any(|line| line.trim() == ".kronn/"),
            "{exclude}"
        );
        let pr = checkout.join(".kronn/pr-1");
        git(
            repo.path(),
            &["worktree", "add", "-q", "-b", "pr-1", &pr.to_string_lossy()],
        );
        write_file(&pr, "wip.txt", "uncommitted");

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(
            outcome
                .as_ref()
                .is_err_and(|error| error.contains(".kronn/")),
            "{outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(pr.join("wip.txt")).unwrap(),
            "uncommitted"
        );
    }

    /// KT-2084 — a regenerable name is matched against the ignored entry itself, never a
    /// folder above it: a `.env` in a tracked `build/` is work.
    #[test]
    fn an_ignored_file_in_a_tracked_build_folder_is_work() {
        let repo = make_test_repo("disc-tracked-build");
        write_file(repo.path(), "build/keep.txt", "tracked");
        git(repo.path(), &["add", "build/keep.txt"]);
        git(repo.path(), &["commit", "-qm", "tracked build"]);
        commit_ignore(repo.path(), ".env\n");
        let info =
            create_discussion_worktree(repo.path(), "proj", "tracked-build", "main").unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, "build/.env", "TOKEN=1");

        let work = work_in(repo.path(), checkout, &regenerable_defaults());

        assert!(work.contains("build/.env"), "{work}");
    }

    /// KT-2084 — an entry holding `/` names a path from the worktree root: that folder, not
    /// another of the same name.
    #[test]
    fn a_regenerable_path_entry_names_that_folder_only() {
        let repo = make_test_repo("disc-regenerable-path");
        commit_ignore(repo.path(), "bindings/\nbundle/\n");
        let info = create_discussion_worktree(repo.path(), "proj", "regen-path", "main").unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, "backend/bindings/Task.ts", "generated");
        write_file(checkout, "vendor/bundle/gems/x.rb", "installed");
        write_file(checkout, "other/bindings/Hand.ts", "hand-written");
        let configured =
            regenerable_outputs_with(&["backend/bindings".to_string(), "/vendor/bundle/".into()]);

        let work = work_in(repo.path(), checkout, &configured);

        assert!(work.contains("other/bindings/"), "{work}");
        assert!(
            !work.contains("backend/bindings") && !work.contains("vendor/bundle"),
            "{work}"
        );
    }

    /// KT-2084 — a wholly ignored folder is named as a folder, with the way to declare it
    /// generated output.
    #[test]
    fn an_ignored_folder_is_counted_as_a_folder() {
        let repo = make_test_repo("disc-ignored-folder");
        commit_ignore(repo.path(), "bindings/\n.env\n");
        let info = create_discussion_worktree(repo.path(), "proj", "ign-folder", "main").unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, "backend/bindings/A.ts", "generated");
        write_file(checkout, "backend/bindings/B.ts", "generated");
        write_file(checkout, ".env", "TOKEN=1");

        let work = work_in(repo.path(), checkout, &regenerable_defaults());

        assert!(
            work.contains(
                "1 ignored file(s) and 1 ignored folder(s) in wt (.env, backend/bindings/)"
            ),
            "{work}"
        );
        assert!(work.contains("`server.regenerable_outputs`"), "{work}");
    }

    /// KT-2084 — an untracked link stays as it is when the delete keeps the worktree.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn a_kept_worktree_gets_its_untracked_link_back() {
        let repo = make_test_repo("deleted-disc-link-back");
        let info = create_discussion_worktree(repo.path(), "proj", "link-back", "main").unwrap();
        let checkout = Path::new(&info.path);
        let (store, _) = outside_file();
        std::os::unix::fs::symlink(store.path(), checkout.join("node_modules")).unwrap();
        fs::write(checkout.join("late.txt"), "late").unwrap();

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(
            fs::read_link(checkout.join("node_modules")).unwrap(),
            store.path()
        );
        assert!(checkout.join("late.txt").exists());
    }

    #[test]
    fn test_reattach_worktree_after_remove() {
        let repo = make_test_repo("reattach");
        let info =
            create_discussion_worktree(repo.path(), "proj", "reattach-test", "main").unwrap();
        let branch = info.branch.clone();

        // Remove worktree but keep branch
        remove_discussion_worktree(repo.path(), &info.path, None).unwrap();
        assert!(!Path::new(&info.path).exists());

        // Re-attach
        let result = reattach_worktree(repo.path(), "proj", "reattach-test", &branch);
        assert!(result.is_ok(), "reattach failed: {:?}", result.err());
        let info2 = result.unwrap();
        assert!(Path::new(&info2.path).exists());
        assert_eq!(info2.branch, branch);
    }

    #[test]
    fn test_create_blocks_when_branch_on_main_repo() {
        let repo = make_test_repo("block");
        // Create branch and check it out in the main repo
        std::process::Command::new("git")
            .args(["checkout", "-b", "kronn/blocked-test"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let result = create_discussion_worktree(repo.path(), "proj", "blocked-test", "main");
        assert!(
            result.is_err(),
            "Should fail when branch is checked out in main repo"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("checked out"),
            "Error should mention 'checked out': {}",
            err
        );
    }

    #[test]
    fn test_reattach_blocks_when_branch_on_main_repo() {
        let repo = make_test_repo("reattach-block");
        std::process::Command::new("git")
            .args(["checkout", "-b", "kronn/reattach-blocked"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let result = reattach_worktree(
            repo.path(),
            "proj",
            "reattach-blocked",
            "kronn/reattach-blocked",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("checked out"));
    }

    #[test]
    fn test_current_branch_returns_branch_name() {
        let repo = make_test_repo("curbranch");
        let branch = current_branch(repo.path());
        assert_eq!(branch, Some("main".to_string()));
    }

    #[test]
    fn test_branch_checked_out_at_finds_main() {
        let repo = make_test_repo("checkout-at");
        let result = branch_checked_out_at(repo.path(), "main").unwrap();
        assert!(result.is_some(), "main should be found as checked out");
        // Compare through same_path: git canonicalizes (`/private/var/…` on
        // macOS) while tempfile returns the symlinked spelling (`/var/…`).
        assert!(same_path(&result.unwrap().path, repo.path()));
    }

    #[test]
    fn test_branch_checked_out_at_returns_none_for_nonexistent() {
        let repo = make_test_repo("checkout-none");
        let result = branch_checked_out_at(repo.path(), "kronn/does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_worktree_existing() {
        let repo = make_test_repo("validate");
        let info = create_discussion_worktree(repo.path(), "proj", "val-test", "main").unwrap();
        assert!(validate_worktree(&info.path));
    }

    // ── Test-mode helpers ────────────────────────────────────────────────────

    #[test]
    fn test_parse_porcelain_basic_shapes() {
        let out = b" M src/lib.rs\0?? new file.txt\0A  staged.rs\0R  new.rs\0old.rs\0 C copy -> x\0orig\0!! a \"b\".log\0";
        let files = parse_porcelain_z(out);
        let shapes: Vec<(&str, &str)> = files
            .iter()
            .map(|file| (file.status.as_str(), file.path.as_str()))
            .collect();
        // A rename or a copy keeps its target; its source is never read as an entry.
        assert_eq!(
            shapes,
            [
                (" M", "src/lib.rs"),
                ("??", "new file.txt"),
                ("A ", "staged.rs"),
                ("R ", "new.rs"),
                (" C", "copy -> x"),
                ("!!", "a \"b\".log"),
            ]
        );
    }

    #[test]
    fn test_worktree_dirty_files_empty_on_clean_repo() {
        let repo = make_test_repo("dirty-clean");
        let files = worktree_dirty_files(repo.path()).unwrap();
        assert!(files.is_empty(), "clean repo should report no dirty files");
    }

    #[test]
    fn test_worktree_dirty_files_detects_modified_and_untracked() {
        let repo = make_test_repo("dirty-mod");
        // Modify tracked file + add untracked file.
        fs::write(repo.path().join("README.md"), "# modified").unwrap();
        fs::write(repo.path().join("new.txt"), "hello").unwrap();
        let files = worktree_dirty_files(repo.path()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"new.txt"));
    }

    // ── KT-1750 — OS junk never makes a checkout "dirty" ─────────────────────

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn os_junk_is_not_work_but_real_files_still_are() {
        let repo = make_test_repo("dirty-junk");
        fs::create_dir_all(repo.path().join("sub")).unwrap();
        for junk in [
            ".DS_Store",
            "Thumbs.db",
            "desktop.ini",
            "._resource",
            "sub/.DS_Store",
        ] {
            fs::write(repo.path().join(junk), "x").unwrap();
        }
        assert!(worktree_dirty_files(repo.path()).unwrap().is_empty());

        fs::write(repo.path().join("sub/real.txt"), "work").unwrap();
        fs::write(repo.path().join("DS_Store.txt"), "work").unwrap();
        let paths: Vec<String> = worktree_dirty_files(repo.path())
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        // git lists an untracked directory as one entry.
        assert_eq!(paths, ["DS_Store.txt", "sub/"]);
    }

    #[test]
    fn a_tracked_os_junk_file_that_changed_is_still_work() {
        let repo = make_test_repo("dirty-tracked-junk");
        fs::write(repo.path().join(".DS_Store"), "v1").unwrap();
        git(repo.path(), &["add", "-f", ".DS_Store"]);
        git(repo.path(), &["commit", "-q", "-m", "track it"]);
        fs::write(repo.path().join(".DS_Store"), "v2").unwrap();
        let files = worktree_dirty_files(repo.path()).unwrap();
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].path, ".DS_Store");
    }

    #[test]
    fn the_os_junk_patterns_follow_the_global_setting() {
        // The setting is process-wide and other tests rely on the defaults: this
        // one only ever adds to them, then puts them back.
        let mut patterns = default_os_junk_patterns();
        assert_eq!(patterns, [".DS_Store", "Thumbs.db", "desktop.ini", "._*"]);
        patterns.push("*.junk-setting".to_string());
        set_os_junk_patterns(&patterns);
        let published = os_junk_patterns();
        assert!(is_os_junk("a/b/file.junk-setting", &published));
        assert!(is_os_junk("._x", &published));
        assert!(is_os_junk("sub/.DS_Store", &published));
        assert!(!is_os_junk("file.junk-settings", &published));
        set_os_junk_patterns(&default_os_junk_patterns());
        assert!(!is_os_junk("a/file.junk-setting", &os_junk_patterns()));
        // An empty list means none: nothing is junk.
        assert!(!is_os_junk(".DS_Store", &[]));
    }

    #[test]
    fn a_task_worktree_ignores_os_junk_from_its_creation() {
        let repo = make_test_repo("junk-exclude");
        let info =
            create_task_worktree(repo.path(), "KT-1750", "junk", &head_sha(repo.path())).unwrap();
        let worktree = Path::new(&info.path);
        for junk in [".DS_Store", "Thumbs.db", "desktop.ini", "._x"] {
            fs::write(worktree.join(junk), "x").unwrap();
            let ignored = crate::core::cmd::git_cmd()
                .args(["check-ignore", "-q", junk])
                .current_dir(worktree)
                .status()
                .unwrap();
            assert!(ignored.success(), "{junk} must be ignored");
        }
        let status = crate::core::cmd::git_cmd()
            .args(["status", "--porcelain=v1"])
            .current_dir(worktree)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");
        // No tracked file was touched to get there.
        assert!(!repo.path().join(".gitignore").exists());
    }

    #[test]
    fn a_checkout_holding_only_a_ds_store_can_be_removed() {
        let repo = make_test_repo("junk-remove");
        let info =
            create_task_worktree(repo.path(), "KT-1750", "stale", &head_sha(repo.path())).unwrap();
        let worktree = PathBuf::from(&info.path);
        // A checkout from before the exclude existed: the junk is not ignored.
        let exclude = repo.path().join(".git/info/exclude");
        fs::write(&exclude, "").unwrap();
        fs::write(worktree.join(".DS_Store"), "x").unwrap();
        remove_ownerless_worktree(repo.path(), &worktree)
            .expect("a stale .DS_Store does not keep the checkout alive");
        assert!(!worktree.exists());
    }

    #[test]
    fn test_main_repo_state_on_named_branch() {
        let repo = make_test_repo("state-branch");
        let state = main_repo_state(repo.path()).unwrap();
        assert_eq!(state.current_branch, "main");
        assert!(!state.is_detached);
        assert!(state.dirty_files.is_empty());
    }

    #[test]
    fn test_main_repo_state_detects_detached_head() {
        let repo = make_test_repo("state-detached");
        // Move HEAD to the commit SHA directly → detached HEAD state.
        let sha_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
        std::process::Command::new("git")
            .args(["checkout", &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let state = main_repo_state(repo.path()).unwrap();
        assert!(state.is_detached, "should report detached HEAD");
        assert_eq!(state.current_branch, "");
    }

    #[test]
    fn test_main_repo_state_reports_dirty_files() {
        let repo = make_test_repo("state-dirty");
        fs::write(repo.path().join("foo.txt"), "x").unwrap();
        let state = main_repo_state(repo.path()).unwrap();
        assert!(!state.dirty_files.is_empty());
        assert!(state.dirty_files.iter().any(|f| f.path == "foo.txt"));
    }

    #[test]
    fn test_checkout_branch_succeeds_on_existing_branch() {
        let repo = make_test_repo("checkout-ok");
        // Create a second branch and switch back to main.
        std::process::Command::new("git")
            .args(["checkout", "-b", "feat/x"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["checkout", "main"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        assert!(checkout_branch(repo.path(), "feat/x").is_ok());
        let state = main_repo_state(repo.path()).unwrap();
        assert_eq!(state.current_branch, "feat/x");
    }

    #[test]
    fn test_checkout_branch_reports_error_on_unknown_branch() {
        let repo = make_test_repo("checkout-fail");
        let err = checkout_branch(repo.path(), "does/not/exist").unwrap_err();
        assert!(
            err.contains("checkout") && err.contains("does/not/exist"),
            "error should mention the failed checkout + branch: {}",
            err
        );
    }

    #[test]
    fn test_stash_push_returns_false_on_clean_tree() {
        let repo = make_test_repo("stash-clean");
        let stashed = stash_push(repo.path(), "kronn:test").unwrap();
        assert!(!stashed, "nothing to stash on a clean tree");
    }

    #[test]
    fn test_stash_push_and_pop_round_trip() {
        let repo = make_test_repo("stash-roundtrip");
        fs::write(repo.path().join("wip.txt"), "in-progress").unwrap();

        let stashed = stash_push(repo.path(), "kronn:auto-d123").unwrap();
        assert!(stashed);
        // Tree is now clean and file is gone from disk.
        assert!(worktree_dirty_files(repo.path()).unwrap().is_empty());
        assert!(!repo.path().join("wip.txt").exists());

        // Pop it back by its message.
        stash_pop_by_message(repo.path(), "kronn:auto-d123").unwrap();
        assert!(repo.path().join("wip.txt").exists());
        let dirty = worktree_dirty_files(repo.path()).unwrap();
        assert!(dirty.iter().any(|f| f.path == "wip.txt"));
    }

    #[test]
    fn test_stash_pop_by_message_missing_stash_returns_clear_error() {
        let repo = make_test_repo("stash-missing");
        let err = stash_pop_by_message(repo.path(), "kronn:not-there").unwrap_err();
        assert!(
            err.contains("not found"),
            "error should be user-friendly: {}",
            err
        );
    }

    // ── Task-execution worktree provisioning (KT-318) ─────────────────────────

    #[test]
    fn pinned_target_checkout_requires_one_checked_out_local_branch() {
        let repo = make_test_repo("pinned-target");
        assert_eq!(
            integration_target_worktree(repo.path(), "main").unwrap(),
            repo.path().canonicalize().unwrap()
        );
        for invalid in ["missing", "HEAD", "--all"] {
            assert!(integration_target_worktree(repo.path(), invalid).is_err());
        }
        assert!(crate::core::cmd::git_cmd()
            .args(["branch", "not-checked-out"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        assert!(integration_target_worktree(repo.path(), "not-checked-out").is_err());
        let duplicate = tempfile::tempdir().unwrap();
        let duplicate_path = duplicate.path().join("same branch");
        assert!(crate::core::cmd::git_cmd()
            .args(["worktree", "add", "--force"])
            .arg(&duplicate_path)
            .arg("main")
            .current_dir(repo.path())
            .output()
            .unwrap()
            .status
            .success());
        assert!(integration_target_worktree(repo.path(), "main")
            .unwrap_err()
            .contains("exactly one"));
    }

    #[test]
    fn pinned_target_checkout_refuses_dirty_or_drifted_heads() {
        for dirty in [true, false] {
            let repo = make_test_repo("pinned-target-guard");
            let original = git_head(repo.path()).unwrap();
            let candidate =
                create_task_worktree(repo.path(), "KT-616", "deadbeef", &original).unwrap();
            let sha = commit_file(
                Path::new(&candidate.path),
                "worker.txt",
                "validated",
                "candidate",
            );
            let expected = if dirty {
                fs::write(repo.path().join("user.txt"), "preserve exactly").unwrap();
                original.clone()
            } else {
                commit_file(repo.path(), "user.txt", "preserve exactly", "user work")
            };
            assert!(fast_forward_target_to(repo.path(), "main", &original, &sha).is_err());
            assert_eq!(git_head(repo.path()).unwrap(), expected);
            assert_eq!(
                fs::read_to_string(repo.path().join("user.txt")).unwrap(),
                "preserve exactly"
            );
            assert!(!repo.path().join("worker.txt").exists());
        }
    }

    /// A candidate commit on a task branch, plus the repo's `main` tip it descends from.
    fn candidate_on_task_branch(repo: &Path) -> (String, String) {
        let original = git_head(repo).unwrap();
        let task = create_task_worktree(repo, "KT-835", "deadbeef", &original).unwrap();
        let sha = commit_file(
            Path::new(&task.path),
            "worker.txt",
            "validated",
            "candidate",
        );
        (original, sha)
    }

    fn make_branch(repo: &Path, name: &str) {
        assert!(crate::core::cmd::git_cmd()
            .args(["branch", name, "main"])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
    }

    fn branch_tip(repo: &Path, name: &str) -> String {
        resolve_commit(repo, &format!("refs/heads/{name}")).unwrap()
    }

    #[test]
    fn integration_target_distinguishes_unchecked_once_and_several_checkouts() {
        let repo = make_test_repo("target-shape");
        make_branch(repo.path(), "elsewhere");
        assert_eq!(
            integration_target(repo.path(), "elsewhere").unwrap(),
            IntegrationTarget::NotCheckedOut {
                branch: "elsewhere".into()
            }
        );
        assert_eq!(
            integration_target(repo.path(), "main").unwrap(),
            IntegrationTarget::CheckedOut(repo.path().canonicalize().unwrap())
        );
        assert!(integration_target(repo.path(), "missing").is_err());
        assert!(integration_target(repo.path(), "HEAD").is_err());
        let extra = tempfile::tempdir().unwrap();
        assert!(crate::core::cmd::git_cmd()
            .args(["worktree", "add", "--force"])
            .arg(extra.path().join("second"))
            .arg("main")
            .current_dir(repo.path())
            .output()
            .unwrap()
            .status
            .success());
        assert!(integration_target(repo.path(), "main")
            .unwrap_err()
            .contains("exactly one"));
        // A target no worktree holds has no uncommitted file to protect.
        let unchecked = IntegrationTarget::NotCheckedOut {
            branch: "elsewhere".into(),
        };
        assert!(integration_target_dirty_files(&unchecked)
            .unwrap()
            .is_empty());
    }

    /// A real path git accepts: no `\\?\` prefix on Windows.
    fn for_git(path: &Path) -> PathBuf {
        crate::core::lane_git::without_verbatim_prefix(path.canonicalize().unwrap())
    }

    fn add_worktree(repo: &Path, path: &Path, branch: &str) {
        assert!(crate::core::cmd::git_cmd()
            .args(["worktree", "add"])
            .arg(path)
            .arg(branch)
            .current_dir(repo)
            .output()
            .unwrap()
            .status
            .success());
    }

    #[test]
    fn integration_target_refuses_the_owners_checkout_and_foreign_worktrees() {
        let repo = make_test_repo("target-ownership");
        ownership_seam::enforce_for(repo.path());
        make_branch(repo.path(), "elsewhere");

        // `main` is checked out in the project's own working checkout.
        let refusal = integration_target_ownership(repo.path(), "main", &[], &|_| false)
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            refusal,
            IntegrationTargetRefusal::OwnerCheckout { ref branch, .. } if branch == "main"
        ));
        assert_eq!(refusal.code(), "integration_target_owner_checkout");
        assert!(refusal
            .message()
            .starts_with("integration_target_owner_checkout: "));

        // A branch no checkout holds is advanced by a ref swap: nothing to refuse.
        assert_eq!(
            integration_target_ownership(repo.path(), "elsewhere", &[], &|_| false).unwrap(),
            Ok(())
        );

        // A worktree outside `.kronn/worktrees` is not Kronn's.
        let outside = tempfile::tempdir().unwrap();
        make_branch(repo.path(), "foreign");
        add_worktree(repo.path(), &outside.path().join("wt"), "foreign");
        let refusal = integration_target_ownership(repo.path(), "foreign", &[], &|_| false)
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            refusal,
            IntegrationTargetRefusal::ForeignWorktree { .. }
        ));
        assert_eq!(refusal.code(), "integration_target_foreign_worktree");

        // A worktree under `<repo>/.kronn/worktrees` is.
        make_branch(repo.path(), "integ");
        let owned = worktree_base_dir(repo.path());
        fs::create_dir_all(&owned).unwrap();
        add_worktree(repo.path(), &owned.join("integ"), "integ");
        assert_eq!(
            integration_target_ownership(repo.path(), "integ", &[], &|_| false).unwrap(),
            Ok(())
        );
        assert!(integration_target_ownership(repo.path(), "missing", &[], &|_| false).is_err());
    }

    /// KT-1748 — a target held by a worktree under Kronn's autoCode root is
    /// Kronn's own, while the owner's checkout and an unknown worktree are not.
    #[test]
    fn integration_target_accepts_autocode_and_registered_worktrees_only() {
        let repo = make_test_repo("target-autocode");
        let kronn_home = tempfile::tempdir().unwrap();
        let kronn_home_path = for_git(kronn_home.path());
        let roots = managed_worktree_roots(repo.path(), Some(&kronn_home_path), &[]);
        let ownership = |branch: &str, registered: &dyn Fn(&Path) -> bool| {
            integration_target_ownership_in(repo.path(), branch, &roots, registered).unwrap()
        };

        // `feat/EW-7704` held in `~/.kronn/autocode-worktrees/<project>/EW-7704`.
        make_branch(repo.path(), "feat/EW-7704");
        let autocode = kronn_home_path
            .join(AUTOCODE_WORKTREES_DIR)
            .join("front_euronews");
        fs::create_dir_all(&autocode).unwrap();
        add_worktree(repo.path(), &autocode.join("EW-7704"), "feat/EW-7704");
        assert_eq!(ownership("feat/EW-7704", &|_| false), Ok(()));

        // A root the operator declared as managed makes its worktrees Kronn's too.
        let extra = tempfile::tempdir().unwrap();
        let extra_path = for_git(extra.path());
        make_branch(repo.path(), "feat/extra");
        add_worktree(repo.path(), &extra_path.join("wt"), "feat/extra");
        assert!(matches!(
            ownership("feat/extra", &|_| false),
            Err(IntegrationTargetRefusal::ForeignWorktree { .. })
        ));
        let with_extra = managed_worktree_roots(
            repo.path(),
            Some(&kronn_home_path),
            std::slice::from_ref(&extra_path),
        );
        assert_eq!(
            integration_target_ownership_in(repo.path(), "feat/extra", &with_extra, &|_| false)
                .unwrap(),
            Ok(())
        );

        // The owner's own checkout stays protected, even if the registry knew it.
        assert!(matches!(
            ownership("main", &|_| true),
            Err(IntegrationTargetRefusal::OwnerCheckout { .. })
        ));

        // An unknown worktree outside every root is refused...
        let outside = tempfile::tempdir().unwrap();
        make_branch(repo.path(), "feat/foreign");
        let foreign = for_git(outside.path()).join("wt");
        add_worktree(repo.path(), &foreign, "feat/foreign");
        let refusal = ownership("feat/foreign", &|_| false).unwrap_err();
        assert_eq!(refusal.code(), "integration_target_foreign_worktree");
        assert!(refusal.detail().contains("autocode-worktrees"));

        // ...unless Kronn's workspace registry recorded it (workflow run, managed workspace).
        assert_eq!(
            ownership("feat/foreign", &|path| path == foreign.as_path()),
            Ok(())
        );
        assert!(matches!(
            ownership("feat/foreign", &|path| path.ends_with("unrelated")),
            Err(IntegrationTargetRefusal::ForeignWorktree { .. })
        ));
    }

    #[test]
    fn managed_roots_ignore_relative_extras_and_a_missing_home() {
        // Absolute on every platform: `/srv` has no drive on Windows.
        let base = std::env::temp_dir();
        let repo = base.join("repo");
        let roots = managed_worktree_roots(&repo, None, &[PathBuf::from("relative/root")]);
        assert_eq!(roots, vec![repo.join(".kronn/worktrees")]);
        let home = base.join("home").join(".kronn");
        let lanes = base.join("srv").join("lanes");
        let roots = managed_worktree_roots(&repo, Some(&home), std::slice::from_ref(&lanes));
        assert_eq!(
            roots,
            vec![
                repo.join(".kronn/worktrees"),
                home.join("autocode-worktrees"),
                lanes,
            ]
        );
    }

    /// KT-1748 — integrating into a managed worktree a run is using right now is
    /// refused, naming the run; the same worktree idle is advanced; a target no
    /// worktree holds is never busy.
    #[test]
    fn integration_target_idle_refuses_a_worktree_a_run_is_using() {
        let repo = make_test_repo("target-idle");
        make_branch(repo.path(), "integ");
        make_branch(repo.path(), "elsewhere");
        let owned = worktree_base_dir(repo.path());
        fs::create_dir_all(&owned).unwrap();
        let held = for_git(&owned).join("integ");
        add_worktree(repo.path(), &held, "integ");

        let busy = integration_target_idle(repo.path(), "integ", &|path| {
            (path == held.as_path()).then(|| "workflow run r-42 is still active".to_string())
        })
        .unwrap()
        .unwrap_err();
        assert_eq!(busy.code(), "integration_target_busy_worktree");
        let text = busy.message();
        assert!(text.starts_with("integration_target_busy_worktree: "));
        assert!(text.contains("workflow run r-42"));
        assert!(!text.contains("KRONN_MANAGED_WORKTREE_ROOTS"));

        // Same worktree, nothing using it: idle, and the advance goes through.
        assert_eq!(
            integration_target_idle(repo.path(), "integ", &|_| None).unwrap(),
            Ok(())
        );
        let (original, sha) = candidate_on_task_branch(repo.path());
        assert_eq!(branch_tip(repo.path(), "integ"), original);
        assert_eq!(
            fast_forward_target_to(repo.path(), "integ", &original, &sha).unwrap(),
            sha
        );

        // Held by nothing: there is no checkout to move, so no holder is asked.
        assert_eq!(
            integration_target_idle(repo.path(), "elsewhere", &|_| panic!("asked")).unwrap(),
            Ok(())
        );
    }

    #[test]
    fn integration_target_is_described_with_its_worktree_and_owner() {
        use crate::models::IntegrationTargetState as State;
        let repo = make_test_repo("target-describe");
        ownership_seam::enforce_for(repo.path());
        make_branch(repo.path(), "elsewhere");
        make_branch(repo.path(), "integ");
        let owned = worktree_base_dir(repo.path());
        fs::create_dir_all(&owned).unwrap();
        add_worktree(repo.path(), &owned.join("integ"), "integ");

        let owner = describe_integration_target(repo.path(), "main", &[], &|_| false);
        assert_eq!(owner.state, State::OwnerCheckout);
        assert_eq!(owner.branch, "main");
        assert_eq!(
            Path::new(owner.worktree.as_deref().unwrap())
                .canonicalize()
                .unwrap(),
            repo.path().canonicalize().unwrap()
        );

        let free = describe_integration_target(repo.path(), "elsewhere", &[], &|_| false);
        assert_eq!(free.state, State::NotCheckedOut);
        assert_eq!(free.worktree, None);

        let kronn = describe_integration_target(repo.path(), "integ", &[], &|_| false);
        assert_eq!(kronn.state, State::KronnWorktree);
        assert!(kronn.worktree.unwrap().ends_with("integ"));

        assert_eq!(
            describe_integration_target(repo.path(), "nope", &[], &|_| false).state,
            State::Unresolved
        );
    }

    #[test]
    fn unchecked_out_target_advances_by_compare_and_swap_without_a_checkout() {
        let repo = make_test_repo("target-update-ref");
        make_branch(repo.path(), "elsewhere");
        let (original, sha) = candidate_on_task_branch(repo.path());
        fs::write(repo.path().join("user.txt"), "preserve exactly").unwrap();

        let landed = fast_forward_target_to(repo.path(), "elsewhere", &original, &sha).unwrap();
        assert_eq!(landed, sha);
        assert_eq!(branch_tip(repo.path(), "elsewhere"), sha);
        assert_eq!(
            integration_target_tip(repo.path(), "elsewhere").unwrap(),
            sha
        );
        assert_eq!(git_head(repo.path()).unwrap(), original, "main untouched");
        assert_eq!(
            fs::read_to_string(repo.path().join("user.txt")).unwrap(),
            "preserve exactly"
        );
        assert!(!repo.path().join("worker.txt").exists());
    }

    #[test]
    fn unchecked_out_target_that_moved_since_the_review_is_never_overwritten() {
        let repo = make_test_repo("target-cas");
        make_branch(repo.path(), "elsewhere");
        let (original, sha) = candidate_on_task_branch(repo.path());
        // The branch moves after the candidate was anchored on `original`.
        let moved = commit_file(repo.path(), "other.txt", "other", "other work");
        assert!(crate::core::cmd::git_cmd()
            .args(["branch", "-f", "elsewhere", &moved])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        let error = fast_forward_target_to(repo.path(), "elsewhere", &original, &sha).unwrap_err();
        assert!(error.contains(&moved), "{error}");
        assert_eq!(branch_tip(repo.path(), "elsewhere"), moved);
    }

    #[test]
    fn unchecked_out_target_refuses_a_candidate_that_is_not_a_fast_forward() {
        let repo = make_test_repo("target-not-ff");
        make_branch(repo.path(), "elsewhere");
        let (original, _) = candidate_on_task_branch(repo.path());
        // A root commit does not descend from the expected tip.
        let tree = crate::core::cmd::git_cmd()
            .args(["rev-parse", "HEAD^{tree}"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
        let out = crate::core::cmd::git_cmd()
            .args(["commit-tree", "-m", "orphan", &tree])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        let orphan = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let error =
            fast_forward_target_to(repo.path(), "elsewhere", &original, &orphan).unwrap_err();
        assert!(error.contains("fast-forward"), "{error}");
        assert_eq!(branch_tip(repo.path(), "elsewhere"), original);
    }

    #[test]
    fn target_checked_out_since_the_review_takes_the_checkout_guards() {
        let repo = make_test_repo("target-became-checked-out");
        make_branch(repo.path(), "elsewhere");
        let (original, sha) = candidate_on_task_branch(repo.path());
        let extra = tempfile::tempdir().unwrap();
        let checkout = extra.path().join("elsewhere");
        assert!(crate::core::cmd::git_cmd()
            .args(["worktree", "add"])
            .arg(&checkout)
            .arg("elsewhere")
            .current_dir(repo.path())
            .output()
            .unwrap()
            .status
            .success());
        // The ref is never moved under a dirty checkout: its index would go stale.
        fs::write(checkout.join("user.txt"), "preserve exactly").unwrap();
        assert!(fast_forward_target_to(repo.path(), "elsewhere", &original, &sha).is_err());
        assert_eq!(branch_tip(repo.path(), "elsewhere"), original);
        // Clean again, it advances through the checkout's own fast-forward.
        fs::remove_file(checkout.join("user.txt")).unwrap();
        assert_eq!(
            fast_forward_target_to(repo.path(), "elsewhere", &original, &sha).unwrap(),
            sha
        );
        assert_eq!(git_head(&checkout).unwrap(), sha);
    }

    #[cfg(unix)] // a file name with a newline cannot exist on Windows
    #[test]
    fn pinned_target_checkout_preserves_newlines_and_unicode_in_paths() {
        let repo = make_test_repo("pinned-target-path");
        let directory = tempfile::tempdir().unwrap();
        let checkout = directory.path().join("review\n target é");
        assert!(crate::core::cmd::git_cmd()
            .args(["worktree", "add", "-b", "review-target"])
            .arg(&checkout)
            .arg("main")
            .current_dir(repo.path())
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(
            integration_target_worktree(repo.path(), "review-target").unwrap(),
            checkout.canonicalize().unwrap()
        );
    }

    /// Commit a file in `dir` and return the new HEAD sha.
    fn commit_file(dir: &Path, name: &str, content: &str, msg: &str) -> String {
        fs::write(dir.join(name), content).unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", msg])
            .current_dir(dir)
            .output()
            .unwrap();
        head_sha(dir)
    }

    fn head_sha(dir: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn add_materialized_test_submodule(repo: &Path) -> tempfile::TempDir {
        let submodule = tempfile::Builder::new()
            .prefix("kronn-wt-submodule-source-")
            .tempdir()
            .unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(submodule.path())
            .output()
            .unwrap();
        assert!(init.status.success());
        for (key, value) in [("user.email", "test@test.com"), ("user.name", "Test")] {
            let output = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(submodule.path())
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        fs::create_dir_all(submodule.path().join("bin")).unwrap();
        fs::write(
            submodule.path().join("bin/bats"),
            "#!/bin/sh\necho bats fixture\n",
        )
        .unwrap();
        let commit = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(submodule.path())
            .output()
            .unwrap();
        assert!(commit.status.success());
        let executable = std::process::Command::new("git")
            .args(["update-index", "--chmod=+x", "bin/bats"])
            .current_dir(submodule.path())
            .output()
            .unwrap();
        assert!(executable.status.success());
        let commit = std::process::Command::new("git")
            .args(["commit", "-m", "submodule source"])
            .current_dir(submodule.path())
            .output()
            .unwrap();
        assert!(commit.status.success());

        fs::write(
            repo.join("Makefile"),
            "test-shell:\n\t./deps/required/bin/bats --version\n",
        )
        .unwrap();
        let add = std::process::Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                &submodule.path().to_string_lossy(),
                "deps/required",
            ])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            add.status.success(),
            "submodule add failed: {}",
            String::from_utf8_lossy(&add.stderr)
        );
        let stage_makefile = std::process::Command::new("git")
            .args(["add", "Makefile"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(stage_makefile.status.success());
        let commit = std::process::Command::new("git")
            .args(["commit", "-m", "add required submodule"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            commit.status.success(),
            "submodule commit failed: {}",
            String::from_utf8_lossy(&commit.stderr)
        );
        let unreachable_remote = std::process::Command::new("git")
            .args([
                "config",
                "submodule.deps/required.url",
                "https://invalid.example/required.git",
            ])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(unreachable_remote.status.success());
        submodule
    }

    #[test]
    fn task_worktree_pins_exact_sha_and_names_deterministically() {
        let repo = make_test_repo("task-pin");
        let base = head_sha(repo.path()); // first commit
        commit_file(repo.path(), "later.txt", "x", "second"); // move main forward
        assert_ne!(base, head_sha(repo.path()), "main tip must have advanced");

        let info = create_task_worktree(repo.path(), "KT-142", "a1b2c3d4", &base).unwrap();
        // Domain-scoped deterministic naming (ADR-002: kronn/task/...), not the ticket.
        assert_eq!(info.branch, "kronn/task/kt-142-a1b2c3d4");
        assert_eq!(
            PathBuf::from(&info.path),
            worktree_base_dir(repo.path()).join("task-kt-142-a1b2c3d4"),
            "task worktree path must use native separators and the managed layout"
        );
        // HEAD is the PINNED base, not main's tip — a concurrent push cannot move it.
        assert_eq!(head_sha(Path::new(&info.path)), base);
        assert!(
            worktree_dirty_files(repo.path()).unwrap().is_empty(),
            "provisioning must not dirty the target just to ignore .kronn"
        );
        assert!(!repo.path().join(".gitignore").exists());
        let exclude = fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|line| line.trim() == ".kronn/"));
    }

    /// KT-1595 — `ensure_local_git_exclude` is reached from every audit-state
    /// write, right after an agent. A `.git/info` folder or an `exclude` file
    /// swapped for a link never makes Kronn create or append outside the
    /// git directory.
    #[cfg(unix)] // the swap probe plants a symlink through std::os::unix
    #[test]
    fn the_local_exclude_is_never_updated_through_a_swapped_info_folder() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("exclude-swap");
        let info = repo.path().join(".git/info");
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("exclude"), "outside\n").unwrap();

        // The swap lands between the read of `exclude` and the write.
        swap_probe::arm_after(&info, external.path(), 1);
        let outcome = ensure_local_git_exclude(repo.path(), ".kronn/");
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(
            fs::read_to_string(external.path().join("exclude")).unwrap(),
            "outside\n",
            "the file behind the link is untouched"
        );
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 1);
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn the_local_exclude_refuses_a_linked_info_folder_and_a_linked_exclude_file() {
        let repo = make_test_repo("exclude-link");
        let info = repo.path().join(".git/info");
        let external = tempfile::tempdir().unwrap();

        // A folder link: nothing is created in the folder it points at.
        fs::rename(&info, repo.path().join(".git/info.aside")).unwrap();
        std::os::unix::fs::symlink(external.path(), &info).unwrap();
        let outcome = ensure_local_git_exclude(repo.path(), ".kronn/");
        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);

        // A file link: the file it points at is not appended to.
        fs::remove_file(&info).unwrap();
        fs::rename(repo.path().join(".git/info.aside"), &info).unwrap();
        fs::remove_file(info.join("exclude")).unwrap();
        let victim = external.path().join("victim");
        fs::write(&victim, "victim\n").unwrap();
        std::os::unix::fs::symlink(&victim, info.join("exclude")).unwrap();
        let outcome = ensure_local_git_exclude(repo.path(), ".kronn/");
        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "victim\n");
    }

    /// KT-1595 (N3) — after an agent, git's answer is not trusted: a planted
    /// `.git/commondir` that names another repository's git directory makes
    /// `rev-parse --git-path info/exclude` point there. The pinned writer
    /// refuses; nothing is created or appended in the other repository.
    #[test]
    fn the_pinned_exclude_refuses_a_planted_commondir() {
        let repo = make_test_repo("exclude-commondir");
        let victim = make_test_repo("exclude-commondir-victim");
        let victim_exclude = victim.path().join(".git/info/exclude");
        let before = fs::read_to_string(&victim_exclude).unwrap_or_default();
        let own_before = fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        fs::write(
            repo.path().join(".git/commondir"),
            victim.path().join(".git").to_string_lossy().as_bytes(),
        )
        .unwrap();

        let outcome = ensure_local_git_exclude_pinned(repo.path(), ".kronn/");

        assert!(outcome.is_err(), "{outcome:?}");
        assert!(
            outcome.unwrap_err().contains("commondir"),
            "the refusal names the pointer"
        );
        assert_eq!(
            fs::read_to_string(&victim_exclude).unwrap_or_default(),
            before,
            "the other repository's exclude is untouched"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap(),
            own_before,
            "nor is its own exclude touched"
        );
    }

    /// KT-1595 (N3) — the same for a `.git` FILE whose `gitdir:` the agent
    /// rewrote (the form of a linked worktree or a submodule).
    #[test]
    fn the_pinned_exclude_refuses_a_gitdir_pointer_file() {
        let repo = make_test_repo("exclude-gitdir");
        let victim = make_test_repo("exclude-gitdir-victim");
        let victim_exclude = victim.path().join(".git/info/exclude");
        let before = fs::read_to_string(&victim_exclude).unwrap_or_default();
        let aside = repo.path().join(".git.aside");
        fs::rename(repo.path().join(".git"), &aside).unwrap();
        fs::write(
            repo.path().join(".git"),
            format!("gitdir: {}\n", victim.path().join(".git").display()),
        )
        .unwrap();

        let outcome = ensure_local_git_exclude_pinned(repo.path(), ".kronn/");

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(
            fs::read_to_string(&victim_exclude).unwrap_or_default(),
            before,
            "the other repository's exclude is untouched"
        );
    }

    /// KT-1595 (N3) — a `.git` that is itself a link to another repository is
    /// refused (the rooted walk never follows it, and neither does the lstat).
    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn the_pinned_exclude_refuses_a_git_directory_that_is_a_link() {
        let repo = make_test_repo("exclude-gitlink");
        let victim = make_test_repo("exclude-gitlink-victim");
        let victim_exclude = victim.path().join(".git/info/exclude");
        let before = fs::read_to_string(&victim_exclude).unwrap_or_default();
        fs::rename(repo.path().join(".git"), repo.path().join(".git.aside")).unwrap();
        std::os::unix::fs::symlink(victim.path().join(".git"), repo.path().join(".git")).unwrap();

        let outcome = ensure_local_git_exclude_pinned(repo.path(), ".kronn/");

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(
            fs::read_to_string(&victim_exclude).unwrap_or_default(),
            before
        );
    }

    #[test]
    fn the_pinned_exclude_keeps_its_lines_and_adds_the_pattern_once() {
        let repo = make_test_repo("exclude-pinned");
        let exclude = repo.path().join(".git/info/exclude");
        fs::write(&exclude, "# mine\n*.log").unwrap();
        ensure_local_git_exclude_pinned(repo.path(), ".kronn/").unwrap();
        ensure_local_git_exclude_pinned(repo.path(), ".kronn/").unwrap();
        assert_eq!(
            fs::read_to_string(&exclude).unwrap(),
            "# mine\n*.log\n.kronn/\n"
        );
    }

    #[test]
    fn the_local_exclude_keeps_its_lines_and_adds_the_pattern_once() {
        let repo = make_test_repo("exclude-append");
        let exclude = repo.path().join(".git/info/exclude");
        fs::write(&exclude, "# mine\n*.log").unwrap();
        ensure_local_git_exclude(repo.path(), ".kronn/").unwrap();
        ensure_local_git_exclude(repo.path(), ".kronn/").unwrap();
        assert_eq!(
            fs::read_to_string(&exclude).unwrap(),
            "# mine\n*.log\n.kronn/\n"
        );
    }

    #[test]
    fn task_worktree_materializes_pinned_submodules_without_sharing_checkouts() {
        let repo = make_test_repo("task-submodule");
        let _source = add_materialized_test_submodule(repo.path());
        let base = head_sha(repo.path());
        let source_submodule = repo.path().join("deps/required");
        let source_gitlink = std::process::Command::new("git")
            .args(["rev-parse", "HEAD:deps/required"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(source_gitlink.status.success());
        assert_eq!(
            String::from_utf8_lossy(&source_gitlink.stdout).trim(),
            head_sha(&source_submodule),
            "the test source checkout must match its committed gitlink"
        );

        let info = create_task_worktree(repo.path(), "KT-478", "submodule", &base).unwrap();
        let worktree = Path::new(&info.path);
        let task_submodule = worktree.join("deps/required");
        assert_eq!(head_sha(&task_submodule), head_sha(&source_submodule));
        assert!(task_submodule.join("bin/bats").is_file());
        #[cfg(unix)]
        {
            let shell_tests = std::process::Command::new("make")
                .arg("test-shell")
                .current_dir(worktree)
                .output()
                .unwrap();
            assert!(
                shell_tests.status.success(),
                "make test-shell failed: {}",
                String::from_utf8_lossy(&shell_tests.stderr)
            );
        }

        let source_git_dir = std::process::Command::new("git")
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(&source_submodule)
            .output()
            .unwrap();
        let task_git_dir = std::process::Command::new("git")
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(&task_submodule)
            .output()
            .unwrap();
        assert_ne!(
            String::from_utf8_lossy(&source_git_dir.stdout).trim(),
            String::from_utf8_lossy(&task_git_dir.stdout).trim(),
            "task worktree must not share the source submodule Git directory"
        );

        // Resume reconstructs only the missing task checkout from the already
        // materialized source; cleanup then remains ownership-safe.
        fs::remove_dir_all(&task_submodule).unwrap();
        materialize_task_submodules(repo.path(), worktree).unwrap();
        assert_eq!(head_sha(&task_submodule), head_sha(&source_submodule));
        remove_task_worktree(repo.path(), &info.path, &info.branch, &base).unwrap();
        assert!(!worktree.exists());
    }

    #[test]
    fn task_worktree_refuses_when_a_required_source_submodule_is_missing() {
        let repo = make_test_repo("task-submodule-missing");
        let _source = add_materialized_test_submodule(repo.path());
        let base = head_sha(repo.path());
        let deinit = std::process::Command::new("git")
            .args(["submodule", "deinit", "-f", "deps/required"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(deinit.status.success());

        let (worktree_path, branch) =
            task_worktree_layout(repo.path(), "KT-478", "missing-source").unwrap();
        let error = create_task_worktree(repo.path(), "KT-478", "missing-source", &base)
            .expect_err("a task checkout must not fetch a missing source submodule");
        assert!(error.contains("required source submodule"), "{error}");
        assert!(
            !worktree_path.exists(),
            "failed provisioning is compensated"
        );
        assert!(branch_commit(repo.path(), &branch).is_none());
    }

    #[test]
    fn task_worktree_layout_neutralizes_windows_and_parent_traversal_syntax() {
        let repo = Path::new("C:\\repos\\kronn");
        let (path, branch) =
            task_worktree_layout(repo, "KT-7\\..\\..\\Windows:C:", "exec/..\\danger").unwrap();
        assert!(path.starts_with(worktree_base_dir(repo)));
        assert!(!branch.contains('\\'));
        assert!(!branch.contains(':'));
        assert!(!branch.contains(".."));
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn task_worktree_refuses_a_symlinked_managed_parent() {
        let repo = make_test_repo("task-symlink-parent");
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), repo.path().join(".kronn")).unwrap();
        let error = create_task_worktree(repo.path(), "KT-70", "symlink", &head_sha(repo.path()))
            .unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        assert!(fs::read_dir(external.path()).unwrap().next().is_none());
    }

    #[cfg(unix)] // creates symlinks through std::os::unix; reads unix inode metadata
    #[test]
    fn task_worktree_refuses_a_dangling_symlink_collision() {
        let repo = make_test_repo("task-dangling-link");
        let (path, _) = task_worktree_layout(repo.path(), "KT-71", "dangling").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(repo.path().join("missing"), &path).unwrap();
        let error = create_task_worktree(repo.path(), "KT-71", "dangling", &head_sha(repo.path()))
            .unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }

    #[test]
    fn task_cleanup_refuses_an_external_checkout_path() {
        let repo = make_test_repo("task-external-cleanup");
        let external = tempfile::tempdir().unwrap();
        let error = remove_cancelled_task_worktree(
            repo.path(),
            &external.path().to_string_lossy(),
            "main",
            external.path(),
        )
        .unwrap_err();
        assert!(error.contains("outside managed root"), "{error}");
        assert!(external.path().exists());
    }

    #[test]
    fn task_cleanup_after_repo_move_fails_closed_on_the_old_path() {
        let old_repo = make_test_repo("task-before-move");
        let new_repo = make_test_repo("task-after-move");
        let info = create_task_worktree(
            old_repo.path(),
            "KT-72",
            "moved",
            &head_sha(old_repo.path()),
        )
        .unwrap();
        let evidence = tempfile::tempdir().unwrap();
        let error = remove_cancelled_task_worktree(
            new_repo.path(),
            &info.path,
            &info.branch,
            evidence.path(),
        )
        .unwrap_err();
        assert!(error.contains("outside managed root"), "{error}");
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn task_worktree_refuses_existing_branch() {
        let repo = make_test_repo("task-dup");
        let base = head_sha(repo.path());
        create_task_worktree(repo.path(), "KT-9", "dead", &base).unwrap();
        // Same ref+exec (a double-launch or a stale attempt) fails closed.
        let again = create_task_worktree(repo.path(), "KT-9", "dead", &base);
        assert!(
            again.is_err(),
            "second launch on same ref+exec must fail closed"
        );
        assert!(again.unwrap_err().contains("already"));
    }

    #[test]
    fn task_worktree_verify_head_detects_advance() {
        let repo = make_test_repo("task-verify");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-3", "beef", &base).unwrap();
        assert!(verify_worktree_head(Path::new(&info.path), &base).is_ok());
        // Commit inside the worktree → HEAD advances → verify must fail.
        commit_file(Path::new(&info.path), "work.txt", "y", "work");
        assert!(verify_worktree_head(Path::new(&info.path), &base).is_err());
    }

    #[test]
    fn resolve_commit_pins_branch_to_full_sha() {
        let repo = make_test_repo("task-resolve");
        let sha = resolve_commit(repo.path(), "main").unwrap();
        assert_eq!(sha.len(), 40);
        assert_eq!(sha, head_sha(repo.path()));
        assert!(resolve_commit(repo.path(), "no/such/ref").is_err());
    }

    #[test]
    fn integration_target_must_be_a_local_branch() {
        let repo = make_test_repo("task-target-branch");
        let sha = resolve_commit(repo.path(), "main").unwrap();
        assert_eq!(resolve_local_branch(repo.path(), "main").unwrap(), "main");
        assert_eq!(
            resolve_local_branch(repo.path(), "refs/heads/main").unwrap(),
            "main"
        );
        assert!(resolve_local_branch(repo.path(), &sha).is_err());
        assert!(resolve_local_branch(repo.path(), "HEAD").is_err());
        assert!(resolve_local_branch(repo.path(), "no/such/branch").is_err());
    }

    /// KT-1256 — a lane target as a worker leaves it: artefacts under
    /// `<repo>/target/wt-<lane>`.
    fn seed_lane_target(repo: &Path, lane: &str) -> PathBuf {
        let target = repo.join("target").join(format!("wt-{lane}"));
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/kronn"), vec![0u8; 4096]).unwrap();
        target
    }

    #[test]
    fn lane_target_follows_the_worktree_name() {
        // Absolute on every platform: `/repo` is not on Windows.
        let scratch = tempfile::tempdir().unwrap();
        let repo = scratch.path();
        let worktree = repo.join(".kronn/worktrees/task-kt-9-abcd1234");
        assert_eq!(
            lane_target_for_worktree(repo, &worktree).unwrap(),
            repo.join("target/wt-kt-9-abcd1234")
        );
    }

    #[test]
    fn remove_lane_target_deletes_the_lane_and_only_the_lane() {
        let repo = make_test_repo("lane-rm");
        let lane = seed_lane_target(repo.path(), "kt-1-aaaa");
        let neighbour = seed_lane_target(repo.path(), "kt-2-bbbb");
        fs::create_dir_all(repo.path().join("target/debug")).unwrap();

        let report = remove_lane_target(repo.path(), &lane).unwrap();

        assert!(!lane.exists(), "the lane target is gone");
        assert!(report.bytes_reclaimed >= 4096, "{report:?}");
        assert!(neighbour.exists(), "another lane is untouched");
        assert!(
            repo.path().join("target/debug").exists(),
            "the main target is untouched"
        );
        // Idempotent: absent is success with a zero figure.
        let again = remove_lane_target(repo.path(), &lane).unwrap();
        assert_eq!(again.bytes_reclaimed, 0);
    }

    #[cfg(unix)] // swaps a folder for a symlink (std::os::unix)
    #[test]
    fn a_target_folder_swapped_after_the_walk_never_loses_an_outside_lane() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("lane-rm-swap");
        let lane = seed_lane_target(repo.path(), "kt-1-aaaa");
        let outside = tempfile::tempdir().unwrap();
        let outside_lane = outside.path().join("wt-kt-1-aaaa");
        fs::create_dir_all(outside_lane.join("debug")).unwrap();
        fs::write(outside_lane.join("debug/kronn"), "KEEP").unwrap();
        swap_probe::arm(&repo.path().join("target"), outside.path());
        let _ = remove_lane_target(repo.path(), &lane);
        assert!(swap_probe::fired(), "the swap really happened");
        assert_eq!(
            fs::read_to_string(outside_lane.join("debug/kronn")).unwrap(),
            "KEEP"
        );
    }

    #[cfg(unix)] // swaps a folder for a symlink (std::os::unix)
    #[test]
    fn a_target_folder_swapped_after_the_walk_never_receives_the_lane_marker() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("lane-mark-swap");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-30", "c3d4", &base).unwrap();
        fs::create_dir_all(repo.path().join("target")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        swap_probe::arm(&repo.path().join("target"), outside.path());
        let marked = mark_lane_target(repo.path(), Path::new(&info.path), "exec-30");
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(marked.is_err(), "{marked:?}");
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[cfg(unix)] // swaps a folder for a symlink (std::os::unix)
    #[test]
    fn a_kronn_folder_swapped_after_the_walk_never_receives_a_task_worktree() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("wt-create-swap");
        let base = head_sha(repo.path());
        fs::create_dir_all(repo.path().join(".kronn")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        swap_probe::arm(&repo.path().join(".kronn"), outside.path());
        let created = create_task_worktree(repo.path(), "KT-31", "e5f6", &base);
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(created.is_err(), "{created:?}");
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[test]
    fn remove_lane_target_refuses_what_is_not_a_lane_target() {
        let repo = make_test_repo("lane-refuse");
        let main_target = repo.path().join("target");
        fs::create_dir_all(main_target.join("debug")).unwrap();
        let nested = seed_lane_target(repo.path(), "kt-3-cccc").join("debug");
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), "x").unwrap();

        for refused in [
            main_target.clone(),
            main_target.join("debug"),
            nested,
            main_target.join("wt-"),
            outside.path().to_path_buf(),
            main_target.join("wt-kt/../debug"),
        ] {
            let error = remove_lane_target(repo.path(), &refused).unwrap_err();
            assert!(!error.is_empty());
        }
        assert!(main_target.join("debug").exists());
        assert!(outside.path().join("keep").exists());

        #[cfg(unix)]
        {
            let link = main_target.join("wt-linked");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            let error = remove_lane_target(repo.path(), &link).unwrap_err();
            assert!(error.contains("symlink"), "{error}");
            assert!(outside.path().join("keep").exists(), "never followed");
        }
    }

    #[test]
    fn remove_lane_target_refuses_while_cargo_holds_its_lock() {
        use fs2::FileExt;
        let repo = make_test_repo("lane-lock");
        let lane = seed_lane_target(repo.path(), "kt-4-dddd");
        let lock = fs::File::create(lane.join("debug/.cargo-lock")).unwrap();
        lock.lock_exclusive().unwrap();

        let error = remove_lane_target(repo.path(), &lane).unwrap_err();
        assert!(error.contains("build"), "{error}");
        assert!(lane.exists(), "a running build keeps its target");

        fs2::FileExt::unlock(&lock).unwrap();
        remove_lane_target(repo.path(), &lane).unwrap();
        assert!(!lane.exists());
    }

    /// A lane the way provisioning leaves it: a task worktree, a lane target
    /// with artefacts, and the ownership marker for `exec`.
    fn marked_lane(
        repo: &Path,
        ticket: &str,
        suffix: &str,
        exec: &str,
    ) -> (PathBuf, PathBuf, String) {
        let base = head_sha(repo);
        let info = create_task_worktree(repo, ticket, suffix, &base).unwrap();
        let worktree = PathBuf::from(&info.path);
        let lane_target = mark_lane_target(repo, &worktree, exec).unwrap();
        fs::create_dir_all(lane_target.join("debug")).unwrap();
        fs::write(lane_target.join("debug/kronn"), vec![0u8; 4096]).unwrap();
        (worktree, lane_target, info.branch)
    }

    fn finished(_: &LaneOwner) -> Result<(), String> {
        Ok(())
    }

    fn alive(_: &LaneOwner) -> Result<(), String> {
        Err("execution is Working".to_string())
    }

    /// Windows opens a directory only with backup semantics, and setting a
    /// time needs write access there.
    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            fs::OpenOptions::new()
                .write(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
                .unwrap()
        };
        #[cfg(not(windows))]
        let file = fs::File::open(path).unwrap();
        file.set_modified(when).unwrap();
    }

    /// Back-date every entry under `path` (children first, so writing a
    /// directory's mtime is not undone by its own children).
    fn age_tree(path: &Path, ago: std::time::Duration) {
        fn walk(path: &Path, when: std::time::SystemTime) {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let child = entry.path();
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        walk(&child, when);
                    } else {
                        set_mtime(&child, when);
                    }
                }
            }
            set_mtime(path, when);
        }
        walk(path, std::time::SystemTime::now() - ago);
    }

    const SIX_HOURS: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

    /// KT-1318 — "no live worktree" is not enough on its own: a lane written
    /// within the idle threshold stays, an aged one goes.
    #[test]
    fn an_orphan_lane_is_swept_only_once_idle_past_the_threshold() {
        let repo = make_test_repo("lane-idle");
        let (worktree, lane_target, _) = marked_lane(repo.path(), "KT-30", "c3d4", "exec-30");
        fs::remove_dir_all(&worktree).unwrap();

        let fresh = sweep_lane_targets_idle(
            repo.path(),
            &finished,
            LaneSweepScope::WorktreeGone,
            SIX_HOURS,
        );
        assert!(fresh.removed.is_empty(), "{fresh:?}");
        assert!(lane_target.join("debug/kronn").exists());
        assert!(
            fresh
                .kept
                .iter()
                .any(|(path, why)| path == &lane_target && why.contains("idle threshold")),
            "{fresh:?}"
        );

        age_tree(&lane_target, SIX_HOURS + std::time::Duration::from_secs(60));
        let aged = sweep_lane_targets_idle(
            repo.path(),
            &finished,
            LaneSweepScope::WorktreeGone,
            SIX_HOURS,
        );
        assert_eq!(aged.removed.len(), 1, "{aged:?}");
        assert!(!lane_target.exists());
    }

    /// KT-1318 — age never overrides liveness: an aged lane of a live
    /// execution, or whose worktree still exists, is not an orphan.
    #[test]
    fn an_aged_lane_is_still_kept_when_its_execution_or_worktree_is_alive() {
        let repo = make_test_repo("lane-idle-live");
        let (worktree, lane_target, _) = marked_lane(repo.path(), "KT-31", "e5f6", "exec-31");
        age_tree(&lane_target, SIX_HOURS * 4);

        let live =
            sweep_lane_targets_idle(repo.path(), &alive, LaneSweepScope::WorktreeGone, SIX_HOURS);
        assert!(
            live.removed.is_empty() && live.claimed.len() == 1,
            "{live:?}"
        );

        // Finished, but its clean worktree is still there: not an orphan.
        let attached = sweep_lane_targets_idle(
            repo.path(),
            &finished,
            LaneSweepScope::WorktreeGone,
            SIX_HOURS,
        );
        assert!(attached.removed.is_empty(), "{attached:?}");
        assert!(worktree.exists() && lane_target.join("debug/kronn").exists());
    }

    /// KT-1318 — a build that just ran inside the lane moves its idle age
    /// forward even though the lane directory itself did not change.
    #[test]
    fn a_write_below_the_lane_resets_its_idle_age() {
        let repo = make_test_repo("lane-idle-age");
        let (_, lane_target, _) = marked_lane(repo.path(), "KT-32", "a7b8", "exec-32");
        age_tree(&lane_target, SIX_HOURS * 2);
        assert!(
            lane_target_idle_for(&lane_target).unwrap()
                >= SIX_HOURS * 2 - std::time::Duration::from_secs(5)
        );

        fs::write(lane_target.join("debug/fresh.o"), b"x").unwrap();
        assert!(
            lane_target_idle_for(&lane_target).unwrap() < std::time::Duration::from_secs(120),
            "a write under debug/ is activity"
        );
        // A lane that does not exist has no age: never "idle".
        assert!(lane_target_idle_for(&repo.path().join("target/wt-none")).is_none());
    }

    #[test]
    fn mark_lane_target_writes_the_marker_the_sweep_reads() {
        let repo = make_test_repo("lane-mark");
        let (worktree, lane_target, _) = marked_lane(repo.path(), "KT-20", "a1b2", "exec-20");

        let owners = lane_target_owners(repo.path());
        assert_eq!(owners.len(), 1, "{owners:?}");
        assert_eq!(owners[0].execution_id, "exec-20");
        assert_eq!(owners[0].worktree, worktree.to_string_lossy());
        assert!(lane_target.join(LANE_OWNER_MARKER).is_file());
        // Idempotent: a second launch of the same lane finds it stamped and
        // leaves the file (and the lane's idle clock) as it is.
        let stamped = fs::metadata(lane_target.join(LANE_OWNER_MARKER))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        mark_lane_target(repo.path(), &worktree, "exec-20").unwrap();
        assert_eq!(lane_target_owners(repo.path()).len(), 1);
        assert_eq!(
            fs::metadata(lane_target.join(LANE_OWNER_MARKER))
                .unwrap()
                .modified()
                .unwrap(),
            stamped
        );
        // Nothing can be marked without an execution.
        assert!(mark_lane_target(repo.path(), &worktree, " ").is_err());
    }

    /// KT-1318 - the lanes Kronn starts a build in without having provisioned
    /// them itself (a validation of any worker kind, a resumed launch) are
    /// stamped from the worktree alone; a tree outside the managed layout has
    /// no lane and gets nothing.
    #[test]
    fn a_lane_is_marked_from_its_worktree_alone_and_only_for_a_managed_one() {
        let repo = make_test_repo("lane-mark-derived");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-21", "c3d4", &base).unwrap();
        let worktree = PathBuf::from(&info.path);

        let lane_target = mark_lane_target_for_worktree(&worktree, "exec-21").unwrap();
        assert_eq!(
            lane_target.canonicalize().unwrap(),
            repo.path()
                .canonicalize()
                .unwrap()
                .join("target/wt-kt-21-c3d4")
        );
        let owners = lane_target_owners(&repo.path().canonicalize().unwrap());
        assert_eq!(owners.len(), 1, "{owners:?}");
        assert_eq!(owners[0].execution_id, "exec-21");
        // The stamp is Kronn's: a checkout that does not ignore `target/` must
        // not read it as uncommitted work (it would block an integration).
        assert!(
            worktree_dirty_files(repo.path()).unwrap().is_empty(),
            "{:?}",
            worktree_dirty_files(repo.path())
        );

        let elsewhere = tempfile::tempdir().unwrap();
        assert!(mark_lane_target_for_worktree(elsewhere.path(), "exec-21").is_err());
        assert!(
            !elsewhere.path().join("target").exists(),
            "an unmanaged tree is left as it is"
        );
        assert!(mark_lane_target_for_worktree(&worktree, "").is_err());
    }

    /// KT-1256 — the principal's review: `target/wt-*` is also where humans
    /// and other tooling build (`wt-0151-integration`, `wt-0150-*`). Nothing
    /// Kronn did not mark is touched, whatever the scope.
    #[test]
    fn an_unmarked_wt_directory_survives_every_sweep() {
        let repo = make_test_repo("lane-unmarked");
        let human = repo.path().join("target/wt-0151-integration");
        fs::create_dir_all(human.join("debug")).unwrap();
        fs::write(human.join("debug/kronn"), b"hand built").unwrap();
        // A marker that does not belong: lane disagrees with the directory,
        // a file that is not JSON, and a marker that is a link.
        let mismatched = repo.path().join("target/wt-0150-a");
        fs::create_dir_all(&mismatched).unwrap();
        fs::write(
            mismatched.join(LANE_OWNER_MARKER),
            r#"{"version":1,"lane":"another","execution_id":"e","worktree":"/x"}"#,
        )
        .unwrap();
        let garbage = repo.path().join("target/wt-0150-b");
        fs::create_dir_all(&garbage).unwrap();
        fs::write(garbage.join(LANE_OWNER_MARKER), "not json").unwrap();
        let stray_file = repo.path().join("target/wt-note.txt");
        fs::write(&stray_file, "not a directory").unwrap();
        #[cfg(unix)]
        {
            let linked = repo.path().join("target/wt-0150-c");
            fs::create_dir_all(&linked).unwrap();
            let elsewhere = repo.path().join("elsewhere.json");
            fs::write(
                &elsewhere,
                r#"{"version":1,"lane":"0150-c","execution_id":"e","worktree":"/x"}"#,
            )
            .unwrap();
            std::os::unix::fs::symlink(&elsewhere, linked.join(LANE_OWNER_MARKER)).unwrap();
        }

        for scope in [
            LaneSweepScope::ReportOnly,
            LaneSweepScope::WorktreeGone,
            LaneSweepScope::WorktreeGoneOrClean,
        ] {
            let sweep = sweep_lane_targets(repo.path(), &finished, scope);
            assert!(sweep.removed.is_empty(), "{scope:?}: {sweep:?}");
            assert!(sweep.would_remove.is_empty(), "{scope:?}: {sweep:?}");
            assert!(sweep.unmarked.contains(&human), "{scope:?}: {sweep:?}");
            assert!(sweep.unmarked.contains(&mismatched));
            assert!(sweep.unmarked.contains(&garbage));
        }
        assert!(human.join("debug/kronn").exists());
        assert!(mismatched.exists() && garbage.exists());
        assert!(stray_file.exists());
        #[cfg(unix)]
        assert!(repo.path().join("target/wt-0150-c").exists());
    }

    #[test]
    fn a_marked_lane_of_a_live_execution_survives() {
        let repo = make_test_repo("lane-live");
        let (_, lane_target, _) = marked_lane(repo.path(), "KT-21", "c3d4", "exec-21");

        let sweep = sweep_lane_targets(repo.path(), &alive, LaneSweepScope::WorktreeGoneOrClean);

        assert!(lane_target.join("debug/kronn").exists());
        assert!(
            sweep.removed.is_empty() && sweep.would_remove.is_empty(),
            "{sweep:?}"
        );
        assert_eq!(
            sweep.claimed,
            vec![(lane_target, "execution is Working".to_string())]
        );
    }

    #[test]
    fn a_marked_lane_of_a_finished_execution_with_a_dirty_worktree_survives() {
        let repo = make_test_repo("lane-dirty");
        let (worktree, lane_target, _) = marked_lane(repo.path(), "KT-22", "e5f6", "exec-22");
        fs::write(worktree.join("wip.rs"), "fn wip() {}\n").unwrap();

        let sweep = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::WorktreeGoneOrClean);

        assert!(lane_target.join("debug/kronn").exists(), "{sweep:?}");
        assert!(worktree.join("wip.rs").exists());
        assert!(
            sweep.removed.is_empty() && sweep.would_remove.is_empty(),
            "{sweep:?}"
        );
        assert_eq!(sweep.kept.len(), 1, "{sweep:?}");
        assert!(sweep.kept[0].1.contains("uncommitted"), "{sweep:?}");
    }

    #[test]
    fn a_marked_lane_of_a_finished_execution_with_a_removed_worktree_is_removed() {
        let repo = make_test_repo("lane-gone");
        let base = head_sha(repo.path());
        let (worktree, lane_target, branch) = marked_lane(repo.path(), "KT-23", "0a1b", "exec-23");
        let neighbour = seed_lane_target(repo.path(), "0151-kt1122");
        let main_debug = repo.path().join("target/debug");
        fs::create_dir_all(&main_debug).unwrap();
        remove_task_worktree(repo.path(), &worktree.to_string_lossy(), &branch, &base).unwrap();
        assert!(!worktree.exists());

        // The dry run measures and deletes nothing.
        let dry = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::ReportOnly);
        assert_eq!(dry.would_remove.len(), 1, "{dry:?}");
        assert_eq!(dry.would_remove[0].0.execution_id, "exec-23");
        assert!(dry.would_remove[0].1.bytes_reclaimed >= 4096);
        assert!(lane_target.exists(), "a dry run deletes nothing");

        let sweep = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::WorktreeGone);
        assert!(!lane_target.exists(), "{sweep:?}");
        assert_eq!(sweep.removed.len(), 1, "{sweep:?}");
        assert!(sweep.bytes_removed() >= 4096);
        assert!(neighbour.exists(), "a lane without a marker is not Kronn's");
        assert_eq!(sweep.unmarked, vec![neighbour]);
        assert!(main_debug.exists(), "the main target is untouched");
    }

    #[test]
    fn a_clean_worktree_keeps_its_lane_unless_the_wider_scope_is_asked() {
        let repo = make_test_repo("lane-clean");
        let (worktree, lane_target, _) = marked_lane(repo.path(), "KT-24", "2c3d", "exec-24");

        let boot = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::WorktreeGone);
        assert!(
            lane_target.exists(),
            "boot scope never takes a present worktree's lane"
        );
        assert_eq!(boot.would_remove.len(), 1, "{boot:?}");

        let wider = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::WorktreeGoneOrClean);
        assert!(!lane_target.exists(), "{wider:?}");
        assert!(worktree.exists(), "the worktree itself is never touched");
    }

    #[test]
    fn a_marker_cannot_aim_the_check_at_another_checkout() {
        let repo = make_test_repo("lane-forged");
        let lane_target = repo.path().join("target/wt-forged");
        fs::create_dir_all(lane_target.join("debug")).unwrap();
        // Names a worktree that derives another lane, outside the managed root.
        fs::write(
            lane_target.join(LANE_OWNER_MARKER),
            serde_json::json!({
                "version": 1,
                "lane": "forged",
                "execution_id": "e",
                "worktree": repo.path().join(".kronn/worktrees/task-other"),
            })
            .to_string(),
        )
        .unwrap();

        let sweep = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::WorktreeGoneOrClean);

        assert!(lane_target.exists(), "{sweep:?}");
        assert!(sweep.removed.is_empty(), "{sweep:?}");
        assert!(sweep.kept[0].1.contains("does not derive"), "{sweep:?}");
    }

    #[test]
    fn the_sweep_of_a_repository_without_target_is_empty() {
        let repo = make_test_repo("lane-none");
        let sweep = sweep_lane_targets(repo.path(), &finished, LaneSweepScope::WorktreeGoneOrClean);
        assert_eq!(sweep, LaneTargetSweep::default());
    }

    #[test]
    fn remove_task_worktree_removes_owned_clean_checkout() {
        let repo = make_test_repo("task-rm");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-7", "c0de", &base).unwrap();
        assert!(Path::new(&info.path).exists());

        remove_task_worktree(repo.path(), &info.path, &info.branch, &base).unwrap();
        assert!(
            !Path::new(&info.path).exists(),
            "owned clean checkout removed"
        );
        assert!(
            branch_commit(repo.path(), &info.branch).is_none(),
            "branch removed with the worktree"
        );
    }

    #[test]
    fn remove_task_worktree_refuses_advanced_head() {
        let repo = make_test_repo("task-rm-adv");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-8", "f00d", &base).unwrap();
        // Work landed → compensation must NOT destroy it.
        commit_file(Path::new(&info.path), "w.txt", "z", "w");
        let res = remove_task_worktree(repo.path(), &info.path, &info.branch, &base);
        assert!(res.is_err(), "must refuse to destroy a checkout with work");
        assert!(
            Path::new(&info.path).exists(),
            "a worktree with committed work must survive compensation"
        );
    }

    #[test]
    fn remove_task_worktree_refuses_dirty_tree() {
        let repo = make_test_repo("task-rm-dirty");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-13", "d1d1", &base).unwrap();
        // Uncommitted change (HEAD still == base) → still refuse, keep resumable.
        fs::write(Path::new(&info.path).join("scratch.txt"), "wip").unwrap();
        let res = remove_task_worktree(repo.path(), &info.path, &info.branch, &base);
        assert!(res.is_err(), "must refuse a dirty tree");
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn remove_task_worktree_refuses_foreign_branch() {
        let repo = make_test_repo("task-rm-foreign");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-11", "1234", &base).unwrap();
        // Ownership proof fails: the branch we claim to own is not this checkout's.
        let res = remove_task_worktree(repo.path(), &info.path, "kronn/task/kt-11-9999", &base);
        assert!(
            res.is_err(),
            "must refuse when the owned branch does not match"
        );
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn remove_task_worktree_is_idempotent_when_already_gone() {
        let repo = make_test_repo("task-rm-idem");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-12", "abcd", &base).unwrap();
        remove_task_worktree(repo.path(), &info.path, &info.branch, &base).unwrap();
        // Second removal is a no-op success (retry-safe compensation), not a panic.
        remove_task_worktree(repo.path(), &info.path, &info.branch, &base).unwrap();
        assert!(branch_commit(repo.path(), &info.branch).is_none());
    }

    #[test]
    fn remove_cancelled_task_worktree_keeps_advanced_branch_for_inspection() {
        let repo = make_test_repo("task-rm-cancelled");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-322", "cancel", &base).unwrap();
        let worker_head = commit_file(Path::new(&info.path), "worker.txt", "keep", "worker work");
        let evidence = tempfile::tempdir().unwrap();

        remove_cancelled_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
            .unwrap();
        assert!(
            !Path::new(&info.path).exists(),
            "clean checkout was removed"
        );
        assert_eq!(
            branch_commit(repo.path(), &info.branch).as_deref(),
            Some(worker_head.as_str()),
            "the cancelled task branch remains inspectable"
        );
    }

    #[test]
    fn remove_cancelled_task_worktree_preserves_dirty_checkout() {
        let repo = make_test_repo("task-rm-cancelled-dirty");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-322", "dirty", &base).unwrap();
        fs::write(Path::new(&info.path).join("scratch.txt"), "uncommitted").unwrap();

        let evidence = tempfile::tempdir().unwrap();
        assert!(remove_cancelled_task_worktree(
            repo.path(),
            &info.path,
            &info.branch,
            evidence.path()
        )
        .is_err());
        assert!(Path::new(&info.path).exists());
    }

    /// KT-1397 — a terminal worktree is released even when dirty: the diff is
    /// saved first, the branch (with its commits) stays.
    #[test]
    fn release_terminal_task_worktree_saves_the_diff_and_keeps_the_branch() {
        let repo = make_test_repo("task-release-terminal");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "release", &base).unwrap();
        let landed = commit_file(Path::new(&info.path), "done.txt", "done", "worker work");
        fs::write(Path::new(&info.path).join("README.md"), "# edited").unwrap();
        fs::write(Path::new(&info.path).join("scratch.txt"), "untracked").unwrap();
        let evidence = tempfile::tempdir().unwrap();

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
                .unwrap();

        let TerminalWorktreeRelease::Removed {
            patch: Some(patch), ..
        } = released
        else {
            panic!("expected a removal with a saved patch, got {released:?}");
        };
        let saved = fs::read_to_string(patch).unwrap();
        assert!(saved.contains("+# edited") && saved.contains("scratch.txt"));
        assert!(
            !saved.contains("done.txt"),
            "committed work is the branch's"
        );
        assert!(!Path::new(&info.path).exists());
        assert_eq!(
            branch_commit(repo.path(), &info.branch).as_deref(),
            Some(landed.as_str())
        );
        assert!(!evidence.path().join("patch-index.tmp").exists());
        // Retry-safe once the checkout is gone.
        assert_eq!(
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
                .unwrap(),
            TerminalWorktreeRelease::AlreadyGone
        );
    }

    /// KT-1397 — ownership is proved before anything is removed: a checkout on
    /// another branch, or outside the managed layout, is refused untouched.
    #[test]
    fn release_terminal_task_worktree_refuses_foreign_checkouts() {
        let repo = make_test_repo("task-release-foreign");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "foreign", &base).unwrap();
        fs::write(Path::new(&info.path).join("scratch.txt"), "keep").unwrap();
        let evidence = tempfile::tempdir().unwrap();

        let wrong_branch =
            release_terminal_task_worktree(repo.path(), &info.path, "main", evidence.path());
        assert!(wrong_branch.is_err());
        assert!(Path::new(&info.path).join("scratch.txt").exists());

        let outside = tempfile::tempdir().unwrap();
        let outside_path = outside.path().to_string_lossy().to_string();
        assert!(release_terminal_task_worktree(
            repo.path(),
            &outside_path,
            &info.branch,
            evidence.path()
        )
        .is_err());
        assert!(outside.path().exists());
    }

    /// KT-1397 — the evidence is bounded: uncommitted files above the cap are
    /// neither saved nor is the checkout removed, and nothing is left staged.
    #[test]
    fn release_terminal_task_worktree_keeps_a_checkout_whose_diff_exceeds_the_cap() {
        let repo = make_test_repo("task-release-cap");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "cap", &base).unwrap();
        fs::write(Path::new(&info.path).join("build.bin"), vec![7u8; 4096]).unwrap();
        let evidence = tempfile::tempdir().unwrap();

        let error = release_terminal_task_worktree_capped(
            repo.path(),
            &info.path,
            &info.branch,
            evidence.path(),
            1024,
        )
        .unwrap_err();

        assert!(error.contains("evidence cap"), "{error}");
        assert!(Path::new(&info.path).join("build.bin").exists());
        assert!(!evidence.path().join(RELEASED_WORKTREE_PATCH).exists());
        // Under a cap that fits, the same checkout is released.
        let released = release_terminal_task_worktree_capped(
            repo.path(),
            &info.path,
            &info.branch,
            evidence.path(),
            1024 * 1024,
        )
        .unwrap();
        assert!(matches!(
            released,
            TerminalWorktreeRelease::Removed { patch: Some(_), .. }
        ));
        assert!(!Path::new(&info.path).exists());
    }

    /// A task worktree whose `.env` and `backend/bindings/` are ignored, with a link inside the
    /// folder to `outside`.
    fn task_with_ignored_files(name: &str) -> (tempfile::TempDir, WorktreeInfo, tempfile::TempDir) {
        let repo = make_test_repo(name);
        commit_ignore(repo.path(), ".env\nbindings/\n");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-2084", "ignored", &base).unwrap();
        let checkout = Path::new(&info.path);
        write_file(checkout, ".env", "TOKEN=1");
        write_file(
            checkout,
            "backend/bindings/Task.ts",
            "export type Task = {};",
        );
        write_file(
            checkout,
            "backend/bindings/deep/Run.ts",
            "export type Run = {};",
        );
        let (outside, _) = outside_file();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), checkout.join("backend/bindings/linked"))
            .unwrap();
        (repo, info, outside)
    }

    /// KT-2084 — a finished checkout's ignored files are copied into its evidence, then it
    /// is released; a link among them is not followed and a stale copy goes first.
    #[test]
    fn release_terminal_task_worktree_keeps_ignored_files_as_evidence() {
        let (repo, info, outside) = task_with_ignored_files("task-release-ignored");
        let evidence = tempfile::tempdir().unwrap();
        write_file(evidence.path(), "ignored/stale.txt", "earlier attempt");

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path());

        let copied = evidence.path().join(RELEASED_WORKTREE_IGNORED_DIR);
        assert_eq!(
            released,
            Ok(TerminalWorktreeRelease::Removed {
                patch: None,
                ignored_copied: Some(copied.clone()),
            })
        );
        assert!(!Path::new(&info.path).exists());
        assert_eq!(fs::read(copied.join(".env")).unwrap(), b"TOKEN=1");
        assert_eq!(
            fs::read_to_string(copied.join("backend/bindings/Task.ts")).unwrap(),
            "export type Task = {};"
        );
        assert_eq!(
            fs::read_to_string(copied.join("backend/bindings/deep/Run.ts")).unwrap(),
            "export type Run = {};"
        );
        assert!(fs::symlink_metadata(copied.join("backend/bindings/linked")).is_err());
        assert!(!copied.join("stale.txt").exists());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    /// KT-2084 — the ignored files count against the evidence cap with the patch: over it,
    /// the checkout stays and nothing is left in the evidence.
    #[test]
    fn release_terminal_task_worktree_keeps_a_checkout_whose_ignored_files_exceed_the_cap() {
        let (repo, info, _outside) = task_with_ignored_files("task-release-ignored-cap");
        let env = "x".repeat(1000);
        fs::write(Path::new(&info.path).join(".env"), &env).unwrap();
        // The ignored files (1042 bytes) and the patch (about 190) each fit; together they
        // do not.
        fs::write(Path::new(&info.path).join("README.md"), "# edited").unwrap();
        let evidence = tempfile::tempdir().unwrap();

        let error = release_terminal_task_worktree_capped(
            repo.path(),
            &info.path,
            &info.branch,
            evidence.path(),
            1100,
        )
        .unwrap_err();

        assert!(error.contains("evidence cap"), "{error}");
        assert_eq!(
            fs::read_to_string(Path::new(&info.path).join(".env")).unwrap(),
            env
        );
        assert!(!evidence.path().join("ignored").exists());
        assert!(!evidence.path().join(RELEASED_WORKTREE_PATCH).exists());
    }

    /// KT-2084 — a task submodule is cloned from the project's own submodule repository: a
    /// pin that repository holds is saved, even when none of its branches reaches it. A
    /// commit only the task's clone holds still keeps the checkout.
    #[test]
    fn a_task_submodule_pinned_from_the_project_checkout_is_saved_there() {
        let repo = make_test_repo("task-release-sub-pin");
        let upstream = add_materialized_test_submodule(repo.path());
        let pinned = commit_file(upstream.path(), "bump.txt", "v2", "upstream bump");
        let source = repo.path().join("deps/required");
        git(&source, &["fetch", "-q", "origin"]);
        git(&source, &["checkout", "-q", "--detach", &pinned]);
        git(repo.path(), &["commit", "-qam", "bump the submodule"]);
        let base = head_sha(repo.path());
        let saved = create_task_worktree(repo.path(), "KT-2084", "sub-saved", &base).unwrap();
        let own = create_task_worktree(repo.path(), "KT-2084", "sub-own", &base).unwrap();
        let own_commit = commit_file(
            &Path::new(&own.path).join("deps/required"),
            "local.txt",
            "only here",
            "task-only commit",
        );
        let evidence = tempfile::tempdir().unwrap();

        let released = release_terminal_task_worktree(
            repo.path(),
            &saved.path,
            &saved.branch,
            evidence.path(),
        );
        let kept =
            release_terminal_task_worktree(repo.path(), &own.path, &own.branch, evidence.path());

        assert!(
            matches!(released, Ok(TerminalWorktreeRelease::Removed { .. })),
            "{released:?}"
        );
        assert!(!Path::new(&saved.path).exists());
        assert!(
            kept.as_ref()
                .is_err_and(|error| error.contains("deps/required") && error.contains(&own_commit)),
            "{kept:?}"
        );
        assert!(Path::new(&own.path)
            .join("deps/required/local.txt")
            .exists());
    }

    /// KT-1397 — the patch itself is streamed under the cap: a tracked edit
    /// whose diff outgrows it (the size pre-check passed) leaves no partial file.
    #[test]
    fn write_uncommitted_patches_stop_at_the_cap_and_leave_nothing() {
        let repo = make_test_repo("task-patch-cap");
        let info = create_discussion_worktree(repo.path(), "proj", "patch-cap", "main").unwrap();
        let wt = Path::new(&info.path);
        fs::write(wt.join("big.txt"), "x".repeat(8192)).unwrap();
        let evidence = tempfile::tempdir().unwrap();
        let judged = Judgement::of(wt).unwrap();

        let error = write_uncommitted_patches(&judged, evidence.path(), 1024).unwrap_err();

        assert!(error.contains("cap"), "{error}");
        let left: Vec<_> = fs::read_dir(evidence.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        // The scratch index lives in Kronn's copy of the git state, not in the evidence.
        assert!(left.is_empty(), "nothing remains: {left:?}");
    }

    /// Git state of a checkout as a comparable value: the index (as a patch
    /// against HEAD), the status, and the content of every file.
    fn checkout_fingerprint(wt: &Path) -> (String, String, Vec<(String, Vec<u8>)>) {
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(wt)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}");
            String::from_utf8_lossy(&output.stdout).to_string()
        };
        let mut files: Vec<(String, Vec<u8>)> = git(&["ls-files", "-z", "--cached", "--others"])
            .split('\0')
            .filter(|name| !name.is_empty())
            .filter_map(|name| {
                fs::read(wt.join(name))
                    .ok()
                    .map(|bytes| (name.to_string(), bytes))
            })
            .collect();
        files.sort();
        (
            git(&["diff", "--cached", "--binary", "--no-ext-diff"]),
            git(&["status", "--porcelain", "-uall"]),
            files,
        )
    }

    /// KT-1397 — staged state is evidence too: a modification kept only in the
    /// index (the file on disk restored), another staged then edited again, a
    /// staged deletion and an untracked file all survive the release, and the
    /// two saved patches rebuild the exact index and working tree on the kept
    /// branch.
    #[test]
    fn release_terminal_task_worktree_saves_the_staged_state_and_both_patches_reapply() {
        let repo = make_test_repo("task-release-staged");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "staged", &base).unwrap();
        let wt = Path::new(&info.path);
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(wt)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}");
        };
        commit_file(wt, "twice.txt", "v0\n", "tracked");
        commit_file(wt, "gone.txt", "bye\n", "tracked");
        let readme = fs::read(wt.join("README.md")).unwrap();
        // Staged only: the file on disk is back at HEAD.
        fs::write(wt.join("README.md"), "STAGED_WORK_TO_PRESERVE\n").unwrap();
        git(&["add", "README.md"]);
        fs::write(wt.join("README.md"), readme).unwrap();
        // Staged, then edited again.
        fs::write(wt.join("twice.txt"), "v1 staged\n").unwrap();
        git(&["add", "twice.txt"]);
        fs::write(wt.join("twice.txt"), "v2 on disk\n").unwrap();
        // Staged deletion, staged binary add, untracked file.
        git(&["rm", "-q", "gone.txt"]);
        fs::write(
            wt.join("blob.bin"),
            (0..2048).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        git(&["add", "blob.bin"]);
        fs::write(wt.join("loose.txt"), "untracked\n").unwrap();
        let before = checkout_fingerprint(wt);
        let evidence = tempfile::tempdir().unwrap();

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
                .unwrap();

        assert!(matches!(
            released,
            TerminalWorktreeRelease::Removed { patch: Some(_), .. }
        ));
        assert!(!wt.exists());
        let staged = evidence.path().join(RELEASED_WORKTREE_STAGED_PATCH);
        let working = evidence.path().join(RELEASED_WORKTREE_PATCH);
        assert!(fs::read_to_string(&staged)
            .unwrap()
            .contains("STAGED_WORK_TO_PRESERVE"));
        assert!(!evidence.path().join("patch-index.tmp").exists());

        // Rebuild on the kept branch.
        let rebuilt = repo.path().join("rebuilt");
        let output = std::process::Command::new("git")
            .args(["worktree", "add"])
            .arg(&rebuilt)
            .arg(&info.branch)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for (args, patch) in [
            (["apply", "--index"], &staged),
            (["apply", "--whitespace=nowarn"], &working),
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .arg(patch)
                .current_dir(&rebuilt)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(checkout_fingerprint(&rebuilt), before);
    }

    /// KT-1397 — nothing staged: a staged patch is not written, the working
    /// patch is the whole uncommitted diff (the pre-existing evidence shape).
    #[test]
    fn release_terminal_task_worktree_writes_no_staged_patch_when_nothing_is_staged() {
        let repo = make_test_repo("task-release-unstaged");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "unstaged", &base).unwrap();
        fs::write(Path::new(&info.path).join("loose.txt"), "x").unwrap();
        let evidence = tempfile::tempdir().unwrap();

        release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
            .unwrap();

        assert!(evidence.path().join(RELEASED_WORKTREE_PATCH).exists());
        assert!(!evidence
            .path()
            .join(RELEASED_WORKTREE_STAGED_PATCH)
            .exists());
    }

    /// KT-1397 — the cap covers both patches together, and a refusal leaves
    /// neither behind (half the evidence is worse than none).
    #[test]
    fn release_terminal_task_worktree_keeps_a_checkout_whose_staged_state_exceeds_the_cap() {
        let repo = make_test_repo("task-release-staged-cap");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "stagedcap", &base).unwrap();
        let wt = Path::new(&info.path);
        fs::write(wt.join("staged.bin"), vec![9u8; 8192]).unwrap();
        let output = std::process::Command::new("git")
            .args(["add", "staged.bin"])
            .current_dir(wt)
            .output()
            .unwrap();
        assert!(output.status.success());
        // The worktree matches the index: the size pre-check sees nothing.
        let evidence = tempfile::tempdir().unwrap();

        let error = release_terminal_task_worktree_capped(
            repo.path(),
            &info.path,
            &info.branch,
            evidence.path(),
            1024,
        )
        .unwrap_err();

        assert!(error.contains("cap"), "{error}");
        assert!(wt.join("staged.bin").exists());
        assert!(!evidence.path().join(RELEASED_WORKTREE_PATCH).exists());
        assert!(!evidence
            .path()
            .join(RELEASED_WORKTREE_STAGED_PATCH)
            .exists());
    }

    /// KT-1397 — an index that cannot be written as a tree (a merge in
    /// progress) is not representable by a patch: the checkout is kept.
    #[test]
    fn release_terminal_task_worktree_keeps_a_checkout_with_unmerged_paths() {
        let repo = make_test_repo("task-release-unmerged");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-1397", "unmerged", &base).unwrap();
        let wt = Path::new(&info.path);
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(wt)
                .output()
                .unwrap()
        };
        git(&["checkout", "-q", "-b", "side"]);
        commit_file(wt, "README.md", "side\n", "side");
        git(&["checkout", "-q", &info.branch]);
        commit_file(wt, "README.md", "main\n", "main");
        let merged = git(&["merge", "side"]);
        assert!(!merged.status.success(), "the merge must conflict");
        let evidence = tempfile::tempdir().unwrap();

        let error =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
                .unwrap_err();

        assert!(error.contains("preserving it"), "{error}");
        assert!(wt.exists());
        assert!(!evidence.path().join(RELEASED_WORKTREE_PATCH).exists());
    }

    /// KT-1397 — the activity probe: a fresh file, a fresh directory entry or an
    /// unreadable tree is not idle; an old tree is; `.git` and a missing
    /// checkout do not count.
    #[test]
    fn checkout_written_since_sees_recent_writes_and_ignores_the_git_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::write(root.join("src/deep/lib.rs"), "fn main() {}").unwrap();
        fs::write(root.join(".git"), "gitdir: elsewhere").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600);
        let age = |path: &Path| set_mtime(path, old);
        // Deepest first: a child write bumps its parent's mtime.
        age(&root.join("src/deep/lib.rs"));
        age(&root.join("src/deep"));
        age(&root.join("src"));
        age(&root.join(".git"));
        age(root);
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);

        assert_eq!(checkout_written_since(root, hour_ago).unwrap(), None);

        // The Git admin entry is not the worker's activity.
        fs::write(root.join(".git"), "gitdir: rewritten").unwrap();
        age(root);
        assert_eq!(checkout_written_since(root, hour_ago).unwrap(), None);

        // A deep write is found.
        fs::write(root.join("src/deep/lib.rs"), "fn main() { 1; }").unwrap();
        let found = checkout_written_since(root, hour_ago).unwrap();
        assert!(found.is_some(), "a fresh deep write must be seen");

        // A directory entry added or removed moves the directory's mtime.
        fs::write(root.join("src/deep/lib.rs"), "fn main() {}").unwrap();
        age(&root.join("src/deep/lib.rs"));
        age(&root.join("src/deep"));
        age(&root.join("src"));
        age(root);
        assert_eq!(checkout_written_since(root, hour_ago).unwrap(), None);
        fs::write(root.join("src/new.txt"), "x").unwrap();
        fs::remove_file(root.join("src/new.txt")).unwrap();
        assert!(checkout_written_since(root, hour_ago).unwrap().is_some());

        assert!(checkout_written_since(&root.join("absent"), hour_ago)
            .unwrap()
            .is_none());
    }

    /// KT-2084 — the newest write of the whole tree, not the first one found:
    /// the delete's "retry after" is when the window really ends.
    #[test]
    fn checkout_newest_write_since_finds_the_latest_write_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/old.txt"), "o").unwrap();
        fs::write(root.join("new.txt"), "n").unwrap();
        let now = std::time::SystemTime::now();
        let at = |secs: u64| now - std::time::Duration::from_secs(secs);
        set_mtime(&root.join("a/b/old.txt"), at(120));
        set_mtime(&root.join("new.txt"), at(30));
        for path in [root.join("a/b"), root.join("a"), root.to_path_buf()] {
            set_mtime(&path, at(600));
        }

        let newest = checkout_newest_write_since(root, at(300)).unwrap().unwrap();
        let drift = newest
            .duration_since(at(30))
            .unwrap_or_else(|e| e.duration());
        assert!(drift < std::time::Duration::from_secs(1), "{newest:?}");
        assert_eq!(checkout_newest_write_since(root, at(10)).unwrap(), None);
    }

    /// KT-514/KT-478 — the reclaim must succeed on a task worktree that carries a
    /// materialized submodule, which `git worktree remove` refuses without
    /// `--force`. An abandoned generation's checkout goes; its branch and the
    /// worker's commits stay inspectable.
    #[test]
    fn remove_cancelled_task_worktree_reclaims_a_submodule_checkout() {
        let (repo, _upstream) = make_test_repo_with_submodule("task-rm-submodule");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-514", "submod", &base).unwrap();
        // The materialized submodule is exactly what breaks a plain remove.
        assert!(
            Path::new(&info.path).join("sub").join("lib.txt").exists(),
            "submodule must be materialized in the task worktree"
        );
        let worker_head = commit_file(Path::new(&info.path), "worker.txt", "keep", "worker work");
        let evidence = tempfile::tempdir().unwrap();

        remove_cancelled_task_worktree(repo.path(), &info.path, &info.branch, evidence.path())
            .unwrap();

        assert!(
            !Path::new(&info.path).exists(),
            "the checkout (submodule and all) is reclaimed"
        );
        assert_eq!(
            branch_commit(repo.path(), &info.branch).as_deref(),
            Some(worker_head.as_str()),
            "the branch and the worker's commits survive the reclaim"
        );
        // The admin entry is pruned too, so the worktree count actually drops.
        assert!(
            !repo
                .path()
                .join(".git/worktrees")
                .join(Path::new(&info.path).file_name().unwrap())
                .exists(),
            "the .git/worktrees admin entry must be pruned"
        );
    }

    /// The successful-integration teardown (a `Done` execution) must likewise
    /// clear a submodule-bearing checkout instead of leaving it to accumulate.
    #[test]
    fn remove_integrated_task_worktree_reclaims_a_submodule_checkout() {
        let (repo, _upstream) = make_test_repo_with_submodule("task-integrated-submodule");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-514", "intsub", &base).unwrap();
        let integrated = commit_file(Path::new(&info.path), "worker.txt", "done", "worker work");

        remove_integrated_task_worktree(repo.path(), &info.path, &info.branch, &integrated)
            .unwrap();
        assert!(
            !Path::new(&info.path).exists(),
            "an integrated submodule worktree is reclaimed"
        );
        assert!(
            branch_commit(repo.path(), &info.branch).is_none(),
            "an integrated branch is deleted with its checkout"
        );
    }

    /// Observability (KT-514): removing an already-gone checkout is a success,
    /// and it prunes so the count reflects reality rather than a phantom entry.
    #[test]
    fn remove_discussion_worktree_is_ok_when_the_checkout_is_already_gone() {
        let (repo, _upstream) = make_test_repo_with_submodule("task-rm-idem-submodule");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-514", "idem", &base).unwrap();
        // Simulate a partial prior cleanup: the directory is gone, the admin
        // entry may linger.
        std::fs::remove_dir_all(&info.path).unwrap();
        remove_discussion_worktree(repo.path(), &info.path, None).unwrap();
        assert!(!Path::new(&info.path).exists());
    }

    #[test]
    fn remove_integrated_task_worktree_accepts_only_the_exact_landed_head() {
        let repo = make_test_repo("task-rm-integrated");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-320", "landed", &base).unwrap();
        let integrated = commit_file(Path::new(&info.path), "worker.txt", "done", "worker work");

        remove_integrated_task_worktree(repo.path(), &info.path, &info.branch, &integrated)
            .unwrap();
        assert!(!Path::new(&info.path).exists());
        assert!(branch_commit(repo.path(), &info.branch).is_none());
        // Retry-safe after a crash between physical and DB cleanup.
        remove_integrated_task_worktree(repo.path(), &info.path, &info.branch, &integrated)
            .unwrap();
    }

    #[test]
    fn remove_integrated_task_worktree_preserves_a_divergent_checkout() {
        let repo = make_test_repo("task-rm-integrated-divergent");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-320", "diverged", &base).unwrap();
        let expected = commit_file(Path::new(&info.path), "worker.txt", "done", "integrated");
        let divergent = commit_file(Path::new(&info.path), "later.txt", "keep", "later work");
        assert_ne!(expected, divergent);

        let error =
            remove_integrated_task_worktree(repo.path(), &info.path, &info.branch, &expected)
                .expect_err("a post-integration divergence is evidence, not garbage");
        assert!(error.contains("diverged"), "unexpected refusal: {error}");
        assert!(Path::new(&info.path).exists());
        assert_eq!(
            branch_commit(repo.path(), &info.branch).as_deref(),
            Some(divergent.as_str())
        );
    }

    /// An unverified backup ref is the same as none: the saga would arm an apply
    /// believing it had a way back. Write it, then read it.
    #[test]
    fn a_backup_ref_is_read_back_before_it_is_trusted() {
        let repo = make_test_repo("backup");
        let sha = head_sha(repo.path());

        let full = write_backup_ref(repo.path(), "KT-320", &sha).unwrap();
        assert_eq!(full, "refs/kronn-backup/KT-320");
        assert_eq!(resolve_commit(repo.path(), &full).unwrap(), sha);

        for bad in ["", "-x", "a b", "../escape"] {
            assert!(
                write_backup_ref(repo.path(), bad, &sha).is_err(),
                "expected slug {bad:?} to be refused"
            );
        }
    }

    /// A task branch checked out in a linked worktree of `repo`, as Kronn runs
    /// them (the main checkout is never switched), with one commit of its own.
    fn child_worktree(repo: &Path, name: &str) -> PathBuf {
        let child = repo.join(".kronn/worktrees").join(name);
        let made = crate::core::cmd::git_cmd()
            .args(["worktree", "add", "-q", "-b", "child"])
            .arg(&child)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        commit_file(&child, "child.txt", "work", "child work");
        child
    }

    fn head_message(dir: &Path, rev: &str) -> String {
        let message = crate::core::cmd::git_cmd()
            .args(["log", "-1", "--format=%B", rev])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&message.stdout).into_owned()
    }

    /// Phase 1 brings the parent INTO the child, so the child ends up descending
    /// from the pinned tip — the precondition that lets phase 2 be a fast-forward.
    /// The merge commit is signed only where the project signs off (KT-1750).
    #[test]
    fn building_the_candidate_makes_the_child_descend_from_the_parent() {
        for signoff in [true, false] {
            let repo = make_test_repo("candidate");
            commit_file(repo.path(), "shared.txt", "one", "base");
            let setting = if signoff { "true" } else { "false" };
            let configured = crate::core::cmd::git_cmd()
                .args(["config", "kronn.signoff", setting])
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(configured.status.success());
            let child = child_worktree(repo.path(), "candidate");
            // Meanwhile the parent moves on, untouched by the child.
            let parent_tip = commit_file(repo.path(), "parent.txt", "moved", "parent moves");

            let outcome = build_candidate(&child, &parent_tip).unwrap();
            let CandidateOutcome::Built { sha } = outcome else {
                panic!("expected a clean candidate");
            };

            // The candidate contains the parent tip, so the parent can fast-forward to it.
            let merge_base = crate::core::cmd::git_cmd()
                .args(["merge-base", "--is-ancestor", &parent_tip, &sha])
                .current_dir(&child)
                .output()
                .unwrap();
            assert!(
                merge_base.status.success(),
                "candidate must descend from the parent tip"
            );

            let message = head_message(&child, &sha);
            assert_eq!(
                message.contains("Signed-off-by: "),
                signoff,
                "the merge commit follows the project's sign-off ({setting}): {message}"
            );
        }
    }

    /// A repository that enforces the DCO rejects an unsigned merge commit at
    /// `commit-msg` time (KT-854: the merge Kronn made for KT-840 had no
    /// `Signed-off-by`, so the release PR went red). The hook is how the
    /// repository says so (KT-1750): the integration candidate is signed, with the
    /// identity the repository commits as, and built.
    #[cfg(unix)] // asserts POSIX mode bits, which Windows does not have; runs a POSIX shell script fixture
    #[test]
    fn the_integration_merge_commit_satisfies_a_dco_enforcing_repository() {
        use std::os::unix::fs::PermissionsExt;

        let repo = make_test_repo("candidate-dco");
        commit_file(repo.path(), "shared.txt", "one", "base");
        let child = child_worktree(repo.path(), "candidate-dco");
        let parent_tip = commit_file(repo.path(), "parent.txt", "moved", "parent moves");

        // `git merge` runs `commit-msg` for the merge commit it creates.
        let hook = repo.path().join(".git/hooks/commit-msg");
        fs::write(
            &hook,
            "#!/bin/sh\ngrep -q '^Signed-off-by: ' \"$1\" || { echo 'DCO: missing Signed-off-by' >&2; exit 1; }\n",
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let outcome = build_candidate(&child, &parent_tip).unwrap();
        let CandidateOutcome::Built { sha } = outcome else {
            panic!("a DCO-enforcing repository must accept the integration merge, got {outcome:?}");
        };

        let message = head_message(&child, &sha);
        assert!(
            message.contains("Signed-off-by: Test <test@test.com>"),
            "the sign-off is the repository's identity: {message}"
        );
        let parents = crate::core::cmd::git_cmd()
            .args(["log", "-1", "--format=%P", &sha])
            .current_dir(&child)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&parents.stdout)
                .split_whitespace()
                .count(),
            2,
            "the candidate is a real merge commit"
        );
    }

    /// A conflict must leave the worker its worktree, not a half-merged tree it
    /// never asked for.
    #[test]
    fn a_conflicting_candidate_aborts_and_names_the_files() {
        let repo = make_test_repo("conflict");
        commit_file(repo.path(), "shared.txt", "one", "base");

        std::process::Command::new("git")
            .args(["checkout", "-b", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        commit_file(repo.path(), "shared.txt", "child version", "child edit");

        std::process::Command::new("git")
            .args(["checkout", "main"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let parent_tip = commit_file(repo.path(), "shared.txt", "parent version", "parent edit");

        std::process::Command::new("git")
            .args(["checkout", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let before = head_sha(repo.path());
        let outcome = build_candidate(repo.path(), &parent_tip).unwrap();

        let CandidateOutcome::Conflict { files } = outcome else {
            panic!("expected a conflict");
        };
        assert!(files.iter().any(|f| f == "shared.txt"), "got {files:?}");
        assert_eq!(
            head_sha(repo.path()),
            before,
            "the child branch must not have moved"
        );
        assert!(
            !repo.path().join(".git/MERGE_HEAD").exists(),
            "the merge must be aborted, not left in progress"
        );
    }

    /// `--ff-only` is the guarantee: a candidate built on a tip the parent has since
    /// left is refused, never forced over the work that landed meanwhile.
    #[test]
    fn a_stale_candidate_cannot_be_forced_over_newer_parent_work() {
        let repo = make_test_repo("ffonly");
        commit_file(repo.path(), "shared.txt", "one", "base");

        std::process::Command::new("git")
            .args(["checkout", "-b", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let candidate = commit_file(repo.path(), "child.txt", "work", "child work");

        std::process::Command::new("git")
            .args(["checkout", "main"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        // The parent moves AFTER the candidate was built: the candidate is stale.
        let landed = commit_file(repo.path(), "other.txt", "landed", "someone else landed");

        assert!(
            fast_forward_to(repo.path(), &candidate).is_err(),
            "a stale candidate must be refused"
        );
        assert_eq!(
            head_sha(repo.path()),
            landed,
            "the parent must not have moved"
        );
    }

    /// `base_rev` reaches git straight from the API. A leading `-` is read as a
    /// flag, not as a commit — harmless on rev-parse, an action on a command that
    /// writes. The refusal belongs here, before any of them.
    #[test]
    fn a_revision_that_looks_like_an_option_is_refused() {
        for rev in [
            "--upload-pack=touch /tmp/x",
            "-C/etc",
            "--exec=rm",
            "  --force",
        ] {
            assert!(
                reject_option_like_rev(rev).is_err(),
                "expected {rev:?} to be refused"
            );
        }
        assert!(
            reject_option_like_rev("").is_err(),
            "an empty revision names nothing"
        );
        for rev in ["main", "HEAD~2", "feat/x-y", "1234567890abcdef"] {
            assert!(
                reject_option_like_rev(rev).is_ok(),
                "expected {rev:?} to pass"
            );
        }
    }

    // ── KT-2084 — Kronn removes only the worktrees and registrations it owns ─

    /// A path spelled under `from` that climbs to the root and ends at `to`.
    #[cfg(unix)]
    fn through_parents(from: &Path, to: &Path) -> String {
        format!(
            "{}/{}{}",
            from.display(),
            "../".repeat(from.components().count() - 1),
            to.display().to_string().trim_start_matches('/')
        )
    }

    /// The registration of a new discussion worktree `name` of `repo`, its `gitdir` file
    /// then saying `content`.
    fn registration_saying(repo: &Path, name: &str, content: &str) -> PathBuf {
        let info = create_discussion_worktree(repo, "proj", name, "main").unwrap();
        let admin = git_dir_of(Path::new(&info.path));
        fs::write(admin.join("gitdir"), content).unwrap();
        admin
    }

    fn a_canonical_worktree(repo: &Path, name: &str) -> PathBuf {
        let info = create_discussion_worktree(repo, "proj", name, "main").unwrap();
        Path::new(&info.path).canonicalize().unwrap()
    }

    fn listed_worktrees(repo: &Path) -> String {
        let output = std::process::Command::new("git")
            .args(["worktree", "list", "--porcelain", "-z"])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stale_stat(file: &Path) {
        fs::File::options()
            .append(true)
            .open(file)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
    }

    #[cfg(unix)] // spells a POSIX absolute path through `../` up to `/`
    #[test]
    fn the_boot_prune_never_removes_a_directory_named_through_a_parent() {
        let repo = make_test_repo("prune-through-parent");
        let (outside, kept) = outside_file();
        let victim = outside.path().canonicalize().unwrap();
        let kronn = repo.path().canonicalize().unwrap().join(".kronn");
        let gitdir = format!("{}/.git\n", through_parents(&kronn, &victim));
        let admin = registration_saying(repo.path(), "through-parent", &gitdir);

        prune_kronn_worktrees(repo.path()).unwrap();

        assert!(
            kept.exists(),
            "a directory outside the repository was removed"
        );
        assert!(
            admin.exists(),
            "a registration Kronn does not own was removed"
        );
    }

    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn the_boot_prune_never_removes_a_directory_named_through_a_link() {
        let repo = make_test_repo("prune-through-link");
        let (outside, _) = outside_file();
        write_file(outside.path(), "v/precious.txt", "outside");
        let admin = registration_saying(repo.path(), "through-link", "");
        let base = repo.path().canonicalize().unwrap().join(".kronn/worktrees");
        std::os::unix::fs::symlink(outside.path(), base.join("ln")).unwrap();
        fs::write(
            admin.join("gitdir"),
            format!("{}/ln/v/.git\n", base.display()),
        )
        .unwrap();

        prune_kronn_worktrees(repo.path()).unwrap();

        assert!(
            outside.path().join("v/precious.txt").exists(),
            "a directory reached through a link was removed"
        );
        assert!(
            admin.exists(),
            "a registration Kronn does not own was removed"
        );
    }

    #[cfg(unix)] // writes a POSIX absolute path into a registration's gitdir
    #[test]
    fn a_registration_spelling_several_entries_never_gets_another_worktree_removed() {
        let repo = make_test_repo("prune-several-entries");
        let other = a_canonical_worktree(repo.path(), "other");
        fs::write(other.join("notes.txt"), "work").unwrap();
        let other_admin = git_dir_of(&other);
        let base = repo.path().canonicalize().unwrap().join(".kronn/worktrees");
        let gitdir = format!(
            "{base}/gone\nworktree {}\nprunable\nworktree {base}/gone2/.git\n",
            other.display(),
            base = base.display()
        );
        registration_saying(repo.path(), "several-entries", &gitdir);

        prune_kronn_worktrees(repo.path()).unwrap();

        assert!(
            other.join("notes.txt").exists(),
            "another worktree was removed"
        );
        assert!(
            other_admin.exists(),
            "another worktree's registration was removed"
        );
    }

    #[cfg(unix)] // spells a POSIX absolute path through `../` up to `/`
    #[test]
    fn the_run_cleanup_never_removes_what_a_nested_registration_names_outside_the_run() {
        let repo = make_test_repo("nested-outside");
        let (outside, kept) = outside_file();
        let victim = outside.path().canonicalize().unwrap();
        let run = a_canonical_worktree(repo.path(), "run");
        let other = a_canonical_worktree(repo.path(), "other");
        fs::write(other.join("notes.txt"), "work").unwrap();
        let gitdir = format!("{}/.git\n", through_parents(&run, &victim));
        registration_saying(repo.path(), "nested-parent", &gitdir);
        let gitdir = format!(
            "{run}/gone\nworktree {run}/../{}\nprunable\nworktree {run}/gone2/.git\n",
            other.file_name().unwrap().to_string_lossy(),
            run = run.display()
        );
        registration_saying(repo.path(), "nested-several", &gitdir);

        let found = nested_worktrees(repo.path(), &run).unwrap();
        remove_nested_worktrees(repo.path(), &run).unwrap();

        assert!(kept.exists(), "a directory outside the run was removed");
        assert!(
            other.join("notes.txt").exists(),
            "another worktree was removed"
        );
        assert!(found.is_empty(), "{found:?}");
    }

    /// A registration naming the worktree through `s/..` is not one Kronn wrote: the delete
    /// keeps the worktree and that registration, whatever the path resolves to.
    #[cfg(unix)] // writes a POSIX absolute path into a registration's gitdir
    #[test]
    fn a_worktree_registered_through_a_parent_is_kept_with_its_registration() {
        let repo = make_test_repo("gitdir-through-parent");
        commit_ignore(repo.path(), "s\n");
        let info =
            create_discussion_worktree(repo.path(), "proj", "through-parent", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        fs::create_dir(checkout.join("s")).unwrap();
        let admin = git_dir_of(&checkout);
        fs::write(
            admin.join("gitdir"),
            format!("{}/s/../.git\n", checkout.display()),
        )
        .unwrap();

        let removed = remove_deleted(repo.path(), &info.path);

        assert!(removed.is_err(), "{removed:?}");
        assert!(admin.join("HEAD").exists(), "its registration was removed");
        assert!(checkout.join("README.md").exists() && checkout.join(".git").exists());
    }

    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn a_link_swapped_in_while_kronn_removes_a_worktree_is_never_followed() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("removal-swap");
        commit_ignore(repo.path(), "s\n");
        let info = create_discussion_worktree(repo.path(), "proj", "removal-swap", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let (outside, _) = outside_file();
        let victim = outside.path().canonicalize().unwrap().join("a");
        write_file(&victim, "b/keep.txt", "outside");
        write_file(&victim, "precious.txt", "outside");
        let s = checkout.join("s");
        fs::create_dir(&s).unwrap();
        let admin = git_dir_of(&checkout);
        let (gitdir, target) = (admin.join("gitdir"), victim.join("b"));
        let through_s = format!("{}/s/../.git\n", checkout.display());
        swap_probe::arm_hook(checkout.parent().unwrap(), 0, move |_| {
            fs::write(&gitdir, through_s).unwrap();
            fs::remove_dir(&s).unwrap();
            std::os::unix::fs::symlink(&target, &s).unwrap();
        });

        let removed = remove_deleted(repo.path(), &info.path);

        let fired = swap_probe::fired();
        assert!(
            victim.join("precious.txt").exists() && victim.join("b/keep.txt").exists(),
            "a directory outside the worktree was removed"
        );
        assert!(fired, "the swap must land during Kronn's removal");
        assert!(removed.is_ok(), "{removed:?}");
        assert!(!checkout.exists());
        assert!(!admin.exists());
    }

    /// Once Kronn judged the worktree, nothing runs in it: a filter its submodule's
    /// configuration names, and assigns, as the removal starts never runs.
    #[cfg(unix)] // the planted clean filter is a POSIX shell command line
    #[test]
    fn a_submodule_filter_never_runs_while_its_worktree_is_removed() {
        use crate::core::fs_guard::swap_probe;
        let (repo, checkout) = discussion_worktree_with_submodule("removal-sub-filter");
        let checkout = checkout.canonicalize().unwrap();
        let sub = checkout.join("sub");
        let module = git_dir_of(&sub);
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        let filter = format!("touch '{}'; cat", ran.display());
        swap_probe::arm_hook(checkout.parent().unwrap(), 0, move |_| {
            git(&sub, &["config", "filter.x.clean", &filter]);
            fs::create_dir_all(module.join("info")).unwrap();
            fs::write(module.join("info/attributes"), "* filter=x\n").unwrap();
            stale_stat(&sub.join("lib.txt"));
        });

        let _ = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            swap_probe::fired(),
            "the filter must appear as Kronn removes the worktree"
        );
        assert!(
            !ran.exists(),
            "a program of the submodule's configuration ran"
        );
    }

    // ── KT-2084 — an ownerless worktree is judged and removed as a discussion's ──

    /// A worktree of `.kronn/worktrees` spelled as Kronn spells it, which no row will name.
    fn ownerless_worktree(repo: &Path, name: &str) -> PathBuf {
        PathBuf::from(
            create_discussion_worktree(repo, "proj", name, "main")
                .unwrap()
                .path,
        )
    }

    /// An ownerless worktree whose registration names it through `s/..` or through a link is
    /// not one Kronn wrote: nothing outside it is removed, and it is kept with the reason.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn an_ownerless_worktree_registered_through_a_parent_or_a_link_is_kept_with_the_reason() {
        let repo = make_test_repo("ownerless-indirect");
        let (outside, kept) = outside_file();
        let through_parent = ownerless_worktree(repo.path(), "through-parent");
        fs::create_dir(through_parent.join("s")).unwrap();
        let parent_admin = git_dir_of(&through_parent);
        let gitdir = format!(
            "{}/s/../.git\n",
            through_parent.canonicalize().unwrap().display()
        );
        fs::write(parent_admin.join("gitdir"), gitdir).unwrap();
        let through_link = ownerless_worktree(repo.path(), "through-link");
        let link = outside.path().join("ln");
        std::os::unix::fs::symlink(through_link.canonicalize().unwrap(), &link).unwrap();
        let link_admin = git_dir_of(&through_link);
        fs::write(
            link_admin.join("gitdir"),
            format!("{}/.git\n", link.display()),
        )
        .unwrap();

        // A path outside the checkout is never resolved (KT-2084): the link there is not
        // followed, so that registration names no worktree of Kronn's.
        for (checkout, admin, why) in [
            (&through_parent, &parent_admin, "registered through"),
            (&through_link, &link_admin, "is not a worktree registered"),
        ] {
            let removed = remove_ownerless_worktree(repo.path(), checkout);

            let reason = removed.expect_err("kept: its registration is not one Kronn wrote");
            assert!(reason.contains(why), "{reason}");
            assert!(checkout.join("README.md").exists() && checkout.join(".git").exists());
            assert!(admin.join("HEAD").exists(), "its registration was removed");
        }
        assert!(kept.exists(), "a file outside the repository was removed");
        assert!(fs::symlink_metadata(&link).is_ok(), "the link was removed");
    }

    /// A filter the submodule's own configuration declares never runs while Kronn judges an
    /// ownerless worktree: one with a populated submodule is kept, as git kept it.
    #[cfg(unix)] // the planted clean filter is a POSIX shell command line
    #[test]
    fn a_submodule_filter_never_runs_while_an_ownerless_worktree_is_judged() {
        let (repo, checkout) = discussion_worktree_with_submodule("ownerless-sub-filter");
        let sub = checkout.join("sub");
        let module = git_dir_of(&sub);
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        let filter = format!("touch '{}'; cat", ran.display());
        git(&sub, &["config", "filter.x.clean", &filter]);
        fs::create_dir_all(module.join("info")).unwrap();
        fs::write(module.join("info/attributes"), "* filter=x\n").unwrap();
        stale_stat(&sub.join("lib.txt"));

        let removed = remove_ownerless_worktree(repo.path(), &checkout);

        assert!(
            !ran.exists(),
            "a program of the submodule's configuration ran"
        );
        let reason = removed.expect_err("kept: it has a populated submodule");
        assert!(reason.contains("submodule"), "{reason}");
        assert!(sub.join("lib.txt").exists());
    }

    /// A clean ownerless worktree goes with its registration: git no longer lists it.
    #[test]
    fn a_clean_ownerless_worktree_goes_with_its_registration() {
        let repo = make_test_repo("ownerless-clean");
        let checkout = ownerless_worktree(repo.path(), "gone-clean");
        let admin = git_dir_of(&checkout);

        remove_ownerless_worktree(repo.path(), &checkout).expect("a clean worktree goes");

        assert!(!checkout.exists() && !admin.exists());
        let listed = listed_worktrees(repo.path());
        assert!(!listed.contains("gone-clean"), "{listed}");
    }

    /// Git's own removal never kept a worktree for its ignored files: an ownerless worktree
    /// holding a `.env` and generated bindings goes with its registration.
    #[test]
    fn an_ownerless_worktree_holding_ignored_files_goes_with_its_registration() {
        let repo = make_test_repo("ownerless-ignored");
        commit_ignore(repo.path(), ".env\nbackend/bindings/\nnode_modules/\n");
        let checkout = ownerless_worktree(repo.path(), "gone-ignored");
        write_file(&checkout, "node_modules/x/index.js", "x");
        write_file(&checkout, ".env", "TOKEN=mine");
        write_file(
            &checkout,
            "backend/bindings/Task.ts",
            "export type Task = {};",
        );
        let admin = git_dir_of(&checkout);

        remove_ownerless_worktree(repo.path(), &checkout).expect("ignored files go with it");

        assert!(!checkout.exists() && !admin.exists());
        let listed = listed_worktrees(repo.path());
        assert!(!listed.contains("gone-ignored"), "{listed}");
    }

    /// Whether a registration names a worktree whose directory is named `name`.
    fn lists_worktree_named(repo: &Path, name: &std::ffi::OsStr) -> bool {
        worktree_entries(repo)
            .unwrap()
            .iter()
            .any(|entry| entry.path.file_name() == Some(name))
    }

    #[test]
    fn a_clean_delete_leaves_neither_the_checkout_nor_its_registration() {
        let repo = make_test_repo("clean-delete-registration");
        let info = create_discussion_worktree(repo.path(), "proj", "clean-reg", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let admin = git_dir_of(&checkout);
        let name = checkout.file_name().unwrap().to_os_string();
        assert!(lists_worktree_named(repo.path(), &name), "listed before");

        remove_deleted(repo.path(), &info.path).unwrap();

        assert!(!checkout.exists());
        assert!(!admin.exists());
        assert!(
            !lists_worktree_named(repo.path(), &name),
            "a registration still names it: {:?}",
            registered_worktree_paths(repo.path())
        );
    }

    /// KT-2084 — a stash in a submodule lives only in the worktree's copy of that submodule's
    /// repository: the delete keeps the worktree and names the submodule.
    #[test]
    fn a_stash_in_a_submodule_keeps_the_worktree_of_a_deleted_discussion() {
        let (repo, checkout) = discussion_worktree_with_submodule("deleted-disc-sub-stash");
        let sub = checkout.join("sub");
        fs::write(sub.join("lib.txt"), "stashed work").unwrap();
        git(
            &sub,
            &[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "stash",
                "-q",
            ],
        );
        assert_eq!(fs::read_to_string(sub.join("lib.txt")).unwrap(), "shared");

        let kept = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            kept.as_ref()
                .is_err_and(|reason| reason.contains("submodule sub")),
            "{kept:?}"
        );
        assert!(sub.join("lib.txt").exists());
    }

    /// KT-2084 — the ignored files of a task checkout's submodule are copied into its
    /// evidence, under the submodule's path, before its release.
    #[test]
    fn a_task_checkout_copies_its_submodule_ignored_files_before_its_release() {
        let repo = make_test_repo("task-release-sub-ignored");
        let _upstream = add_materialized_test_submodule(repo.path());
        let source = repo.path().join("deps/required");
        fs::write(source.join(".gitignore"), ".env\n").unwrap();
        let identity = ["-c", "user.email=t@t.com", "-c", "user.name=T"];
        git(&source, &["add", ".gitignore"]);
        git(
            &source,
            &[&identity[..], &["commit", "-qm", "ignore"]].concat(),
        );
        git(repo.path(), &["add", "deps/required"]);
        git(
            repo.path(),
            &[&identity[..], &["commit", "-qm", "bump the submodule"]].concat(),
        );
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-2084", "sub-ignored", &base).unwrap();
        let sub = Path::new(&info.path).join("deps/required");
        assert!(sub.join(".gitignore").is_file());
        write_file(&sub, ".env", "TOKEN=sub");
        let evidence = tempfile::tempdir().unwrap();

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path());

        assert!(
            matches!(released, Ok(TerminalWorktreeRelease::Removed { .. })),
            "{released:?}"
        );
        assert!(!Path::new(&info.path).exists());
        assert_eq!(
            fs::read(
                evidence
                    .path()
                    .join(RELEASED_WORKTREE_IGNORED_DIR)
                    .join("deps/required/.env")
            )
            .unwrap(),
            b"TOKEN=sub"
        );
    }

    /// KT-2084 — with `core.splitIndex` a worktree's index names a shared index beside it:
    /// Kronn's copy holds both, and a clean worktree is deleted.
    #[test]
    fn a_clean_worktree_with_a_split_index_is_deleted() {
        let repo = make_test_repo("split-index");
        let info = create_discussion_worktree(repo.path(), "proj", "split", "main").unwrap();
        let checkout = Path::new(&info.path);
        git(checkout, &["config", "core.splitIndex", "true"]);
        git(checkout, &["update-index", "--split-index"]);
        let admin = git_dir_of(checkout);
        let shared = fs::read_dir(&admin).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("sharedindex.")
        });
        assert!(shared, "git wrote no shared index in {}", admin.display());

        let removed = remove_deleted(repo.path(), &info.path);

        assert!(removed.is_ok(), "{removed:?}");
        assert!(!checkout.exists());
    }

    /// KT-2084 — Kronn cannot judge a repository whose refs are in the reftable format yet
    /// (KT-2101): it creates no worktree there, and keeps one git made in its layout; each
    /// refusal names the format and what to do.
    #[test]
    fn a_reftable_repository_names_its_format_where_kronn_keeps_a_worktree() {
        let dir = tempfile::Builder::new()
            .prefix("kronn-wt-reftable")
            .tempdir()
            .unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main", "--ref-format=reftable"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        if !init.status.success() {
            eprintln!(
                "skipped: this git cannot create a reftable repository ({})",
                String::from_utf8_lossy(&init.stderr).trim()
            );
            return;
        }
        let repo = dir.path();
        fs::write(repo.join("README.md"), "# test").unwrap();
        git(repo, &["add", "."]);
        git(
            repo,
            &[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "commit",
                "-qm",
                "init",
            ],
        );
        let base = head_sha(repo);
        let created = create_discussion_worktree(repo, "proj", "reftable", "main");
        let task_created = create_task_worktree(repo, "KT-2084", "reftable", &base);
        let discussion = worktree_base_dir(repo).join("proj--reftable");
        let (task_path, task_branch) = task_worktree_layout(repo, "KT-2084", "reftable").unwrap();
        assert!(!discussion.exists() && !task_path.exists());
        for (path, branch) in [
            (&discussion, "kronn/reftable"),
            (&task_path, task_branch.as_str()),
        ] {
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    path.to_str().unwrap(),
                    &base,
                ],
            );
        }
        let evidence = tempfile::tempdir().unwrap();

        let refused = unsaved_work(repo, &discussion, "wt");
        let kept = remove_deleted(repo, discussion.to_str().unwrap());
        let released = release_terminal_task_worktree(
            repo,
            task_path.to_str().unwrap(),
            &task_branch,
            evidence.path(),
        );

        for reason in [
            created.map(|_| String::new()).unwrap_err(),
            task_created.map(|_| String::new()).unwrap_err(),
            refused.map(|_| String::new()).unwrap_err(),
            kept.unwrap_err(),
            released.unwrap_err(),
        ] {
            assert!(
                reason.contains("reftable format") && reason.contains("by hand"),
                "{reason}"
            );
        }
        assert!(discussion.join("README.md").exists());
        assert!(task_path.join("README.md").exists());
    }

    /// KT-2084 — what package managers and build tools make again is matched by its own name
    /// when ignored. `bin`, `deps` and `obj` are not among them: generic names that may hold
    /// hand-written scripts or hand-placed SDKs (`server.regenerable_outputs` names them).
    #[test]
    fn vendored_dependencies_and_build_folders_are_regenerable_and_generic_names_are_not() {
        let repo = make_test_repo("disc-regenerable-defaults");
        commit_ignore(
            repo.path(),
            "vendor/\nPods/\n.terraform/\n_build/\ndeps/\n.build/\nobj/\nbin/\n",
        );
        let info = create_discussion_worktree(repo.path(), "proj", "defaults", "main").unwrap();
        let checkout = Path::new(&info.path);
        for dir in ["vendor", "Pods", ".terraform", "_build", ".build"] {
            write_file(checkout, &format!("app/{dir}/made/again.txt"), "x");
        }
        assert_eq!(work_in(repo.path(), checkout, &regenerable_defaults()), "");

        for dir in ["bin", "deps", "obj"] {
            write_file(checkout, &format!("{dir}/hand-placed.txt"), "x");
        }
        let work = work_in(repo.path(), checkout, &regenerable_defaults());
        for dir in ["bin/", "deps/", "obj/"] {
            assert!(work.contains(dir), "{dir}: {work}");
        }
    }

    #[cfg(unix)] // writes a POSIX absolute path into a registration's gitdir
    #[test]
    fn a_worktree_two_registrations_name_is_kept() {
        let repo = make_test_repo("two-registrations");
        let info = create_discussion_worktree(repo.path(), "proj", "named-twice", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let own_admin = git_dir_of(&checkout);
        let other = a_canonical_worktree(repo.path(), "naming");
        let other_admin = git_dir_of(&other);
        fs::write(
            other_admin.join("gitdir"),
            format!("{}/.git\n", checkout.display()),
        )
        .unwrap();

        let removed = remove_discussion_worktree(repo.path(), &info.path, None);

        assert!(removed.is_err(), "{removed:?}");
        assert!(checkout.join("README.md").exists());
        assert!(own_admin.exists() && other_admin.exists());
    }

    #[test]
    fn a_branch_other_than_the_expected_one_is_never_deleted_with_its_worktree() {
        let repo = make_test_repo("expected-branch");
        git(repo.path(), &["branch", "feature"]);
        let info = create_discussion_worktree(repo.path(), "proj", "expected", "main").unwrap();
        git(Path::new(&info.path), &["checkout", "-q", "feature"]);

        remove_discussion_worktree(repo.path(), &info.path, Some(&info.branch)).unwrap();

        assert!(!Path::new(&info.path).exists());
        assert!(
            branch_commit(repo.path(), "feature").is_some(),
            "a branch the worktree was switched to was deleted"
        );
    }

    #[test]
    fn removing_a_gone_worktree_keeps_the_registration_of_another_missing_checkout() {
        let repo = make_test_repo("targeted-registration");
        let elsewhere = tempfile::Builder::new()
            .prefix("kronn-wt-user-")
            .tempdir()
            .unwrap();
        let mine = elsewhere.path().join("mine");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "mine",
                mine.to_str().unwrap(),
            ],
        );
        let user_admin = git_dir_of(&mine);
        fs::rename(&mine, elsewhere.path().join("unmounted")).unwrap();
        let info = create_discussion_worktree(repo.path(), "proj", "gone", "main").unwrap();
        let admin = git_dir_of(Path::new(&info.path));
        fs::remove_dir_all(&info.path).unwrap();

        remove_deleted(repo.path(), &info.path).unwrap();

        assert!(
            user_admin.exists(),
            "the registration of a checkout outside Kronn's directory was dropped"
        );
        assert!(!admin.exists(), "the gone worktree's registration stays");
    }

    // ── KT-2084: a judgement reads Kronn's own copy of the git state ─────────

    #[cfg(unix)]
    fn fifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo(3) on a NUL-terminated path inside the test's folder.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }

    /// Releases the FIFO once dropped: held open read-write it lets every pending open
    /// through, removed no later open waits on it, and closed every reader reads an end of
    /// file. A git a regression left blocked on it stops.
    #[cfg(unix)]
    struct Unblock(PathBuf);

    #[cfg(unix)]
    impl Drop for Unblock {
        fn drop(&mut self) {
            use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
            if !fs::symlink_metadata(&self.0).is_ok_and(|meta| meta.file_type().is_fifo()) {
                return;
            }
            let held = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.0);
            let _ = fs::remove_file(&self.0);
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(held);
        }
    }

    /// What `f` returns, or `None` when it has not returned within `bound`.
    fn within<T: Send + 'static>(
        bound: std::time::Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(f());
        });
        received.recv_timeout(bound).ok()
    }

    /// An index git reads (version 2, no extension, a zero checksum) holding `entries`
    /// `(mode, path)`, each naming the same object.
    #[cfg(unix)]
    fn crafted_index(entries: &[(u32, &str)]) -> Vec<u8> {
        let mut sorted: Vec<&(u32, &str)> = entries.iter().collect();
        sorted.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
        let mut out = b"DIRC".to_vec();
        out.extend(2u32.to_be_bytes());
        out.extend((sorted.len() as u32).to_be_bytes());
        for (mode, path) in sorted {
            let start = out.len();
            for field in [0u32, 0, 0, 0, 0, 0, *mode, 0, 0, 0] {
                out.extend(field.to_be_bytes());
            }
            out.extend([0x11u8; 20]);
            out.extend((path.len() as u16).to_be_bytes());
            out.extend(path.as_bytes());
            let padded = (out.len() - start + 8) / 8 * 8;
            out.resize(start + padded, 0);
        }
        out.extend([0u8; 20]);
        out
    }

    #[cfg(unix)]
    fn modified(path: &Path) -> std::time::SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    #[cfg(unix)]
    fn backdate(path: &Path) -> std::time::SystemTime {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        fs::File::options()
            .append(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        modified(path)
    }

    /// A filter a submodule repository's own configuration declares, assigned by the
    /// attributes of its git directory (`<admin>/modules/<name>/info/attributes`), never runs
    /// while a delete judges the worktree. That repository is vetted like a `.git` directory
    /// (KT-2084): Kronn does not judge it, and the worktree stays, work or not.
    #[cfg(unix)] // the planted clean filter is a POSIX shell command line
    #[test]
    fn a_submodule_repository_filter_never_runs_while_a_delete_judges() {
        for (name, work) in [
            ("judge-sub-filter-clean", false),
            ("judge-sub-filter-work", true),
        ] {
            let (repo, checkout) = discussion_worktree_with_submodule(name);
            let sub = checkout.join("sub");
            let module = git_dir_of(&sub);
            assert!(
                module.starts_with(git_dir_of(&checkout).join("modules")),
                "{}",
                module.display()
            );
            let (marks, _) = outside_file();
            let ran = marks.path().join("ran");
            git(
                &sub,
                &[
                    "config",
                    "filter.x.clean",
                    &format!("touch '{}'; cat", ran.display()),
                ],
            );
            fs::create_dir_all(module.join("info")).unwrap();
            fs::write(module.join("info/attributes"), "* filter=x\n").unwrap();
            stale_stat(&sub.join("lib.txt"));
            if work {
                fs::write(sub.join("notes.txt"), "the agent's").unwrap();
            }

            let outcome = remove_deleted(repo.path(), &checkout.to_string_lossy());

            assert!(
                !ran.exists(),
                "{name}: a program of the submodule's configuration ran"
            );
            assert!(
                outcome
                    .as_ref()
                    .is_err_and(|error| error.contains("info/attributes")),
                "{name}: {outcome:?}"
            );
            assert!(checkout.exists(), "{name}");
        }
    }

    /// A `commondir` in the registration naming another repository, whose configuration
    /// includes a FIFO: the delete answers at once and keeps the worktree.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn a_registration_naming_another_common_directory_is_refused_at_once() {
        let repo = make_test_repo("judge-planted-commondir");
        let info =
            create_discussion_worktree(repo.path(), "proj", "planted-common", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let admin = git_dir_of(&checkout);
        let (elsewhere, _) = outside_file();
        git(elsewhere.path(), &["init", "-q", "--bare", "common"]);
        let planted = elsewhere.path().join("common");
        let pipe = elsewhere.path().join("pipe");
        fifo(&pipe);
        let _unblock = Unblock(pipe.clone());
        let config = format!(
            "[core]\n\trepositoryformatversion = 0\n\tbare = true\n[include]\n\tpath = {}\n",
            pipe.display()
        );
        fs::write(planted.join("config"), config).unwrap();
        fs::write(admin.join("commondir"), format!("{}\n", planted.display())).unwrap();
        let (repo_path, path) = (repo.path().to_path_buf(), info.path.clone());

        let outcome = within(std::time::Duration::from_secs(30), move || {
            remove_deleted(&repo_path, &path)
        });

        assert!(
            outcome
                .as_ref()
                .is_some_and(|outcome| outcome.as_ref().is_err_and(|e| e.contains("commondir"))),
            "{outcome:?}"
        );
        assert!(checkout.exists());
    }

    /// A FIFO in place of the registration's index: the delete answers at once and keeps
    /// the worktree.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn a_fifo_in_place_of_the_index_is_refused_at_once() {
        let repo = make_test_repo("judge-fifo-index");
        let info = create_discussion_worktree(repo.path(), "proj", "fifo-index", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let index = git_dir_of(&checkout).join("index");
        fs::remove_file(&index).unwrap();
        fifo(&index);
        let _unblock = Unblock(index.clone());
        let (repo_path, path) = (repo.path().to_path_buf(), info.path.clone());

        let outcome = within(std::time::Duration::from_secs(30), move || {
            remove_deleted(&repo_path, &path)
        });

        assert!(
            outcome
                .as_ref()
                .is_some_and(|outcome| outcome.as_ref().is_err_and(|e| e.contains("index"))),
            "{outcome:?}"
        );
        assert!(checkout.exists());
    }

    /// A filter a task submodule's own `.git` configuration declares, assigned in its tree,
    /// never runs while a release judges the checkout; Kronn's own layout of the same
    /// submodule still releases when nothing is in it.
    #[cfg(unix)] // the planted clean filter is a POSIX shell command line
    #[test]
    fn a_task_submodule_filter_never_runs_and_a_clean_task_checkout_still_releases() {
        let repo = make_test_repo("judge-task-sub-filter");
        let _upstream = add_materialized_test_submodule(repo.path());
        let base = head_sha(repo.path());
        let clean = create_task_worktree(repo.path(), "KT-2084", "sub-clean", &base).unwrap();
        let planted = create_task_worktree(repo.path(), "KT-2084", "sub-filter", &base).unwrap();
        let sub = Path::new(&planted.path).join("deps/required");
        assert!(sub.join(".git").is_dir());
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        git(
            &sub,
            &[
                "config",
                "filter.x.clean",
                &format!("touch '{}'; cat", ran.display()),
            ],
        );
        fs::write(sub.join(".gitattributes"), "* filter=x\n").unwrap();
        stale_stat(&sub.join("bin/bats"));
        let evidence = tempfile::tempdir().unwrap();

        let kept = release_terminal_task_worktree(
            repo.path(),
            &planted.path,
            &planted.branch,
            evidence.path(),
        );
        let released = release_terminal_task_worktree(
            repo.path(),
            &clean.path,
            &clean.branch,
            evidence.path(),
        );

        assert!(
            !ran.exists(),
            "a program of the submodule's configuration ran"
        );
        assert!(kept.is_err(), "{kept:?}");
        assert!(sub.exists());
        assert!(
            matches!(released, Ok(TerminalWorktreeRelease::Removed { .. })),
            "{released:?}"
        );
        assert!(!Path::new(&clean.path).exists());
    }

    /// A submodule path the agent replaced by a link to another repository: no git runs
    /// there, and the worktree is kept with the reason.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn a_submodule_path_turned_into_a_link_keeps_the_worktree_and_runs_nothing_there() {
        let (repo, checkout) = discussion_worktree_with_submodule("judge-sub-link");
        let (elsewhere, _) = outside_file();
        let other = elsewhere.path().join("other");
        fs::create_dir(&other).unwrap();
        git(&other, &["init", "-q", "-b", "main"]);
        fs::write(other.join("a.txt"), "a").unwrap();
        git(&other, &["add", "a.txt"]);
        let index = other.join(".git/index");
        let old = backdate(&index);
        let sub = checkout.join("sub");
        fs::remove_dir_all(&sub).unwrap();
        std::os::unix::fs::symlink(&other, &sub).unwrap();

        let outcome = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            outcome.as_ref().is_err_and(|error| error.contains("sub")),
            "{outcome:?}"
        );
        assert!(checkout.exists());
        assert_eq!(
            modified(&index),
            old,
            "a git command ran in the repository the link leads to"
        );
    }

    /// A gitlink reached through a linked directory, into a repository laid out inside the
    /// worktree's registration: refused, and nothing it configures runs.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn a_gitlink_through_a_linked_directory_into_the_registration_is_refused() {
        let repo = make_test_repo("judge-gitlink-through-link");
        let info = create_discussion_worktree(repo.path(), "proj", "through-link", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let admin = git_dir_of(&checkout);
        let source = tempfile::tempdir().unwrap();
        git(source.path(), &["init", "-q", "-b", "main"]);
        fs::write(source.path().join("f.txt"), "f").unwrap();
        git(source.path(), &["add", "f.txt"]);
        git(
            source.path(),
            &[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "commit",
                "-qm",
                "f",
            ],
        );
        let sha = head_sha(source.path());
        let module = admin.join("modules/other");
        fs::create_dir_all(admin.join("modules")).unwrap();
        fs::rename(source.path().join(".git"), &module).unwrap();
        let tree = admin.join("sub");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("f.txt"), "f").unwrap();
        fs::write(tree.join(".git"), format!("gitdir: {}\n", module.display())).unwrap();
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        git(
            &tree,
            &[
                "config",
                "filter.x.clean",
                &format!("touch '{}'; cat", ran.display()),
            ],
        );
        fs::create_dir_all(module.join("info")).unwrap();
        fs::write(module.join("info/attributes"), "* filter=x\n").unwrap();
        stale_stat(&tree.join("f.txt"));
        std::os::unix::fs::symlink(&admin, checkout.join("lnk")).unwrap();
        git(
            &checkout,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},lnk/sub"),
            ],
        );

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(
            !ran.exists(),
            "a program of that repository's configuration ran"
        );
        assert!(outcome.is_err(), "{outcome:?}");
        assert!(checkout.exists());
    }

    /// Gitlink paths git never writes (`..`, absolute), in a crafted index: refused, and no
    /// git runs in the repository they lead to.
    #[cfg(unix)] // the crafted index names a POSIX absolute path and `..` parts
    #[test]
    fn gitlink_paths_that_are_not_plain_are_refused() {
        for (name, odd) in [
            ("judge-gitlink-parent", "../../../outside"),
            ("judge-gitlink-absolute", ""),
        ] {
            let repo = make_test_repo(name);
            let info = create_discussion_worktree(repo.path(), "proj", name, "main").unwrap();
            let checkout = Path::new(&info.path).canonicalize().unwrap();
            let outside = repo.path().canonicalize().unwrap().join("outside");
            fs::create_dir(&outside).unwrap();
            git(&outside, &["init", "-q", "-b", "main"]);
            fs::write(outside.join("a.txt"), "a").unwrap();
            git(&outside, &["add", "a.txt"]);
            let index = outside.join(".git/index");
            let old = backdate(&index);
            let absolute = outside.to_string_lossy().into_owned();
            let odd = if odd.is_empty() {
                absolute.as_str()
            } else {
                odd
            };
            fs::write(
                git_dir_of(&checkout).join("index"),
                crafted_index(&[(0o160000, odd)]),
            )
            .unwrap();

            let outcome = remove_deleted(repo.path(), &info.path);

            assert!(outcome.is_err(), "{name}: {outcome:?}");
            assert!(checkout.exists());
            assert_eq!(modified(&index), old, "{name}: a git command ran there");
        }
    }

    /// A link at the registration's `modules` folder: the submodule repositories are not
    /// judged through it, and the worktree is kept.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn a_link_at_the_registration_modules_keeps_the_worktree() {
        let repo = make_test_repo("judge-modules-link");
        let info = create_discussion_worktree(repo.path(), "proj", "modules-link", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let admin = git_dir_of(&checkout);
        let (elsewhere, _) = outside_file();
        git(
            elsewhere.path(),
            &["clone", "-q", &repo.path().to_string_lossy(), "clone"],
        );
        let modules = elsewhere.path().join("modules");
        fs::create_dir(&modules).unwrap();
        fs::rename(elsewhere.path().join("clone/.git"), modules.join("x")).unwrap();
        std::os::unix::fs::symlink(&modules, admin.join("modules")).unwrap();

        let outcome = remove_deleted(repo.path(), &info.path);

        assert!(
            outcome.as_ref().is_err_and(|error| error.contains("link")),
            "{outcome:?}"
        );
        assert!(checkout.exists());
    }

    /// A `modules` folder wider or deeper than the walk's bounds: the walk stops there and
    /// the worktree is kept.
    #[cfg(unix)] // git runs in a `canonicalize()`d folder: Windows does not spell it as git takes it
    #[test]
    fn a_module_walk_past_its_bounds_keeps_the_worktree() {
        for (name, deep) in [("judge-modules-wide", false), ("judge-modules-deep", true)] {
            let repo = make_test_repo(name);
            let info = create_discussion_worktree(repo.path(), "proj", name, "main").unwrap();
            let checkout = Path::new(&info.path).canonicalize().unwrap();
            let modules = git_dir_of(&checkout).join("modules");
            if deep {
                fs::create_dir_all(modules.join(["d"; 20].join("/"))).unwrap();
            } else {
                for n in 0..1100 {
                    fs::create_dir_all(modules.join(format!("m{n}"))).unwrap();
                }
            }

            let outcome = remove_deleted(repo.path(), &info.path);

            let why = if deep { "levels deep" } else { "directories" };
            assert!(
                outcome.as_ref().is_err_and(|error| error.contains(why)),
                "{name}: {outcome:?}"
            );
            assert!(checkout.exists());
        }
    }

    /// Kronn reads the worktree's own configuration once, into its copy: a filter written
    /// there afterwards is never what git reads.
    #[cfg(unix)] // the swap probe (unix-only) plants a POSIX shell filter mid-read
    #[test]
    fn a_worktree_configuration_written_after_kronn_read_it_is_not_used() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("judge-wtconfig-race");
        git(
            repo.path(),
            &["config", "core.repositoryformatversion", "1"],
        );
        git(
            repo.path(),
            &["config", "extensions.worktreeConfig", "true"],
        );
        fs::create_dir_all(repo.path().join(".git/info")).unwrap();
        fs::write(repo.path().join(".git/info/attributes"), "* filter=x\n").unwrap();
        let info =
            create_discussion_worktree(repo.path(), "proj", "wtconfig-race", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let admin = git_dir_of(&checkout).canonicalize().unwrap();
        let plain = "[user]\n\tname = T\n";
        let config = admin.join("config.worktree");
        fs::write(&config, plain).unwrap();
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        let filter = format!("[filter \"x\"]\n\tclean = touch '{}'; cat\n", ran.display());
        // Past the reads of its `gitdir`, `commondir` and `config.worktree`: as HEAD is copied.
        swap_probe::arm_hook(&admin, 3, move |_| fs::write(&config, filter).unwrap());

        let judged = Snapshot::of_worktree(&checkout).unwrap();

        assert!(
            swap_probe::fired(),
            "the filter must appear once Kronn read the configuration"
        );
        assert_eq!(
            fs::read_to_string(judged.dir().join("config.worktree")).unwrap(),
            plain
        );
        stale_stat(&checkout.join("README.md"));
        let dirty = judged_dirty_files(&judged);
        assert!(
            !ran.exists(),
            "a filter of the configuration written afterwards ran"
        );
        assert_eq!(dirty, Ok(Vec::new()));
        // The next judgement reads the filter and does not judge.
        let next = Snapshot::of_worktree(&checkout).map(|_| ());
        assert!(
            next.as_ref()
                .is_err_and(|error| error.contains("config.worktree")),
            "{next:?}"
        );
        assert!(!ran.exists());
    }

    /// A git command that never ends (a FIFO where git reads an ignore file): the delete
    /// stops at its time bound, keeps the worktree and says why.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn a_git_command_that_never_ends_keeps_the_worktree_and_names_the_timeout() {
        let repo = make_test_repo("judge-timeout");
        let info = create_discussion_worktree(repo.path(), "proj", "timeout", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let pipe = checkout.join(".gitignore");
        fifo(&pipe);
        let _unblock = Unblock(pipe.clone());
        let (repo_path, path) = (repo.path().to_path_buf(), info.path.clone());
        let started = std::time::Instant::now();

        let outcome = within(std::time::Duration::from_secs(60), move || {
            crate::core::lane_git::snapshot::set_timeout_for_tests(Some(
                std::time::Duration::from_secs(2),
            ));
            remove_deleted(&repo_path, &path)
        });

        assert!(
            outcome
                .as_ref()
                .is_some_and(|outcome| outcome.as_ref().is_err_and(|e| e.contains("timeout"))),
            "{outcome:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        assert!(checkout.exists());
    }

    /// The copy lives in Kronn's data directory and goes with the judgement.
    #[cfg(unix)] // Windows does not spell `canonicalize()` (a `\\?\` path) like Kronn's copy
    #[test]
    fn kronns_copy_of_the_git_state_is_removed_after_use() {
        let repo = make_test_repo("judge-copy-removed");
        let info = create_discussion_worktree(repo.path(), "proj", "copy-removed", "main").unwrap();
        let checkout = Path::new(&info.path).canonicalize().unwrap();
        let judged = Snapshot::of_worktree(&checkout).unwrap();
        let copy = judged.dir().to_path_buf();
        let data = crate::core::config::config_dir().unwrap();
        assert!(
            copy.starts_with(data.canonicalize().unwrap()),
            "{}",
            copy.display()
        );
        assert!(!copy.starts_with(&checkout));
        assert!(copy.join("HEAD").is_file());

        drop(judged);

        assert!(!copy.exists());
    }

    // ── KT-2084: no git in a submodule, bounded listings and locks, rooted walks ──

    /// A filter written into a task submodule's own configuration once Kronn judged the
    /// checkout never runs while the release saves its patch: no git starts in the submodule,
    /// and the patch holds the checkout's own change.
    #[cfg(unix)] // the swap probe (unix-only) plants a POSIX shell filter mid-release
    #[test]
    fn a_release_starts_no_git_in_a_submodule_while_it_saves_the_patch() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("release-no-sub-child");
        let _upstream = add_materialized_test_submodule(repo.path());
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-2084", "no-sub-child", &base).unwrap();
        let checkout = Path::new(&info.path);
        let module = checkout.join("deps/required/.git");
        assert!(module.is_dir());
        let (marks, _) = outside_file();
        let ran = marks.path().join("ran");
        let trace = marks.path().join("trace");
        let filter = format!("[filter \"x\"]\n\tclean = touch '{}'; cat\n", ran.display());
        stale_stat(&checkout.join("deps/required/bin/bats"));
        fs::write(checkout.join("README.md"), "the agent's change\n").unwrap();
        let evidence = tempfile::tempdir().unwrap();
        // As the release clears its evidence folder: the judgement and its vetting are done.
        swap_probe::arm_hook(evidence.path(), 0, move |_| {
            let mut config = fs::OpenOptions::new()
                .append(true)
                .open(module.join("config"))
                .unwrap();
            std::io::Write::write_all(&mut config, filter.as_bytes()).unwrap();
            fs::create_dir_all(module.join("info")).unwrap();
            fs::write(module.join("info/attributes"), "* filter=x\n").unwrap();
        });
        crate::core::lane_git::snapshot::set_trace_for_tests(Some(trace.clone()));

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path());

        crate::core::lane_git::snapshot::set_trace_for_tests(None);
        assert!(
            swap_probe::fired(),
            "the filter must appear once Kronn judged the checkout"
        );
        assert!(
            !ran.exists(),
            "a program of the submodule's configuration ran"
        );
        let traced = fs::read_to_string(&trace).unwrap_or_default();
        assert!(!traced.is_empty(), "git traced nothing");
        assert!(
            !traced.contains("run_command"),
            "git started a program: {traced}"
        );
        let patch = match released {
            Ok(TerminalWorktreeRelease::Removed {
                patch: Some(patch), ..
            }) => fs::read_to_string(patch).unwrap(),
            other => panic!("{other:?}"),
        };
        assert!(patch.contains("README.md"), "{patch}");
    }

    /// A task submodule whose HEAD moved to a commit a tag holds: the release's patch records
    /// the submodule at that commit.
    #[test]
    fn a_moved_submodule_head_is_recorded_in_the_release_patch() {
        let repo = make_test_repo("release-sub-moved");
        let _upstream = add_materialized_test_submodule(repo.path());
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-2084", "sub-moved", &base).unwrap();
        let sub = Path::new(&info.path).join("deps/required");
        fs::write(sub.join("bin/extra"), "extra\n").unwrap();
        git(&sub, &["add", "bin/extra"]);
        git(
            &sub,
            &[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "commit",
                "-qm",
                "extra",
            ],
        );
        git(&sub, &["tag", "kept"]);
        let moved = head_sha(&sub);
        let evidence = tempfile::tempdir().unwrap();

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path());

        let patch = match released {
            Ok(TerminalWorktreeRelease::Removed {
                patch: Some(patch), ..
            }) => fs::read_to_string(patch).unwrap(),
            other => panic!("{other:?}"),
        };
        assert!(
            patch.contains(&format!("+Subproject commit {moved}")),
            "{patch}"
        );
    }

    /// An untracked repository in a task checkout never becomes a submodule line of the
    /// release's patch: the checkout is kept and the repository named.
    #[test]
    fn an_untracked_repository_keeps_a_released_task_checkout() {
        let repo = make_test_repo("release-untracked-repo");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-2084", "untracked-repo", &base).unwrap();
        let nested = Path::new(&info.path).join("vendored");
        fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "main"]);
        fs::write(nested.join("a.txt"), "only here").unwrap();
        git(&nested, &["add", "a.txt"]);
        git(
            &nested,
            &[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "commit",
                "-qm",
                "only here",
            ],
        );
        let evidence = tempfile::tempdir().unwrap();

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path());

        assert!(
            released
                .as_ref()
                .is_err_and(|reason| reason.contains("vendored") && reason.contains("repository")),
            "{released:?}"
        );
        assert!(nested.join(".git").is_dir() && nested.join("a.txt").exists());
    }

    /// A FIFO in another worktree's registration (its `locked`, `gitdir` or `HEAD`): the
    /// prune answers at once and removes no registration.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn a_fifo_in_a_registration_keeps_the_prune_bounded_and_removes_nothing() {
        for file in ["locked", "gitdir", "HEAD"] {
            let repo = make_test_repo(&format!("prune-fifo-{}", file.to_lowercase()));
            let gone = a_canonical_worktree(repo.path(), "gone");
            let gone_admin = git_dir_of(&gone);
            fs::remove_dir_all(&gone).unwrap();
            let other = a_canonical_worktree(repo.path(), "other");
            let pipe = git_dir_of(&other).join(file);
            let _ = fs::remove_file(&pipe);
            fifo(&pipe);
            let _unblock = Unblock(pipe.clone());
            let repo_path = repo.path().to_path_buf();

            let pruned = within(std::time::Duration::from_secs(20), move || {
                let _ = prune_kronn_worktrees(&repo_path);
            });

            assert!(pruned.is_some(), "{file}: the prune did not return");
            assert!(
                gone_admin.join("HEAD").exists(),
                "{file}: a registration was removed"
            );
        }
    }

    /// A Cargo lock name that is not a regular file (a FIFO, a link to an outside FIFO or
    /// file): the lane target's removal answers at once, refuses with the reason and keeps it.
    #[cfg(unix)] // FIFO made with libc::mkfifo, symlinks through std::os::unix
    #[test]
    fn a_lock_name_that_is_not_a_file_refuses_the_lane_target_removal_at_once() {
        for case in ["fifo", "link-to-fifo", "link-to-file"] {
            let repo = make_test_repo(&format!("lane-lock-{case}"));
            let lane = seed_lane_target(repo.path(), "kt-7-abcd");
            let (outside, kept) = outside_file();
            let lock = lane.join("debug/.cargo-lock");
            let pipe = outside.path().join("pipe");
            match case {
                "fifo" => fifo(&lock),
                "link-to-fifo" => {
                    fifo(&pipe);
                    std::os::unix::fs::symlink(&pipe, &lock).unwrap();
                }
                _ => std::os::unix::fs::symlink(&kept, &lock).unwrap(),
            }
            let _unblock = (Unblock(lock.clone()), Unblock(pipe.clone()));
            let (repo_path, target) = (repo.path().to_path_buf(), lane.clone());

            let outcome = within(std::time::Duration::from_secs(5), move || {
                remove_lane_target(&repo_path, &target)
            });

            assert!(
                outcome.as_ref().is_some_and(|outcome| outcome
                    .as_ref()
                    .is_err_and(|error| error.contains(".cargo-lock"))),
                "{case}: {outcome:?}"
            );
            assert!(lane.join("debug/kronn").exists(), "{case}: the lane went");
            assert_eq!(fs::read_to_string(&kept).unwrap(), "outside");
        }
    }

    /// A lane owner marker that is a FIFO: the sweep's read of it answers at once, and the
    /// lane is not marked.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn a_lane_owner_marker_that_is_a_fifo_is_read_at_once() {
        let repo = make_test_repo("lane-marker-fifo");
        let lane = seed_lane_target(repo.path(), "kt-8-abcd");
        let marker = lane.join(LANE_OWNER_MARKER);
        fifo(&marker);
        let _unblock = Unblock(marker.clone());
        let repo_path = repo.path().to_path_buf();

        let owners = within(std::time::Duration::from_secs(5), move || {
            lane_target_owners(&repo_path)
        });

        assert_eq!(owners, Some(Vec::new()));
        assert!(lane.exists());
    }

    /// A folder of a checkout swapped for a link to an outside folder while Kronn walks it:
    /// no outside name is ever visited.
    #[cfg(unix)] // the swap probe puts a symlink in place of a folder (std::os::unix)
    #[test]
    fn a_checkout_walk_never_lists_a_folder_swapped_for_a_link() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("walk-swap");
        let checkout = a_canonical_worktree(repo.path(), "walk-swap");
        write_file(&checkout, "inner/inside.txt", "in");
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside-only.txt"), "out").unwrap();
        swap_probe::arm(&checkout.join("inner"), outside.path());
        let mut seen = Vec::new();

        let _ = walk_checkout_writes(&checkout, |path, _| {
            seen.push(path.to_path_buf());
            true
        });

        assert!(swap_probe::fired(), "the folder must be swapped mid-walk");
        assert!(
            !seen.iter().any(|path| path.ends_with("outside-only.txt")),
            "{seen:?}"
        );
    }

    /// An untracked folder of OS junk whose subfolder is swapped for a link to an outside
    /// folder holding work while Kronn walks it: the outside work is never seen.
    #[cfg(unix)] // the swap probe puts a symlink in place of a folder (std::os::unix)
    #[test]
    fn an_os_junk_walk_never_lists_a_folder_swapped_for_a_link() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("junk-walk-swap");
        let checkout = a_canonical_worktree(repo.path(), "junk-walk-swap");
        write_file(&checkout, "junk/sub/.DS_Store", "x");
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("work.txt"), "work").unwrap();
        swap_probe::arm(&checkout.join("junk/sub"), outside.path());

        let dirty = dirty_files_of(&checkout, b"?? junk/\0");

        assert!(swap_probe::fired(), "the folder must be swapped mid-walk");
        assert!(dirty.is_empty(), "{dirty:?}");
    }

    /// A lane target's folder swapped for a link to an outside folder written later while
    /// Kronn looks for its last write: the outside write is never seen.
    #[cfg(unix)] // the swap probe puts a symlink in place of a folder (std::os::unix)
    #[test]
    fn a_lane_target_last_write_never_lists_a_folder_swapped_for_a_link() {
        use crate::core::fs_guard::swap_probe;
        let repo = make_test_repo("lane-write-swap");
        let lane = seed_lane_target(repo.path(), "kt-9-abcd");
        let outside = tempfile::tempdir().unwrap();
        let later = outside.path().join("later");
        fs::write(&later, "later").unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(10 * 86_400);
        fs::File::options()
            .append(true)
            .open(&later)
            .unwrap()
            .set_modified(future)
            .unwrap();
        swap_probe::arm(&lane.join("debug"), outside.path());

        let written = lane_target_last_write(&lane);

        assert!(swap_probe::fired(), "the folder must be swapped mid-walk");
        assert!(
            written.is_some_and(|written| written < future),
            "{written:?}"
        );
    }

    /// A setting Kronn does not trust in a submodule's module repository (the `.git` file
    /// layout): the delete keeps the worktree and names it.
    #[test]
    fn a_module_repository_setting_kronn_does_not_trust_keeps_the_worktree() {
        let (repo, checkout) = discussion_worktree_with_submodule("module-config-vetted");
        let sub = checkout.join("sub");
        git(&sub, &["config", "filter.x.clean", "cat"]);

        let kept = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            kept.as_ref()
                .is_err_and(|reason| reason.contains("filter.x.clean")),
            "{kept:?}"
        );
        assert!(sub.join("lib.txt").exists());
    }

    /// An `objects/info/alternates` in a submodule's repository: Kronn does not judge it, and
    /// the delete keeps the worktree with the reason and what to do.
    #[test]
    fn a_submodule_repository_with_alternates_keeps_the_worktree() {
        let (repo, checkout) = discussion_worktree_with_submodule("module-alternates");
        let module = git_dir_of(&checkout.join("sub"));
        let (elsewhere, _) = outside_file();
        fs::create_dir_all(module.join("objects/info")).unwrap();
        fs::write(
            module.join("objects/info/alternates"),
            format!("{}\n", elsewhere.path().display()),
        )
        .unwrap();

        let kept = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            kept.as_ref()
                .is_err_and(|reason| reason.contains("alternates")
                    && reason.contains("--reference")
                    && reason.contains("by hand")),
            "{kept:?}"
        );
        assert!(checkout.join("sub/lib.txt").exists());
    }

    /// A judgement never reads git's untracked cache: every command against Kronn's copy
    /// turns it off.
    #[test]
    fn a_judgement_turns_the_untracked_cache_off() {
        let repo = make_test_repo("judge-untracked-cache");
        let checkout = a_canonical_worktree(repo.path(), "untracked-cache");
        let judged = Snapshot::of_worktree(&checkout).unwrap();

        let args: Vec<String> = judged
            .command()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-c" && pair[1] == "core.untrackedCache=false"),
            "{args:?}"
        );
    }

    /// An ignored file of a task checkout with another name (a hard link): its content is not
    /// copied into the evidence, and the checkout is kept with the reason.
    #[cfg(unix)] // a hard link's name count is read through std::os::unix only
    #[test]
    fn an_ignored_file_with_another_name_keeps_a_released_task_checkout() {
        let repo = make_test_repo("release-hard-link");
        commit_ignore(repo.path(), ".env\n");
        let base = head_sha(repo.path());
        let info = create_task_worktree(repo.path(), "KT-2084", "hard-link", &base).unwrap();
        let (_outside, kept) = outside_file();
        let env = Path::new(&info.path).join(".env");
        fs::hard_link(&kept, &env).unwrap();
        let evidence = tempfile::tempdir().unwrap();

        let released =
            release_terminal_task_worktree(repo.path(), &info.path, &info.branch, evidence.path());

        assert!(
            released
                .as_ref()
                .is_err_and(|reason| reason.contains(".env") && reason.contains("link")),
            "{released:?}"
        );
        assert!(env.exists());
        assert!(!evidence
            .path()
            .join(RELEASED_WORKTREE_IGNORED_DIR)
            .join(".env")
            .exists());
    }

    /// An ownerless worktree whose `target` is a link to an outside folder holding a Cargo
    /// lock: the link is not followed to look for a build, and goes as a link.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn an_ownerless_worktree_target_link_is_never_followed() {
        use fs2::FileExt;
        let repo = make_test_repo("ownerless-target-link");
        let checkout = ownerless_worktree(repo.path(), "target-link");
        let (outside, kept) = outside_file();
        let lock = fs::File::create(outside.path().join(".cargo-lock")).unwrap();
        lock.lock_exclusive().unwrap();
        std::os::unix::fs::symlink(outside.path(), checkout.join("target")).unwrap();

        let removed = remove_ownerless_worktree(repo.path(), &checkout);

        assert!(removed.is_ok(), "{removed:?}");
        assert!(!checkout.exists());
        assert!(kept.exists() && outside.path().join(".cargo-lock").exists());
    }

    // ── KT-2084 seventh round ───────────────────────────────────────────────

    /// A git repository in `dir` holding `file`, committed.
    fn upstream_repo(dir: &Path, file: &str) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
        ] {
            git(dir, &args);
        }
        fs::write(dir.join(file), "shared").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-qm", "init"]);
    }

    /// A discussion worktree whose submodule `sub` has a submodule `in` of its own, set up by
    /// `git submodule update --init --recursive` in the worktree. `update`: the strategy the
    /// outer submodule's `.gitmodules` names for `in`.
    fn discussion_worktree_with_nested_submodule(
        name: &str,
        update: Option<&str>,
    ) -> (tempfile::TempDir, PathBuf) {
        let inner = tempfile::Builder::new()
            .prefix(&format!("kronn-wt-{name}-in"))
            .tempdir()
            .unwrap();
        upstream_repo(inner.path(), "inner.txt");
        let outer = tempfile::Builder::new()
            .prefix(&format!("kronn-wt-{name}-out"))
            .tempdir()
            .unwrap();
        upstream_repo(outer.path(), "outer.txt");
        let file_allowed = ["-c", "protocol.file.allow=always"];
        let inner_url = inner.path().to_string_lossy().to_string();
        git(
            outer.path(),
            &[
                &file_allowed[..],
                &["submodule", "add", "-q", &inner_url, "in"],
            ]
            .concat(),
        );
        if let Some(update) = update {
            git(
                outer.path(),
                &[
                    "config",
                    "--file",
                    ".gitmodules",
                    "submodule.in.update",
                    update,
                ],
            );
            git(outer.path(), &["add", ".gitmodules"]);
        }
        git(outer.path(), &["commit", "-qm", "nested"]);
        let repo = make_test_repo(name);
        let outer_url = outer.path().to_string_lossy().to_string();
        git(
            repo.path(),
            &[
                &file_allowed[..],
                &["submodule", "add", "-q", &outer_url, "sub"],
            ]
            .concat(),
        );
        git(repo.path(), &["commit", "-qm", "add submodule"]);
        let info = create_discussion_worktree(repo.path(), "proj", name, "main").unwrap();
        let checkout = PathBuf::from(&info.path).canonicalize().unwrap();
        git(
            &checkout,
            &[
                &file_allowed[..],
                &["submodule", "update", "--init", "--recursive", "-q"],
            ]
            .concat(),
        );
        assert!(checkout.join("sub/in/inner.txt").exists());
        (repo, checkout)
    }

    /// The configuration of the outer submodule's repository, as git set it up.
    fn outer_module_config(checkout: &Path) -> PathBuf {
        git_dir_of(&checkout.join("sub")).join("config")
    }

    /// KT-2084 — a recursive setup writes the inner submodule's `url` and `active` into the
    /// outer one's configuration: Kronn's lane git still resolves, and a clean delete goes.
    #[test]
    fn a_submodule_with_its_own_submodule_set_up_is_judged_and_removed() {
        let (repo, checkout) = discussion_worktree_with_nested_submodule("nested-sub", None);
        let config = fs::read_to_string(outer_module_config(&checkout)).unwrap();
        assert!(config.contains("[submodule \"in\"]"), "{config}");

        let resolved = crate::core::lane_git::git(&checkout).map(|_| ());
        let removed = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert_eq!(resolved, Ok(()));
        assert!(removed.is_ok(), "{removed:?}");
        assert!(!checkout.exists());
    }

    /// The same with an update strategy `git submodule init` copies from `.gitmodules`.
    #[test]
    fn a_nested_submodule_with_a_rebase_update_is_judged_and_removed() {
        let (repo, checkout) =
            discussion_worktree_with_nested_submodule("nested-sub-rebase", Some("rebase"));
        let config = fs::read_to_string(outer_module_config(&checkout)).unwrap();
        assert!(config.contains("update = rebase"), "{config}");

        let resolved = crate::core::lane_git::git(&checkout).map(|_| ());
        let removed = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert_eq!(resolved, Ok(()));
        assert!(removed.is_ok(), "{removed:?}");
        assert!(!checkout.exists());
    }

    /// A command as update strategy, or an `ignore` setting, in a module's configuration:
    /// the worktree is kept and Kronn's lane git refuses it.
    #[test]
    fn a_command_update_or_an_ignore_in_a_module_configuration_is_refused() {
        for (name, key, value) in [
            ("nested-sub-command", "submodule.in.update", "!touch ran"),
            ("nested-sub-ignore", "submodule.in.ignore", "all"),
        ] {
            let (repo, checkout) = discussion_worktree_with_nested_submodule(name, None);
            let config = outer_module_config(&checkout);
            git(
                &checkout,
                &["config", "--file", &config.to_string_lossy(), key, value],
            );

            let resolved = crate::core::lane_git::git(&checkout).map(|_| ());
            let kept = remove_deleted(repo.path(), &checkout.to_string_lossy());

            assert!(
                resolved.as_ref().is_err_and(|error| error.contains(key)),
                "{name}: {resolved:?}"
            );
            assert!(
                kept.as_ref().is_err_and(|error| error.contains(key)),
                "{name}: {kept:?}"
            );
            assert!(checkout.join("sub/in/inner.txt").exists());
        }
    }

    /// A stray file among git's worktree records (a `.DS_Store`) is no registration: git skips
    /// it, and so does Kronn's listing.
    #[test]
    fn a_stray_file_among_the_worktree_records_is_skipped() {
        let repo = make_test_repo("stray-record");
        create_discussion_worktree(repo.path(), "proj", "first", "main").unwrap();
        fs::write(repo.path().join(".git/worktrees/.DS_Store"), "junk").unwrap();

        let created = create_discussion_worktree(repo.path(), "proj", "second", "main");

        assert!(created.is_ok(), "{created:?}");
        assert_eq!(registered_worktree_paths(repo.path()).unwrap().len(), 3);
    }

    /// A bare repository has no main checkout: only its linked worktrees are listed, even
    /// when its folder is named `.git`.
    #[test]
    fn a_bare_repository_lists_only_its_linked_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = make_test_repo("bare-source");
        let bare = dir.path().join(".git");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                upstream.path().to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let linked = dir.path().join("linked");
        git(
            &bare,
            &["worktree", "add", "-q", linked.to_str().unwrap(), "main"],
        );

        let listed = registered_worktree_paths(&bare).unwrap();

        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(same_path(&listed[0], &linked), "{listed:?}");
    }

    /// A link in place of a registration directory still fails the listing.
    #[cfg(unix)] // symlink made through std::os::unix
    #[test]
    fn a_link_among_the_worktree_records_fails_the_listing() {
        let repo = make_test_repo("linked-record");
        create_discussion_worktree(repo.path(), "proj", "first", "main").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(elsewhere.path().join("gitdir"), "/nowhere/.git\n").unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), repo.path().join(".git/worktrees/linked"))
            .unwrap();

        let listed = registered_worktree_paths(repo.path());

        assert!(
            listed.as_ref().is_err_and(|error| error.contains("linked")),
            "{listed:?}"
        );
    }

    /// The `HEAD` of a reftable repository is a stub naming no branch: the branch a checkout
    /// is on is unknown, never read as `.invalid`.
    #[test]
    fn a_reftable_head_makes_the_worktree_listing_name_the_format() {
        let repo = make_test_repo("reftable-listing");
        let checkout = a_canonical_worktree(repo.path(), "reftable-head");
        fs::write(
            git_dir_of(&checkout).join("HEAD"),
            crate::core::lane_git::snapshot::REFTABLE_STUB,
        )
        .unwrap();

        let listed = registered_worktree_paths(repo.path());

        assert!(
            listed.as_ref().is_err_and(|error| error
                .contains(crate::core::lane_git::snapshot::REFTABLE_FORMAT)
                && error.contains("by hand")),
            "{listed:?}"
        );
    }

    /// A submodule whose refs are in the reftable format is named as such, with what to do.
    #[test]
    fn a_reftable_submodule_keeps_the_worktree_with_the_format_named() {
        let (repo, checkout) = discussion_worktree_with_submodule("module-reftable");
        let config = git_dir_of(&checkout.join("sub")).join("config");
        let config = config.to_string_lossy();
        git(
            &checkout,
            &[
                "config",
                "--file",
                &config,
                "core.repositoryformatversion",
                "1",
            ],
        );
        git(
            &checkout,
            &[
                "config",
                "--file",
                &config,
                "extensions.refStorage",
                "reftable",
            ],
        );

        let kept = remove_deleted(repo.path(), &checkout.to_string_lossy());

        assert!(
            kept.as_ref().is_err_and(|reason| reason
                .contains(crate::core::lane_git::snapshot::REFTABLE_FORMAT)
                && reason.contains("by hand")),
            "{kept:?}"
        );
        assert!(checkout.join("sub/lib.txt").exists());
    }

    /// A partial clone of a submodule (`--filter`) fetches its objects on demand: Kronn does
    /// not judge it, and says what to do. Git writes a promisor remote; older versions wrote
    /// `extensions.partialClone`.
    #[test]
    fn a_partial_clone_submodule_keeps_the_worktree_with_what_to_do() {
        let written: [&[(&str, &str)]; 2] = [
            &[
                ("remote.origin.promisor", "true"),
                ("remote.origin.partialclonefilter", "blob:none"),
            ],
            &[("extensions.partialClone", "origin")],
        ];
        for (n, settings) in written.into_iter().enumerate() {
            let (repo, checkout) =
                discussion_worktree_with_submodule(&format!("module-partial{n}"));
            let config = git_dir_of(&checkout.join("sub")).join("config");
            let config = config.to_string_lossy();
            git(
                &checkout,
                &[
                    "config",
                    "--file",
                    &config,
                    "core.repositoryformatversion",
                    "1",
                ],
            );
            for (key, value) in settings {
                git(&checkout, &["config", "--file", &config, key, value]);
            }

            let kept = remove_deleted(repo.path(), &checkout.to_string_lossy());

            assert!(
                kept.as_ref().is_err_and(
                    |reason| reason.contains("partial clone") && reason.contains("by hand")
                ),
                "{settings:?}: {kept:?}"
            );
            assert!(checkout.join("sub/lib.txt").exists());
        }
    }

    // ── KT-2084: Kronn's own registrations, one file system, git within bounds ──

    /// The registration of the discussion worktree `proj--<slug>` of `repo`.
    fn admin_of(repo: &Path, slug: &str) -> PathBuf {
        repo.join(".git/worktrees").join(format!("proj--{slug}"))
    }

    /// Lanes `a` and `b`, the `gitdir` of `a` rewritten to name `b`'s `.git` (`a` sorts
    /// first: git's own pruning keeps it and deletes `b`'s registration).
    fn lane_a_naming_lane_b(name: &str) -> (tempfile::TempDir, WorktreeInfo, WorktreeInfo) {
        let repo = make_test_repo(name);
        let a = create_discussion_worktree(repo.path(), "proj", "a", "main").unwrap();
        let b = create_discussion_worktree(repo.path(), "proj", "b", "main").unwrap();
        let named = fs::read(admin_of(repo.path(), "b").join("gitdir")).unwrap();
        fs::write(admin_of(repo.path(), "a").join("gitdir"), named).unwrap();
        (repo, a, b)
    }

    #[test]
    fn a_registration_naming_another_lane_costs_that_lane_nothing_on_a_reattach() {
        let (repo, _a, b) = lane_a_naming_lane_b("dup-reattach");
        git(repo.path(), &["branch", "kronn/c"]);

        let _ = reattach_worktree(repo.path(), "proj", "c", "kronn/c");

        let admin = admin_of(repo.path(), "b");
        assert!(
            admin.join("index").is_file(),
            "the registration of {} lost its index",
            b.path
        );
        assert!(admin.join("HEAD").is_file());
    }

    #[test]
    fn removing_a_lane_whose_registration_names_another_removes_its_own_and_keeps_the_other() {
        let (repo, a, b) = lane_a_naming_lane_b("dup-remove");

        let removed = remove_owned_worktree(repo.path(), Path::new(&a.path));

        assert!(removed.is_ok(), "{removed:?}");
        assert!(!Path::new(&a.path).exists());
        assert!(
            !admin_of(repo.path(), "a").exists(),
            "the rewritten registration was left behind"
        );
        assert!(admin_of(repo.path(), "b").join("index").is_file());
        assert!(Path::new(&b.path).join("README.md").exists());
    }

    #[test]
    fn two_registrations_naming_one_worktree_fail_every_listing_with_both_names() {
        let (repo, _a, b) = lane_a_naming_lane_b("dup-listing");
        let both = |error: &String| error.contains("proj--a") && error.contains("proj--b");
        // As git can open them: Windows canonicalises to `\\?\C:\…`, which no record spells.
        let plain = crate::core::lane_git::without_verbatim_prefix;
        let root = plain(repo.path().canonicalize().unwrap());
        let lane = plain(Path::new(&b.path).canonicalize().unwrap());

        let listed = registered_worktree_paths(repo.path());
        assert!(listed.as_ref().is_err_and(both), "{listed:?}");
        let found = crate::core::lane_git::registration_of(&root, &lane);
        assert!(found.as_ref().is_err_and(both), "{found:?}");

        // The same path in another case, on a volume that ignores case (`core.ignorecase`).
        let ignores_case = std::process::Command::new("git")
            .args(["config", "--bool", "core.ignorecase"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        if String::from_utf8_lossy(&ignores_case.stdout).trim() == "true" {
            let upper = root.join(".kronn/worktrees/PROJ--B/.git");
            fs::write(
                admin_of(repo.path(), "a").join("gitdir"),
                format!("{}\n", upper.display()),
            )
            .unwrap();
            let listed = registered_worktree_paths(repo.path());
            assert!(listed.as_ref().is_err_and(both), "{listed:?}");
            let found = crate::core::lane_git::registration_of(&root, &lane);
            assert!(found.as_ref().is_err_and(both), "{found:?}");
        }
    }

    /// A registration naming a path outside the owner's checkout is not Kronn's: listed as
    /// written, never looked at (an automounted `/net/<host>` would make the server resolve
    /// and mount that host) and never pruned. Unix only: `/net/…` is absolute there.
    #[cfg(unix)] // `/net/<host>` is an absolute path on POSIX only
    #[test]
    fn a_registration_naming_a_path_outside_the_checkout_is_listed_without_a_look_there() {
        let repo = make_test_repo("foreign-gitdir");
        create_discussion_worktree(repo.path(), "proj", "lane", "main").unwrap();
        let gitdir = admin_of(repo.path(), "lane").join("gitdir");
        let listed_lane = |repo: &Path| {
            crate::core::lane_git::registered_worktrees(repo)
                .unwrap()
                .into_iter()
                .find(|entry| {
                    entry
                        .admin
                        .as_deref()
                        .is_some_and(|admin| admin.ends_with("proj--lane"))
                })
                .unwrap()
        };
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(
            &gitdir,
            format!("{}/gone/.git\n", elsewhere.path().display()),
        )
        .unwrap();

        let entry = listed_lane(repo.path());
        assert!(!entry.prunable && !entry.inside_checkout, "{entry:?}");

        fs::write(&gitdir, "/net/kronn-test.invalid/x/.git\n").unwrap();
        let started = std::time::Instant::now();
        let entry = listed_lane(repo.path());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(entry.path, Path::new("/net/kronn-test.invalid/x"));
        assert!(!entry.prunable && !entry.inside_checkout, "{entry:?}");
    }

    /// A `target` deeper than a removal goes: the cleanup says it removed only part of it.
    /// Unix only: the Windows removal has no depth bound of its own.
    #[cfg(unix)] // Windows has no depth bound in its removal: nothing to report
    #[test]
    fn a_build_folder_deeper_than_the_removal_goes_is_reported_partly_removed() {
        let repo = make_test_repo("clean-deep");
        let worktree = repo.path().join(".kronn/worktrees/KT-9-aaaa0009");
        let target = managed_target(repo.path(), "KT-9-aaaa0009", 16);
        let mut deep = target.join("deep");
        for _ in 0..70 {
            deep.push("d");
        }
        fs::create_dir_all(&deep).unwrap();

        let cleaned =
            clean_worktree_build_artifacts(repo.path(), &worktree, ExecutionLiveness::Terminal);

        assert!(
            cleaned
                .as_ref()
                .is_err_and(|error| error.contains(PARTLY_REMOVED)),
            "{cleaned:?}"
        );
        assert!(worktree.exists());
    }

    /// The registered git directory of a lane comes from the owner's records alone: a `.git`
    /// pointer moved to a git directory whose configuration never ends is refused at once.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn the_registered_git_dir_of_a_lane_is_found_without_running_git_in_it() {
        let repo = make_test_repo("lane-git-dir");
        let info = create_discussion_worktree(repo.path(), "proj", "lane", "main").unwrap();
        let lane = PathBuf::from(&info.path);
        let fake = lane.join("fake");
        fs::create_dir_all(fake.join("objects")).unwrap();
        fs::create_dir_all(fake.join("refs")).unwrap();
        fs::write(fake.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fifo(&fake.join("config"));
        let _unblock = Unblock(fake.join("config"));
        fs::write(lane.join(".git"), format!("gitdir: {}\n", fake.display())).unwrap();
        let main = repo.path().to_path_buf();

        let found = within(std::time::Duration::from_secs(30), move || {
            registered_worktree_git_dir(&main, &lane)
        });

        let found = found.expect("the lookup waited on the lane's git directory");
        assert!(
            found
                .as_ref()
                .is_err_and(|error| error.contains("pointer was moved")),
            "{found:?}"
        );
    }

    /// Counts the opens of the FIFO `path`: each reader gets an end of file at once, so
    /// nothing that reads it waits.
    #[cfg(unix)]
    struct OpenCounter {
        path: PathBuf,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        opens: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        writer: Option<std::thread::JoinHandle<()>>,
    }

    #[cfg(unix)]
    impl OpenCounter {
        fn start(path: &Path) -> Self {
            use std::sync::atomic::Ordering;
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let opens = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let writer = {
                let (path, stop, opens) = (path.to_path_buf(), stop.clone(), opens.clone());
                std::thread::spawn(move || loop {
                    // Waits for a reader.
                    let opened = fs::OpenOptions::new().write(true).open(&path);
                    if stop.load(Ordering::SeqCst) || opened.is_err() {
                        break;
                    }
                    opens.fetch_add(1, Ordering::SeqCst);
                    drop(opened);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                })
            };
            Self {
                path: path.to_path_buf(),
                stop,
                opens,
                writer: Some(writer),
            }
        }

        /// How many times it was opened; a reader of its own lets the waiting writer end.
        fn stop(mut self) -> usize {
            use std::os::unix::fs::OpenOptionsExt;
            use std::sync::atomic::Ordering;
            self.stop.store(true, Ordering::SeqCst);
            let _reader = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.path);
            if let Some(writer) = self.writer.take() {
                writer.join().unwrap();
            }
            self.opens.load(Ordering::SeqCst)
        }
    }

    /// A gitlink whose path leads into a folder of the owner's checkout a worker writes (a lane
    /// target) holding a `.git` it planted (a folder, a link to one, a `gitdir:` file): it is
    /// no source the owner declares, no git runs there, and the commit only the task's
    /// submodule holds keeps the checkout.
    #[cfg(unix)] // FIFO made with libc::mkfifo, symlinks through std::os::unix
    #[test]
    fn a_planted_source_of_a_task_submodule_runs_no_git_and_saves_no_commit() {
        let repo = make_test_repo("planted-source");
        let upstream = make_test_repo("planted-upstream");
        let base = head_sha(repo.path());
        let task = create_task_worktree(repo.path(), "KT-2084", "planted", &base).unwrap();
        let checkout = PathBuf::from(&task.path);
        let lane = "target/wt-kt-2084-planted";
        for name in ["a", "b", "c"] {
            let relative = format!("{lane}/{name}");
            git(
                &checkout,
                &["clone", "-q", upstream.path().to_str().unwrap(), &relative],
            );
            git(
                &checkout,
                &["-c", "advice.addEmbeddedRepo=false", "add", "-f", &relative],
            );
        }
        let own = commit_file(
            &checkout.join(lane).join("a"),
            "local.txt",
            "only here",
            "task-only commit",
        );
        let planted = repo.path().join(lane);
        fs::create_dir_all(planted.join("b")).unwrap();
        fs::create_dir_all(planted.join("c")).unwrap();
        git(
            repo.path(),
            &[
                "clone",
                "-q",
                "--bare",
                checkout.join(lane).join("a").to_str().unwrap(),
                planted.join("a/.git").to_str().unwrap(),
            ],
        );
        let config = planted.join("a/.git/config");
        fs::remove_file(&config).unwrap();
        fifo(&config);
        let _unblock = Unblock(config.clone());
        std::os::unix::fs::symlink(planted.join("a/.git"), planted.join("b/.git")).unwrap();
        fs::write(
            planted.join("c/.git"),
            format!("gitdir: {}\n", planted.join("a/.git").display()),
        )
        .unwrap();
        let opens = OpenCounter::start(&config);
        let evidence = tempfile::tempdir().unwrap();
        let (owner, path, branch, kept_in) = (
            repo.path().to_path_buf(),
            task.path.clone(),
            task.branch.clone(),
            evidence.path().to_path_buf(),
        );

        let released = within(std::time::Duration::from_secs(120), move || {
            crate::core::lane_git::snapshot::set_timeout_for_tests(Some(
                std::time::Duration::from_secs(10),
            ));
            release_terminal_task_worktree(&owner, &path, &branch, &kept_in)
        });

        let opened = opens.stop();
        assert_eq!(opened, 0, "git read the planted configuration");
        let released = released.expect("the release did not return");
        assert!(
            released
                .as_ref()
                .is_err_and(|error| error.contains(&format!("{lane}/a")) && error.contains(&own)),
            "{released:?}"
        );
        assert!(checkout.join(lane).join("a/local.txt").exists());
    }

    // ── KT-2084 round 8: records Kronn owns, reuse, repair steps, bounded test mode ──

    /// A lane's record rewritten to name a folder of `.kronn/worktrees` that carries another
    /// registration's name: the prune removes a record only when its own `gitdir` names a gone
    /// checkout as Kronn or git writes it, never one found by a name.
    #[test]
    fn the_prune_removes_no_registration_a_rewritten_record_only_names() {
        let repo = make_test_repo("prune-by-name");
        let elsewhere = tempfile::tempdir().unwrap();
        let own = elsewhere.path().join("feature");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                own.to_str().unwrap(),
            ],
        );
        create_discussion_worktree(repo.path(), "proj", "a", "main").unwrap();
        let gone = a_canonical_worktree(repo.path(), "gone");
        fs::remove_dir_all(&gone).unwrap();
        let root =
            crate::core::lane_git::without_verbatim_prefix(repo.path().canonicalize().unwrap());
        fs::write(
            admin_of(repo.path(), "a").join("gitdir"),
            format!("{}/.kronn/worktrees/feature\n", root.display()),
        )
        .unwrap();
        let kept = repo.path().join(".git/worktrees/feature");

        let pruned = prune_kronn_worktrees(repo.path());

        assert!(
            kept.join("HEAD").is_file() && kept.join("gitdir").is_file(),
            "{pruned:?}: the registration of {} was removed",
            own.display()
        );
        assert!(
            admin_of(repo.path(), "a").join("HEAD").is_file(),
            "{pruned:?}"
        );
        // Its own gone checkout's record still goes.
        assert_eq!(pruned, Ok(1));
        assert!(!admin_of(repo.path(), "gone").exists());
    }

    /// Git registers a worktree under another name (`<folder>1`) when the folder's name is
    /// taken: Kronn undoes that creation and refuses it, and the holder's record is untouched.
    #[test]
    fn a_worktree_git_registers_under_another_name_is_refused_and_undone() {
        let repo = make_test_repo("taken-id");
        let elsewhere = tempfile::tempdir().unwrap();
        let own = elsewhere.path().join("proj--x");
        git(
            repo.path(),
            &["worktree", "add", "-q", "-b", "own", own.to_str().unwrap()],
        );
        let taken = repo.path().join(".git/worktrees/proj--x");
        let before = fs::read(taken.join("gitdir")).unwrap();

        let created = create_discussion_worktree(repo.path(), "proj", "x", "main");

        assert!(
            created.as_ref().is_err_and(
                |error| error.contains("`proj--x1`") && error.contains("git worktree remove")
            ),
            "{created:?}"
        );
        assert_eq!(
            fs::read(taken.join("gitdir")).unwrap(),
            before,
            "the record of {} was rewritten",
            own.display()
        );
        assert!(own.join("README.md").exists());
        assert!(!repo.path().join(".kronn/worktrees/proj--x").exists());
        assert!(!repo.path().join(".git/worktrees/proj--x1").exists());
        let branches = std::process::Command::new("git")
            .args(["branch", "--list", "kronn/x"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(branches.stdout.is_empty(), "the new branch was left");
    }

    /// A discussion reuses the checkout Kronn gave it, and no other worktree its branch is
    /// checked out in: that one is the user's own, or another lane's.
    #[test]
    fn a_discussion_reuses_its_own_checkout_and_no_other_worktree_on_its_branch() {
        let repo = make_test_repo("reuse-own-only");
        let own = repo.path().join("own-wt");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "kronn/my-topic",
                own.to_str().unwrap(),
            ],
        );

        let refused = create_discussion_worktree(repo.path(), "proj", "my-topic", "main");

        assert!(
            refused.as_ref().is_err_and(
                |error| error.contains("own-wt") && error.contains("git worktree remove")
            ),
            "{refused:?}"
        );
        let first = create_discussion_worktree(repo.path(), "proj", "mine", "main").unwrap();
        let again = create_discussion_worktree(repo.path(), "proj", "mine", "main").unwrap();
        assert!(same_path(Path::new(&first.path), Path::new(&again.path)));
        assert!(!again.is_main_repo);
    }

    /// Another lane's record that names this lane with a byte added, or grown past the cap, is
    /// not this lane's registration: the lane is still found, by its own record alone.
    #[test]
    fn a_padded_or_oversized_record_of_another_lane_is_not_this_lanes_registration() {
        for (case, padding) in [("space", " \n".to_string()), ("grown", "\n".repeat(8192))] {
            let repo = make_test_repo(&format!("seed-gitdir-{case}"));
            create_discussion_worktree(repo.path(), "proj", "a", "main").unwrap();
            let b = create_discussion_worktree(repo.path(), "proj", "b", "main").unwrap();
            let named = fs::read_to_string(admin_of(repo.path(), "b").join("gitdir")).unwrap();
            fs::write(
                admin_of(repo.path(), "a").join("gitdir"),
                format!("{}{padding}", named.trim_end()),
            )
            .unwrap();

            let found = registered_worktree_git_dir(repo.path(), Path::new(&b.path));

            assert!(
                found.as_ref().is_ok_and(|admin| admin.ends_with("proj--b")),
                "{case}: {found:?}"
            );
        }
    }

    /// Two registrations naming one worktree: the refusal names both, says which one the
    /// worktree uses, and gives the one-file step that repairs it, with no git command to
    /// paste. A panel action gets the same text.
    #[test]
    fn the_refusal_of_two_registrations_gives_the_command_that_repairs() {
        let (repo, a, b) = lane_a_naming_lane_b("dup-repair-step");
        let owner =
            crate::core::lane_git::without_verbatim_prefix(repo.path().canonicalize().unwrap());
        let lane_a = crate::core::lane_git::without_verbatim_prefix(
            Path::new(&a.path).canonicalize().unwrap(),
        );
        let stale = owner.join(".git/worktrees/proj--a/gitdir");
        let line = format!("{}/.git", lane_a.display());
        let step = format!(
            "replace the content of the file\n    {}\nwith this single line:\n    {line}\n",
            stale.display()
        );

        let refused = create_discussion_worktree(repo.path(), "proj", "c", "main").unwrap_err();
        let panel = crate::api::git_ops::run_git_push(Path::new(&b.path), &[])
            .expect_err("the panel's push is refused");

        for message in [&refused, &panel] {
            assert!(
                message.contains("proj--a") && message.contains("proj--b"),
                "{message}"
            );
            assert!(message.contains(&step), "{message}");
            // No git command to paste: one that repairs a record rewrites others too.
            assert!(
                !message.contains("worktree repair") && !message.contains("worktree prune"),
                "{message}"
            );
        }
        // The step it gives is the repair.
        fs::write(&stale, format!("{line}\n")).unwrap();
        let listed = registered_worktree_paths(repo.path());
        assert!(listed.is_ok(), "{listed:?}");
        let again = create_discussion_worktree(repo.path(), "proj", "c", "main");
        assert!(again.is_ok(), "{again:?}");
    }

    /// A reattach git refuses because another worktree holds the branch: the reason names that
    /// one registration and the command that removes it, whatever git's own words are.
    #[test]
    fn a_refused_reattach_names_the_one_registration_that_holds_the_branch() {
        let repo = make_test_repo("reattach-holder");
        let elsewhere = tempfile::tempdir().unwrap();
        let own = elsewhere.path().join("held");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "kronn/c",
                own.to_str().unwrap(),
            ],
        );
        let admin = Path::new("worktrees").join("held");

        let refused = reattach_worktree(repo.path(), "proj", "c", "kronn/c").unwrap_err();

        // A path no shell reads goes into the command, in single quotes.
        let removal = format!(
            "worktree remove '{}'`",
            own.canonicalize().unwrap().display()
        );
        assert!(
            refused.contains(&admin.display().to_string()) && refused.contains(&removal),
            "{refused}"
        );
        assert!(!refused.contains("worktree prune"), "{refused}");
        assert!(own.join("README.md").exists());

        // A path a shell would read is shown as data, with no command to paste.
        let odd = elsewhere.path().join("he$(touch x)`l\"d");
        fs::write(
            repo.path().join(".git/worktrees/held/gitdir"),
            format!("{}/.git\n", odd.display()),
        )
        .unwrap();
        let refused = reattach_worktree(repo.path(), "proj", "c", "kronn/c").unwrap_err();
        assert!(
            refused.contains(&format!("\n    {}\n", odd.display())),
            "{refused}"
        );
        assert!(
            !refused.contains("git -C") && !refused.contains("worktree remove"),
            "{refused}"
        );
    }

    /// A path with anything a shell reads never enters a command a message prints; shown as
    /// data, its control characters start no line of their own.
    #[test]
    fn a_path_a_shell_would_read_is_never_put_in_a_printed_command() {
        use crate::core::lane_git::{path_as_data, quoted_for_command};
        if cfg!(unix) {
            assert_eq!(
                quoted_for_command(Path::new("/srv/p-1/.kronn/worktrees/a_b")).as_deref(),
                Some("'/srv/p-1/.kronn/worktrees/a_b'")
            );
        }
        for odd in [
            "/r/$(touch x)",
            "/r/`id`",
            "/r/it's",
            "/r/a\"b",
            "/r/a b",
            "/r/a\nb",
            "/r/a;b",
            "/r/~a",
            "/r/../b",
            "relative/a",
        ] {
            assert_eq!(quoted_for_command(Path::new(odd)), None, "{odd}");
        }
        assert_eq!(
            path_as_data(Path::new("/r/a\nrm -rf $(touch x)`")),
            "\n    /r/a\\nrm -rf $(touch x)`\n"
        );
    }

    /// A sparse-checkout lane whose pattern file is a link or a FIFO: git would read through
    /// it, so no lane git runs there, and the refusal names the file at once.
    #[cfg(unix)] // FIFO made with libc::mkfifo, symlink made through std::os::unix
    #[test]
    fn a_sparse_pattern_file_that_is_a_link_or_a_fifo_refuses_the_lane() {
        for case in ["link", "fifo"] {
            let repo = make_test_repo(&format!("sparse-{case}"));
            fs::create_dir_all(repo.path().join("kept")).unwrap();
            fs::write(repo.path().join("kept/a.txt"), "a").unwrap();
            git(repo.path(), &["add", "."]);
            git(repo.path(), &["commit", "-qm", "a folder"]);
            git(repo.path(), &["sparse-checkout", "set", "kept"]);
            let lane = a_canonical_worktree(repo.path(), "sparse");
            assert!(crate::core::lane_git::git(&lane).is_ok());
            let patterns = git_dir_of(&lane).join("info/sparse-checkout");
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("secret"), "outside line\n").unwrap();
            let _ = fs::remove_file(&patterns);
            fs::create_dir_all(patterns.parent().unwrap()).unwrap();
            if case == "link" {
                std::os::unix::fs::symlink(outside.path().join("secret"), &patterns).unwrap();
            } else {
                fifo(&patterns);
            }
            let _unblock = Unblock(patterns.clone());

            let checked = within(std::time::Duration::from_secs(30), move || {
                crate::core::lane_git::git(&lane).map(|_| ())
            });

            let refused = checked.expect("the lane's check waited on the file");
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|error| error.contains("info/sparse-checkout")),
                "{case}: {refused:?}"
            );
        }
    }

    /// A project that keeps a submodule's repository in the submodule's own `.git` directory:
    /// Kronn does not read that copy, and the reason a task checkout is kept says so.
    #[test]
    fn a_kept_task_submodule_says_why_the_projects_own_copy_was_not_read() {
        let repo = make_test_repo("task-release-sub-dir");
        let upstream = add_materialized_test_submodule(repo.path());
        let pinned = commit_file(upstream.path(), "bump.txt", "v2", "upstream bump");
        let source = repo.path().join("deps/required");
        git(&source, &["fetch", "-q", "origin"]);
        git(&source, &["checkout", "-q", "--detach", &pinned]);
        git(repo.path(), &["commit", "-qam", "bump the submodule"]);
        let base = head_sha(repo.path());
        let task = create_task_worktree(repo.path(), "KT-2084", "sub-dir", &base).unwrap();
        let module = git_dir_of(&source);
        git(
            repo.path(),
            &[
                "config",
                "-f",
                module.join("config").to_str().unwrap(),
                "--unset",
                "core.worktree",
            ],
        );
        fs::remove_file(source.join(".git")).unwrap();
        fs::rename(&module, source.join(".git")).unwrap();
        let evidence = tempfile::tempdir().unwrap();

        let kept =
            release_terminal_task_worktree(repo.path(), &task.path, &task.branch, evidence.path());

        assert!(
            kept.as_ref()
                .is_err_and(|error| error.contains("deps/required")
                    && error.contains(&pinned)
                    && error.contains("its own .git directory")
                    && error.contains("submodule absorbgitdirs")),
            "{kept:?}"
        );
        assert!(Path::new(&task.path).join("deps/required").exists());
    }

    /// A project that uses `git sparse-checkout`: git copies its own sparse switches into
    /// each new worktree's `config.worktree`, and Kronn's git still runs and judges there.
    #[test]
    fn a_sparse_checkout_project_keeps_its_discussion_worktrees_usable() {
        let repo = make_test_repo("sparse-project");
        fs::create_dir_all(repo.path().join("kept")).unwrap();
        fs::create_dir_all(repo.path().join("left")).unwrap();
        fs::write(repo.path().join("kept/a.txt"), "a").unwrap();
        fs::write(repo.path().join("left/b.txt"), "b").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "two folders"]);
        git(repo.path(), &["sparse-checkout", "set", "kept"]);
        let lane = create_discussion_worktree(repo.path(), "proj", "sparse", "main").unwrap();
        let lane = Path::new(&lane.path);

        let status = crate::core::lane_git::git(lane).and_then(|mut git| {
            git.args(["status", "--porcelain"]);
            bounded(git, "status")
        });
        let judged = Snapshot::of_worktree(lane).and_then(|judged| judged_dirty_files(&judged));

        assert!(
            status.as_ref().is_ok_and(|output| output.status.success()),
            "{status:?}"
        );
        assert!(
            judged.as_ref().is_ok_and(Vec::is_empty),
            "{:?}",
            judged.err()
        );
    }

    /// A FIFO named `.gitignore` in a lane or in the project's checkout: the checks test mode
    /// runs first answer within git's time bound instead of waiting on it.
    #[cfg(unix)] // FIFO made with libc::mkfifo
    #[test]
    fn a_fifo_gitignore_keeps_the_test_mode_checks_within_their_bound() {
        let repo = make_test_repo("test-mode-fifo");
        let lane = a_canonical_worktree(repo.path(), "lane");
        let owner = repo.path().to_path_buf();
        let pipes = [lane.join(".gitignore"), owner.join(".gitignore")];
        for pipe in &pipes {
            fifo(pipe);
        }
        let _unblock = (Unblock(pipes[0].clone()), Unblock(pipes[1].clone()));

        let checked = within(std::time::Duration::from_secs(60), move || {
            crate::core::lane_git::snapshot::set_timeout_for_tests(Some(
                std::time::Duration::from_secs(2),
            ));
            (
                worktree_dirty_files(&lane).map(|files| files.len()),
                main_repo_state(&owner).map(|state| state.dirty_files.len()),
            )
        });

        let (lane_files, owner_state) = checked.expect("a test-mode check did not return");
        assert!(
            lane_files
                .as_ref()
                .is_err_and(|error| error.contains("timeout")),
            "{lane_files:?}"
        );
        assert!(
            owner_state
                .as_ref()
                .is_err_and(|error| error.contains("timeout")),
            "{owner_state:?}"
        );
    }
}
