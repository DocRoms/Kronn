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

/// Run one backup now: SQLite online-copy the live DB into `dir`, then prune to
/// `keep_n`. Returns the written path. Skips (Ok(None)) for an in-memory DB.
pub async fn perform_backup(
    db: &Database,
    dir: &Path,
    keep_n: usize,
) -> anyhow::Result<Option<PathBuf>> {
    if db.path().to_string_lossy() == ":memory:" {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(backup_filename(Utc::now()));
    let dest_owned = dest.clone();
    db.with_conn(move |conn| {
        let mut dst = rusqlite::Connection::open(&dest_owned)?;
        let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
        // One-shot copy: sqlite3_backup_step(-1) copies every page in a
        // single call. The paged 5-pages/50ms variant is designed to let
        // OTHER connections write between steps — but Kronn has a single
        // shared connection, so the pauses just held the global DB mutex
        // ~2.5s/MB while every API handler queued behind it.
        // MUST be `step(-1)`, NOT `run_to_completion(-1, …)`: the latter
        // asserts pages_per_step > 0 and PANICS — observed live 2026-07-09,
        // where the boot-tick backup poisoned the DB mutex for the whole
        // process (see `perform_backup_writes_a_readable_copy`).
        match backup.step(-1)? {
            rusqlite::backup::StepResult::Done => {}
            other => anyhow::bail!("backup did not complete in one step: {other:?}"),
        }
        Ok(())
    })
    .await
    .map_err(|e| {
        let _ = std::fs::remove_file(&dest);
        anyhow::anyhow!("scheduled backup failed: {e}")
    })?;
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
/// restart loops (cargo-watch, container crash loop): the immediate
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

    #[tokio::test]
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
