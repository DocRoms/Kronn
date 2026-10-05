//! B6 (0.8.11) — scheduled DB backups. The manual `/api/db/backup` writes into
//! `<data_dir>/backups` — i.e. INSIDE the live DB volume, so losing the volume
//! loses the DB AND its backups. This module runs an automatic periodic backup
//! that can target a directory OUTSIDE the volume (bind-mounted host dir via
//! `KRONN_BACKUP_DIR`) and prunes to a rolling window.
//!
//! Pure helpers (`resolve_backup_dir`, `prune_old_backups`, `backup_filename`)
//! are unit-tested; the SQLite copy + the interval loop are thin wrappers.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::db::Database;

/// Prefix + extension for scheduled backup files (distinct enough to prune
/// safely without touching unrelated files in a shared dir).
const BACKUP_PREFIX: &str = "kronn-auto-";
const BACKUP_EXT: &str = "db";

/// Where scheduled backups go: `KRONN_BACKUP_DIR` if set (operator points this
/// at a bind-mounted host dir OUTSIDE the data volume), else `<data_dir>/backups`
/// (same place as the manual backup — in-volume, logged as a warning). Returns
/// `(dir, is_external)`.
pub fn resolve_backup_dir(data_dir: &Path) -> (PathBuf, bool) {
    match std::env::var("KRONN_BACKUP_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(dir) => (PathBuf::from(dir.trim()), true),
        None => (data_dir.join("backups"), false),
    }
}

/// Timestamped backup filename for a given instant.
pub fn backup_filename(now: DateTime<Utc>) -> String {
    format!(
        "{BACKUP_PREFIX}{}.{BACKUP_EXT}",
        now.format("%Y%m%d-%H%M%S")
    )
}

/// True when `name` is one of our scheduled backup files.
fn is_backup_name(name: &str) -> bool {
    name.starts_with(BACKUP_PREFIX) && name.ends_with(&format!(".{BACKUP_EXT}"))
}

/// Delete the oldest scheduled backups in `dir`, keeping the `keep_n` most
/// recent (by filename, which sorts chronologically thanks to the timestamp
/// format). Only touches files matching our prefix/ext. Returns how many were
/// removed. Never errors on individual unlink failures (best-effort), but
/// warns — a permissions problem on an external KRONN_BACKUP_DIR would
/// otherwise accumulate backups unbounded while logging success.
pub fn prune_old_backups(dir: &Path, keep_n: usize) -> usize {
    let mut ours: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(is_backup_name)
                    .unwrap_or(false)
            })
            .collect(),
        Err(e) => {
            tracing::warn!(target: "backup", "cannot list backup dir {}: {e} — pruning skipped", dir.display());
            return 0;
        }
    };
    if ours.len() <= keep_n {
        return 0;
    }
    ours.sort(); // chronological (timestamped names)
    let to_remove = ours.len() - keep_n;
    let mut removed = 0;
    for p in ours.into_iter().take(to_remove) {
        match std::fs::remove_file(&p) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(target: "backup", "failed to prune {}: {e}", p.display()),
        }
    }
    removed
}

/// Serializes the operations that copy or rebuild the whole database file
/// (scheduled and manual backups, compaction): two at once would double the
/// disk they need and race for the same free space.
pub(crate) static MAINTENANCE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Free space a full copy of `live_bytes` needs: the copy itself, a tenth for
/// what is written meanwhile, and a floor so the disk is not left full.
pub fn required_free_bytes(live_bytes: u64) -> u64 {
    const FLOOR: u64 = 512 * 1024 * 1024;
    live_bytes
        .saturating_add(live_bytes / 10)
        .saturating_add(FLOOR)
}

/// Bytes of the pages the database actually uses: what `VACUUM INTO` writes.
pub fn live_bytes(conn: &rusqlite::Connection) -> anyhow::Result<u64> {
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let free_pages: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    Ok((page_count - free_pages).max(0) as u64 * page_size.max(0) as u64)
}

/// Refuse a copy that would not fit, with a message the operator can act on.
pub fn ensure_free_space(dir: &Path, required: u64, available: u64) -> anyhow::Result<()> {
    if available < required {
        anyhow::bail!(
            "not enough free space in {}: the copy needs about {} MB, {} MB are free",
            dir.display(),
            required / (1024 * 1024),
            available / (1024 * 1024)
        );
    }
    Ok(())
}

/// Write a consistent, compact copy of the database at `source` to `dest`.
///
/// `VACUUM INTO` runs on its own read-only connection: it reads one WAL
/// snapshot, so the write connection keeps committing for the whole copy, and
/// it skips free pages. A paged `backup.step(N)` from another connection
/// restarts whenever the writer commits between two steps, so on a busy
/// instance a large copy may never finish; from the write connection it holds
/// the global mutex. The copy is staged in an owner-only directory, synced,
/// then renamed: a failed copy never leaves a file that looks like a backup.
/// Blocking: call it from `spawn_blocking`.
pub fn snapshot_database(
    source: &Path,
    dest: &Path,
    available_space: impl Fn(&Path) -> std::io::Result<u64>,
) -> anyhow::Result<u64> {
    let dir = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("backup path {} has no parent", dest.display()))?;
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup path {} has no file name", dest.display()))?;
    if dest.exists() {
        anyhow::bail!("{} already exists", dest.display());
    }
    let conn = rusqlite::Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    let required = required_free_bytes(live_bytes(&conn)?);
    ensure_free_space(dir, required, available_space(dir)?)?;

    let staging = dir.join(format!(".{name}.partial"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir(&staging)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
    }
    let staged = staging.join(name);
    let copied = (|| -> anyhow::Result<u64> {
        let staged_str = staged
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("backup path {} is not UTF-8", staged.display()))?;
        conn.execute("VACUUM INTO ?1", [staged_str])?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
        }
        // SQLite does not sync a VACUUM INTO target.
        std::fs::File::open(&staged)?.sync_all()?;
        std::fs::rename(&staged, dest)?;
        Ok(std::fs::metadata(dest)?.len())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    copied
}

/// The `.db` file and its WAL, in bytes.
fn file_sizes(path: &Path) -> (u64, u64) {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    (
        size(path),
        size(Path::new(&format!("{}-wal", path.display()))),
    )
}

/// KT-984 — give the free pages back to the filesystem: `VACUUM`, then a
/// truncating checkpoint. Deleting or trimming rows only grows the free list,
/// so this is the step that makes the file smaller. It rewrites the live data
/// while holding the write connection, so it is an explicit user action, never
/// automatic, refused while a workflow run is active or when the disk lacks
/// room for the rebuilt copy (temporary file plus WAL).
pub async fn compact_database(db: &Database) -> anyhow::Result<crate::models::DbCompaction> {
    compact_database_with(db, |path| fs2::available_space(path)).await
}

async fn compact_database_with(
    db: &Database,
    available_space: fn(&Path) -> std::io::Result<u64>,
) -> anyhow::Result<crate::models::DbCompaction> {
    if db.path().to_string_lossy() == ":memory:" {
        anyhow::bail!("the database is in memory; there is nothing to compact");
    }
    let Ok(_maintenance) = MAINTENANCE_LOCK.try_lock() else {
        anyhow::bail!("a backup or a compaction is already running; try again when it ends");
    };
    let path = db.path().clone();
    db.with_conn(move |conn| {
        if crate::db::workflows::has_running_run(conn)? {
            anyhow::bail!("workflow runs are in progress; compact the database once they finish");
        }
        let live = live_bytes(conn)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        // The rebuilt copy goes through the WAL beside the database.
        ensure_free_space(dir, required_free_bytes(live), available_space(dir)?)?;
        // SQLite builds it first in a temporary file.
        let temp = std::env::temp_dir();
        ensure_free_space(&temp, required_free_bytes(live), available_space(&temp)?)?;
        let (file_bytes_before, wal_bytes_before) = file_sizes(&path);
        let started = std::time::Instant::now();
        conn.execute_batch("VACUUM;")?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        let (file_bytes_after, wal_bytes_after) = file_sizes(&path);
        Ok(crate::models::DbCompaction {
            file_bytes_before,
            wal_bytes_before,
            file_bytes_after,
            wal_bytes_after,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    })
    .await
}

/// Run one backup now: copy the live DB into `dir`, then prune to `keep_n`.
/// Returns the written path. Skips (Ok(None)) for an in-memory DB.
pub async fn perform_backup(
    db: &Database,
    dir: &Path,
    keep_n: usize,
) -> anyhow::Result<Option<PathBuf>> {
    perform_backup_with(db, dir, keep_n, |path| fs2::available_space(path)).await
}

async fn perform_backup_with(
    db: &Database,
    dir: &Path,
    keep_n: usize,
    available_space: fn(&Path) -> std::io::Result<u64>,
) -> anyhow::Result<Option<PathBuf>> {
    if db.path().to_string_lossy() == ":memory:" {
        return Ok(None);
    }
    let _maintenance = MAINTENANCE_LOCK.lock().await;
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(backup_filename(Utc::now()));
    let source = db.path().clone();
    let dest_owned = dest.clone();
    tokio::task::spawn_blocking(move || snapshot_database(&source, &dest_owned, available_space))
        .await
        .map_err(|e| anyhow::anyhow!("scheduled backup task failed: {e}"))?
        .map_err(|e| anyhow::anyhow!("scheduled backup skipped: {e}"))?;
    let pruned = prune_old_backups(dir, keep_n);
    if pruned > 0 {
        tracing::info!(target: "backup", "pruned {pruned} old scheduled backup(s)");
    }
    Ok(Some(dest))
}

/// Parse an env var, falling back to `default` when unset. A SET but
/// unparseable value warns instead of silently defaulting.
fn env_or_default<T: std::str::FromStr + std::fmt::Display>(var: &str, default: T) -> T {
    match std::env::var(var) {
        Ok(s) => s.trim().parse().unwrap_or_else(|_| {
            tracing::warn!(target: "backup", "{var}={s:?} is not a valid number — using default {default}");
            default
        }),
        Err(_) => default,
    }
}

/// True when the newest existing backup is younger than half the schedule
/// interval — a fresh tick then adds nothing but churn. Guards against
/// restart loops (dev watcher, container crash loop): the immediate
/// first interval tick would otherwise write one backup per process start
/// and, with count-based pruning, wipe a week of history in minutes.
fn should_skip_backup(newest_age: Duration, interval: Duration) -> bool {
    newest_age < interval / 2
}

/// Age (by mtime) of the newest scheduled backup in `dir`, if any.
fn newest_backup_age(dir: &Path) -> Option<Duration> {
    let newest = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().map(is_backup_name).unwrap_or(false))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()?;
    newest.elapsed().ok()
}

/// Periodic backup task. Mirrors `learning_sweep`: tick on an interval, run one
/// backup, log failures, never crash the loop.
pub struct BackupScheduler {
    db: Arc<Database>,
    interval: Duration,
    keep_n: usize,
}

impl BackupScheduler {
    /// Build from env: `KRONN_BACKUP_INTERVAL_HOURS` (default 24, 0 disables),
    /// `KRONN_BACKUP_KEEP` (default 7).
    pub fn from_env(db: Arc<Database>) -> Option<Arc<Self>> {
        let hours: u64 = env_or_default("KRONN_BACKUP_INTERVAL_HOURS", 24);
        if hours == 0 {
            tracing::info!(target: "backup", "scheduled backups disabled (KRONN_BACKUP_INTERVAL_HOURS=0)");
            return None;
        }
        let keep_n: usize = env_or_default("KRONN_BACKUP_KEEP", 7);
        Some(Arc::new(Self {
            db,
            interval: Duration::from_secs(hours * 3600),
            keep_n,
        }))
    }

    pub async fn start(self: Arc<Self>) {
        let (dir, external) = resolve_backup_dir(self.db.path().parent().unwrap_or(Path::new(".")));
        if !external {
            tracing::warn!(
                target: "backup",
                "scheduled backups write to {} (INSIDE the data volume). Set KRONN_BACKUP_DIR to a host-mounted dir so a lost volume doesn't lose the backups too.",
                dir.display()
            );
        } else {
            tracing::info!(target: "backup", "scheduled backups → {} (external)", dir.display());
        }
        let mut tick = tokio::time::interval(self.interval);
        loop {
            tick.tick().await;
            // The first tick fires immediately (good: boot backup after long
            // downtime) — but skip when a recent backup already exists, or a
            // restart loop would prune the whole history in minutes.
            if let Some(age) = newest_backup_age(&dir) {
                if should_skip_backup(age, self.interval) {
                    tracing::debug!(
                        target: "backup",
                        "skipping scheduled backup: newest is {}s old (< interval/2)",
                        age.as_secs()
                    );
                    continue;
                }
            }
            match perform_backup(&self.db, &dir, self.keep_n).await {
                Ok(Some(p)) => {
                    tracing::info!(target: "backup", "scheduled backup written: {}", p.display())
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(target: "backup", "{e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[cfg(unix)]
    #[tokio::test]
    #[serial(db_maintenance)]
    async fn perform_backup_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::open_path(&tmp.path().join("kronn.db")).expect("open db");
        let written = perform_backup(&db, &tmp.path().join("backups"), 3)
            .await
            .unwrap()
            .unwrap();
        let mode = std::fs::metadata(&written).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a backup in a host dir must not be world-readable"
        );
    }
    #[tokio::test]
    #[serial(db_maintenance)]
    async fn backup_is_refused_without_enough_free_space() {
        // KT-1019 — 24 h × keep 7 of a 10 GB base filled disks unchecked.
        let tmp = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::open_path(&tmp.path().join("kronn.db")).expect("open db");
        let dir = tmp.path().join("backups");
        let error = perform_backup_with(&db, &dir, 3, |_| Ok(1024))
            .await
            .expect_err("a full disk must refuse the backup");
        assert!(
            error.to_string().contains("not enough free space"),
            "{error}"
        );
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert!(left.is_empty(), "nothing is written: {left:?}");
    }

    #[test]
    fn required_free_space_covers_the_copy_and_a_floor() {
        let gib = 1024 * 1024 * 1024;
        assert!(required_free_bytes(10 * gib) > 10 * gib);
        assert_eq!(required_free_bytes(0), 512 * 1024 * 1024);
        assert_eq!(required_free_bytes(u64::MAX), u64::MAX);
        assert!(ensure_free_space(Path::new("/x"), 10, 9).is_err());
        assert!(ensure_free_space(Path::new("/x"), 10, 10).is_ok());
    }

    #[tokio::test]
    #[serial(db_maintenance)]
    async fn backup_does_not_wait_for_the_write_connection() {
        // KT-1019 — the copy ran inside with_conn: every writer waited for it.
        // Here the write connection stays held for the whole backup.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("kronn.db");
        let db = Arc::new(crate::db::Database::open_path(&path).expect("open db"));
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TABLE filler(blob BLOB);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 400)
                 INSERT INTO filler SELECT randomblob(16384) FROM n;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (held, is_held) = tokio::sync::oneshot::channel::<()>();
        let holder = {
            let db = db.clone();
            tokio::spawn(async move {
                db.with_conn(move |_conn| {
                    let _ = held.send(());
                    let _ = released.recv();
                    Ok(())
                })
                .await
            })
        };
        is_held.await.unwrap();

        let written = tokio::time::timeout(
            Duration::from_secs(30),
            perform_backup(&db, &tmp.path().join("backups"), 3),
        )
        .await
        .expect("the backup must not wait for the write connection")
        .unwrap()
        .unwrap();
        release.send(()).unwrap();
        holder.await.unwrap().unwrap();

        let copy = rusqlite::Connection::open(&written).unwrap();
        let rows: i64 = copy
            .query_row("SELECT COUNT(*) FROM filler", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 400, "the copy holds every committed row");
        let staging_left = std::fs::read_dir(tmp.path().join("backups"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".partial"));
        assert!(!staging_left, "the staging directory is removed");
    }

    #[test]
    fn snapshot_leaves_no_backup_when_the_copy_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source = tmp.path().join("kronn.db");
        rusqlite::Connection::open(&source)
            .unwrap()
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .unwrap();
        let missing_dir = tmp.path().join("missing").join("kronn-auto-x.db");
        assert!(snapshot_database(&source, &missing_dir, |_| Ok(u64::MAX)).is_err());
        assert!(!missing_dir.exists());
        let dest = tmp.path().join("kronn-auto-y.db");
        std::fs::write(&dest, b"previous").unwrap();
        assert!(
            snapshot_database(&source, &dest, |_| Ok(u64::MAX)).is_err(),
            "an existing file is never overwritten"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous");
    }

    async fn database_with_trimmed_runs(dir: &Path) -> crate::db::Database {
        let db = crate::db::Database::open_path(&dir.join("kronn.db")).expect("open db");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                 VALUES ('wf', 'wf', '\"Manual\"', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
                [],
            )?;
            let payload = serde_json::json!([{"step_name": "s", "status": "Success",
                "output": "résultat ".repeat(40_000), "duration_ms": 1}])
            .to_string();
            for index in 0..40 {
                conn.execute(
                    "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json,
                         started_at, finished_at, run_type)
                     VALUES (?1, 'wf', 'Success', ?2, '2026-01-01T00:00:00+00:00',
                             '2026-01-01T00:00:00+00:00', 'linear')",
                    rusqlite::params![format!("run-{index}"), payload],
                )?;
            }
            while crate::db::run_retention::compact_run_payloads_chunk(
                conn,
                "2026-06-01T00:00:00+00:00",
                crate::db::run_retention::CHUNK_ROWS,
            )? > 0
            {}
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    #[serial(db_maintenance)]
    async fn trimming_frees_pages_and_compaction_shrinks_the_file() {
        // KT-984 DAT-2 — a purge only grows the free list; the file keeps its
        // size until a VACUUM gives the pages back.
        let tmp = tempfile::TempDir::new().unwrap();
        let db = database_with_trimmed_runs(tmp.path()).await;
        let free_pages: i64 = db
            .with_conn(|conn| Ok(conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert!(free_pages > 1_000, "the trim left free pages: {free_pages}");

        let report = compact_database(&db).await.unwrap();
        assert!(
            report.file_bytes_after * 4 < report.file_bytes_before,
            "the file shrinks: {report:?}"
        );
        assert_eq!(report.wal_bytes_after, 0, "{report:?}");
        let (runs, outputs): (i64, String) = db
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*), MIN(step_results_json) FROM workflow_runs",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(runs, 40, "every run row is kept");
        assert!(outputs.contains(crate::db::run_retention::REMOVED_OUTPUT));
    }

    #[tokio::test]
    #[serial(db_maintenance)]
    async fn compaction_is_refused_without_room_or_during_a_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db = database_with_trimmed_runs(tmp.path()).await;
        let error = compact_database_with(&db, |_| Ok(1024)).await.unwrap_err();
        assert!(
            error.to_string().contains("not enough free space"),
            "{error}"
        );

        db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_runs SET status = 'Running' WHERE id = 'run-0'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let error = compact_database(&db).await.unwrap_err();
        assert!(error.to_string().contains("in progress"), "{error}");
    }

    #[tokio::test]
    #[serial(db_maintenance)]
    async fn perform_backup_writes_a_readable_copy() {
        // End-to-end through the REAL copy path. This is the test that was
        // missing on 2026-07-09: `run_to_completion(-1, …)` type-checked but
        // panicked at runtime (rusqlite asserts pages_per_step > 0), and the
        // panic fired inside `with_conn` — poisoning the DB mutex at the
        // boot backup tick and killing every later DB call in the process.
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("kronn.db");
        let db = crate::db::Database::open_path(&db_path).expect("open db");
        let backup_dir = tmp.path().join("backups");

        let written = perform_backup(&db, &backup_dir, 3)
            .await
            .expect("backup must not fail (a panic here poisons the DB mutex)")
            .expect("file-backed DB → a backup file is written");
        assert!(written.exists());

        // The copy is a valid SQLite DB with the migrated schema.
        let copy = rusqlite::Connection::open(&written).unwrap();
        let n: i64 = copy
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n > 0, "backup carries the schema ({n} tables)");

        // And the source connection is still usable afterwards (not poisoned).
        db.with_conn(|conn| {
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .map_err(Into::into)
        })
        .await
        .expect("source DB usable after backup");
    }

    #[test]
    #[serial]
    fn resolve_backup_dir_defaults_in_volume_then_env_external() {
        std::env::remove_var("KRONN_BACKUP_DIR");
        let (dir, ext) = resolve_backup_dir(Path::new("/data"));
        assert_eq!(dir, PathBuf::from("/data/backups"));
        assert!(!ext, "default is in-volume");

        std::env::set_var("KRONN_BACKUP_DIR", "/host/backups");
        let (dir, ext) = resolve_backup_dir(Path::new("/data"));
        assert_eq!(dir, PathBuf::from("/host/backups"));
        assert!(ext, "env-provided dir is external");
        std::env::remove_var("KRONN_BACKUP_DIR");
    }

    #[test]
    fn backup_filename_is_prefixed_and_timestamped() {
        let ts = DateTime::parse_from_rfc3339("2026-07-07T06:05:04Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(backup_filename(ts), "kronn-auto-20260707-060504.db");
    }

    #[test]
    fn should_skip_backup_only_when_newest_is_younger_than_half_interval() {
        let day = Duration::from_secs(24 * 3600);
        assert!(
            should_skip_backup(Duration::from_secs(60), day),
            "restart 1min after a backup → skip"
        );
        assert!(should_skip_backup(day / 2 - Duration::from_secs(1), day));
        assert!(
            !should_skip_backup(day / 2, day),
            "at half the interval → back up"
        );
        assert!(
            !should_skip_backup(day * 7, day),
            "boot after long downtime → back up"
        );
    }

    #[test]
    fn newest_backup_age_none_when_dir_empty_or_foreign_only() {
        let tmp = std::env::temp_dir().join(format!("kronn-newest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("important.db"), b"foreign").unwrap();
        assert!(
            newest_backup_age(&tmp).is_none(),
            "foreign files must not count as backups"
        );

        std::fs::write(tmp.join("kronn-auto-20260101-000000.db"), b"x").unwrap();
        let age = newest_backup_age(&tmp).expect("our backup must be seen");
        assert!(
            age < Duration::from_secs(60),
            "just-written backup must have ~zero age"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn prune_keeps_n_most_recent_and_ignores_foreign_files() {
        let tmp = std::env::temp_dir().join(format!("kronn-prune-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // 5 of ours + 1 foreign.
        for ts in [
            "20260101-000000",
            "20260102-000000",
            "20260103-000000",
            "20260104-000000",
            "20260105-000000",
        ] {
            std::fs::write(tmp.join(format!("kronn-auto-{ts}.db")), b"x").unwrap();
        }
        std::fs::write(tmp.join("important.db"), b"keep").unwrap();

        let removed = prune_old_backups(&tmp, 2);
        assert_eq!(removed, 3, "5 ours, keep 2 → remove 3");
        assert!(
            tmp.join("kronn-auto-20260104-000000.db").exists(),
            "newest kept"
        );
        assert!(
            tmp.join("kronn-auto-20260105-000000.db").exists(),
            "newest kept"
        );
        assert!(
            !tmp.join("kronn-auto-20260101-000000.db").exists(),
            "oldest pruned"
        );
        assert!(tmp.join("important.db").exists(), "foreign file untouched");

        // Under the keep count → no-op.
        assert_eq!(prune_old_backups(&tmp, 10), 0);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
