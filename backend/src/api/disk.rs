//! KT-1318 — what Kronn keeps on the disk, who owns it, and how to take it back.
//!
//! One surface for four consumers (HTTP, the MCP tool, the Settings panel and
//! the launch guard):
//! - [`build_disk_report`]: free space, plus every artefact Kronn created
//!   (worktrees, lane build dirs, caches) with its size, age, owning task and
//!   whether a purge may take it;
//! - [`purge_orphans`]: the default purge — only lane build dirs that Kronn
//!   marked, whose execution is finished, with no live worktree, idle past
//!   `server.disk_sweep_idle_hours`;
//! - [`purge_entries`]: a confirmed, per-entry delete, each entry re-judged now;
//! - [`ensure_launch_headroom`]: the pressure guard — sweep first, then refuse.
//!
//! Nothing here decides ownership. That is `core::worktree` (the ownership
//! marker and the durable execution state); this module only asks it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::core::disk_usage::{self, DiskKind, DiskState, MeasureBudget};
use crate::core::worktree::{self, ExecutionLiveness, LaneSweepScope, LaneTargetState};
use crate::core::{scanner, task_worker_env::LANE_TARGET_PREFIX};
use crate::db::orchestration::DiskOwnerRow;
use crate::db::Database;
use crate::models::{ApiResponse, Project};
use crate::AppState;

/// One entry may spend this much walking its tree...
const ENTRY_MEASURE_TIME: Duration = Duration::from_millis(1500);
const ENTRY_MEASURE_MAX_ENTRIES: usize = 200_000;
/// ...and a whole report this much. A report is a screen to look at, not an
/// audit: what the budget did not cover is flagged `bytes_are_partial`.
const REPORT_MEASURE_TIME: Duration = Duration::from_secs(25);
/// How often the periodic sweep runs.
const PERIODIC_SWEEP_EVERY: Duration = Duration::from_secs(30 * 60);

// ─── Report ─────────────────────────────────────────────────────────────────

/// The task and state an artefact belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskOwner {
    /// `KT-<n>`, when the task is still known.
    pub task_ref: Option<String>,
    pub execution_id: String,
    pub status: String,
}

impl From<DiskOwnerRow> for DiskOwner {
    fn from(row: DiskOwnerRow) -> Self {
        Self {
            task_ref: row.task_ref,
            execution_id: row.execution_id,
            status: row.status,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskEntry {
    pub project_id: String,
    pub kind: DiskKind,
    pub path: String,
    /// Last path segment: the lane (`wt-…`) or worktree name.
    pub name: String,
    /// **Apparent** bytes: what the files add up to, which counts the blocks a
    /// copy-on-write clone shares with another tree in full. A floor when
    /// `bytes_are_partial`.
    pub bytes: u64,
    /// **Real** bytes (KT-1691): those no other file shares, i.e. what deleting
    /// the entry frees. `None` when the file system cannot say (anything but
    /// APFS), and then `bytes` is all there is.
    pub unique_bytes: Option<u64>,
    pub bytes_are_partial: bool,
    /// Seconds since the artefact was last written, when the filesystem says.
    pub idle_seconds: Option<u64>,
    pub owner: Option<DiskOwner>,
    pub state: DiskState,
    /// Why an entry is active, kept or foreign; what makes a reclaimable one
    /// not an orphan yet.
    pub reason: Option<String>,
    /// A confirmed per-entry delete may remove it (never an active, kept or
    /// foreign entry).
    pub purgeable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskKindTotal {
    pub kind: DiskKind,
    pub entries: usize,
    /// Apparent bytes (clone-shared blocks counted in full).
    pub bytes: u64,
    /// Real bytes: `None` as soon as one entry of the kind could not say.
    pub unique_bytes: Option<u64>,
    pub bytes_are_partial: bool,
    /// Bytes of the entries the default purge would remove (apparent).
    pub orphan_bytes: u64,
    /// The same, real where known, else apparent.
    pub orphan_unique_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskVolume {
    pub project_id: String,
    pub project_name: String,
    pub path: String,
    pub available_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskPolicy {
    pub launch_min_free_gib: u64,
    pub sweep_idle_hours: u64,
    /// KT-1691 — days a Blocked or Interrupted execution's build dir must be
    /// idle before it is offered for a purge.
    pub stalled_build_days: u64,
    pub warning_gib: u64,
    pub critical_gib: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskReport {
    /// The smallest free space among the projects' volumes.
    pub available_bytes: Option<u64>,
    /// Free space is under `launch_min_free_gib`: a launch would sweep, then
    /// refuse.
    pub under_pressure: bool,
    pub policy: DiskPolicy,
    pub volumes: Vec<DiskVolume>,
    pub totals: Vec<DiskKindTotal>,
    /// Everything listed, apparent (clone-shared blocks counted in full)...
    pub apparent_bytes: u64,
    /// ...and real (KT-1691): `None` when the file system cannot say.
    pub unique_bytes: Option<u64>,
    /// Bytes / count of what the default purge would remove right now.
    pub orphan_bytes: u64,
    /// Real bytes of those orphans where known, else apparent.
    pub orphan_unique_bytes: u64,
    pub orphan_count: usize,
    /// Largest first.
    pub entries: Vec<DiskEntry>,
    /// A bridge token's view (see [`scoped_for_bridge`]): the lanes and
    /// volumes are its own project's only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scoped: bool,
}

/// State, reason and purgeability of a lane build dir (pure, so the ownership
/// rules are tested without a filesystem).
///
/// `stalled_after` is `Some(threshold)` only when durable state says the lane's
/// execution is Blocked or Interrupted with nothing working for it (KT-1691): a
/// lane such an execution holds becomes purgeable once idle that long. Nothing
/// else about a claimed lane changes: a live execution's lane is never purgeable.
pub fn lane_entry_state(
    state: &LaneTargetState,
    idle: Option<Duration>,
    min_idle: Duration,
    stalled_after: Option<Duration>,
) -> (DiskState, Option<String>, bool) {
    match state {
        LaneTargetState::Unmarked => match idle {
            Some(idle) if idle >= min_idle => (
                DiskState::Foreign,
                Some(format!(
                    "no {} marker: Kronn did not create it, so no sweep ever removes it; only \
                     a confirmed delete here does, so be sure it is not yours",
                    worktree::LANE_OWNER_MARKER
                )),
                true,
            ),
            _ => (
                DiskState::Foreign,
                Some(format!(
                    "no {} marker: Kronn did not create it and never sweeps it; it was written \
                     within the last {} hour(s) or its age is unreadable, so it may be in use",
                    worktree::LANE_OWNER_MARKER,
                    min_idle.as_secs() / 3600
                )),
                false,
            ),
        },
        LaneTargetState::Claimed(_, reason) => match (stalled_after, idle) {
            (Some(after), Some(idle)) if idle >= after => (
                DiskState::Stalled,
                Some(format!(
                    "its execution is blocked or interrupted and nothing worked in this build \
                     dir for {} day(s): the worktree and branch stay, the next resume rebuilds",
                    idle.as_secs() / 86_400
                )),
                true,
            ),
            (Some(after), _) => (
                DiskState::Active,
                Some(format!(
                    "{reason}; becomes purgeable once its build dir has been idle for {} day(s)",
                    after.as_secs() / 86_400
                )),
                false,
            ),
            (None, _) => (DiskState::Active, Some(reason.clone()), false),
        },
        LaneTargetState::Kept(_, reason) => (DiskState::Kept, Some(reason.clone()), false),
        LaneTargetState::Reclaimable {
            worktree_present: true,
            ..
        } => (
            DiskState::Reclaimable,
            Some("its execution is finished and its clean worktree is kept".to_string()),
            true,
        ),
        LaneTargetState::Reclaimable {
            worktree_present: false,
            ..
        } => match idle {
            Some(idle) if idle >= min_idle => (DiskState::Orphan, None, true),
            Some(idle) => (
                DiskState::Reclaimable,
                Some(format!(
                    "written {} minute(s) ago, under the {}-hour idle threshold",
                    idle.as_secs() / 60,
                    min_idle.as_secs() / 3600
                )),
                true,
            ),
            None => (
                DiskState::Reclaimable,
                Some("its last write cannot be read, so it is not provably idle".to_string()),
                true,
            ),
        },
    }
}

/// State of a checkout (or the `target/` inside it) that no workspace row names
/// (KT-1691): nothing durable finishes it, so no release or sweep ever takes it.
/// Kept, and offered for a confirmed delete once nothing was written in it for
/// `min_idle`.
pub fn ownerless_entry_state(
    idle: Option<Duration>,
    min_idle: Duration,
) -> (DiskState, Option<String>, bool) {
    match idle {
        Some(idle) if idle >= min_idle => (
            DiskState::Kept,
            Some(
                "no workspace row names it, so nothing durable will ever release it: only a \
                 confirmed delete here does (a checkout with uncommitted work is refused, \
                 the branch stays)"
                    .to_string(),
            ),
            true,
        ),
        _ => (
            DiskState::Kept,
            Some(format!(
                "no workspace row names it; written within the last {} hour(s) or its age \
                 is unreadable, so it may be in use",
                min_idle.as_secs() / 3600
            )),
            false,
        ),
    }
}

/// State of a worktree-bound artefact from the durable liveness verdict.
fn liveness_state(liveness: &ExecutionLiveness) -> (DiskState, Option<String>) {
    match liveness {
        ExecutionLiveness::Terminal => (DiskState::Reclaimable, None),
        ExecutionLiveness::Active(reason) => (DiskState::Active, Some(reason.clone())),
        ExecutionLiveness::Unknown(reason) => (DiskState::Kept, Some(reason.clone())),
    }
}

/// A fresh budget for one entry, bounded by what the whole report has left.
fn entry_budget(report_deadline: Instant) -> MeasureBudget {
    let left = report_deadline.saturating_duration_since(Instant::now());
    MeasureBudget::within(ENTRY_MEASURE_MAX_ENTRIES, ENTRY_MEASURE_TIME.min(left))
}

struct RawLane {
    path: PathBuf,
    state: LaneTargetState,
    bytes: disk_usage::Measured,
    idle: Option<Duration>,
}

struct RawWorktree {
    path: PathBuf,
    sources: disk_usage::Measured,
    cache: Option<(PathBuf, disk_usage::Measured)>,
    idle_seconds: Option<u64>,
}

fn is_plain_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn scan_lanes(
    repo: &Path,
    claims: &std::collections::HashMap<String, String>,
    attributions: &worktree::LaneAttributions,
    report_deadline: Instant,
) -> Vec<RawLane> {
    let judge = |owner: &worktree::LaneOwner| match claims.get(&owner.lane) {
        Some(reason) => Err(reason.clone()),
        None => Ok(()),
    };
    worktree::lane_target_dirs(repo)
        .into_iter()
        .map(|path| {
            let state =
                worktree::classify_lane_target_attributed(repo, &path, &judge, attributions);
            let plain = is_plain_dir(&path);
            let bytes = if plain {
                disk_usage::measure_bounded(&path, &[], entry_budget(report_deadline))
            } else {
                disk_usage::Measured::default()
            };
            let idle = plain
                .then(|| worktree::lane_target_idle_for(&path))
                .flatten();
            RawLane {
                path,
                state,
                bytes,
                idle,
            }
        })
        .collect()
}

fn scan_worktrees(repo: &Path, report_deadline: Instant) -> Vec<RawWorktree> {
    let base = repo.join(".kronn").join("worktrees");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_plain_dir(path))
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|path| {
            let sources =
                disk_usage::measure_bounded(&path, &["target"], entry_budget(report_deadline));
            let target = path.join("target");
            let cache = is_plain_dir(&target).then(|| {
                let bytes =
                    disk_usage::measure_bounded(&target, &[], entry_budget(report_deadline));
                (target, bytes)
            });
            let idle_seconds = disk_usage::idle_seconds(&path);
            RawWorktree {
                path,
                sources,
                cache,
                idle_seconds,
            }
        })
        .collect()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// The idle period after which the build dir of `execution_id` is purgeable,
/// when durable state says the execution is Blocked or Interrupted and nothing
/// works for it; `None` for any other execution or when state cannot answer.
async fn stalled_window(db: &Database, execution_id: &str) -> Option<Duration> {
    let id = execution_id.to_string();
    let blocker = db
        .with_conn(move |conn| crate::db::orchestration::stalled_execution_blocker(conn, &id))
        .await;
    matches!(blocker, Ok(None)).then(disk_usage::stalled_build_idle)
}

/// Everything Kronn keeps on the disk for one project.
async fn project_entries(
    db: &Database,
    project: &Project,
    repo: &Path,
    report_deadline: Instant,
    min_idle: Duration,
) -> Result<Vec<DiskEntry>, String> {
    let mut entries = Vec::new();

    // Lane build dirs: ownership marker + durable claims + idle age.
    let (claims, attributions) =
        crate::api::orchestration::lane_target_claims(db, &project.id, repo).await?;
    let lanes = {
        let repo = repo.to_path_buf();
        tokio::task::spawn_blocking(move || {
            scan_lanes(&repo, &claims, &attributions, report_deadline)
        })
        .await
        .map_err(|error| format!("lane scan failed: {error}"))?
    };
    for lane in lanes {
        let owner_row = match &lane.state {
            LaneTargetState::Claimed(owner, _) | LaneTargetState::Reclaimable { owner, .. } => {
                Some(owner.clone())
            }
            LaneTargetState::Kept(owner, _) => owner.clone(),
            LaneTargetState::Unmarked => None,
        };
        let owner = match owner_row {
            Some(owner) => {
                let id = owner.execution_id.clone();
                db.with_conn(move |conn| {
                    crate::db::orchestration::disk_owner_of_execution(conn, &id)
                })
                .await
                .ok()
                .flatten()
                .map(DiskOwner::from)
                .or(Some(DiskOwner {
                    task_ref: None,
                    execution_id: owner.execution_id,
                    status: "unknown".to_string(),
                }))
            }
            None => None,
        };
        let stalled_after = match &lane.state {
            LaneTargetState::Claimed(owner, _) => stalled_window(db, &owner.execution_id).await,
            _ => None,
        };
        let (state, reason, purgeable) =
            lane_entry_state(&lane.state, lane.idle, min_idle, stalled_after);
        entries.push(DiskEntry {
            project_id: project.id.clone(),
            kind: DiskKind::LaneBuildDir,
            name: name_of(&lane.path),
            path: lane.path.to_string_lossy().to_string(),
            bytes: lane.bytes.bytes,
            unique_bytes: lane.bytes.unique_bytes,
            bytes_are_partial: lane.bytes.partial,
            idle_seconds: lane.idle.map(|idle| idle.as_secs()),
            owner,
            state,
            reason,
            purgeable,
        });
    }

    // Worktrees and the caches inside them.
    let worktrees = {
        let repo = repo.to_path_buf();
        tokio::task::spawn_blocking(move || scan_worktrees(&repo, report_deadline))
            .await
            .map_err(|error| format!("worktree scan failed: {error}"))?
    };
    for raw in worktrees {
        let canonical = raw.path.to_string_lossy().to_string();
        let (owner, liveness) = {
            let path = canonical.clone();
            db.with_conn(move |conn| {
                let owner = crate::db::orchestration::disk_owner_of_workspace(conn, &path)?;
                let liveness = crate::db::orchestration::worktree_cleanup_liveness(conn, &path)?;
                Ok((owner, liveness))
            })
            .await
            .unwrap_or_else(|error| {
                (
                    None,
                    ExecutionLiveness::Unknown(format!("durable state unreadable: {error}")),
                )
            })
        };
        let ownerless = {
            let path = canonical.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::workspace_row_names_path(conn, &path)
            })
            .await
            // Unreadable durable state is never "ownerless".
            .map(|named| !named)
            .unwrap_or(false)
        };
        let owner = owner.map(DiskOwner::from);
        let (mut cache_state, mut cache_reason) = liveness_state(&liveness);
        let terminal = liveness == ExecutionLiveness::Terminal;
        let (mut state, mut reason, mut purgeable) = if terminal {
            (
                DiskState::Kept,
                Some(
                    "finished; the worktree is removed by its integration or cancel flow, \
                     not by a purge"
                        .to_string(),
                ),
                false,
            )
        } else {
            (cache_state, cache_reason.clone(), false)
        };
        let mut cache_purgeable = terminal;
        if ownerless {
            (state, reason, purgeable) =
                ownerless_entry_state(raw.idle_seconds.map(Duration::from_secs), min_idle);
            let cache_idle = raw
                .cache
                .as_ref()
                .and_then(|(target, _)| disk_usage::idle_seconds(target))
                .map(Duration::from_secs);
            (cache_state, cache_reason, cache_purgeable) =
                ownerless_entry_state(cache_idle, min_idle);
        }
        entries.push(DiskEntry {
            project_id: project.id.clone(),
            kind: DiskKind::Worktree,
            name: name_of(&raw.path),
            path: canonical.clone(),
            bytes: raw.sources.bytes,
            unique_bytes: raw.sources.unique_bytes,
            bytes_are_partial: raw.sources.partial,
            idle_seconds: raw.idle_seconds,
            owner: owner.clone(),
            // A checkout with an owner is removed by the flow that owns it
            // (integration, cancellation, KT-1197), never by a purge.
            state,
            reason,
            purgeable,
        });
        if let Some((target, bytes)) = raw.cache {
            entries.push(DiskEntry {
                project_id: project.id.clone(),
                kind: DiskKind::Cache,
                name: format!("{}/target", name_of(&raw.path)),
                path: target.to_string_lossy().to_string(),
                bytes: bytes.bytes,
                unique_bytes: bytes.unique_bytes,
                bytes_are_partial: bytes.partial,
                idle_seconds: disk_usage::idle_seconds(&target),
                owner,
                state: cache_state,
                reason: cache_reason,
                purgeable: cache_purgeable,
            });
        }
    }
    Ok(entries)
}

fn thresholds_policy() -> DiskPolicy {
    let (launch_min_free_gib, sweep_idle_hours) = disk_usage::disk_policy();
    let (warning_gib, critical_gib) = worktree::configured_disk_thresholds();
    DiskPolicy {
        launch_min_free_gib,
        sweep_idle_hours,
        stalled_build_days: disk_usage::stalled_build_days(),
        warning_gib,
        critical_gib,
    }
}

async fn list_projects(db: &Database, only: Option<&str>) -> Result<Vec<Project>, String> {
    let mut projects = db
        .with_conn(crate::db::projects::list_projects)
        .await
        .map_err(|error| format!("projects unreadable: {error}"))?;
    if let Some(only) = only {
        projects.retain(|project| project.id == only);
        if projects.is_empty() {
            return Err(format!("project {only} not found"));
        }
    }
    Ok(projects)
}

/// Real bytes of a set of entries: `None` as soon as one could not say.
fn sum_unique<'a>(entries: impl IntoIterator<Item = &'a DiskEntry>) -> Option<u64> {
    entries
        .into_iter()
        .try_fold(0u64, |total, entry| Some(total + entry.unique_bytes?))
}

/// Fold per-entry numbers into the per-kind totals shown above the list.
pub fn totals_by_kind(entries: &[DiskEntry]) -> Vec<DiskKindTotal> {
    DiskKind::ALL
        .iter()
        .map(|kind| {
            let of_kind: Vec<&DiskEntry> =
                entries.iter().filter(|entry| entry.kind == *kind).collect();
            let orphans = of_kind
                .iter()
                .filter(|entry| entry.state == DiskState::Orphan);
            DiskKindTotal {
                kind: *kind,
                entries: of_kind.len(),
                bytes: of_kind.iter().map(|entry| entry.bytes).sum(),
                unique_bytes: sum_unique(of_kind.iter().copied()),
                bytes_are_partial: of_kind.iter().any(|entry| entry.bytes_are_partial),
                orphan_bytes: orphans.clone().map(|entry| entry.bytes).sum(),
                orphan_unique_bytes: orphans.map(|entry| real_bytes(entry)).sum(),
            }
        })
        .collect()
}

/// What deleting an entry frees: its unique bytes when the file system said,
/// else its apparent size (an upper bound).
fn real_bytes(entry: &DiskEntry) -> u64 {
    entry.unique_bytes.unwrap_or(entry.bytes)
}

/// The disk view: free space, then every Kronn artefact (largest first).
pub async fn build_disk_report(
    db: &Database,
    project_id: Option<&str>,
) -> Result<DiskReport, String> {
    let projects = list_projects(db, project_id).await?;
    let report_deadline = Instant::now() + REPORT_MEASURE_TIME;
    let min_idle = disk_usage::sweep_idle();
    let mut entries = Vec::new();
    let mut volumes = Vec::new();
    for project in &projects {
        let repo = scanner::resolve_host_path(&project.path);
        volumes.push(DiskVolume {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            path: repo.to_string_lossy().to_string(),
            available_bytes: disk_usage::available_bytes(&repo),
            total_bytes: disk_usage::total_bytes(&repo),
        });
        match project_entries(db, project, &repo, report_deadline, min_idle).await {
            Ok(found) => entries.extend(found),
            Err(error) => tracing::warn!(project = %project.id, "disk report: {error}"),
        }
    }
    entries.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let available_bytes = volumes
        .iter()
        .filter_map(|volume| volume.available_bytes)
        .min();
    let policy = thresholds_policy();
    let under_pressure = matches!(
        disk_usage::classify_launch_headroom(available_bytes, policy.launch_min_free_gib),
        disk_usage::LaunchHeadroom::Low { .. }
    );
    let orphans: Vec<&DiskEntry> = entries
        .iter()
        .filter(|entry| entry.state == DiskState::Orphan)
        .collect();
    Ok(DiskReport {
        available_bytes,
        under_pressure,
        policy,
        volumes,
        totals: totals_by_kind(&entries),
        apparent_bytes: entries.iter().map(|entry| entry.bytes).sum(),
        unique_bytes: sum_unique(&entries),
        orphan_bytes: orphans.iter().map(|entry| entry.bytes).sum(),
        orphan_unique_bytes: orphans.iter().map(|entry| real_bytes(entry)).sum(),
        orphan_count: orphans.len(),
        entries,
        scoped: false,
    })
}

/// What a Kronn-launched agent's bridge token may learn (KT-1347): free space,
/// the policy and the totals by kind stay (aggregates over every project), but
/// the volumes and the per-lane entries (paths, lane names, task refs) are the
/// token's own project's, or none when it has no project. The operator and the
/// UI keep the full report.
pub fn scoped_for_bridge(mut report: DiskReport, own_project: Option<&str>) -> DiskReport {
    report
        .volumes
        .retain(|volume| Some(volume.project_id.as_str()) == own_project);
    report
        .entries
        .retain(|entry| Some(entry.project_id.as_str()) == own_project);
    report.scoped = true;
    report
}

// ─── Purge ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PurgeItem {
    pub path: String,
    pub purged: bool,
    /// Apparent bytes removed (clone-shared blocks counted in full).
    pub bytes: u64,
    /// Real bytes removed (KT-1691): those no other file shared. `None` when
    /// the file system cannot say.
    pub unique_bytes: Option<u64>,
    pub bytes_are_partial: bool,
    /// Why an entry was refused.
    pub reason: Option<String>,
}

impl PurgeItem {
    /// What the delete really freed: the unique bytes when known, else the
    /// apparent ones (an upper bound).
    pub fn real_bytes(&self) -> u64 {
        self.unique_bytes.unwrap_or(self.bytes)
    }

    fn purged_from(path: &str, report: &worktree::CleanupReport) -> Self {
        Self {
            path: path.to_string(),
            purged: true,
            bytes: report.bytes_reclaimed,
            unique_bytes: report.unique_bytes,
            bytes_are_partial: report.bytes_are_partial,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PurgeResult {
    /// `orphans` (the default) or `entries` (named, confirmed).
    pub mode: &'static str,
    pub items: Vec<PurgeItem>,
    /// Apparent bytes removed.
    pub freed_bytes: u64,
    /// Real bytes removed (unique where known, else apparent): what the volume
    /// gained, since a clone's shared blocks stay with their other owner.
    pub freed_unique_bytes: u64,
    /// Lanes the sweep looked at and kept (live, recent, foreign...).
    pub kept: usize,
    /// The volume's free space (the smallest among the projects') before the
    /// purge and after it: the real gain is their difference.
    pub available_bytes_before: Option<u64>,
    pub available_bytes_after: Option<u64>,
}

/// Remove every orphan lane build dir of the given projects (all when `None`):
/// Kronn-marked, execution finished, no worktree, idle past the threshold.
pub async fn sweep_orphans(db: &Database, projects: &[Project]) -> (Vec<PurgeItem>, usize) {
    let mut items = Vec::new();
    let mut kept = 0usize;
    let min_idle = disk_usage::sweep_idle();
    for project in projects {
        let repo = scanner::resolve_host_path(&project.path);
        if !repo.join("target").is_dir() {
            continue;
        }
        match crate::api::orchestration::sweep_project_lane_targets_idle(
            db,
            &project.id,
            &repo,
            LaneSweepScope::WorktreeGone,
            min_idle,
        )
        .await
        {
            Ok(sweep) => {
                kept += sweep.would_remove.len()
                    + sweep.claimed.len()
                    + sweep.kept.len()
                    + sweep.unmarked.len();
                for (_, report) in sweep.removed {
                    items.push(PurgeItem::purged_from(
                        &report.target_path.to_string_lossy(),
                        &report,
                    ));
                }
            }
            Err(error) => tracing::warn!(project = %project.id, "orphan sweep: {error}"),
        }
    }
    (items, kept)
}

/// The default purge: orphans only.
pub async fn purge_orphans(db: &Database, project_id: Option<&str>) -> Result<PurgeResult, String> {
    let projects = list_projects(db, project_id).await?;
    let before = volumes_available(&projects);
    let (items, kept) = sweep_orphans(db, &projects).await;
    Ok(finish_purge("orphans", items, kept, &projects, before))
}

/// The smallest free space among the projects' volumes.
fn volumes_available(projects: &[Project]) -> Option<u64> {
    projects
        .iter()
        .filter_map(|project| {
            disk_usage::available_bytes(&scanner::resolve_host_path(&project.path))
        })
        .min()
}

fn finish_purge(
    mode: &'static str,
    items: Vec<PurgeItem>,
    kept: usize,
    projects: &[Project],
    available_bytes_before: Option<u64>,
) -> PurgeResult {
    let purged = || items.iter().filter(|item| item.purged);
    PurgeResult {
        mode,
        freed_bytes: purged().map(|item| item.bytes).sum(),
        freed_unique_bytes: purged().map(PurgeItem::real_bytes).sum(),
        kept,
        available_bytes_before,
        available_bytes_after: volumes_available(projects),
        items,
    }
}

fn refused(path: &str, reason: impl Into<String>) -> PurgeItem {
    PurgeItem {
        path: path.to_string(),
        purged: false,
        bytes: 0,
        unique_bytes: None,
        bytes_are_partial: false,
        reason: Some(reason.into()),
    }
}

/// Delete exactly the named entries. Each is re-judged now — the report that
/// listed it informs the human, it never authorises the deletion — and an
/// active, kept or foreign entry is refused whatever was asked.
pub async fn purge_entries(
    db: &Database,
    project_id: Option<&str>,
    paths: Vec<String>,
) -> Result<PurgeResult, String> {
    let projects = list_projects(db, project_id).await?;
    let before = volumes_available(&projects);
    let mut items = Vec::new();
    for raw in paths {
        let path = PathBuf::from(&raw);
        let owning = projects.iter().find(|project| {
            let repo = scanner::resolve_host_path(&project.path);
            path.starts_with(&repo)
        });
        let Some(project) = owning else {
            items.push(refused(
                &raw,
                "not inside any Kronn project: not Kronn's to delete",
            ));
            continue;
        };
        let repo = scanner::resolve_host_path(&project.path);
        items.push(purge_one(db, project, &repo, &path, &raw).await);
    }
    Ok(finish_purge("entries", items, 0, &projects, before))
}

async fn purge_one(
    db: &Database,
    project: &Project,
    repo: &Path,
    path: &Path,
    raw: &str,
) -> PurgeItem {
    let name = name_of(path);
    // A lane build dir: `<repo>/target/wt-<lane>`.
    if path.parent() == Some(repo.join("target").as_path()) && name.starts_with(LANE_TARGET_PREFIX)
    {
        return purge_lane(db, project, repo, path, raw).await;
    }
    let managed_root = repo.join(".kronn").join("worktrees");
    // A cache: `<repo>/.kronn/worktrees/<name>/target`.
    if name == "target" && path.parent().and_then(Path::parent) == Some(managed_root.as_path()) {
        let worktree_path = path.parent().expect("checked above").to_path_buf();
        let canonical = worktree_path.to_string_lossy().to_string();
        if let Some(item) = purge_if_ownerless(db, repo, &worktree_path, raw, false).await {
            return item;
        }
        let liveness = {
            let canonical = canonical.clone();
            db.with_conn(move |conn| {
                crate::db::orchestration::worktree_cleanup_liveness(conn, &canonical)
            })
            .await
        };
        let liveness = match liveness {
            Ok(liveness) => liveness,
            Err(error) => return refused(raw, format!("durable state unreadable: {error}")),
        };
        let outcome = worktree::clean_worktree_build_artifacts(repo, &worktree_path, liveness);
        let audited = match &outcome {
            Ok(report) => Ok((report.bytes_reclaimed, report.bytes_are_partial)),
            Err(reason) => Err(reason.clone()),
        };
        if let Err(error) = db
            .with_conn(move |conn| {
                crate::db::orchestration::record_artifact_reclaim(conn, &canonical, audited)
            })
            .await
        {
            tracing::warn!(target = %raw, "disk purge audit failed: {error}");
        }
        return match outcome {
            Ok(report) => PurgeItem::purged_from(raw, &report),
            Err(reason) => refused(raw, reason),
        };
    }
    // A checkout no workspace row names: `<repo>/.kronn/worktrees/<name>`.
    if path.parent() == Some(managed_root.as_path()) {
        if let Some(item) = purge_if_ownerless(db, repo, path, raw, true).await {
            return item;
        }
    }
    refused(
        raw,
        "not a lane build dir or a worktree cache: a worktree a task owns is removed by its \
         integration or cancel flow, never by a purge",
    )
}

/// Delete `worktree_path` (`whole`) or its `target/` when — and only when — no
/// workspace row names it and nothing was written in it for the idle threshold.
/// `None` when a row names it: its owner's flow judges it, not this purge.
async fn purge_if_ownerless(
    db: &Database,
    repo: &Path,
    worktree_path: &Path,
    raw: &str,
    whole: bool,
) -> Option<PurgeItem> {
    let canonical = worktree_path.to_string_lossy().to_string();
    let named = db
        .with_conn(move |conn| crate::db::orchestration::workspace_row_names_path(conn, &canonical))
        .await;
    match named {
        Ok(true) => return None,
        Ok(false) => {}
        Err(error) => return Some(refused(raw, format!("durable state unreadable: {error}"))),
    }
    let judged = if whole {
        worktree_path.to_path_buf()
    } else {
        worktree_path.join("target")
    };
    let min_idle = disk_usage::sweep_idle();
    let (_, reason, purgeable) = ownerless_entry_state(
        disk_usage::idle_seconds(&judged).map(Duration::from_secs),
        min_idle,
    );
    if !purgeable {
        return Some(refused(raw, reason.unwrap_or_default()));
    }
    let (repo, worktree_path) = (repo.to_path_buf(), worktree_path.to_path_buf());
    let removed = tokio::task::spawn_blocking(move || {
        if whole {
            worktree::remove_ownerless_worktree(&repo, &worktree_path)
        } else {
            worktree::remove_ownerless_worktree_cache(&repo, &worktree_path)
        }
    })
    .await
    .map_err(|error| format!("delete failed: {error}"))
    .and_then(|outcome| outcome);
    Some(match removed {
        Ok(report) => PurgeItem::purged_from(raw, &report),
        Err(reason) => refused(raw, reason),
    })
}

/// A lane build dir, re-judged now exactly as the report judges it: a finished
/// execution's goes through the sweep's own flow; a foreign one (no marker, no
/// claim) and a stalled one (Blocked or Interrupted, idle past
/// `server.disk_stalled_build_days`, nothing working for it) are the two this
/// purge adds, each only on the owner's confirmation. A live execution's lane is
/// refused whatever was asked.
async fn purge_lane(
    db: &Database,
    project: &Project,
    repo: &Path,
    path: &Path,
    raw: &str,
) -> PurgeItem {
    let (claims, attributions) =
        match crate::api::orchestration::lane_target_claims(db, &project.id, repo).await {
            Ok(found) => found,
            Err(error) => return refused(raw, error),
        };
    let state = {
        let (repo, path, claims) = (repo.to_path_buf(), path.to_path_buf(), claims.clone());
        tokio::task::spawn_blocking(move || {
            let judge = |owner: &worktree::LaneOwner| match claims.get(&owner.lane) {
                Some(reason) => Err(reason.clone()),
                None => Ok(()),
            };
            let state =
                worktree::classify_lane_target_attributed(&repo, &path, &judge, &attributions);
            let idle = worktree::lane_target_idle_for(&path);
            (state, idle)
        })
        .await
    };
    let (state, idle) = match state {
        Ok(found) => found,
        Err(error) => return refused(raw, format!("lane judgement failed: {error}")),
    };
    let stalled_after = match &state {
        LaneTargetState::Claimed(owner, _) => stalled_window(db, &owner.execution_id).await,
        _ => None,
    };
    let (entry_state, reason, purgeable) =
        lane_entry_state(&state, idle, disk_usage::sweep_idle(), stalled_after);
    let lane_name = name_of(path);
    let lane = lane_name
        .strip_prefix(LANE_TARGET_PREFIX)
        .unwrap_or(&lane_name);
    match (entry_state, &state) {
        // Finished executions' lanes: the sweep's own re-judgement decides.
        (DiskState::Orphan | DiskState::Reclaimable, _) => {
            reclaim_finished_lane(db, project, repo, path, raw).await
        }
        // No marker and nothing durable names it: never a sweep's, the owner's.
        (DiskState::Foreign, _) if purgeable && !claims.contains_key(lane) => {
            remove_lane_dir(repo, path, raw, None, db).await
        }
        (DiskState::Stalled, LaneTargetState::Claimed(owner, _)) if purgeable => {
            remove_lane_dir(repo, path, raw, Some(owner.execution_id.clone()), db).await
        }
        _ => refused(raw, reason.unwrap_or_else(|| "not removable".to_string())),
    }
}

/// Remove a lane build dir Kronn does not sweep (foreign or stalled), refusing a
/// running build, and record the removal on the execution that owned it.
async fn remove_lane_dir(
    repo: &Path,
    path: &Path,
    raw: &str,
    execution_id: Option<String>,
    db: &Database,
) -> PurgeItem {
    let (repo, target) = (repo.to_path_buf(), path.to_path_buf());
    let removed = tokio::task::spawn_blocking(move || worktree::remove_lane_target(&repo, &target))
        .await
        .map_err(|error| format!("delete failed: {error}"))
        .and_then(|outcome| outcome);
    let report = match removed {
        Ok(report) => report,
        Err(reason) => return refused(raw, reason),
    };
    if let Some(execution_id) = execution_id {
        let target = raw.to_string();
        let outcome = Ok((report.bytes_reclaimed, report.bytes_are_partial));
        if let Err(error) = db
            .with_conn(move |conn| {
                crate::db::orchestration::record_lane_target_reclaim_for_execution(
                    conn,
                    &execution_id,
                    &target,
                    outcome,
                )
            })
            .await
        {
            tracing::warn!(target = %raw, "disk purge audit failed: {error}");
        }
    }
    PurgeItem::purged_from(raw, &report)
}

async fn reclaim_finished_lane(
    db: &Database,
    project: &Project,
    repo: &Path,
    path: &Path,
    raw: &str,
) -> PurgeItem {
    match crate::api::orchestration::reclaim_named_lane_targets(
        db,
        &project.id,
        repo,
        vec![path.to_path_buf()],
    )
    .await
    {
        Ok(outcomes) => match outcomes.into_iter().next().map(|(_, outcome)| outcome) {
            Some(worktree::LaneTargetOutcome::Removed(_, report)) => {
                PurgeItem::purged_from(raw, &report)
            }
            Some(worktree::LaneTargetOutcome::Claimed(_, reason))
            | Some(worktree::LaneTargetOutcome::Kept(_, reason)) => refused(raw, reason),
            Some(worktree::LaneTargetOutcome::Unmarked) => refused(
                raw,
                format!(
                    "no {} marker: Kronn did not create it and never removes it",
                    worktree::LANE_OWNER_MARKER
                ),
            ),
            _ => refused(raw, "not removable"),
        },
        Err(error) => refused(raw, error),
    }
}

// ─── Launch guard ───────────────────────────────────────────────────────────

/// A launch the disk cannot take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskPressure {
    pub available_gib: u64,
    pub min_gib: u64,
    /// What the sweep that ran first gave back.
    pub swept_lanes: usize,
    pub swept_bytes: u64,
}

impl DiskPressure {
    /// The refusal, naming the number, the setting and what was already tried.
    pub fn reason(&self, what: &str) -> String {
        format!(
            "refusing to {what}: only {} GiB free (launch needs {} GiB, \
             server.disk_launch_min_free_gib). Kronn swept its orphaned lane build dirs \
             first ({} removed, {} MiB really freed) and it was not enough. Free space — the Disk panel \
             in Settings lists what Kronn holds — or lower the threshold, then retry.",
            self.available_gib,
            self.min_gib,
            self.swept_lanes,
            self.swept_bytes / (1024 * 1024)
        )
    }
}

/// Free bytes on the volume(s) a launch writes to: the repository's worktrees
/// and its `target/`.
pub fn launch_volume_available(repo: &Path) -> Option<u64> {
    [repo.join(".kronn").join("worktrees"), repo.join("target")]
        .iter()
        .filter_map(|path| disk_usage::available_bytes(path))
        .min()
        .or_else(|| disk_usage::available_bytes(repo))
}

/// The pressure guard of a worker launch or a full audit: below the configured
/// free space, sweep first; still below, refuse.
pub async fn ensure_launch_headroom(
    db: &Database,
    project_id: &str,
    repo: &Path,
) -> Result<(), DiskPressure> {
    let (min_gib, _) = disk_usage::disk_policy();
    ensure_launch_headroom_with(db, project_id, min_gib, &|| launch_volume_available(repo)).await
}

/// [`ensure_launch_headroom`] with the threshold and the free-space reading
/// injected, so both outcomes are testable on any disk.
pub async fn ensure_launch_headroom_with(
    db: &Database,
    project_id: &str,
    min_gib: u64,
    available: &(dyn Fn() -> Option<u64> + Send + Sync),
) -> Result<(), DiskPressure> {
    let low = |bytes: Option<u64>| match disk_usage::classify_launch_headroom(bytes, min_gib) {
        disk_usage::LaunchHeadroom::Ok => None,
        disk_usage::LaunchHeadroom::Low { available_gib, .. } => Some(available_gib),
    };
    let Some(before) = low(available()) else {
        return Ok(());
    };
    // Sweep first: the launching project's orphans, then everyone else's. Only
    // lanes Kronn marked, finished, with no worktree and idle past the
    // threshold are ever taken; an active lane is never in that set.
    let mut swept_lanes = 0usize;
    let mut swept_bytes = 0u64;
    let projects = list_projects(db, None).await.unwrap_or_default();
    let (own, others): (Vec<Project>, Vec<Project>) = projects
        .into_iter()
        .partition(|project| project.id == project_id);
    let mut still_low = Some(before);
    for group in [own, others] {
        if group.is_empty() {
            continue;
        }
        let (items, _) = sweep_orphans(db, &group).await;
        swept_lanes += items.len();
        // Real bytes: a lane cloned from the seed counts its shared blocks in
        // full in `bytes`, and deleting it frees only the ones it owns.
        swept_bytes += items.iter().map(PurgeItem::real_bytes).sum::<u64>();
        still_low = low(available());
        if still_low.is_none() {
            break;
        }
    }
    match still_low {
        None => {
            tracing::info!(
                swept_lanes,
                swept_bytes,
                "disk pressure relieved by sweeping orphaned lane build dirs"
            );
            Ok(())
        }
        Some(available_gib) => Err(DiskPressure {
            available_gib,
            min_gib,
            swept_lanes,
            swept_bytes,
        }),
    }
}

// ─── Periodic sweep ─────────────────────────────────────────────────────────

/// Release finished executions' checkouts and sweep orphaned lane build dirs
/// every [`PERIODIC_SWEEP_EVERY`]. The boot reconciliation does the first pass;
/// this one keeps the disk from refilling between boots.
pub fn spawn_periodic_sweep(db: std::sync::Arc<Database>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PERIODIC_SWEEP_EVERY);
        interval.tick().await;
        loop {
            interval.tick().await;
            let projects = match list_projects(&db, None).await {
                Ok(projects) => projects,
                Err(error) => {
                    tracing::warn!("periodic lane sweep: {error}");
                    continue;
                }
            };
            // KT-1397 — finished executions' checkouts first (a dirty or
            // Preserve-cancelled one is only ever released here, once idle),
            // so the lane sweep below sees their worktrees gone.
            crate::api::orchestration::release_finished_worktrees(&db).await;
            // KT-1645 — and the clean worktrees of archived reviews, which
            // are disposable and otherwise count against the worktree cap.
            let reviews = crate::api::disc_git::sweep_archived_review_worktrees(&db).await;
            if reviews > 0 {
                tracing::info!(reviews, "periodic sweep released archived review worktrees");
            }
            let (items, _) = sweep_orphans(&db, &projects).await;
            if !items.is_empty() {
                tracing::info!(
                    removed = items.len(),
                    bytes = items.iter().map(PurgeItem::real_bytes).sum::<u64>(),
                    "periodic sweep removed orphaned lane build dirs"
                );
            }
        }
    });
}

// ─── HTTP ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DiskQuery {
    #[serde(default)]
    pub project_id: Option<String>,
}

/// `GET /api/maintenance/disk[?project_id=…]` — free space and Kronn's usage.
/// A bridge token gets [`scoped_for_bridge`]'s view.
pub async fn get_disk_report(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    axum::extract::Query(query): axum::extract::Query<DiskQuery>,
) -> Json<ApiResponse<DiskReport>> {
    match build_disk_report(&state.db, query.project_id.as_deref()).await {
        Ok(report) => Json(ApiResponse::ok(match bridge {
            Some(axum::Extension(caller)) => scoped_for_bridge(report, caller.project.as_deref()),
            None => report,
        })),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

#[derive(Debug, Deserialize)]
pub struct PurgeRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    /// Empty (the default): purge orphans. Otherwise delete exactly these
    /// entries, which needs `confirm`.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub confirm: bool,
}

/// The purge, without the HTTP: no `paths` purges orphans; named `paths` need
/// `confirm`, so a stray request can never delete a named entry by accident.
pub async fn run_purge(db: &Database, request: PurgeRequest) -> Result<PurgeResult, String> {
    if request.paths.is_empty() {
        purge_orphans(db, request.project_id.as_deref()).await
    } else if !request.confirm {
        Err(
            "deleting named entries needs `confirm: true`; the default purge takes \
             orphans only"
                .to_string(),
        )
    } else {
        purge_entries(db, request.project_id.as_deref(), request.paths).await
    }
}

/// `POST /api/maintenance/disk/purge` — orphans by default; named entries only
/// with `confirm: true`. An active lane is never removed by either. Human / UI
/// only: the route is not on `BRIDGE_ROUTES`, so a bridge token is refused
/// before this runs (KT-1347).
pub async fn post_disk_purge(
    State(state): State<AppState>,
    Json(request): Json<PurgeRequest>,
) -> Json<ApiResponse<PurgeResult>> {
    match run_purge(&state.db, request).await {
        Ok(result) => Json(ApiResponse::ok(result)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

#[cfg(test)]
#[path = "disk_test.rs"]
mod disk_test;
