//! Git worktree management for discussion isolation.
//!
//! Each isolated discussion gets its own git worktree so agents can make
//! changes without interfering with the main working tree or other discussions.

use std::path::{Path, PathBuf};

use crate::core::task_worker_env::LANE_TARGET_PREFIX;

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
#[cfg(not(test))]
const SCAN_ENTRY_BUDGET: usize = 20_000;
/// Same walk, a budget a unit test can exceed without writing 20,000 files.
#[cfg(test)]
const SCAN_ENTRY_BUDGET: usize = 500;

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

/// Sum file sizes under `root`, stopping at `SCAN_ENTRY_BUDGET` entries.
///
/// Returns `(bytes, partial)`. Symlinks are counted as entries but never
/// followed, so a link into a huge tree cannot make this walk unbounded.
pub(crate) fn measure_within_budget(root: &Path) -> (u64, bool) {
    let mut bytes = 0u64;
    let mut visited = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > SCAN_ENTRY_BUDGET {
                return (bytes, true);
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                bytes += meta.len();
            }
        }
    }
    (bytes, false)
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

    let (bytes_reclaimed, bytes_are_partial) = measure_within_budget(&target_path);
    std::fs::remove_dir_all(&target_path).map_err(|error| {
        format!(
            "failed to clean {}: {error}. Nothing further was attempted; the target is \
             left as it stands.",
            target_path.display()
        )
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

    let (bytes_reclaimed, bytes_are_partial) = measure_within_budget(lane_target);
    std::fs::remove_dir_all(lane_target).map_err(|error| {
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
    std::fs::create_dir_all(&lane_target)
        .map_err(|error| format!("cannot create {}: {error}", lane_target.display()))?;
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
    if !ignore.exists() {
        std::fs::write(&ignore, b"*\n").map_err(|error| {
            format!(
                "cannot write the lane ignore file in {}: {error}",
                lane_target.display()
            )
        })?;
    }
    // Already stamped for this very execution: leave the file (and the lane's
    // idle clock) alone, so a relaunch does not look like a build.
    if read_lane_owner(&lane_target).as_ref() == Some(&owner) {
        return Ok(lane_target);
    }
    let bytes = serde_json::to_vec(&owner).map_err(|error| error.to_string())?;
    let staging = lane_target.join(format!("{LANE_OWNER_MARKER}.tmp"));
    std::fs::write(&staging, bytes)
        .and_then(|()| std::fs::rename(&staging, lane_target.join(LANE_OWNER_MARKER)))
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
fn read_lane_owner(lane_target: &Path) -> Option<LaneOwner> {
    let marker = lane_target.join(LANE_OWNER_MARKER);
    let meta = std::fs::symlink_metadata(&marker).ok()?;
    if !meta.is_file() || meta.len() > LANE_OWNER_MAX_BYTES {
        return None;
    }
    let owner: LaneOwner = serde_json::from_slice(&std::fs::read(&marker).ok()?).ok()?;
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
pub(crate) fn lane_target_dirs(repo_path: &Path) -> Vec<PathBuf> {
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
            if worktree_dirty_files(worktree)?.is_empty() {
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
/// written just now, never as idle (KT-1318).
pub fn lane_target_last_write(lane_target: &Path) -> Option<std::time::SystemTime> {
    const ENTRY_BUDGET: usize = 4_000;
    let modified = |path: &Path| std::fs::symlink_metadata(path).ok()?.modified().ok();
    let mut newest = modified(lane_target)?;
    let mut visited = 0usize;
    let Ok(children) = std::fs::read_dir(lane_target) else {
        return Some(newest);
    };
    for child in children.flatten() {
        visited += 1;
        if let Some(time) = child.metadata().ok().and_then(|meta| meta.modified().ok()) {
            newest = newest.max(time);
        }
        let is_dir = child.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_dir {
            continue;
        }
        let Ok(grandchildren) = std::fs::read_dir(child.path()) else {
            continue;
        };
        for grandchild in grandchildren.flatten() {
            visited += 1;
            if visited > ENTRY_BUDGET {
                return Some(newest);
            }
            if let Some(time) = grandchild
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
            {
                newest = newest.max(time);
            }
        }
    }
    Some(newest)
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
                let (bytes_reclaimed, bytes_are_partial) = measure_within_budget(lane_target);
                return LaneTargetOutcome::WouldRemove(
                    owner,
                    CleanupReport {
                        target_path: lane_target.to_path_buf(),
                        bytes_reclaimed,
                        bytes_are_partial,
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
        let Ok(file) = std::fs::File::open(&lock_path) else {
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
    let dot_git = worktree_path.join(".git");
    if dot_git.exists() {
        let content = format!("gitdir: {}.git/worktrees/{}", up, wt_name);
        if let Err(e) = std::fs::write(&dot_git, &content) {
            tracing::warn!("Failed to fix worktree .git file: {}", e);
        }
    }

    // 2. Fix <repo>/.git/worktrees/<name>/gitdir — point back to the worktree.
    //    Git resolves this file RELATIVE TO ITS OWN DIRECTORY (`<repo>/.git/worktrees/
    //    <name>/`), always 3 levels under the repo root — so climb 3, then descend the
    //    repo-root-relative path to the worktree's .git. A bare repo-root-relative path
    //    (no `../../../`) resolves under `.git/worktrees/<name>/` and git marks the
    //    worktree `prunable`, so `git worktree prune` can drop the admin entry (KT-331).
    let gitdir_file = repo_path
        .join(".git")
        .join("worktrees")
        .join(&wt_name)
        .join("gitdir");
    if gitdir_file.exists() {
        let content = format!("../../../{}/.git\n", rel_from_repo);
        if let Err(e) = std::fs::write(&gitdir_file, &content) {
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
fn same_path(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

fn branch_checked_out_at(repo_path: &Path, branch: &str) -> Option<PathBuf> {
    let output = crate::core::cmd::git_cmd()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current_path: Option<String> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.to_string());
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if b == branch {
                return current_path.map(PathBuf::from);
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
    None
}

/// Base directory for worktrees: `.kronn/worktrees/` inside the repo.
fn worktree_base_dir(repo_path: &Path) -> PathBuf {
    repo_path.join(".kronn/worktrees")
}

/// Ignore Kronn's machine-owned checkout directory without editing the user's
/// tracked `.gitignore`. Provisioning must not make the target dirty itself — a
/// dirty target is (correctly) refused later by the integration preflight.
pub(crate) fn ensure_local_git_exclude(repo_path: &Path, pattern: &str) -> Result<(), String> {
    // Already ignored (a committed rule, or a previous run): write nothing.
    if crate::core::cmd::git_cmd()
        .args([
            "check-ignore",
            "-q",
            "--no-index",
            pattern.trim_start_matches('/'),
        ])
        .current_dir(repo_path)
        .status()
        .is_ok_and(|status| status.success())
    {
        return Ok(());
    }
    let output = crate::core::cmd::git_cmd()
        .args(["rev-parse", "--git-path", "info/exclude"])
        .current_dir(repo_path)
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
    if std::fs::symlink_metadata(&exclude).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(format!(
            "{} is a symlink — refusing to update the local Git exclude",
            exclude.display()
        ));
    }
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    let parent = exclude
        .parent()
        .ok_or_else(|| format!("{} has no parent", exclude.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .map_err(|error| format!("cannot open {}: {error}", exclude.display()))?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file).map_err(|error| format!("cannot update {}: {error}", exclude.display()))?;
    }
    writeln!(file, "{pattern}")
        .map_err(|error| format!("cannot update {}: {error}", exclude.display()))?;
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
pub(crate) fn long_path(p: &Path) -> PathBuf {
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
    if let Some(existing_path) = branch_checked_out_at(repo_path, &branch) {
        if same_path(&existing_path, repo_path) {
            return Err(format!(
                "Branch {} is currently checked out in the main repo. Please switch to another branch before continuing.",
                branch
            ));
        }
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
    std::fs::create_dir_all(worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    // Ensure .kronn/worktrees/ is gitignored
    ensure_local_git_exclude(repo_path, ".kronn/")?;

    // Mark repo as safe directory (needed in Docker where mount owner differs)
    if crate::core::env::is_docker() {
        let _ = crate::core::cmd::git_cmd()
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &repo_path.to_string_lossy(),
            ])
            .output();
        let _ = crate::core::cmd::git_cmd()
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &worktree_path.to_string_lossy(),
            ])
            .output();
    }

    // Create the worktree with a new branch based on base_branch
    let output = crate::core::cmd::git_cmd()
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

    // Fix gitdir paths so the worktree works from the host too (not just inside Docker)
    fix_worktree_paths(repo_path, &worktree_path);

    copy_agent_configs(repo_path, &worktree_path);

    Ok(WorktreeInfo {
        path: worktree_path.to_string_lossy().to_string(),
        branch,
        is_main_repo: false,
    })
}

/// One `git worktree list --porcelain` entry.
struct WorktreeEntry {
    path: PathBuf,
    prunable: bool,
}

fn worktree_entries(repo_path: &Path) -> Vec<WorktreeEntry> {
    let Ok(output) = crate::core::cmd::git_cmd()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
    else {
        return Vec::new();
    };
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.push(WorktreeEntry {
                path: PathBuf::from(path),
                prunable: false,
            });
        } else if line == "prunable" || line.starts_with("prunable ") {
            if let Some(last) = entries.last_mut() {
                last.prunable = true;
            }
        }
    }
    entries
}

/// Strictly below `root`, whether git recorded the path as spelled or as
/// canonicalized (`/var` vs `/private/var` on macOS).
fn is_strictly_inside(path: &Path, root: &Path) -> bool {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    [root, canonical_root.as_path()].iter().any(|root| {
        [path, canonical_path.as_path()]
            .iter()
            .any(|path| path.starts_with(root) && path != root)
    })
}

fn force_remove_worktree(repo_path: &Path, path: &Path) -> bool {
    let removed = crate::core::cmd::git_cmd()
        .args(["worktree", "remove", "--force"])
        .arg(path)
        .current_dir(repo_path)
        .output()
        .is_ok_and(|output| output.status.success());
    if !removed && path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
    removed || !path.exists()
}

/// Removes the worktrees registered inside `parent`, deepest first. A workflow
/// step may add one under its run's worktree (`<run>/.kronn/pr-N`); deleting
/// the run's directory alone would leave its admin entry behind as prunable.
pub fn remove_nested_worktrees(repo_path: &Path, parent: &Path) -> usize {
    nested_worktrees(repo_path, parent)
        .iter()
        .filter(|path| force_remove_worktree(repo_path, path))
        .count()
}

/// The worktrees registered inside `parent`, deepest first.
pub fn nested_worktrees(repo_path: &Path, parent: &Path) -> Vec<PathBuf> {
    let mut nested: Vec<PathBuf> = worktree_entries(repo_path)
        .into_iter()
        .map(|entry| entry.path)
        .filter(|path| is_strictly_inside(path, parent))
        .collect();
    nested.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    nested
}

/// Drops the admin entries of Kronn's worktrees whose checkout is gone. Scoped
/// to `<repo>/.kronn/`: a blanket `git worktree prune` would also drop a user's
/// own worktree sitting on an unmounted drive.
pub fn prune_kronn_worktrees(repo_path: &Path) -> usize {
    let kronn_root = repo_path.join(".kronn");
    worktree_entries(repo_path)
        .into_iter()
        .filter(|entry| entry.prunable && is_strictly_inside(&entry.path, &kronn_root))
        .filter(|entry| force_remove_worktree(repo_path, &entry.path))
        .count()
}

/// The agents' project configs are gitignored, so a new worktree lacks them:
/// without `.mcp.json` a Claude agent gets no `--strict-mcp-config` and
/// inherits the host's personal MCP servers instead of the project's.
pub(crate) fn copy_agent_configs(repo_path: &Path, worktree_path: &Path) {
    const FILES: [&[&str]; 4] = [
        &[".mcp.json"],
        &[".vibe", "config.toml"],
        &[".kiro", "settings", "mcp.json"],
        &[".gemini", "settings.json"],
    ];
    for parts in FILES {
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
    if let Some(existing_path) = branch_checked_out_at(repo_path, existing_branch) {
        if same_path(&existing_path, repo_path) {
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

    std::fs::create_dir_all(worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    ensure_local_git_exclude(repo_path, ".kronn/")?;

    if crate::core::env::is_docker() {
        let _ = crate::core::cmd::git_cmd()
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &repo_path.to_string_lossy(),
            ])
            .output();
        let _ = crate::core::cmd::git_cmd()
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &worktree_path.to_string_lossy(),
            ])
            .output();
    }

    // Prune stale worktree entries first (old /data/workspaces/ refs)
    let _ = crate::core::cmd::git_cmd()
        .args(["worktree", "prune"])
        .current_dir(repo_path)
        .output();

    // Attach existing branch to new worktree path (no -b, branch already exists)
    let output = crate::core::cmd::git_cmd()
        .args(["worktree", "add"])
        .arg(&worktree_path)
        .arg(existing_branch)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree reattach failed: {}", stderr));
    }

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

/// Find the branch associated with a worktree path (before removal).
fn find_branch_for_worktree(repo_path: &Path, worktree_path: &str) -> Option<String> {
    let output = crate::core::cmd::git_cmd()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    // Extract the dir name for matching (git may list relative or absolute paths)
    let wt_dir_name = Path::new(worktree_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found = false;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found = path == worktree_path || path.ends_with(&wt_dir_name);
        } else if found && line.starts_with("branch refs/heads/") {
            return Some(line.trim_start_matches("branch refs/heads/").to_string());
        } else if found && line.is_empty() {
            found = false;
        }
    }
    None
}

/// Remove a worktree and optionally delete the branch.
///
/// `--force` is required, not a preference: a managed task worktree materializes
/// its submodules (KT-478), and Git refuses to remove a worktree that declares
/// submodules without it — `working trees containing submodules cannot be moved
/// or removed`. `--force` covers that on current Git; the manual directory
/// removal covers the rest.
///
/// The one thing this must never do is report success while the checkout is
/// still on disk. A worktree that silently survives its own reclamation is
/// exactly how the `.git/worktrees/*` count climbed unbounded until the sandbox
/// guard blocked every worker (KT-514). So the outcome is verified against the
/// filesystem before returning, and a residual checkout is a traced error rather
/// than a phantom success — with the branch left intact, since its worktree is
/// still live.
pub fn remove_discussion_worktree(
    repo_path: &Path,
    worktree_path: &str,
    delete_branch: bool,
) -> Result<(), String> {
    // Determine the branch name BEFORE removing the worktree (it won't be listed after)
    let branch_to_delete = if delete_branch {
        find_branch_for_worktree(repo_path, worktree_path)
    } else {
        None
    };

    // Remove the worktree via git (try absolute path, then relative)
    let wt_abs = Path::new(worktree_path);
    let wt_relative = wt_abs
        .strip_prefix(repo_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let output = crate::core::cmd::git_cmd()
        .args(["worktree", "remove", "--force", worktree_path])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree remove: {}", e))?;

    if !output.status.success() && !wt_relative.is_empty() {
        // Git may know the worktree by relative path (due to relative gitdir refs)
        let _ = crate::core::cmd::git_cmd()
            .args(["worktree", "remove", "--force", &wt_relative])
            .current_dir(repo_path)
            .output();
    }

    // Final fallback: remove the checkout directory by hand. Traced, never
    // silently discarded — an ignored failure here is precisely how the worktree
    // count rises unseen.
    let mut manual_error: Option<String> = None;
    if wt_abs.exists() {
        if let Err(e) = std::fs::remove_dir_all(wt_abs) {
            manual_error = Some(e.to_string());
        }
    }

    // Prune the admin entry (`.git/worktrees/<name>`) so a removed checkout stops
    // counting, whichever path above removed the directory.
    let _ = crate::core::cmd::git_cmd()
        .args(["worktree", "prune"])
        .current_dir(repo_path)
        .output();

    // Reclamation only counts if the count actually goes down. If the checkout
    // outlived every attempt, say so instead of reporting a phantom success, and
    // leave the branch alone.
    if wt_abs.exists() {
        return Err(format!(
            "worktree {} could not be removed{} — leaving it rather than reporting a phantom reclaim",
            worktree_path,
            manual_error
                .map(|e| format!(" ({e})"))
                .unwrap_or_default()
        ));
    }

    if let Some(branch) = branch_to_delete {
        let _ = crate::core::cmd::git_cmd()
            .args(["branch", "-D", &branch])
            .current_dir(repo_path)
            .output();
        tracing::info!("Deleted branch: {}", branch);
    }

    tracing::info!("Removed worktree: {}", worktree_path);
    Ok(())
}

// ── Task-execution worktree provisioning (KT-318) ─────────────────────────────
//
// The provisioning saga needs a stronger contract than `create_discussion_worktree`:
// a checkout pinned to an EXACT commit (not a mutable branch), a deterministic
// per-execution branch that never silently reuses a foreign/stale checkout, and a
// compensation that removes ONLY resources this attempt created — never work.

/// Read the current HEAD commit SHA of a git dir (repo or worktree).
fn git_head(dir: &Path) -> Result<String, String> {
    let out = crate::core::cmd::git_cmd()
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
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
    let out = crate::core::cmd::git_cmd()
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_path)
        .output()
        .ok()?;
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
    let out = crate::core::cmd::git_cmd()
        .args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git rev-parse failed: {e}"))?;
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
    let output = crate::core::cmd::git_cmd()
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(repo_path)
        .output()
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
    let head = crate::core::cmd::git_cmd()
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .current_dir(&path)
        .output()
        .map_err(|error| format!("cannot verify target checkout branch: {error}"))?;
    if !head.status.success()
        || String::from_utf8_lossy(&head.stdout).trim() != format!("refs/heads/{branch}")
    {
        return Err("integration target checkout changed branch; refusing to mutate it".into());
    }
    let common_dir = |directory: &Path| -> Result<PathBuf, String> {
        let output = crate::core::cmd::git_cmd()
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .current_dir(directory)
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
    reject_option_like_rev(base_sha)?;
    // The merge stops before its commit so the doc citations the two sides
    // shifted are settled into it (KT-1321) and the commit is made once.
    let out = crate::core::cmd::git_cmd()
        .args(["merge", "--no-edit", "--no-commit", base_sha])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| format!("git merge failed: {e}"))?;
    if !crate::core::docs_citations::merge::merge_in_progress(worktree_path) {
        // Fast-forwarded, already up to date, or refused before any merge state.
        if out.status.success() {
            return Ok(CandidateOutcome::Built {
                sha: git_head(worktree_path)?,
            });
        }
        return Ok(CandidateOutcome::Conflict { files: Vec::new() });
    }
    let conflicted_files = |path: &Path| {
        crate::core::cmd::git_cmd()
            .args(["diff", "--name-only", "--diff-filter=U"])
            .current_dir(path)
            .output()
            .ok()
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
        return Ok(CandidateOutcome::Conflict { files });
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
            SettleError::Refused { paths: files, .. } | SettleError::Unplaced { docs: files, .. },
        ) => {
            tracing::warn!("doc citations left to the worker: {files:?}");
            abort_merge(worktree_path);
            return Ok(CandidateOutcome::Conflict { files });
        }
    };
    // Repositories that enforce DCO reject an unsigned merge commit.
    let mut commit = crate::core::cmd::git_cmd();
    commit.args(["commit", "--signoff", "--cleanup=strip"]);
    match docs.as_ref().and_then(|docs| docs.summary()) {
        Some(note) => commit.args(["-m", &merge_subject(worktree_path, base_sha), "-m", &note]),
        None => commit.arg("--no-edit"),
    };
    let committed = commit
        .current_dir(worktree_path)
        .output()
        .map_err(|e| format!("git commit failed: {e}"))?;
    if !committed.status.success() {
        abort_merge(worktree_path);
        return Ok(CandidateOutcome::Conflict { files: Vec::new() });
    }
    Ok(CandidateOutcome::Built {
        sha: git_head(worktree_path)?,
    })
}

fn abort_merge(worktree_path: &Path) {
    let _ = crate::core::cmd::git_cmd()
        .args(["merge", "--abort"])
        .current_dir(worktree_path)
        .output();
}

/// The subject git prepared for the merge in progress.
fn merge_subject(worktree_path: &Path, base_sha: &str) -> String {
    let prepared = crate::core::cmd::git_cmd()
        .args(["rev-parse", "--git-path", "MERGE_MSG"])
        .current_dir(worktree_path)
        .output()
        .ok()
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
    let out = crate::core::cmd::git_cmd()
        .args(["merge", "--ff-only", candidate_sha])
        .current_dir(repo_path)
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
    let out = crate::core::cmd::git_cmd()
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo_path)
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
    let top_level = crate::core::cmd::git_cmd()
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
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
    if let Some(existing) = branch_checked_out_at(repo_path, &branch) {
        return Err(format!(
            "task branch {branch} is already checked out at {} — refusing to reuse it",
            existing.display()
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
    ensure_local_git_exclude(repo_path, ".kronn/")?;
    if crate::core::env::is_docker() {
        let _ = crate::core::cmd::git_cmd()
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &repo_path.to_string_lossy(),
            ])
            .output();
        let _ = crate::core::cmd::git_cmd()
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &worktree_path.to_string_lossy(),
            ])
            .output();
    }

    // Create from the EXACT pinned commit (a SHA, not a branch name).
    let output = crate::core::cmd::git_cmd()
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
    fix_worktree_paths(repo_path, &worktree_path);

    // Defense-in-depth: confirm HEAD landed on the pinned base. On mismatch,
    // undo our own just-created (unique-branch) resource so no orphan remains.
    if let Err(e) = verify_worktree_head(&worktree_path, base_sha) {
        let _ = remove_discussion_worktree(repo_path, &worktree_path.to_string_lossy(), true);
        return Err(e);
    }
    if let Err(e) = materialize_task_submodules(repo_path, &worktree_path) {
        let _ = remove_discussion_worktree(repo_path, &worktree_path.to_string_lossy(), true);
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
                let _ = crate::core::cmd::git_cmd()
                    .args(["worktree", "prune"])
                    .current_dir(repo_path)
                    .output();
                Ok(())
            }
            Some(sha) if sha == expected_base_sha => {
                let _ = crate::core::cmd::git_cmd()
                    .args(["worktree", "prune"])
                    .current_dir(repo_path)
                    .output();
                let _ = crate::core::cmd::git_cmd()
                    .args(["branch", "-D", expected_branch])
                    .current_dir(repo_path)
                    .output();
                Ok(())
            }
            Some(sha) => Err(format!(
                "task branch {expected_branch} advanced to {sha} (pinned {expected_base_sha}) but its worktree is gone — leaving it for manual/KT-320 cleanup"
            )),
        };
    }

    // Prove ownership before touching anything.
    let actual_branch = find_branch_for_worktree(repo_path, worktree_path).ok_or_else(|| {
        format!("cannot determine branch for worktree {worktree_path} — refusing blind removal")
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — refusing to remove a checkout we do not own"
        ));
    }
    let head = git_head(wt)?;
    if head != expected_base_sha {
        return Err(format!(
            "worktree {worktree_path} HEAD advanced to {head} (pinned {expected_base_sha}) — refusing to remove a checkout with work"
        ));
    }
    if !worktree_dirty_files(wt)?.is_empty() {
        return Err(format!(
            "worktree {worktree_path} has uncommitted changes — refusing to remove; leave the execution resumable"
        ));
    }

    // Ownership proven and tree clean → safe to remove worktree + its branch.
    remove_discussion_worktree(repo_path, worktree_path, true)
}

/// Remove the checkout of a cancelled/failed task while preserving its branch
/// for inspection. Ownership is proved by the exact branch and the tree must be
/// clean; unlike provisioning compensation, the branch may legitimately have
/// advanced beyond its base commit.
pub fn remove_cancelled_task_worktree(
    repo_path: &Path,
    worktree_path: &str,
    expected_branch: &str,
) -> Result<(), String> {
    reject_option_like_rev(expected_branch)?;
    let wt = Path::new(worktree_path);
    assert_managed_task_worktree_path(repo_path, wt)?;
    if !wt.exists() {
        let _ = crate::core::cmd::git_cmd()
            .args(["worktree", "prune"])
            .current_dir(repo_path)
            .output();
        return Ok(());
    }
    let actual_branch = find_branch_for_worktree(repo_path, worktree_path).ok_or_else(|| {
        format!("cannot determine branch for cancelled worktree {worktree_path} — preserving it")
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "cancelled worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — preserving it"
        ));
    }
    if !worktree_dirty_files(wt)?.is_empty() {
        return Err(format!(
            "cancelled worktree {worktree_path} has uncommitted changes — preserving it"
        ));
    }
    remove_discussion_worktree(repo_path, worktree_path, false)
}

/// What [`release_terminal_task_worktree`] did with the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalWorktreeRelease {
    /// The checkout was already gone; only the admin entry was pruned.
    AlreadyGone,
    /// The checkout was removed. `patch` is the saved uncommitted diff, when
    /// there was one.
    Removed { patch: Option<PathBuf> },
}

/// Name of the uncommitted-changes patch kept as evidence of a released worktree.
pub const RELEASED_WORKTREE_PATCH: &str = "uncommitted-changes.patch";

/// The most uncommitted state a release will turn into evidence. Untracked
/// build output that `.gitignore` does not cover would otherwise become a
/// multi-GB patch (and as many loose objects in the shared repository): above
/// this the checkout is kept and the reason recorded (KT-1397).
pub const RELEASED_WORKTREE_PATCH_CAP_BYTES: u64 = 256 * 1024 * 1024;

fn run_git_in_index(wt: &Path, index: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = crate::core::cmd::git_cmd()
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .current_dir(wt)
        .output()
        .map_err(|e| format!("git {} failed: {e}", args.join(" ")))?;
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
/// files of `wt` is known to exceed `cap`. Measured from file sizes before
/// anything is staged, so an oversized tree is refused without copying a byte.
fn uncommitted_content_over(wt: &Path, cap: u64) -> Result<Option<u64>, String> {
    let output = crate::core::cmd::git_cmd()
        .args([
            "ls-files",
            "-z",
            "--modified",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(wt)
        .output()
        .map_err(|e| format!("git ls-files failed: {e}"))?;
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

/// The uncommitted state of a checkout (tracked edits and untracked files) as a
/// binary-safe patch against `HEAD`, written to `out`, built in a scratch index
/// so the worktree's own index is never touched. The patch is streamed under
/// `cap`: past it nothing is left behind and the result is an error. `Ok(false)`
/// is "nothing to save".
fn write_uncommitted_patch(wt: &Path, index: &Path, out: &Path, cap: u64) -> Result<bool, String> {
    use std::io::Read;
    run_git_in_index(wt, index, &["read-tree", "HEAD"])?;
    run_git_in_index(wt, index, &["add", "-A"])?;
    let mut child = crate::core::cmd::git_cmd()
        .args(["diff", "--cached", "--binary", "--no-ext-diff", "HEAD"])
        .env("GIT_INDEX_FILE", index)
        .current_dir(wt)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("git diff failed: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("git diff has no output")?;
    let staging = out.with_extension("patch.tmp");
    let copied = std::fs::File::create(&staging)
        .and_then(|mut file| std::io::copy(&mut stdout.by_ref().take(cap + 1), &mut file));
    let abandon = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&staging);
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
    if !status.success() {
        let _ = std::fs::remove_file(&staging);
        return Err("git diff --cached failed".to_string());
    }
    if copied == 0 {
        let _ = std::fs::remove_file(&staging);
        return Ok(false);
    }
    std::fs::rename(&staging, out).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        format!("cannot save the patch to {}: {e}", out.display())
    })?;
    Ok(true)
}

/// Release the checkout of a task execution that has reached a terminal status
/// (KT-1397): the branch is kept, the uncommitted changes are saved as a patch
/// under `evidence_dir` first, and the checkout is then removed.
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
    reject_option_like_rev(expected_branch)?;
    let wt = Path::new(worktree_path);
    assert_managed_task_worktree_path(repo_path, wt)?;
    if !wt.exists() {
        let _ = crate::core::cmd::git_cmd()
            .args(["worktree", "prune"])
            .current_dir(repo_path)
            .output();
        return Ok(TerminalWorktreeRelease::AlreadyGone);
    }
    let actual_branch = find_branch_for_worktree(repo_path, worktree_path).ok_or_else(|| {
        format!("cannot determine branch for terminal worktree {worktree_path} — preserving it")
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "terminal worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — preserving it"
        ));
    }
    let mut patch = None;
    if !worktree_dirty_files(wt)?.is_empty() {
        if let Some(bytes) = uncommitted_content_over(wt, patch_cap_bytes)? {
            return Err(format!(
                "the uncommitted files of {worktree_path} hold {bytes}+ bytes, over the \
                 {patch_cap_bytes}-byte evidence cap — preserving it"
            ));
        }
        std::fs::create_dir_all(evidence_dir).map_err(|e| {
            format!(
                "cannot create {} for the uncommitted changes of {worktree_path}: {e} — preserving it",
                evidence_dir.display()
            )
        })?;
        let scratch_index = evidence_dir.join("patch-index.tmp");
        let out = evidence_dir.join(RELEASED_WORKTREE_PATCH);
        let saved = write_uncommitted_patch(wt, &scratch_index, &out, patch_cap_bytes);
        let _ = std::fs::remove_file(&scratch_index);
        let saved = saved.map_err(|e| {
            format!("cannot save the uncommitted changes of {worktree_path}: {e} — preserving it")
        })?;
        if saved {
            patch = Some(out);
        }
    }
    remove_discussion_worktree(repo_path, worktree_path, false)?;
    Ok(TerminalWorktreeRelease::Removed { patch })
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
                let _ = crate::core::cmd::git_cmd()
                    .args(["worktree", "prune"])
                    .current_dir(repo_path)
                    .output();
                Ok(())
            }
            Some(sha) if sha == integrated_sha => {
                let _ = crate::core::cmd::git_cmd()
                    .args(["worktree", "prune"])
                    .current_dir(repo_path)
                    .output();
                let output = crate::core::cmd::git_cmd()
                    .args(["branch", "-D", expected_branch])
                    .current_dir(repo_path)
                    .output()
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
        format!("cannot determine branch for integrated worktree {worktree_path} — preserving it")
    })?;
    if actual_branch != expected_branch {
        return Err(format!(
            "integrated worktree {worktree_path} is on branch {actual_branch}, expected {expected_branch} — preserving it"
        ));
    }
    let head = git_head(wt)?;
    if head != integrated_sha {
        return Err(format!(
            "integrated worktree {worktree_path} diverged to {head} (expected {integrated_sha}) — preserving it"
        ));
    }
    if !worktree_dirty_files(wt)?.is_empty() {
        return Err(format!(
            "integrated worktree {worktree_path} has uncommitted changes — preserving it"
        ));
    }
    remove_discussion_worktree(repo_path, worktree_path, true)
}

/// List all kronn worktrees for a project.
pub fn list_project_worktrees(repo_path: &Path) -> Vec<WorktreeInfo> {
    let output = match crate::core::cmd::git_cmd()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.to_string());
            current_branch = None;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        } else if line.is_empty() {
            if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take()) {
                if branch.starts_with("kronn/") {
                    worktrees.push(WorktreeInfo {
                        path,
                        branch,
                        is_main_repo: false,
                    });
                }
            }
        }
    }

    // Handle last entry (no trailing empty line)
    if let (Some(path), Some(branch)) = (current_path, current_branch) {
        if branch.starts_with("kronn/") {
            worktrees.push(WorktreeInfo {
                path,
                branch,
                is_main_repo: false,
            });
        }
    }

    worktrees
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

/// Parse `git status --porcelain=v1` output into a list of dirty files.
///
/// Porcelain v1 format: `XY <path>` — two-char status code, space, path.
/// Some status lines carry a rename (` -> `) — we keep the target side (the
/// actual file the user needs to be aware of) and discard the arrow.
fn parse_porcelain(stdout: &str) -> Vec<DirtyFile> {
    stdout
        .lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let status = line[..2].to_string();
            let rest = line[3..].trim();
            // Rename / copy lines look like "R  old -> new" — keep only `new`.
            // Rename / copy lines look like "R  old -> new" — keep only `new`.
            // `rsplit` iterates right-to-left, so `next()` gives the post-arrow
            // side; for plain lines it returns the whole thing.
            let path = rest.rsplit(" -> ").next().unwrap_or(rest).to_string();
            Some(DirtyFile { path, status })
        })
        .collect()
}

/// Check whether the given worktree has uncommitted changes (modified,
/// staged, or untracked).
///
/// Used as preflight #1 on `test-mode/enter`: if an agent left the worktree
/// dirty, unlocking it would lose the changes. We surface the list so the UI
/// can block with a "commit first" CTA rather than a blind refusal.
pub fn worktree_dirty_files(worktree_path: &Path) -> Result<Vec<DirtyFile>, String> {
    let output = crate::core::cmd::git_cmd()
        .args(["status", "--porcelain=v1"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| format!("git status failed in worktree: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git status failed in worktree ({}): {}",
            worktree_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
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
    let output = crate::core::cmd::git_cmd()
        .args([
            "diff",
            "--name-status",
            "--no-renames",
            "-z",
            base_rev,
            head_rev,
            "--",
        ])
        .current_dir(worktree_path)
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

/// `(sha, full message)` of every commit in `base_rev..head_rev`.
pub fn commit_messages(
    worktree_path: &Path,
    base_rev: &str,
    head_rev: &str,
) -> Result<Vec<(String, String)>, String> {
    commit_messages_excluding(worktree_path, base_rev, head_rev, None)
}

/// The commits of `base_rev..head_rev` that are not already reachable from
/// `exclude` (the integration target): a delivery that merged its target in
/// is judged on its own commits, not on the target's.
pub fn commit_messages_excluding(
    worktree_path: &Path,
    base_rev: &str,
    head_rev: &str,
    exclude: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    reject_option_like_rev(base_rev)?;
    reject_option_like_rev(head_rev)?;
    let range = format!("{base_rev}..{head_rev}");
    let mut args = vec!["log", "-z", "--format=%H%n%B", range.as_str()];
    let excluded = match exclude {
        Some(target) => {
            let branch = resolve_local_branch(worktree_path, target)?;
            Some(format!("refs/heads/{branch}"))
        }
        None => None,
    };
    if let Some(excluded) = excluded.as_deref() {
        args.extend(["--not", excluded]);
    }
    args.push("--");
    let output = crate::core::cmd::git_cmd()
        .args(&args)
        .current_dir(worktree_path)
        .output()
        .map_err(|e| format!("git log failed in worktree: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git log failed in worktree ({}): {}",
            worktree_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let record = record.trim_start();
            let (sha, message) = record.split_once('\n').unwrap_or((record, ""));
            (sha.trim().to_string(), message.to_string())
        })
        .collect())
}

/// Raw bytes of `path` as committed at `rev`.
pub fn file_at_revision(worktree_path: &Path, rev: &str, path: &str) -> Result<Vec<u8>, String> {
    reject_option_like_rev(rev)?;
    let output = crate::core::cmd::git_cmd()
        .args(["cat-file", "blob", &format!("{rev}:{path}")])
        .current_dir(worktree_path)
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
    let output = crate::core::cmd::git_cmd()
        .args(["var", "GIT_COMMITTER_IDENT"])
        .current_dir(worktree_path)
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
    let sym = crate::core::cmd::git_cmd()
        .args(["symbolic-ref", "-q", "--short", "HEAD"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git symbolic-ref failed: {}", e))?;
    let (current_branch, is_detached) = if sym.status.success() {
        (
            String::from_utf8_lossy(&sym.stdout).trim().to_string(),
            false,
        )
    } else {
        (String::new(), true)
    };

    let status = crate::core::cmd::git_cmd()
        .args(["status", "--porcelain=v1"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git status failed: {}", e))?;
    if !status.status.success() {
        return Err(format!(
            "git status failed in repo ({}): {}",
            repo_path.display(),
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    let dirty_files = parse_porcelain(&String::from_utf8_lossy(&status.stdout));
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
    let output = crate::core::cmd::git_cmd()
        .args(["checkout", branch])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git checkout failed: {}", e))?;
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
    let output = crate::core::cmd::git_cmd()
        .args(["stash", "push", "-u", "-m", message])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git stash push failed: {}", e))?;
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
    let list = crate::core::cmd::git_cmd()
        .args(["stash", "list"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git stash list failed: {}", e))?;
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

    let output = crate::core::cmd::git_cmd()
        .args(["stash", "pop", stash_ref])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git stash pop failed: {}", e))?;
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
        for i in 0..(SCAN_ENTRY_BUDGET + 50) {
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
    fn test_list_project_worktrees_no_repo() {
        // Should return empty vec for a non-repo path
        let result = list_project_worktrees(Path::new("/tmp"));
        // May or may not be empty depending on system, but should not panic
        let _ = result;
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

        let result = remove_discussion_worktree(repo.path(), &wt_path, false);
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

        remove_discussion_worktree(repo.path(), &info.path, false).unwrap();

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

        remove_discussion_worktree(repo.path(), &info.path, true).unwrap();

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

    #[test]
    fn test_reattach_worktree_after_remove() {
        let repo = make_test_repo("reattach");
        let info =
            create_discussion_worktree(repo.path(), "proj", "reattach-test", "main").unwrap();
        let branch = info.branch.clone();

        // Remove worktree but keep branch
        remove_discussion_worktree(repo.path(), &info.path, false).unwrap();
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
        let result = branch_checked_out_at(repo.path(), "main");
        assert!(result.is_some(), "main should be found as checked out");
        // Compare through same_path: git canonicalizes (`/private/var/…` on
        // macOS) while tempfile returns the symlinked spelling (`/var/…`).
        assert!(same_path(&result.unwrap(), repo.path()));
    }

    #[test]
    fn test_branch_checked_out_at_returns_none_for_nonexistent() {
        let repo = make_test_repo("checkout-none");
        let result = branch_checked_out_at(repo.path(), "kronn/does-not-exist");
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
        // Explicit \n join — a multi-line literal with `"\` would let the
        // line-continuation escape eat the leading space on the " M" line.
        let out = [
            " M src/lib.rs",
            "?? newfile.txt",
            "A  staged.rs",
            "R  old.rs -> new.rs",
        ]
        .join("\n");
        let files = parse_porcelain(&out);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].status, " M");
        assert_eq!(files[0].path, "src/lib.rs");
        assert_eq!(files[1].status, "??");
        assert_eq!(files[1].path, "newfile.txt");
        assert_eq!(files[3].status, "R ");
        // Rename: we keep the target filename (post-arrow).
        assert_eq!(files[3].path, "new.rs");
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

    #[cfg(unix)]
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
    /// A delivery that merged its integration target is judged on its own
    /// commits: a foreign trailer already on the target (another contributor's
    /// sign-off arriving with a main merge) is not the worker's.
    #[test]
    fn commits_already_on_the_target_are_left_out_of_a_delivery() {
        let repo = make_test_repo("delivery-trailers");
        let path = repo.path();
        let git = |args: &[&str]| {
            assert!(crate::core::cmd::git_cmd()
                .args(args)
                .current_dir(path)
                .status()
                .unwrap()
                .success());
        };
        let base = head_sha(path);
        make_branch(path, "target");
        git(&["checkout", "-q", "target"]);
        let foreign = commit_file(
            path,
            "upstream.txt",
            "x",
            "fix: upstream\n\nSigned-off-by: Someone Else <else@example.com>",
        );
        git(&["checkout", "-q", "-b", "task", &base]);
        let own = commit_file(path, "worker.txt", "y", "feat: worker change");
        git(&["merge", "-q", "--no-ff", "-m", "merge target", "target"]);

        let all = commit_messages(path, &base, "HEAD").unwrap();
        assert!(all.iter().any(|(sha, _)| *sha == foreign), "{all:?}");

        let delivered = commit_messages_excluding(path, &base, "HEAD", Some("target")).unwrap();
        let shas: Vec<&str> = delivered.iter().map(|(sha, _)| sha.as_str()).collect();
        assert!(!shas.contains(&foreign.as_str()), "{delivered:?}");
        assert!(shas.contains(&own.as_str()), "{delivered:?}");
        assert!(
            delivered
                .iter()
                .all(|(_, message)| !message.contains("Someone Else")),
            "{delivered:?}"
        );
    }

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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
        let error =
            remove_cancelled_task_worktree(repo.path(), &external.path().to_string_lossy(), "main")
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
        let error =
            remove_cancelled_task_worktree(new_repo.path(), &info.path, &info.branch).unwrap_err();
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

        remove_cancelled_task_worktree(repo.path(), &info.path, &info.branch).unwrap();
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

        assert!(remove_cancelled_task_worktree(repo.path(), &info.path, &info.branch).is_err());
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

        let TerminalWorktreeRelease::Removed { patch: Some(patch) } = released else {
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
            TerminalWorktreeRelease::Removed { patch: Some(_) }
        ));
        assert!(!Path::new(&info.path).exists());
    }

    /// KT-1397 — the patch itself is streamed under the cap: a tracked edit
    /// whose diff outgrows it (the size pre-check passed) leaves no partial file.
    #[test]
    fn write_uncommitted_patch_stops_at_the_cap_and_leaves_nothing() {
        let repo = make_test_repo("task-patch-cap");
        fs::write(repo.path().join("big.txt"), "x".repeat(8192)).unwrap();
        let out = repo.path().join("out.patch");
        let index = repo.path().join("scratch-index");

        let error = write_uncommitted_patch(repo.path(), &index, &out, 1024).unwrap_err();

        assert!(error.contains("cap"), "{error}");
        assert!(!out.exists());
        assert!(!out.with_extension("patch.tmp").exists());
        let _ = fs::remove_file(&index);
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

        remove_cancelled_task_worktree(repo.path(), &info.path, &info.branch).unwrap();

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
        remove_discussion_worktree(repo.path(), &info.path, false).unwrap();
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

    /// Phase 1 brings the parent INTO the child, so the child ends up descending
    /// from the pinned tip — the precondition that lets phase 2 be a fast-forward.
    #[test]
    fn building_the_candidate_makes_the_child_descend_from_the_parent() {
        let repo = make_test_repo("candidate");
        let base = commit_file(repo.path(), "shared.txt", "one", "base");

        // A child branch that diverges on its own file.
        std::process::Command::new("git")
            .args(["checkout", "-b", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        commit_file(repo.path(), "child.txt", "work", "child work");

        // Meanwhile the parent moves on, untouched by the child.
        std::process::Command::new("git")
            .args(["checkout", "main"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let parent_tip = commit_file(repo.path(), "parent.txt", "moved", "parent moves");
        assert_ne!(parent_tip, base);

        std::process::Command::new("git")
            .args(["checkout", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let outcome = build_candidate(repo.path(), &parent_tip).unwrap();
        let CandidateOutcome::Built { sha } = outcome else {
            panic!("expected a clean candidate");
        };

        // The candidate contains the parent tip, so the parent can fast-forward to it.
        let merge_base = crate::core::cmd::git_cmd()
            .args(["merge-base", "--is-ancestor", &parent_tip, &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            merge_base.status.success(),
            "candidate must descend from the parent tip"
        );

        let message = crate::core::cmd::git_cmd()
            .args(["log", "-1", "--format=%B", &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&message.stdout).contains("Signed-off-by: "),
            "the merge commit must carry a sign-off"
        );
    }

    /// A repository that enforces the DCO rejects an unsigned merge commit at
    /// `commit-msg` time (KT-854: the merge Kronn made for KT-840 had no
    /// `Signed-off-by`, so the release PR went red). The integration candidate must
    /// be built, signed with the identity git is configured with.
    #[cfg(unix)]
    #[test]
    fn the_integration_merge_commit_satisfies_a_dco_enforcing_repository() {
        use std::os::unix::fs::PermissionsExt;

        let repo = make_test_repo("candidate-dco");
        commit_file(repo.path(), "shared.txt", "one", "base");
        std::process::Command::new("git")
            .args(["checkout", "-b", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        commit_file(repo.path(), "child.txt", "work", "child work");
        std::process::Command::new("git")
            .args(["checkout", "main"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let parent_tip = commit_file(repo.path(), "parent.txt", "moved", "parent moves");
        std::process::Command::new("git")
            .args(["checkout", "child"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        // `git merge` runs `commit-msg` for the merge commit it creates.
        let hook = repo.path().join(".git/hooks/commit-msg");
        fs::write(
            &hook,
            "#!/bin/sh\ngrep -q '^Signed-off-by: ' \"$1\" || { echo 'DCO: missing Signed-off-by' >&2; exit 1; }\n",
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let outcome = build_candidate(repo.path(), &parent_tip).unwrap();
        let CandidateOutcome::Built { sha } = outcome else {
            panic!("a DCO-enforcing repository must accept the integration merge, got {outcome:?}");
        };

        let message = crate::core::cmd::git_cmd()
            .args(["log", "-1", "--format=%B", &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&message.stdout)
                .contains("Signed-off-by: Test <test@test.com>"),
            "the sign-off is the configured git identity"
        );
        let parents = crate::core::cmd::git_cmd()
            .args(["log", "-1", "--format=%P", &sha])
            .current_dir(repo.path())
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
}
