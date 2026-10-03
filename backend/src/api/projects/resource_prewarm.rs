//! Background warm-up of the rendered resources (KT-915).
//!
//! Rendering a resource masks every string it holds, and the result is kept in
//! the render memo under a fingerprint of its content (see
//! `core::repository_resources`). Left to requests, the memo is empty after
//! every restart and the first listing of a project pays for all of it: on a
//! project with a hundred automations, most of a second.
//!
//! This task renders them ahead of time. It starts a moment after the backend
//! is up, and again shortly after the resources of the database are edited (the
//! write connection reports them, see `db::resource_changes`), once the writes
//! stop for a while. It stays out of the way:
//!
//! - it never delays the startup nor a request: it runs on its own task, reads
//!   the database in short slices — one project's automations, then one
//!   Artifact at a time — so the read connection is free between them, and
//!   masks off the database lock, in `spawn_blocking`, with a pause between two
//!   resources;
//! - it fills the very memo the listing reads and stops when that memo is
//!   nearly full, so it never grows past the memory the memo already may take
//!   nor evicts what it has just computed;
//! - it stops at shutdown, between two resources.
//!
//! Skills are not warmed: their rendering is pinned to a date that comes from
//! each project's alignment records and its repository files.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::resources::{resource_slug, typed_seeds, ResourceSeed};
use crate::db::Database;

/// When a run starts, and how patient it is with the rest of the backend.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// Wait after the backend starts, so the boot and the first requests are
    /// served first.
    pub startup: Duration,
    /// Quiet time after the last edit before the resources are warmed again.
    pub debounce: Duration,
    /// A steady stream of writes (a workflow filling a dataset) cannot keep
    /// the run waiting longer than this.
    pub max_debounce: Duration,
    /// Pause after each resource, so a request never queues behind a run of
    /// them.
    pub breath: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            startup: Duration::from_secs(2),
            debounce: Duration::from_millis(1500),
            max_debounce: Duration::from_secs(15),
            breath: Duration::from_millis(2),
        }
    }
}

/// What the warm-up has done so far, for diagnostics and tests.
#[derive(Debug, Default)]
pub struct Progress {
    /// Runs over all the projects that went to the end or were stopped.
    pub passes: AtomicU64,
    /// Resources handed to the renderer (already memoized ones included).
    pub rendered: AtomicU64,
}

/// How long stopping the warm-up waits for the resource it is in the middle of.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// The warm-up of a running backend: started with it, stopped with it.
pub struct Prewarm {
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl Prewarm {
    /// Starts the warm-up on its own task and returns at once.
    pub fn start(db: Arc<Database>) -> Self {
        let shutdown = CancellationToken::new();
        let task = spawn(db, shutdown.clone());
        Self { shutdown, task }
    }

    /// Cancelling this stops the warm-up, from anywhere (a graceful-shutdown
    /// future, say) and without waiting for it.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Stops the warm-up and waits for it to leave — between two resources, so
    /// a moment at most — rather than cut a rendering short.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = tokio::time::timeout(STOP_GRACE, self.task).await;
    }
}

/// Starts the warm-up on its own task and returns at once. `shutdown` stops it.
pub fn spawn(db: Arc<Database>, shutdown: CancellationToken) -> JoinHandle<()> {
    spawn_with(db, shutdown, Timings::default(), Arc::default())
}

pub fn spawn_with(
    db: Arc<Database>,
    shutdown: CancellationToken,
    timings: Timings,
    progress: Arc<Progress>,
) -> JoinHandle<()> {
    tokio::spawn(run(db, shutdown, timings, progress))
}

/// Waits `duration`; `false` when the shutdown came first.
async fn pause(shutdown: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

async fn run(
    db: Arc<Database>,
    shutdown: CancellationToken,
    timings: Timings,
    progress: Arc<Progress>,
) {
    let changes = db.resource_changes();
    if !pause(&shutdown, timings.startup).await {
        return;
    }
    // Whatever was written before now is in the first run: it is not a reason
    // for a second one.
    let _ = tokio::time::timeout(Duration::ZERO, changes.changed()).await;
    loop {
        warm_all(&db, &shutdown, timings.breath, &progress).await;
        progress.passes.fetch_add(1, Ordering::Release);

        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = changes.changed() => {}
        }
        // The resources changed: wait for the edits to stop, within reason.
        let first_edit = Instant::now();
        loop {
            let left = timings.max_debounce.saturating_sub(first_edit.elapsed());
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = changes.changed(), if !left.is_zero() => {}
                _ = tokio::time::sleep(timings.debounce.min(left)) => break,
            }
        }
    }
}

/// One run over every project.
async fn warm_all(
    db: &Arc<Database>,
    shutdown: &CancellationToken,
    breath: Duration,
    progress: &Progress,
) {
    let projects = db
        .with_read_conn(|conn| {
            Ok(crate::db::projects::list_projects(conn)?
                .into_iter()
                .map(|project| project.id)
                .collect::<Vec<_>>())
        })
        .await;
    let projects = match projects {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!("Resource warm-up: cannot list the projects: {error}");
            return;
        }
    };
    for project_id in projects {
        if let Err(error) = warm_project(db, &project_id, shutdown, breath, progress).await {
            tracing::warn!("Resource warm-up of project {project_id} failed: {error}");
        }
        if shutdown.is_cancelled() {
            return;
        }
    }
}

/// Renders the resources of one project into the render memo: the same ones,
/// under the same slugs, as its listing renders. Returns how many were handed
/// to the renderer.
pub async fn warm_project(
    db: &Arc<Database>,
    project_id: &str,
    shutdown: &CancellationToken,
    breath: Duration,
    progress: &Progress,
) -> anyhow::Result<usize> {
    warm_project_while(db, project_id, shutdown, breath, progress, &|| {
        crate::core::repository_resources::render_memo_has_room()
    })
    .await
}

/// [`warm_project`], stopping as soon as `has_room` says the memo is full.
pub(super) async fn warm_project_while(
    db: &Arc<Database>,
    project_id: &str,
    shutdown: &CancellationToken,
    breath: Duration,
    progress: &Progress,
    has_room: &(dyn Fn() -> bool + Sync),
) -> anyhow::Result<usize> {
    let mut rendered = 0;

    // The automations of the project, read in one short slice of the read
    // connection: they are small, and each is rendered afterwards, off it.
    let id = project_id.to_string();
    let (project_key, planned, pages) = db
        .with_read_conn(move |conn| {
            let project_key = crate::db::resource_identities::project_key(conn, Some(&id))?;
            let mut planned = Vec::new();
            for seed in typed_seeds(conn, &id)? {
                let slug = resource_slug(conn, &project_key, &seed)?;
                planned.push((seed, slug));
            }
            let pages = crate::db::live_pages::list_live_pages_for_project(conn, &id)?;
            Ok((project_key, planned, pages))
        })
        .await?;

    for (seed, slug) in planned {
        if shutdown.is_cancelled() || !has_room() {
            return Ok(rendered);
        }
        render_seed(seed, slug, breath, progress).await;
        rendered += 1;
    }

    // An Artifact is the heaviest read — its revision and every dataset with
    // its points — so each gets a slice of its own.
    for page in pages {
        if shutdown.is_cancelled() || !has_room() {
            return Ok(rendered);
        }
        let project_key = project_key.clone();
        let read = db
            .with_read_conn(move |conn| {
                let seed = ResourceSeed::artifact(conn, page)?;
                let slug = resource_slug(conn, &project_key, &seed)?;
                Ok((seed, slug))
            })
            .await;
        match read {
            Ok((seed, slug)) => {
                render_seed(seed, slug, breath, progress).await;
                rendered += 1;
            }
            // One unreadable Artifact does not keep the others cold.
            Err(error) => tracing::debug!("Resource warm-up skips an Artifact: {error}"),
        }
    }
    Ok(rendered)
}

/// Masks one resource into the memo, on a blocking thread and holding no
/// database connection, then lets the rest of the backend go first.
async fn render_seed(seed: ResourceSeed, slug: String, breath: Duration, progress: &Progress) {
    let outcome =
        tokio::task::spawn_blocking(move || seed.loaded.render(&slug, None).map(|_| ())).await;
    match outcome {
        Ok(Ok(())) => {}
        // A resource that does not render is reported by its own listing.
        Ok(Err(error)) => tracing::debug!("Resource warm-up: a resource did not render: {error}"),
        Err(error) => tracing::warn!("Resource warm-up: the renderer stopped: {error}"),
    }
    progress.rendered.fetch_add(1, Ordering::Release);
    if breath.is_zero() {
        tokio::task::yield_now().await;
    } else {
        tokio::time::sleep(breath).await;
    }
}
