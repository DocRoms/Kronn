//! A signal that a stored resource changed, raised by SQLite itself.
//!
//! The background warm-up of the rendered resources (KT-915) has to run again
//! after a resource is edited. Resources are written from many places — the
//! API, the agent tools, imports, workflow runs publishing page data — so
//! instead of asking each writer to announce itself, the write connection
//! reports every row it touches and the tables a rendering is made from raise
//! the signal. Nothing is read or copied here: a signal only says "something
//! changed", the warm-up decides what to do about it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::Notify;

/// The tables a repository resource is rendered from: the five kinds of
/// resource Kronn holds, an Artifact's revisions and data, and the identity
/// that names the file each one is written to.
const RENDERED_FROM: &[&str] = &[
    "workflows",
    "quick_prompts",
    "quick_apis",
    "quick_execs",
    "live_pages",
    "live_page_revisions",
    "live_page_datasets",
    "live_page_dataset_points",
    "resource_identities",
];

#[derive(Default)]
pub struct ResourceChanges {
    notify: Notify,
    raised: AtomicU64,
}

impl ResourceChanges {
    /// Says that a resource changed. Never blocks and never fails; changes
    /// raised while nobody waits are kept as one, so none is lost.
    pub fn raise(&self) {
        self.raised.fetch_add(1, Ordering::Release);
        self.notify.notify_one();
    }

    /// What a write to `table` means: a change, when a rendering is made of it.
    pub fn observe_write(&self, table: &str) {
        if RENDERED_FROM.contains(&table) {
            self.raise();
        }
    }

    /// How many times the signal was raised since the database was opened.
    pub fn raised(&self) -> u64 {
        self.raised.load(Ordering::Acquire)
    }

    /// Resolves once a change was raised since the last time this resolved.
    pub async fn changed(&self) {
        self.notify.notified().await;
    }
}

/// Has `changes` hear every write `conn` makes. Set on the write connection
/// only: the read connection cannot change anything. A hook that cannot be set
/// costs the warm-up its re-runs, never a write.
pub(super) fn watch(conn: &Connection, changes: &Arc<ResourceChanges>) {
    let changes = Arc::clone(changes);
    let hooked = conn.update_hook(Some(
        move |_action: rusqlite::hooks::Action, _database: &str, table: &str, _row: i64| {
            changes.observe_write(table);
        },
    ));
    if let Err(error) = hooked {
        tracing::warn!("Resource change hook not set ({error}): the warm-up will not follow edits");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database_with_signal() -> (crate::db::Database, Arc<ResourceChanges>) {
        let database = crate::db::Database::open_in_memory().unwrap();
        let changes = database.resource_changes();
        (database, changes)
    }

    #[tokio::test]
    async fn writing_a_resource_raises_the_signal_and_writing_anything_else_does_not() {
        let (database, changes) = database_with_signal();
        let quiet = changes.raised();

        // A row of a table no rendering is made of.
        database
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TABLE unrelated_rows (value TEXT);
                     INSERT INTO unrelated_rows VALUES ('a');
                     UPDATE unrelated_rows SET value = 'b';
                     DELETE FROM unrelated_rows;",
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        assert_eq!(
            changes.raised(),
            quiet,
            "rows of another table are not a resource"
        );

        database
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &exec())?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        assert!(changes.raised() > quiet, "a Quick Exec is");
        tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
            .await
            .expect("a waiter is woken by the change");
    }

    /// The production shape: a file, a write connection and a read companion.
    /// The signal comes from the writer, and the reader sees what it announced.
    #[tokio::test]
    async fn a_database_file_raises_the_signal_and_its_reader_sees_the_change() {
        let directory = tempfile::tempdir().unwrap();
        let database = crate::db::Database::open_path(&directory.path().join("kronn.db")).unwrap();
        let changes = database.resource_changes();
        let quiet = changes.raised();

        database
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &exec())?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        assert!(changes.raised() > quiet);
        tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
            .await
            .expect("the writer's change is announced");
        let seen = database
            .with_read_conn(|conn| {
                Ok(crate::db::quick_execs::get_quick_exec(conn, "qe-1")?.is_some())
            })
            .await
            .unwrap();
        assert!(seen, "and the read connection already holds it");
    }

    #[tokio::test]
    async fn changes_raised_while_nobody_waits_are_not_lost() {
        let (database, changes) = database_with_signal();
        database
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &exec())?;
                crate::db::quick_execs::update_quick_exec(conn, &exec())?;
                crate::db::quick_execs::delete_quick_exec(conn, "qe-1")?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        assert!(changes.raised() >= 3, "insert, update and delete all count");
        // Three changes, one wake-up: whoever waits next runs once, not thrice.
        tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
            .await
            .expect("the change made before anyone waited is still there");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), changes.changed())
                .await
                .is_err(),
            "and it is consumed by that wake-up"
        );
    }

    fn exec() -> crate::models::QuickExec {
        let timestamp = chrono::Utc::now();
        crate::models::QuickExec {
            id: "qe-1".into(),
            name: "Lint".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: None,
            command: "cargo".into(),
            args: vec!["check".into()],
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
        }
    }
}
