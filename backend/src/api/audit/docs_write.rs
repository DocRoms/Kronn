use std::sync::{Arc, Mutex};

use crate::{AppState, AuditTracker};

pub(crate) const DOCS_BUSY: &str =
    "An audit or documentation update is currently running on this project — wait for it to finish.";

#[derive(Clone)]
pub(crate) struct DocsWriteLease(Arc<LeaseOwner>);

struct LeaseOwner {
    tracker: Arc<Mutex<AuditTracker>>,
    project_id: String,
}

impl Drop for LeaseOwner {
    fn drop(&mut self) {
        if let Ok(mut tracker) = self.tracker.lock() {
            tracker.release_lease(&self.project_id);
        }
    }
}

impl DocsWriteLease {
    pub(crate) fn try_acquire(state: &AppState, project_id: &str) -> Result<Self, String> {
        let mut tracker = state.audit_tracker.lock().map_err(|_| DOCS_BUSY)?;
        if !tracker.try_acquire_lease(project_id) {
            return Err(DOCS_BUSY.into());
        }
        Ok(Self(Arc::new(LeaseOwner {
            tracker: state.audit_tracker.clone(),
            project_id: project_id.into(),
        })))
    }

    pub(crate) async fn acquire(state: &AppState, project_id: &str) -> Result<Self, String> {
        let lease = Self::try_acquire(state, project_id)?;
        super::full::stop_validation_agents(state, project_id).await;
        Ok(lease)
    }

    /// The blocking worker keeps the lease even if its HTTP waiter is dropped.
    pub(crate) async fn blocking<T, F>(&self, write: F) -> Result<T, tokio::task::JoinError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let lease = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            write()
        })
        .await
    }
}

pub(crate) async fn validation_cancel_guard(
    state: &AppState,
    discussion_id: String,
    dispatch_id: Option<String>,
) -> Result<crate::CancelGuard, String> {
    let tracker = state.audit_tracker.clone();
    let registry = state.cancel_registry.clone();
    state
        .db
        .with_read_conn(move |conn| {
            if let Some(project) =
                crate::db::audit_runs::project_of_validation_discussion(conn, &discussion_id)?
            {
                let tracker = tracker.lock().map_err(|_| anyhow::anyhow!(DOCS_BUSY))?;
                if tracker.leased.contains(&project) {
                    anyhow::bail!(DOCS_BUSY);
                }
                if let Some(id) = dispatch_id.as_deref() {
                    let running = crate::db::agent_dispatch::get(conn, id)?.is_some_and(|job| {
                        job.status == crate::db::agent_dispatch::DispatchStatus::Running
                    });
                    if !running {
                        anyhow::bail!("The validation dispatch was stopped before it could start.");
                    }
                }
                // Admission and lease acquisition share the mutex: the next writer
                // either refuses this admission or can see and stop its token.
                return Ok(crate::CancelGuard::insert(
                    &registry,
                    dispatch_id.unwrap_or(discussion_id),
                ));
            }
            Ok(crate::CancelGuard::insert(
                &registry,
                dispatch_id.unwrap_or(discussion_id),
            ))
        })
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn docs_writer_lease_survives_http_cancellation_until_the_write_finishes() {
        let state = AppState::new_defaults(
            Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            Arc::new(crate::db::Database::open_in_memory().unwrap()),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("finished");
        let target = path.clone();
        let lease = DocsWriteLease::try_acquire(&state, "p1").unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let request = tokio::spawn(async move {
            lease
                .blocking(move || {
                    started_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap();
                    std::fs::write(target, "complete").unwrap();
                    finished_tx.send(()).unwrap();
                })
                .await
                .unwrap();
        });
        started_rx.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(DocsWriteLease::try_acquire(&state, "p1").is_err());
        assert!(!path.exists());
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(lease) = DocsWriteLease::try_acquire(&state, "p1") {
                    drop(lease);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "complete");
    }
}
