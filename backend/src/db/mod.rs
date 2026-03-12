pub mod migrations;
pub mod projects;
pub mod discussions;
pub mod mcps;
pub mod workflows;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::PathBuf;
use anyhow::{Context, Result};
use rusqlite::Connection;
use tokio::sync::Mutex;

use crate::core::config;

/// Thread-safe database handle.
/// rusqlite::Connection is not Send, so we wrap in a sync Mutex
/// accessed only via `tokio::task::spawn_blocking`.
pub struct Database {
    conn: Mutex<Connection>,
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
        Ok(Self { conn: Mutex::new(conn), path: PathBuf::from(":memory:") })
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
        if use_wal {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        } else {
            tracing::warn!("WAL mode disabled (KRONN_DB_WAL=0); using DELETE journal mode");
            conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON;")?;
        }

        // Run migrations before wrapping in Mutex (avoids blocking_lock inside async runtime)
        migrations::run(&conn)?;

        Ok(Self { conn: Mutex::new(conn), path: path.clone() })
    }

    /// Get the database file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Execute a blocking closure with the database connection.
    /// All DB access goes through this to avoid holding async locks.
    pub async fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.lock().await;
        // rusqlite Connection is not Send, so we can't use spawn_blocking directly.
        // Instead we hold the mutex and run synchronously (SQLite ops are fast).
        f(&conn)
    }
}
