pub mod agent_decisions;
pub mod api_call_logs;
pub mod audit_runs;
pub mod contacts;
pub mod disc_source;
pub mod discussion_sessions;
pub mod discussions;
pub mod learnings;
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
    /// ADR-001 O2 (stab-2) — dedicated READ connection (`PRAGMA query_only`).
    /// WAL lets it read a consistent snapshot while the write connection
    /// holds its lock, so heavy reads stop freezing the whole API. `None`
    /// for in-memory databases (a second `:memory:` handle would be a
    /// DIFFERENT db) — `with_read_conn` falls back to the write connection.
    read_conn: Option<Arc<Mutex<Connection>>>,
    path: PathBuf,
    /// Runs flipped `Running`/`Pending` → `Interrupted` by THIS boot's
    /// reconcile. Drained exactly once by the boot notifier
    /// (`run_notify::notify_boot_interrupted`) — the process that would have
    /// webhooked these failures died with them.
    boot_interrupted: Mutex<Vec<workflows::ReconciledRun>>,
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
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            read_conn: None,
            path: PathBuf::from(":memory:"),
            boot_interrupted: Mutex::new(Vec::new()),
        })
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

        let mut wal_effective = false;
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

            wal_effective = actual_mode.eq_ignore_ascii_case("wal");
            if !wal_effective {
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
        // the recap chip strip + the "active audits" badge.
        // Cutoff 0, same reasoning as the workflow_runs reconcile below:
        // at boot no in-process audit runner exists, so every `Running`
        // row is a zombie — and the flip is resume-safe because
        // last_completed_step survives (see
        // `reconcile_stale_runs_preserves_last_completed_step`).
        match audit_runs::reconcile_stale_runs(&conn, 0) {
            Ok(0) => {}
            Ok(n) => tracing::info!("Reconciled {} zombie audit_runs left 'Running' by a previous process → Interrupted", n),
            Err(e) => tracing::warn!("Failed to reconcile stale audit_runs: {}", e),
        }

        // 0.8.11 (B5) — same reconcile for workflow_runs: a run that was in
        // flight when the process died stays `Running`/`Pending` forever,
        // poisoning the active-runs badge and cron "last run" checks.
        // Cutoff 0 (not 30 min): at BOOT there is no in-process runner state,
        // so every `Running`/`Pending` row is by definition a zombie — a grace
        // window would just leave a freshly-interrupted run lying about its
        // status for up to that long (Copilot review, PR #114).
        let boot_interrupted = match workflows::reconcile_stale_runs(&conn, 0) {
            Ok(v) => {
                if !v.is_empty() {
                    tracing::info!("Reconciled {} zombie workflow_runs left 'Running'/'Pending' by a previous process → Interrupted", v.len());
                }
                v
            }
            Err(e) => {
                tracing::warn!("Failed to reconcile stale workflow_runs: {}", e);
                Vec::new()
            }
        };

        // 0.8.6 — auto-purge api_call_logs older than 90 days at boot.
        // Generous default : keeps a quarter of audit trail for debug
        // while preventing unbounded growth. User can manually trigger
        // a tighter purge via the Settings → API audit "Purge" button.
        match api_call_logs::purge_older_than(&conn, 90) {
            Ok(0) => {}
            Ok(n) => tracing::info!("Purged {} api_call_logs rows older than 90 days", n),
            Err(e) => tracing::warn!("Failed to auto-purge api_call_logs: {}", e),
        }

        // ADR-001 O2 — open the read-only companion connection, ONLY when WAL
        // is actually effective: in DELETE/TRUNCATE journal modes a reader's
        // shared lock can BLOCK the writer, which would be worse than the
        // single-connection status quo (Codex review). Best-effort: any
        // failure degrades to the historical single-connection behaviour.
        let read_conn = if !wal_effective {
            tracing::info!(
                "WAL not effective (KRONN_DB_WAL=0 or filesystem fallback) — \
                 skipping the read-only companion connection, reads share the \
                 write connection (pre-O2 behaviour)"
            );
            None
        } else {
            match Self::open_read_connection(path) {
                Ok(c) => Some(Arc::new(Mutex::new(c))),
                Err(e) => {
                    tracing::warn!(
                        "Could not open the read-only DB connection ({e}) — heavy reads \
                         will share the write connection (pre-O2 behaviour)"
                    );
                    None
                }
            }
        };

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            read_conn,
            path: path.clone(),
            boot_interrupted: Mutex::new(boot_interrupted),
        })
    }

    /// The O2 read companion, opened with SQLite's READ_ONLY flag — the
    /// guarantee lives in the file handle itself, not in a per-connection
    /// pragma a future closure could flip back (Copilot round 3).
    /// `query_only=1` stays as a second, cheap belt.
    fn open_read_connection(path: &PathBuf) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("read connection at {}", path.display()))?;
        conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA query_only=1;")?;
        Ok(conn)
    }

    /// Get the database file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Drain the boot-reconciled Interrupted runs (once). Returns empty on
    /// every subsequent call — the boot notifier is the single consumer.
    pub fn take_boot_interrupted(&self) -> Vec<workflows::ReconciledRun> {
        self.boot_interrupted
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }

    /// ADR-001 O2 — execute a READ-ONLY closure on the dedicated read
    /// connection (WAL snapshot, never blocked by the writer). Falls back to
    /// the write connection when no read connection exists: in-memory DBs,
    /// a failed open at boot, or WAL not effective (KRONN_DB_WAL=0 /
    /// filesystem fallback — a DELETE-mode reader could BLOCK the writer).
    /// When the dedicated connection is active, `PRAGMA query_only` makes
    /// any write attempt through this path an immediate SQLite error.
    pub async fn with_read_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = match &self.read_conn {
            Some(rc) => rc.clone(),
            None => self.conn.clone(),
        };
        Self::run_on(conn, f).await
    }

    /// Execute a blocking closure with the database connection.
    /// Runs inside `spawn_blocking` so the Tokio worker thread is never blocked
    /// waiting on the mutex or executing a synchronous SQLite query.
    pub async fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        Self::run_on(self.conn.clone(), f).await
    }

    /// Shared executor for both connections: spawn_blocking + poison
    /// recovery + panic containment (0.8.11 hardening semantics).
    async fn run_on<F, T>(conn: Arc<Mutex<Connection>>, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(move || {
            // Poison is recoverable here: a panicked closure can't leave
            // SQLite mid-transaction (rusqlite rolls back on drop), while
            // treating poison as fatal turns one panic into a full outage.
            let guard = match conn.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("DB mutex was poisoned by a previous panic — recovering the lock");
                    // Clear the flag or every later lock() re-enters this
                    // error path (log spam on each DB call, forever).
                    conn.clear_poison();
                    poisoned.into_inner()
                }
            };
            // Catch panics BEFORE they unwind through the guard: the mutex
            // never poisons, and the panic message reaches the API error.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&guard))) {
                Ok(r) => r,
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "non-string panic payload".into());
                    tracing::error!("DB closure panicked: {msg}");
                    Err(anyhow::anyhow!("DB closure panicked: {msg}"))
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking failed: {e}"))?
    }
}
