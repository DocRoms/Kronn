pub mod agent_decisions;
pub mod api_call_logs;
pub mod audit_runs;
pub mod contacts;
pub mod disc_source;
pub mod discussion_sessions;
pub mod discussions;
pub mod mcps;
pub mod migrations;
pub mod projects;
pub mod quick_apis;
pub mod quick_prompts;
pub mod workflows;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::core::config;

/// Thread-safe database handle.
/// Uses std::sync::Mutex so the lock can be held inside spawn_blocking
/// (tokio::sync::Mutex cannot be used in a blocking context).
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl Database {
    /// Open (or create) the database file in the Kronn data directory.
    pub fn open() -> Result<Self> {
        let dir = config::config_dir()?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("kronn.db");
        Self::open_path(&path)
    }

    /// Open an in-memory database (useful for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .context("Failed to open in-memory database")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        migrations::run(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: PathBuf::from(":memory:") })
    }

    /// Open a database at a specific path (useful for testing).
    pub fn open_path(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        // WAL mode for better concurrent read performance.
        // Disable with KRONN_DB_WAL=0 if database is on a network mount (NFS, SMB, iCloud).
        let use_wal = std::env::var("KRONN_DB_WAL")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(true);
        // busy_timeout: wait up to 5s if the DB is locked by another writer
        conn.execute_batch("PRAGMA busy_timeout=5000;")?;

        if use_wal {
            // Setting journal_mode requires a query that yields the *actual*
            // mode. SQLite silently falls back to TRUNCATE/PERSIST when the
            // backing filesystem cannot support WAL (NFS, SMB, iCloud Drive,
            // some FUSE mounts). If we don't notice we lose concurrent-write
            // safety without ever telling the user — verify and warn loudly.
            let actual_mode: String = conn.query_row(
                "PRAGMA journal_mode=WAL;",
                [],
                |row| row.get(0),
            ).context("Failed to set WAL journal mode")?;
            conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;

            if !actual_mode.eq_ignore_ascii_case("wal") {
                tracing::warn!(
                    "Requested journal_mode=WAL but SQLite fell back to '{}'. \
                     The database file at {} is likely on a network or sync \
                     filesystem (NFS, SMB, iCloud Drive, FUSE) that does not \
                     support WAL — concurrent writes may block or corrupt. \
                     Move the data dir off the network mount or set KRONN_DB_WAL=0 \
                     to suppress this warning.",
                    actual_mode,
                    path.display()
                );
            }
        } else {
            tracing::warn!("WAL mode disabled (KRONN_DB_WAL=0); using DELETE journal mode");
            conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        }

        // Run migrations before wrapping in Mutex (avoids blocking_lock inside async runtime).
        // Pass db path so a backup is created before pending migrations.
        migrations::run_with_backup(&conn, Some(path))?;

        // 0.8.4 (#317 / B1) — reconcile stale `Running` audit_runs at
        // boot. A backend crash, container restart, or kill -9 during
        // an audit leaves the row stuck `Running` forever, polluting
        // the recap chip strip + the "active audits" badge. Any run
        // older than 30 min is force-flipped to `Interrupted` (the
        // resume mechanism still works because last_completed_step
        // survives — see `reconcile_stale_runs_preserves_last_completed_step`).
        match audit_runs::reconcile_stale_runs(&conn, 30 * 60) {
            Ok(0) => {}
            Ok(n) => tracing::info!("Reconciled {} stale audit_runs (status was 'Running' for > 30 min)", n),
            Err(e) => tracing::warn!("Failed to reconcile stale audit_runs: {}", e),
        }

        // 0.8.6 — auto-purge api_call_logs older than 90 days at boot.
        // Generous default : keeps a quarter of audit trail for debug
        // while preventing unbounded growth. User can manually trigger
        // a tighter purge via the Settings → API audit "Purge" button.
        match api_call_logs::purge_older_than(&conn, 90) {
            Ok(0) => {}
            Ok(n) => tracing::info!("Purged {} api_call_logs rows older than 90 days", n),
            Err(e) => tracing::warn!("Failed to auto-purge api_call_logs: {}", e),
        }

        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: path.clone() })
    }

    /// Get the database file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Execute a blocking closure with the database connection.
    /// Runs inside `spawn_blocking` so the Tokio worker thread is never blocked
    /// waiting on the mutex or executing a synchronous SQLite query.
    pub async fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
            f(&conn)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {e}"))?
    }
}
