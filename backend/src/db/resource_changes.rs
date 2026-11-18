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
pub(super) fn watch(conn: &mut Connection, changes: &Arc<ResourceChanges>) {
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
