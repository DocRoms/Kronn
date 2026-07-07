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
    match std::env::var("KRONN_BACKUP_DIR").ok().filter(|s| !s.trim().is_empty()) {
        Some(dir) => (PathBuf::from(dir.trim()), true),
        None => (data_dir.join("backups"), false),
    }
}

/// Timestamped backup filename for a given instant.
pub fn backup_filename(now: DateTime<Utc>) -> String {
    format!("{BACKUP_PREFIX}{}.{BACKUP_EXT}", now.format("%Y%m%d-%H%M%S"))
}

/// Delete the oldest scheduled backups in `dir`, keeping the `keep_n` most
/// recent (by filename, which sorts chronologically thanks to the timestamp
/// format). Only touches files matching our prefix/ext. Returns how many were
/// removed. Never errors on individual unlink failures (best-effort).
pub fn prune_old_backups(dir: &Path, keep_n: usize) -> usize {
    let mut ours: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(BACKUP_PREFIX) && n.ends_with(&format!(".{BACKUP_EXT}")))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => return 0,
    };
    if ours.len() <= keep_n {
        return 0;
    }
    ours.sort(); // chronological (timestamped names)
    let to_remove = ours.len() - keep_n;
    let mut removed = 0;
    for p in ours.into_iter().take(to_remove) {
        if std::fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Run one backup now: SQLite online-copy the live DB into `dir`, then prune to
/// `keep_n`. Returns the written path. Skips (Ok(None)) for an in-memory DB.
pub async fn perform_backup(db: &Database, dir: &Path, keep_n: usize) -> anyhow::Result<Option<PathBuf>> {
    if db.path().to_string_lossy() == ":memory:" {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(backup_filename(Utc::now()));
    let dest_owned = dest.clone();
    db.with_conn(move |conn| {
        let mut dst = rusqlite::Connection::open(&dest_owned)?;
        let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
        backup.run_to_completion(5, Duration::from_millis(50), None)?;
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
        let hours: u64 = std::env::var("KRONN_BACKUP_INTERVAL_HOURS")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(24);
        if hours == 0 {
            tracing::info!(target: "backup", "scheduled backups disabled (KRONN_BACKUP_INTERVAL_HOURS=0)");
            return None;
        }
        let keep_n: usize = std::env::var("KRONN_BACKUP_KEEP")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(7);
        Some(Arc::new(Self { db, interval: Duration::from_secs(hours * 3600), keep_n }))
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
            match perform_backup(&self.db, &dir, self.keep_n).await {
                Ok(Some(p)) => tracing::info!(target: "backup", "scheduled backup written: {}", p.display()),
                Ok(None) => {}
                Err(e) => tracing::warn!(target: "backup", "{e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
        let ts = DateTime::parse_from_rfc3339("2026-07-07T06:05:04Z").unwrap().with_timezone(&Utc);
        assert_eq!(backup_filename(ts), "kronn-auto-20260707-060504.db");
    }

    #[test]
    fn prune_keeps_n_most_recent_and_ignores_foreign_files() {
        let tmp = std::env::temp_dir().join(format!("kronn-prune-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // 5 of ours + 1 foreign.
        for ts in ["20260101-000000","20260102-000000","20260103-000000","20260104-000000","20260105-000000"] {
            std::fs::write(tmp.join(format!("kronn-auto-{ts}.db")), b"x").unwrap();
        }
        std::fs::write(tmp.join("important.db"), b"keep").unwrap();

        let removed = prune_old_backups(&tmp, 2);
        assert_eq!(removed, 3, "5 ours, keep 2 → remove 3");
        assert!(tmp.join("kronn-auto-20260104-000000.db").exists(), "newest kept");
        assert!(tmp.join("kronn-auto-20260105-000000.db").exists(), "newest kept");
        assert!(!tmp.join("kronn-auto-20260101-000000.db").exists(), "oldest pruned");
        assert!(tmp.join("important.db").exists(), "foreign file untouched");

        // Under the keep count → no-op.
        assert_eq!(prune_old_backups(&tmp, 10), 0);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
