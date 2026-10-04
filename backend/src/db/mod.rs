pub mod acp_runtime_sessions;
pub mod agent_decisions;
pub mod agent_dispatch;
pub mod agent_jobs;
pub mod api_call_logs;
pub mod audit_runs;
pub mod cli_telemetry;
pub(crate) mod cli_worker_bindings;
pub mod compare;
pub mod contacts;
pub mod context_audits;
pub mod delivery_summaries;
pub mod disc_source;
pub mod discussion_actions;
pub mod discussion_ceiling_requests;
pub mod discussion_effort;
pub mod discussion_important;
pub(crate) mod discussion_launch_settings;
pub mod discussion_monitor;
pub mod discussion_questions;
pub mod discussion_sessions;
pub mod discussion_video_sequences;
pub mod discussion_weight;
pub mod discussion_workspaces;
pub mod discussions;
pub mod execution_variable_snapshots;
pub mod external_api_connections;
pub mod human_credentials;
pub mod id_resolver;
pub(crate) mod kronn_action_engine;
pub mod learnings;
pub mod lite_llm_model_failures;
pub mod live_page_actions;
pub mod live_pages;
pub mod mcps;
pub mod media_jobs;
pub mod message_usage;
pub mod migrations;
pub mod model_catalog;
pub mod orchestration;
pub mod planning;
pub mod planning_proposals;
pub mod project_skill_references;
pub mod projects;
pub mod quick_apis;
pub mod quick_exec_runs;
pub mod quick_execs;
pub mod quick_prompts;
pub mod repository_resources;
pub mod resource_changes;
pub mod resource_identities;
pub mod review_ledger;
pub mod run_outcome;
pub mod run_state;
pub mod shared_runs;
pub mod ui_preferences;
pub mod worker_deliveries;
pub mod worker_offers;
pub mod worker_reviews;
pub mod workflow_step_rooms;
pub mod workflows;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
mod release_gate_tests;

/// `EXPLAIN QUERY PLAN` details of `sql`, one line per plan node.
#[cfg(test)]
pub(crate) fn query_plan(conn: &Connection, sql: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("query plan");
    statement
        .query_map([], |row| row.get::<_, String>(3))
        .expect("query plan rows")
        .collect::<rusqlite::Result<_>>()
        .expect("query plan details")
}

/// Plan lines that read one of `aliases` from its table rather than from a
/// covering index. On `workflow_runs` such a read walks the run's step results
/// to reach any later column.
#[cfg(test)]
pub(crate) fn table_reads_outside_index(plan: &[String], aliases: &[&str]) -> Vec<String> {
    plan.iter()
        .filter(|line| {
            let mut words = line.split_whitespace();
            let verb = words.next().unwrap_or_default();
            let target = words.next().unwrap_or_default();
            matches!(verb, "SCAN" | "SEARCH")
                && aliases.contains(&target)
                && !line.contains("COVERING INDEX")
        })
        .cloned()
        .collect()
}

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::core::config;

fn parse_dt(s: String) -> DateTime<Utc> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
        return dt.with_timezone(&Utc);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S") {
        return dt.and_utc();
    }

    tracing::warn!("Failed to parse datetime '{}', using now()", s);
    Utc::now()
}

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
    catalog_refresh_locks: Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Raised whenever the write connection changes a row a repository
    /// resource is rendered from — see [`resource_changes`].
    resource_changes: Arc<resource_changes::ResourceChanges>,
}

impl Database {
    /// The signal of resource writes, for whoever has to react to them.
    pub fn resource_changes(&self) -> Arc<resource_changes::ResourceChanges> {
        Arc::clone(&self.resource_changes)
    }

    /// Open (or create) the database file in the Kronn data directory.
    pub fn open() -> Result<Self> {
        let dir = config::config_dir()?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("kronn.db");
        let db = Self::open_path_for_backend_boot(&path)?;
        // After the open, so the WAL and shared-memory files exist too.
        restrict_data_dir_to_owner(&dir);
        Ok(db)
    }

    /// Open an in-memory database (useful for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory database")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        migrations::run(&conn)?;
        let resource_changes = Arc::new(resource_changes::ResourceChanges::default());
        resource_changes::watch(&conn, &resource_changes);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            read_conn: None,
            path: PathBuf::from(":memory:"),
            boot_interrupted: Mutex::new(Vec::new()),
            catalog_refresh_locks: Mutex::new(std::collections::HashMap::new()),
            resource_changes,
        })
    }

    /// Open a database at a specific path (useful for testing).
    pub fn open_path(path: &PathBuf) -> Result<Self> {
        Self::open_path_internal(path, false)
    }

    /// Open the one production database after the caller has acquired the
    /// process-wide data-directory lock. Only this mode may discard commit
    /// leases: no Git worker from an earlier backend can still be live.
    pub(crate) fn open_path_for_backend_boot(path: &PathBuf) -> Result<Self> {
        Self::open_path_internal(path, true)
    }

    fn open_path_internal(path: &PathBuf, recover_commit_leases: bool) -> Result<Self> {
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
            let actual_mode: String = conn
                .query_row("PRAGMA journal_mode=WAL;", [], |row| row.get(0))
                .context("Failed to set WAL journal mode")?;
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
            conn.execute_batch(
                "PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
            )?;
        }

        // Run migrations before wrapping in Mutex (avoids blocking_lock inside async runtime).
        // Pass db path so a backup is created before pending migrations.
        migrations::run_with_backup(&conn, Some(path))?;

        if recover_commit_leases {
            let recovered_commit_leases =
                orchestration::recover_spawned_commit_leases_after_restart(&conn)
                    .context("Failed to reconcile spawned commit leases after restart")?;
            if recovered_commit_leases > 0 {
                tracing::info!(
                    "Reconciled {recovered_commit_leases} spawned commit lease(s) left by the previous backend"
                );
            }
        }

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
        match workflow_step_rooms::revoke_all_after_restart(&conn) {
            Ok(0) => {}
            Ok(n) => tracing::info!("Revoked {n} room membership(s) of workflow steps that died with the previous process"),
            Err(e) => tracing::warn!("Failed to revoke stale workflow step room memberships: {e}"),
        }
        match shared_runs::repair_stale_workflow_projections(&conn) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "Re-synced {} shared runs still marked live for finished workflow runs",
                n
            ),
            Err(e) => tracing::warn!("Failed to repair stale shared-run projections: {}", e),
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

        // Watched once everything the open itself writes (migrations, the
        // boot reconcile) is behind: the warm-up starts from what is there.
        let resource_changes = Arc::new(resource_changes::ResourceChanges::default());
        resource_changes::watch(&conn, &resource_changes);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            read_conn,
            path: path.clone(),
            boot_interrupted: Mutex::new(boot_interrupted),
            catalog_refresh_locks: Mutex::new(std::collections::HashMap::new()),
            resource_changes,
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

    pub(crate) async fn lock_catalog_refresh(
        &self,
        target: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>> {
        let lock = self
            .catalog_refresh_locks
            .lock()
            .map_err(|_| anyhow::anyhow!("catalogue refresh lock poisoned"))?
            .entry(target.to_string())
            .or_default()
            .clone();
        Ok(lock.lock_owned().await)
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
                    tracing::error!(
                        "DB mutex was poisoned by a previous panic — recovering the lock"
                    );
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

/// The data directory holds the database, its backups and copies of the
/// encryption key: no other account on the machine may read them (KT-990).
/// Agents run as the same user, so this does not keep them out; that is the
/// isolation work planned for 0.14.3.
pub(crate) fn restrict_data_dir_to_owner(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let restrict = |path: &std::path::Path, mode: u32| {
            if let Err(error) =
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            {
                tracing::warn!(
                    "Could not restrict {} to its owner: {error}",
                    path.display()
                );
            }
        };
        restrict(dir, 0o700);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let is_database_file = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("kronn.db"));
            if is_database_file && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                restrict(&entry.path(), 0o600);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(all(test, unix))]
mod data_dir_permission_tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_data_dir_and_every_database_file_end_up_owner_only() {
        // KT-990 — kronn.db, its WAL/SHM and its backups were 0644 in a 0755
        // directory on Linux: readable by any account on the machine.
        let dir = tempfile::tempdir().unwrap();
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let database_files = [
            "kronn.db",
            "kronn.db-wal",
            "kronn.db-shm",
            "kronn.db.backup",
            "kronn.db.pre-0.14.2-20260927",
        ];
        for name in database_files.iter().chain(["notes.txt"].iter()) {
            let path = dir.path().join(name);
            std::fs::write(&path, b"x").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        super::restrict_data_dir_to_owner(dir.path());

        assert_eq!(mode(dir.path()), 0o700);
        for name in database_files {
            assert_eq!(mode(&dir.path().join(name)), 0o600, "{name}");
        }
        assert_eq!(
            mode(&dir.path().join("notes.txt")),
            0o644,
            "files that are not the database are left alone"
        );
    }
}
