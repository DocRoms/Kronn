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
    /// Bytes measured, a floor when `bytes_are_partial`.
    pub bytes: u64,
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
    pub bytes: u64,
    pub bytes_are_partial: bool,
    /// Bytes of the entries the default purge would remove.
    pub orphan_bytes: u64,
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
    /// Bytes / count of what the default purge would remove right now.
    pub orphan_bytes: u64,
    pub orphan_count: usize,
    /// Largest first.
    pub entries: Vec<DiskEntry>,
}

/// State, reason and purgeability of a lane build dir (pure, so the ownership
/// rules are tested without a filesystem).
pub fn lane_entry_state(
    state: &LaneTargetState,
    idle: Option<Duration>,
    min_idle: Duration,
) -> (DiskState, Option<String>, bool) {
    match state {
        LaneTargetState::Unmarked => (
            DiskState::Foreign,
            Some(format!(
                "no {} marker: Kronn did not create it and never removes it",
                worktree::LANE_OWNER_MARKER
            )),
            false,
        ),
        LaneTargetState::Claimed(_, reason) => (DiskState::Active, Some(reason.clone()), false),
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
    report_deadline: Instant,
) -> Vec<RawLane> {
    let judge = |owner: &worktree::LaneOwner| match claims.get(&owner.lane) {
        Some(reason) => Err(reason.clone()),
        None => Ok(()),
    };
    worktree::lane_target_dirs(repo)
        .into_iter()
        .map(|path| {
            let state = worktree::classify_lane_target(repo, &path, &judge);
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
    let claims = crate::api::orchestration::lane_target_claims(db, &project.id, repo).await?;
    let lanes = {
        let repo = repo.to_path_buf();
        tokio::task::spawn_blocking(move || scan_lanes(&repo, &claims, report_deadline))
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
        let (state, reason, purgeable) = lane_entry_state(&lane.state, lane.idle, min_idle);
        entries.push(DiskEntry {
            project_id: project.id.clone(),
            kind: DiskKind::LaneBuildDir,
            name: name_of(&lane.path),
            path: lane.path.to_string_lossy().to_string(),
            bytes: lane.bytes.bytes,
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
        let owner = owner.map(DiskOwner::from);
        let (cache_state, cache_reason) = liveness_state(&liveness);
        let terminal = liveness == ExecutionLiveness::Terminal;
        entries.push(DiskEntry {
            project_id: project.id.clone(),
            kind: DiskKind::Worktree,
            name: name_of(&raw.path),
            path: canonical.clone(),
            bytes: raw.sources.bytes,
            bytes_are_partial: raw.sources.partial,
            idle_seconds: raw.idle_seconds,
            owner: owner.clone(),
            // A checkout is removed by the flow that owns it (integration,
            // cancellation, KT-1197), never by a purge.
            state: if terminal {
                DiskState::Kept
            } else {
                cache_state
            },
            reason: if terminal {
                Some(
                    "finished; the worktree is removed by its integration or cancel flow, \
                     not by a purge"
                        .to_string(),
                )
            } else {
                cache_reason.clone()
            },
            purgeable: false,
        });
        if let Some((target, bytes)) = raw.cache {
            entries.push(DiskEntry {
                project_id: project.id.clone(),
                kind: DiskKind::Cache,
                name: format!("{}/target", name_of(&raw.path)),
                path: target.to_string_lossy().to_string(),
                bytes: bytes.bytes,
                bytes_are_partial: bytes.partial,
                idle_seconds: disk_usage::idle_seconds(&target),
                owner,
                state: cache_state,
                reason: cache_reason,
                purgeable: terminal,
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

/// Fold per-entry numbers into the per-kind totals shown above the list.
pub fn totals_by_kind(entries: &[DiskEntry]) -> Vec<DiskKindTotal> {
    DiskKind::ALL
        .iter()
        .map(|kind| {
            let of_kind: Vec<&DiskEntry> =
                entries.iter().filter(|entry| entry.kind == *kind).collect();
            DiskKindTotal {
                kind: *kind,
                entries: of_kind.len(),
                bytes: of_kind.iter().map(|entry| entry.bytes).sum(),
                bytes_are_partial: of_kind.iter().any(|entry| entry.bytes_are_partial),
                orphan_bytes: of_kind
                    .iter()
                    .filter(|entry| entry.state == DiskState::Orphan)
                    .map(|entry| entry.bytes)
                    .sum(),
            }
        })
        .collect()
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
        orphan_bytes: orphans.iter().map(|entry| entry.bytes).sum(),
        orphan_count: orphans.len(),
        entries,
    })
}

// ─── Purge ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PurgeItem {
    pub path: String,
    pub purged: bool,
    pub bytes: u64,
    pub bytes_are_partial: bool,
    /// Why an entry was refused.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PurgeResult {
    /// `orphans` (the default) or `entries` (named, confirmed).
    pub mode: &'static str,
    pub items: Vec<PurgeItem>,
    pub freed_bytes: u64,
    /// Lanes the sweep looked at and kept (live, recent, foreign...).
    pub kept: usize,
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
                    items.push(PurgeItem {
                        path: report.target_path.to_string_lossy().to_string(),
                        purged: true,
                        bytes: report.bytes_reclaimed,
                        bytes_are_partial: report.bytes_are_partial,
                        reason: None,
                    });
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
    let (items, kept) = sweep_orphans(db, &projects).await;
    Ok(finish_purge("orphans", items, kept, &projects))
}

fn finish_purge(
    mode: &'static str,
    items: Vec<PurgeItem>,
    kept: usize,
    projects: &[Project],
) -> PurgeResult {
    let available_bytes_after = projects
        .iter()
        .filter_map(|project| {
            disk_usage::available_bytes(&scanner::resolve_host_path(&project.path))
        })
        .min();
    PurgeResult {
        mode,
        freed_bytes: items
            .iter()
            .filter(|item| item.purged)
            .map(|item| item.bytes)
            .sum(),
        items,
        kept,
        available_bytes_after,
    }
}

fn refused(path: &str, reason: impl Into<String>) -> PurgeItem {
    PurgeItem {
        path: path.to_string(),
        purged: false,
        bytes: 0,
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
    Ok(finish_purge("entries", items, 0, &projects))
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
        return match crate::api::orchestration::reclaim_named_lane_targets(
            db,
            &project.id,
            repo,
            vec![path.to_path_buf()],
        )
        .await
        {
            Ok(outcomes) => match outcomes.into_iter().next().map(|(_, outcome)| outcome) {
                Some(worktree::LaneTargetOutcome::Removed(_, report)) => PurgeItem {
                    path: raw.to_string(),
                    purged: true,
                    bytes: report.bytes_reclaimed,
                    bytes_are_partial: report.bytes_are_partial,
                    reason: None,
                },
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
        };
    }
    // A cache: `<repo>/.kronn/worktrees/<name>/target`.
    let managed_root = repo.join(".kronn").join("worktrees");
    if name == "target" && path.parent().and_then(Path::parent) == Some(managed_root.as_path()) {
        let worktree_path = path.parent().expect("checked above").to_path_buf();
        let canonical = worktree_path.to_string_lossy().to_string();
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
            Ok(report) => PurgeItem {
                path: raw.to_string(),
                purged: true,
                bytes: report.bytes_reclaimed,
                bytes_are_partial: report.bytes_are_partial,
                reason: None,
            },
            Err(reason) => refused(raw, reason),
        };
    }
    refused(
        raw,
        "not a lane build dir or a worktree cache: worktrees are removed by their \
         integration or cancel flow, never by a purge",
    )
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
             first ({} removed, {} MiB) and it was not enough. Free space — the Disk panel \
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
        swept_bytes += items.iter().map(|item| item.bytes).sum::<u64>();
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

/// Sweep orphaned lane build dirs every [`PERIODIC_SWEEP_EVERY`]. The boot
/// reconciliation does the first pass; this one keeps the disk from refilling
/// between boots.
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
            let (items, _) = sweep_orphans(&db, &projects).await;
            if !items.is_empty() {
                tracing::info!(
                    removed = items.len(),
                    bytes = items.iter().map(|item| item.bytes).sum::<u64>(),
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
pub async fn get_disk_report(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<DiskQuery>,
) -> Json<ApiResponse<DiskReport>> {
    match build_disk_report(&state.db, query.project_id.as_deref()).await {
        Ok(report) => Json(ApiResponse::ok(report)),
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
/// with `confirm: true`. An active lane is never removed by either.
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
