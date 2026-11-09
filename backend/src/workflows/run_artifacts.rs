//! Per-run artifacts directory (KT-910), exposed as `{{run.artifacts_dir}}`.
//!
//! Exec steps write there (screenshots, logs, large reports); an Agent step
//! whose prompt names it reads it read-only through the `read_only_repos`
//! policy (`--add-dir`, `Edit` denied, mandatory sandbox). It lives outside
//! every checkout, so it never overlaps the agent's writable directory.
//!
//! Retention: the directory lives as long as its run can still execute. It
//! is removed when the run ends (any terminal status, the same moment its
//! worktree goes), and at startup for a run that ended, no longer exists, or
//! stayed Interrupted beyond `interrupted_worktree_ttl_days`.
//!
//! Location (KT-1585): never inside the Kronn data directory, which the vault
//! protection hides from every agent process (KT-1495) whatever `--add-dir`
//! says. Under [`ROOT_ENV`] when set, else the platform cache directory, and
//! always in a directory of Kronn's own there ([`resolve_root`]).

use std::path::{Path, PathBuf};

use crate::core::task_worker_sandbox::real_path;
use crate::models::RunStatus;

pub(crate) use crate::core::rooted_fd as fd;

/// Where versions before KT-1585 kept the run directories, in the data dir.
const LEGACY_DIR_NAME: &str = "run-artifacts";

/// Kronn's own directory under the chosen base: the sweep only ever lists
/// what Kronn created, never a directory a person pointed it at.
const OWN_DIR: &str = "kronn-run-artifacts";

/// A copy in progress during the migration; never read as a run.
const MIGRATING_PREFIX: &str = ".migrating-";

/// Where an operator puts the run directories; the Docker image sets it to a
/// volume of its own.
pub const ROOT_ENV: &str = "KRONN_RUN_ARTIFACTS_DIR";

/// Where every run's directory lives.
pub fn root() -> Option<PathBuf> {
    #[cfg(test)]
    {
        let data = crate::core::config::config_dir().ok();
        let cache = std::env::temp_dir().join(format!("kronn-test-cache-{}", std::process::id()));
        let protected = crate::core::trust_sandbox::protected_dirs();
        resolve_root(None, data.as_deref(), Some(&cache), &protected).0
    }
    #[cfg(not(test))]
    {
        static ROOT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
        ROOT.get_or_init(|| {
            let configured = crate::core::child_env::var(ROOT_ENV).ok();
            let data = crate::core::config::config_dir().ok();
            let cache = directories::ProjectDirs::from("com", "kronn", "kronn")
                .map(|dirs| dirs.cache_dir().to_path_buf());
            let protected = crate::core::trust_sandbox::protected_dirs();
            let (root, warnings) = resolve_root(
                configured.as_deref(),
                data.as_deref(),
                cache.as_deref(),
                &protected,
            );
            for warning in warnings {
                tracing::warn!("{warning}");
            }
            tracing::info!(root = ?root, "run artifacts root");
            root
        })
        .clone()
    }
}

/// The root for a data directory: `<base>/kronn-run-artifacts/<key>`, where
/// the base is `configured` when an agent can read it, else `cache_dir`, and
/// the key comes from the data directory so two instances never sweep each
/// other's runs. Pure; the warnings say what was refused and why.
pub fn resolve_root(
    configured: Option<&str>,
    data_dir: Option<&Path>,
    cache_dir: Option<&Path>,
    protected: &[PathBuf],
) -> (Option<PathBuf>, Vec<String>) {
    let mut warnings = Vec::new();
    let Some(data_dir) = data_dir else {
        warnings.push("no data directory: runs get no artifacts directory".to_string());
        return (None, warnings);
    };
    let key =
        crate::core::repository_resources::sha256(real_path(data_dir).to_string_lossy().as_bytes());
    let under = |base: &Path| base.join(OWN_DIR).join(&key[..12]);
    let configured = configured
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    if let Some(base) = configured {
        match refusal(&base, protected) {
            None => return (Some(under(&base)), warnings),
            Some(why) => warnings.push(format!(
                "{ROOT_ENV}={} {why}; the default run artifacts directory is used",
                base.display()
            )),
        }
    }
    let Some(base) = cache_dir else {
        warnings.push(format!(
            "no cache directory: runs get no artifacts directory; set {ROOT_ENV}"
        ));
        return (None, warnings);
    };
    match refusal(base, protected) {
        None => (Some(under(base)), warnings),
        Some(why) => {
            warnings.push(format!(
                "the default run artifacts directory {} {why}: runs get no artifacts \
                 directory; set {ROOT_ENV}",
                base.display()
            ));
            (None, warnings)
        }
    }
}

/// Why `base` cannot hold run directories: relative, inside a directory
/// hidden from agents, or containing one (the sweep must never reach it).
fn refusal(base: &Path, protected: &[PathBuf]) -> Option<String> {
    if !base.is_absolute() {
        return Some("is not absolute".to_string());
    }
    let real = real_path(base);
    protected.iter().find_map(|closed| {
        let closed_real = real_path(closed);
        let overlaps = [base, real.as_path()].into_iter().any(|dir| {
            [closed.as_path(), closed_real.as_path()]
                .into_iter()
                .any(|closed| dir.starts_with(closed) || closed.starts_with(dir))
        });
        overlaps.then(|| format!("overlaps {}, which agents cannot read", closed.display()))
    })
}

/// A run id names a directory only when it cannot leave the root.
fn safe_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What the sweep and the migration may touch: a directory named exactly as
/// Kronn names a run (a hyphenated lowercase UUID).
fn is_run_dir_name(name: &str) -> bool {
    uuid::Uuid::parse_str(name).is_ok_and(|id| id.hyphenated().to_string() == name)
}

pub fn dir_for(run_id: &str) -> Option<PathBuf> {
    safe_run_id(run_id).then(|| root().map(|root| root.join(run_id)))?
}

/// The root as a directory fd (`core::rooted_fd`): no part is a link and it
/// is outside every protected directory, else refused and logged, so nothing
/// is read, created or removed. `Ok(None)`: not created yet.
fn open_root(root: &Path, create: bool) -> std::io::Result<Option<fd::Dir>> {
    let parts = (|| {
        let key = root.file_name()?.to_str()?;
        let own = root.parent()?;
        Some((own.parent()?, own.file_name()?.to_str()?, key))
    })();
    let Some((base, own, key)) = parts else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "run artifacts root must be <base>/<own>/<key>",
        ));
    };
    let protected = crate::core::trust_sandbox::protected_dirs();
    let opened = fd::Dir::open_under(base, &[own, key], create).and_then(|dir| {
        if let Some(dir) = &dir {
            dir.check_outside(&protected)?;
        }
        Ok(dir)
    });
    if let Err(error) = &opened {
        tracing::error!(
            root = %root.display(),
            "run artifacts root refused, nothing is read, created or removed: {error}"
        );
    }
    opened
}

/// Creates the run's directory (owner-only) and returns its canonical path.
pub fn ensure(run_id: &str) -> std::io::Result<PathBuf> {
    let root = root().filter(|_| safe_run_id(run_id)).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "run id cannot name an artifacts directory",
        )
    })?;
    ensure_under(&root, run_id)
}

/// `mkdirat` against the root fd: a link planted at the run dir or above it
/// is refused, never adopted.
fn ensure_under(root: &Path, run_id: &str) -> std::io::Result<PathBuf> {
    let dir =
        open_root(root, true)?.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
    let path = dir.create_child(run_id)?.path()?;
    let protected = crate::core::trust_sandbox::protected_dirs();
    if let Some(why) = refusal(&path, &protected) {
        tracing::error!(path = %path.display(), "run artifacts directory refused: {why}");
        return Err(std::io::Error::other(why));
    }
    Ok(path)
}

/// Removes the run's directory; a missing one is not an error.
pub fn remove(run_id: &str) {
    if let Some(root) = root().filter(|_| safe_run_id(run_id)) {
        remove_under(&root, run_id);
    }
}

/// Through the root fd: a link is unlinked as an entry, never followed.
fn remove_under(root: &Path, run_id: &str) {
    let Ok(Some(dir)) = open_root(root, false) else {
        return;
    };
    if let Err(error) = dir.remove_child(run_id) {
        tracing::warn!(
            run_id,
            root = %root.display(),
            "run artifacts directory not removed: {error}"
        );
    }
}

/// A run's directory (`{{run.artifacts_dir}}`) opened from the root fd,
/// never through a link: where Kronn writes approved copies and full outputs.
pub(crate) fn open_run_dir(artifacts_dir: &Path) -> std::io::Result<fd::Dir> {
    let missing = || std::io::Error::from(std::io::ErrorKind::NotFound);
    let run_id = artifacts_dir.file_name().ok_or_else(missing)?;
    let root = artifacts_dir.parent().ok_or_else(missing)?;
    open_root(root, false)?
        .ok_or_else(missing)?
        .open_child(run_id)?
        .ok_or_else(missing)
}

/// Whether a run in `status` may still execute and read its directory.
pub fn keeps_directory(
    status: &RunStatus,
    started_at: chrono::DateTime<chrono::Utc>,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    match status {
        RunStatus::Pending
        | RunStatus::Running
        | RunStatus::WaitingApproval
        | RunStatus::WaitingQuota => true,
        RunStatus::Interrupted => {
            now - started_at < chrono::Duration::days(i64::from(interrupted_ttl_days))
        }
        RunStatus::Success
        | RunStatus::Partial
        | RunStatus::Failed
        | RunStatus::Cancelled
        | RunStatus::StoppedByGuard => false,
    }
}

/// Startup sweep: moves the run directories an older version kept in the
/// data directory, then removes the ones no run will read again. Returns how
/// many were removed. The file system work runs off the async executor.
pub async fn sweep(
    db: &crate::db::Database,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    let Some(root) = root() else {
        return 0;
    };
    if let Ok(data) = crate::core::config::config_dir() {
        let legacy = data.join(LEGACY_DIR_NAME);
        let (moved, dropped) = migrate_from(&legacy, &root, db, interrupted_ttl_days, now).await;
        if moved + dropped > 0 {
            tracing::info!(
                moved,
                dropped,
                root = %root.display(),
                "run artifacts moved out of the data directory"
            );
        }
    }
    sweep_root(&root, db, interrupted_ttl_days, now).await
}

/// `dir` opened by `open`, and its run-named entries, off the executor.
async fn listed(
    open: impl FnOnce() -> std::io::Result<Option<fd::Dir>> + Send + 'static,
) -> Option<(fd::Dir, Vec<String>)> {
    tokio::task::spawn_blocking(move || {
        let dir = open().ok()??;
        let names = dir.names().ok()?;
        let runs = names
            .into_iter()
            .filter_map(|name| name.into_string().ok())
            .filter(|name| is_run_dir_name(name))
            .collect();
        Some((dir, runs))
    })
    .await
    .ok()
    .flatten()
}

/// Whether the run behind `run_id` will read its directory again; `None`
/// when the database could not say (the directory is then left alone).
async fn keeps(
    db: &crate::db::Database,
    run_id: &str,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<bool> {
    let lookup = run_id.to_string();
    match db
        .with_read_conn(move |conn| crate::db::workflows::get_run(conn, &lookup))
        .await
    {
        Ok(run) => Some(run.is_some_and(|run| {
            keeps_directory(&run.status, run.started_at, interrupted_ttl_days, now)
        })),
        Err(error) => {
            tracing::warn!(%run_id, "artifacts sweep could not read the run: {error}");
            None
        }
    }
}

/// Moves each live run directory of `legacy` into `root` and deletes the
/// dead ones, all through directory fds. Best effort: one that fails is
/// logged and left where it is.
async fn migrate_from(
    legacy: &Path,
    root: &Path,
    db: &crate::db::Database,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> (usize, usize) {
    if real_path(legacy) == real_path(root) {
        return (0, 0);
    }
    let legacy_path = legacy.to_path_buf();
    let Some((legacy_dir, runs)) =
        listed(move || fd::Dir::open_under(&legacy_path, &[], false)).await
    else {
        return (0, 0);
    };
    if runs.is_empty() {
        return (0, 0);
    }
    let root_path = root.to_path_buf();
    let Some((root_dir, _)) = listed(move || open_root(&root_path, true)).await else {
        return (0, 0);
    };
    let mut plan = Vec::new();
    for run_id in runs {
        if let Some(keep) = keeps(db, &run_id, interrupted_ttl_days, now).await {
            plan.push((run_id, keep));
        }
    }
    let legacy = legacy.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let counts = apply_migration(&legacy_dir, &root_dir, &plan);
        // Only once empty.
        let _ = std::fs::remove_dir(&legacy);
        counts
    })
    .await
    .unwrap_or_default()
}

fn apply_migration(legacy: &fd::Dir, root: &fd::Dir, plan: &[(String, bool)]) -> (usize, usize) {
    // A copy a crash interrupted: never a run, and redone below.
    for name in root.names().unwrap_or_default() {
        if name.to_string_lossy().starts_with(MIGRATING_PREFIX) {
            let _ = root.remove_child(&name);
        }
    }
    let (mut moved, mut dropped) = (0, 0);
    for (run_id, keep) in plan {
        let result = if !keep {
            legacy.remove_child(run_id).map(|()| dropped += 1)
        } else if !matches!(root.open_child(run_id), Ok(None)) {
            Err(std::io::Error::other("already present in the new root"))
        } else {
            move_into(legacy, root, run_id).map(|()| moved += 1)
        };
        if let Err(error) = result {
            tracing::warn!(%run_id, "legacy run artifacts directory not migrated: {error}");
        }
    }
    (moved, dropped)
}

/// `renameat`, or across file systems (two Docker volumes) a copy under a
/// temporary name renamed into place, so a crash never leaves a partial run.
fn move_into(legacy: &fd::Dir, root: &fd::Dir, run_id: &str) -> std::io::Result<()> {
    match root.rename_from(legacy, run_id, run_id) {
        Ok(()) => return Ok(()),
        Err(error) if error.raw_os_error() != Some(EXDEV) => return Err(error),
        Err(_) => {}
    }
    let temp = format!("{MIGRATING_PREFIX}{run_id}");
    root.remove_child(&temp)?;
    let copied = root
        .copy_from(legacy, run_id, &temp)
        .and_then(|()| root.rename_from(root, &temp, run_id));
    if let Err(error) = copied {
        let _ = root.remove_child(&temp);
        return Err(error);
    }
    legacy.remove_child(run_id)
}

#[cfg(unix)]
const EXDEV: i32 = libc::EXDEV;
#[cfg(not(unix))]
const EXDEV: i32 = 17;

/// Removes the run directories of `root` no run will read again, through the
/// root fd held across the database reads: a swapped link is unlinked, never
/// followed. Only names shaped like a run id are ever touched.
async fn sweep_root(
    root: &Path,
    db: &crate::db::Database,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    let root_path = root.to_path_buf();
    let Some((dir, runs)) = listed(move || open_root(&root_path, false)).await else {
        return 0;
    };
    let mut dead = Vec::new();
    for run_id in runs {
        if keeps(db, &run_id, interrupted_ttl_days, now).await == Some(false) {
            dead.push(run_id);
        }
    }
    tokio::task::spawn_blocking(move || {
        dead.iter()
            .filter(|run_id| match dir.remove_child(run_id) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%run_id, "run artifacts directory not removed: {error}");
                    false
                }
            })
            .count()
    })
    .await
    .unwrap_or_default()
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    const LIVE: &str = "0b4c1a52-7c51-4f0e-9d8a-1d2f3e4a5b6c";
    const DONE: &str = "1c5d2b63-8d62-4a1f-8e9b-2e3f4a5b6c7d";
    const GONE: &str = "2d6e3c74-9e73-4b2a-9fac-3f4a5b6c7d8e";

    #[test]
    fn each_run_gets_its_own_owner_only_directory() {
        let first = ensure("run-artifacts-a1").unwrap();
        let second = ensure("run-artifacts-b2").unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        remove("run-artifacts-a1");
        remove("run-artifacts-b2");
        assert!(!first.exists() && !second.exists());
        remove("run-artifacts-a1");
    }

    /// KT-1585 — a base that is relative, inside a protected directory or
    /// containing one is refused; the root is always Kronn's own directory.
    #[test]
    fn the_root_is_kronns_own_directory_outside_every_protected_one() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".config/kronn");
        let cache = home.path().join(".cache/kronn");
        std::fs::create_dir_all(&data).unwrap();
        let protected = vec![data.clone()];
        let (default, warnings) = resolve_root(None, Some(&data), Some(&cache), &protected);
        let default = default.unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(default.starts_with(cache.join(OWN_DIR)), "{default:?}");
        assert!(!default.starts_with(&data));

        let chosen = home.path().join("artifacts");
        let (root, _) = resolve_root(chosen.to_str(), Some(&data), Some(&cache), &protected);
        assert!(root.unwrap().starts_with(chosen.join(OWN_DIR)));

        let containing = [
            home.path().to_path_buf(),
            home.path().join(".config"),
            PathBuf::from("/"),
        ];
        for base in containing
            .iter()
            .chain([data.clone(), data.join("run-artifacts")].iter())
        {
            let (root, warnings) =
                resolve_root(base.to_str(), Some(&data), Some(&cache), &protected);
            assert_eq!(root.as_ref(), Some(&default), "{base:?}");
            assert!(warnings[0].contains("overlaps"), "{warnings:?}");
        }
        let (root, warnings) =
            resolve_root(Some("relative/dir"), Some(&data), Some(&cache), &protected);
        assert_eq!(root, Some(default.clone()));
        assert!(warnings[0].contains("not absolute"));

        // Linux with XDG_CACHE_HOME inside the data dir: no root at all.
        let (root, warnings) =
            resolve_root(None, Some(&data), Some(&data.join("cache")), &protected);
        assert_eq!(root, None);
        assert!(warnings[0].contains(ROOT_ENV));

        // A second data directory gets a root of its own.
        let other = home.path().join("other-data");
        let (second, _) = resolve_root(None, Some(&other), Some(&cache), &protected);
        assert_ne!(second.unwrap(), default);
    }

    /// The root this test process resolves is the production resolution.
    #[test]
    fn the_test_root_is_resolved_like_production() {
        let data = crate::core::config::config_dir().unwrap();
        let root = root().unwrap();
        assert!(!root.starts_with(&data));
        assert!(root.parent().unwrap().ends_with(OWN_DIR));
        let protected = crate::core::trust_sandbox::protected_dirs();
        assert!(protected.iter().all(|closed| !root.starts_with(closed)));
    }

    #[test]
    fn only_uuid_names_are_run_directories() {
        assert!(is_run_dir_name(LIVE));
        for name in [
            "kronn",
            "run-done",
            "Documents",
            "0B4C1A52-7C51-4F0E-9D8A-1D2F3E4A5B6C",
            "0b4c1a527c514f0e9d8a1d2f3e4a5b6c",
        ] {
            assert!(!is_run_dir_name(name), "{name}");
        }
    }

    async fn db_with_runs() -> crate::db::Database {
        let db = crate::db::Database::open_in_memory().unwrap();
        let workflow: crate::models::Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf-sweep", "name": "Sweep", "project_id": null,
            "trigger": {"type": "Manual"}, "steps": [], "actions": [], "safety": {},
            "workspace_config": null, "concurrency_limit": null, "enabled": true,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let run = |id: &str, status: &str| -> crate::models::WorkflowRun {
            serde_json::from_value(serde_json::json!({
                "id": id, "workflow_id": "wf-sweep", "status": status,
                "trigger_context": null, "step_results": [], "tokens_used": 0,
                "workspace_path": null, "started_at": chrono::Utc::now().to_rfc3339(),
                "finished_at": null, "run_type": "linear", "batch_total": 0,
                "batch_completed": 0, "batch_failed": 0, "batch_no_response": 0,
                "batch_name": null, "parent_run_id": null, "state": {},
                "produced_branches": [], "concurrency_key": null
            }))
            .unwrap()
        };
        let (live, done) = (run(LIVE, "WaitingApproval"), run(DONE, "Success"));
        db.with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &workflow)?;
            crate::db::workflows::insert_run(conn, &live)?;
            crate::db::workflows::insert_run(conn, &done)
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn the_sweep_removes_what_no_run_will_read_again() {
        let db = db_with_runs().await;
        let root = tempfile::tempdir().unwrap();
        for id in [GONE, LIVE, DONE, "notes", "run-x"] {
            std::fs::create_dir_all(root.path().join(id)).unwrap();
        }

        let removed = sweep_root(root.path(), &db, 7, chrono::Utc::now()).await;

        assert_eq!(removed, 2);
        assert!(root.path().join(LIVE).is_dir());
        assert!(!root.path().join(GONE).exists());
        assert!(!root.path().join(DONE).exists());
        assert!(root.path().join("notes").is_dir() && root.path().join("run-x").is_dir());
    }

    /// KT-1585 — a base a person chose (`$HOME`-like, or containing the data
    /// dir) is never listed: only Kronn's own directory under it is swept.
    #[tokio::test]
    async fn the_sweep_never_removes_anything_outside_kronns_own_directory() {
        let db = db_with_runs().await;
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(GONE);
        std::fs::create_dir_all(data.join("secrets")).unwrap();
        std::fs::create_dir_all(home.path().join(DONE)).unwrap();
        let protected = vec![data.clone()];
        let cache = home.path().join("cache");
        let (root, _) = resolve_root(home.path().to_str(), Some(&data), Some(&cache), &protected);
        let root = root.unwrap();
        assert!(
            root.starts_with(cache.join(OWN_DIR)),
            "a base holding the data dir is refused"
        );
        let tmp_like = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_like.path().join(DONE)).unwrap();
        let (chosen, _) = resolve_root(
            tmp_like.path().to_str(),
            Some(&data),
            Some(&cache),
            &protected,
        );
        let chosen = chosen.unwrap();
        std::fs::create_dir_all(chosen.join(DONE)).unwrap();

        for swept in [&root, &chosen] {
            sweep_root(swept, &db, 7, chrono::Utc::now()).await;
        }

        assert!(data.join("secrets").is_dir(), "the data dir is untouched");
        assert!(
            home.path().join(DONE).is_dir(),
            "a run-named dir outside is untouched"
        );
        assert!(tmp_like.path().join(DONE).is_dir());
        assert!(
            !chosen.join(DONE).exists(),
            "inside Kronn's own directory it is swept"
        );
    }

    /// KT-1585 — a step can swap `kronn-run-artifacts` or `<key>` for a link
    /// into the vault: sweep, migration, ensure and remove then touch nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_linked_root_is_never_followed() {
        let db = db_with_runs().await;
        for linked_level in [OWN_DIR, "key"] {
            let vault = tempfile::tempdir().unwrap();
            let evidence = vault.path().join("task-evidence");
            for id in [DONE, GONE] {
                std::fs::create_dir_all(evidence.join(id).join("proof")).unwrap();
            }
            let base = tempfile::tempdir().unwrap();
            let root = base.path().join(OWN_DIR).join("0123456789ab");
            if linked_level == OWN_DIR {
                std::fs::create_dir_all(vault.path().join("own")).unwrap();
                std::os::unix::fs::symlink(&evidence, vault.path().join("own/0123456789ab"))
                    .unwrap();
                std::os::unix::fs::symlink(vault.path().join("own"), base.path().join(OWN_DIR))
                    .unwrap();
            } else {
                std::fs::create_dir_all(root.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&evidence, &root).unwrap();
            }
            let legacy = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(legacy.path().join(LIVE)).unwrap();

            assert_eq!(sweep_root(&root, &db, 7, chrono::Utc::now()).await, 0);
            assert_eq!(
                migrate_from(legacy.path(), &root, &db, 7, chrono::Utc::now()).await,
                (0, 0)
            );
            assert!(ensure_under(&root, LIVE).is_err());
            remove_under(&root, DONE);

            for id in [DONE, GONE] {
                assert!(
                    evidence.join(id).join("proof").is_dir(),
                    "{linked_level}: {id}"
                );
            }
            assert!(
                !evidence.join(LIVE).exists(),
                "{linked_level}: nothing created"
            );
            assert!(
                legacy.path().join(LIVE).is_dir(),
                "{linked_level}: legacy kept"
            );
        }
    }

    /// A base swapped for a link into the data dir: the root now resolves
    /// inside a protected directory and is refused at use, not only at start.
    #[cfg(unix)]
    #[test]
    fn a_root_that_now_resolves_into_the_vault_is_refused() {
        let data = crate::core::config::config_dir().unwrap();
        let inside = data.join(format!("kt1585-link-target-{}", std::process::id()));
        let kept = inside.join(OWN_DIR).join("0123456789ab").join(DONE);
        std::fs::create_dir_all(&kept).unwrap();
        let base = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&inside, base.path().join("cache")).unwrap();
        let root = base.path().join("cache").join(OWN_DIR).join("0123456789ab");
        let error = open_root(&root, false).err().unwrap();
        assert!(error.to_string().contains("overlaps"), "{error}");
        assert!(ensure_under(&root, LIVE).is_err());
        remove_under(&root, DONE);
        assert!(kept.is_dir());
        std::fs::remove_dir_all(&inside).unwrap();
    }

    /// A run directory planted as a link is refused, not adopted.
    #[cfg(unix)]
    #[test]
    fn a_linked_run_directory_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = base.path().join(OWN_DIR).join("0123456789ab");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join(LIVE)).unwrap();
        assert!(ensure_under(&root, LIVE).is_err());
        assert!(ensure_under(&root, DONE)
            .unwrap()
            .starts_with(root.canonicalize().unwrap()));
    }

    #[tokio::test]
    async fn the_legacy_directories_move_and_the_dead_ones_are_dropped() {
        let db = db_with_runs().await;
        let data = tempfile::tempdir().unwrap();
        let legacy = data.path().join(LEGACY_DIR_NAME);
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(legacy.join(LIVE).join("exec-output")).unwrap();
        std::fs::write(legacy.join(LIVE).join("exec-output/out.txt"), "kept").unwrap();
        std::fs::create_dir_all(legacy.join(DONE)).unwrap();
        std::fs::create_dir_all(legacy.join("not-a-run")).unwrap();
        // A copy a crash interrupted is never read as a run.
        std::fs::create_dir_all(root.path().join(format!("{MIGRATING_PREFIX}{LIVE}"))).unwrap();

        let (moved, dropped) = migrate_from(&legacy, root.path(), &db, 7, chrono::Utc::now()).await;

        assert_eq!((moved, dropped), (1, 1));
        assert_eq!(
            std::fs::read_to_string(root.path().join(LIVE).join("exec-output/out.txt")).unwrap(),
            "kept"
        );
        assert!(!legacy.join(LIVE).exists() && !legacy.join(DONE).exists());
        assert!(!root.path().join(DONE).exists(), "a dead run is not copied");
        assert!(
            legacy.join("not-a-run").is_dir(),
            "only run-named directories move"
        );
        assert!(!root
            .path()
            .join(format!("{MIGRATING_PREFIX}{LIVE}"))
            .exists());
    }

    #[test]
    fn a_run_id_never_escapes_the_root() {
        for id in ["", "..", "../x", "a/b", "a\\b", "é"] {
            assert!(dir_for(id).is_none(), "{id:?}");
            assert!(ensure(id).is_err(), "{id:?}");
        }
    }

    #[test]
    fn the_directory_lives_as_long_as_the_run_can_execute() {
        let now = chrono::Utc::now();
        let recent = now - chrono::Duration::hours(1);
        let old = now - chrono::Duration::days(30);
        assert!(keeps_directory(&RunStatus::WaitingApproval, old, 7, now));
        assert!(keeps_directory(&RunStatus::Running, old, 7, now));
        assert!(keeps_directory(&RunStatus::Interrupted, recent, 7, now));
        assert!(!keeps_directory(&RunStatus::Interrupted, old, 7, now));
        for ended in [
            RunStatus::Success,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Partial,
            RunStatus::StoppedByGuard,
        ] {
            assert!(!keeps_directory(&ended, recent, 7, now));
        }
    }
}
