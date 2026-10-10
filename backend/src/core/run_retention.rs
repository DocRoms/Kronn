//! KT-984 — periodic run retention. A pass trims the step outputs of old runs
//! (`server.run_payload_retention_days`, off until the user chooses a window; Settings suggests 30) and, when opted in,
//! deletes old runs (`server.run_retention_days`). KT-1100 splits deletion by
//! class: no-op runs go after [`DEFAULT_NO_OP_RETENTION_HOURS`], and a
//! workflow's own retention overrides each global window. All go through
//! `db::run_retention`, in chunks, pausing between them so the shared write
//! connection keeps serving requests. Never at boot: the first pass waits.
//!
//! Spawned in BOTH binaries (`backend/src/main.rs` and
//! `desktop/src-tauri/src/main.rs`) through [`spawn`].
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::db::run_retention::{
    compact_run_payloads_chunk, cutoff, cutoff_hours, delete_runs_chunk, workflow_retentions,
    RunClass, WorkflowScope, CHUNK_ROWS,
};
use crate::db::Database;
use crate::models::{AppConfig, WorkflowRetention};

/// How long a run that changed nothing is kept unless its workflow says
/// otherwise.
pub const DEFAULT_NO_OP_RETENTION_HOURS: u32 = 24;

const FIRST_PASS_DELAY: Duration = Duration::from_secs(10 * 60);
const PASS_INTERVAL: Duration = Duration::from_secs(6 * 3600);
const CHUNK_PAUSE: Duration = Duration::from_millis(200);

/// The global retention windows; zero disables one.
#[derive(Debug, Clone, Copy)]
pub struct RetentionDays {
    pub payload: u32,
    /// Successful and failed runs, in days.
    pub delete: u32,
    /// Runs that changed nothing, in hours.
    pub no_op_hours: u32,
}

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassReport {
    pub trimmed: usize,
    /// Every deleted run, no-op ones included.
    pub deleted: usize,
    pub deleted_no_op: usize,
}

/// A window in hours: no-op runs count in hours, the others in days.
fn window_hours(class: RunClass, global: RetentionDays, own: Option<&WorkflowRetention>) -> u32 {
    let days = |value: u32| value.saturating_mul(24);
    match class {
        RunClass::NoOp => own
            .and_then(|r| r.no_op_hours)
            .unwrap_or(global.no_op_hours),
        RunClass::Success => days(own.and_then(|r| r.success_days).unwrap_or(global.delete)),
        RunClass::Failure => days(own.and_then(|r| r.failure_days).unwrap_or(global.delete)),
    }
}

fn overrides(class: RunClass, retention: &WorkflowRetention) -> bool {
    match class {
        RunClass::NoOp => retention.no_op_hours.is_some(),
        RunClass::Success => retention.success_days.is_some(),
        RunClass::Failure => retention.failure_days.is_some(),
    }
}

/// Repeat `chunk` until it returns fewer than [`CHUNK_ROWS`], releasing the
/// write connection for `pause` between two chunks.
async fn drain<F>(db: &Database, cutoff: String, pause: Duration, chunk: F) -> anyhow::Result<usize>
where
    F: Fn(&rusqlite::Connection, &str, usize) -> anyhow::Result<usize> + Clone + Send + 'static,
{
    let mut total = 0;
    loop {
        let cutoff = cutoff.clone();
        let chunk = chunk.clone();
        let changed = db
            .with_conn(move |conn| chunk(conn, &cutoff, CHUNK_ROWS))
            .await?;
        total += changed;
        if changed < CHUNK_ROWS {
            return Ok(total);
        }
        tokio::time::sleep(pause).await;
    }
}

/// One retention pass.
pub async fn run_pass(
    db: &Database,
    days: RetentionDays,
    pause: Duration,
) -> anyhow::Result<PassReport> {
    let now = chrono::Utc::now();
    let mut report = PassReport::default();
    if days.payload > 0 {
        report.trimmed = drain(
            db,
            cutoff(now, days.payload),
            pause,
            compact_run_payloads_chunk,
        )
        .await?;
    }
    let retentions = db.with_conn(workflow_retentions).await?;
    for class in RunClass::ALL {
        let mut own_ids = Vec::new();
        let mut deleted = 0;
        for (id, retention) in retentions.iter().filter(|(_, r)| overrides(class, r)) {
            own_ids.push(id.clone());
            let hours = window_hours(class, days, Some(retention));
            if hours > 0 {
                let scope = WorkflowScope::Only(id.clone());
                deleted += drain_class(db, class, scope, cutoff_hours(now, hours), pause).await?;
            }
        }
        let hours = window_hours(class, days, None);
        if hours > 0 {
            let scope = WorkflowScope::Except(own_ids);
            deleted += drain_class(db, class, scope, cutoff_hours(now, hours), pause).await?;
        }
        report.deleted += deleted;
        if class == RunClass::NoOp {
            report.deleted_no_op = deleted;
        }
    }
    Ok(report)
}

async fn drain_class(
    db: &Database,
    class: RunClass,
    scope: WorkflowScope,
    cutoff: String,
    pause: Duration,
) -> anyhow::Result<usize> {
    drain(db, cutoff, pause, move |conn, cutoff, limit| {
        delete_runs_chunk(conn, class, &scope, cutoff, limit)
    })
    .await
}

/// Start the periodic pass. The settings are read again before each pass, so
/// a change in Settings applies at the next one.
pub fn spawn(db: Arc<Database>, config: Arc<RwLock<AppConfig>>) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;
        let mut tick = tokio::time::interval(PASS_INTERVAL);
        loop {
            tick.tick().await;
            let days = {
                let config = config.read().await;
                RetentionDays {
                    payload: config.server.run_payload_retention_days,
                    delete: config.server.run_retention_days,
                    no_op_hours: DEFAULT_NO_OP_RETENTION_HOURS,
                }
            };
            match run_pass(&db, days, CHUNK_PAUSE).await {
                Ok(report) if report == PassReport::default() => {}
                Ok(report) => tracing::info!(
                    target: "run_retention",
                    "run retention: trimmed the step outputs of {} run(s) older than {} days, deleted {} run(s), {} of them without changes",
                    report.trimmed,
                    days.payload,
                    report.deleted,
                    report.deleted_no_op
                ),
                Err(e) => tracing::warn!(target: "run_retention", "run retention pass failed: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seeded(runs: usize) -> Database {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                 VALUES ('wf', 'wf', '\"Manual\"', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
                [],
            )?;
            let payload = serde_json::json!([{"step_name": "s", "status": "Success",
                "output": "o".repeat(20_000), "duration_ms": 1}])
            .to_string();
            for index in 0..runs {
                conn.execute(
                    "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json,
                         started_at, finished_at, run_type)
                     VALUES (?1, 'wf', 'Success', ?2, '2026-01-01T00:00:00+00:00',
                             '2026-01-01T00:00:00+00:00', 'linear')",
                    rusqlite::params![format!("run-{index:03}"), payload],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    async fn trimmed(db: &Database) -> i64 {
        db.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM workflow_runs WHERE payload_compacted_at IS NOT NULL",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_pass_trims_every_eligible_run_and_reports_it() {
        let db = seeded(CHUNK_ROWS * 2 + 1).await;
        let report = run_pass(
            &db,
            RetentionDays {
                payload: 30,
                delete: 0,
                no_op_hours: 0,
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            report,
            PassReport {
                trimmed: CHUNK_ROWS * 2 + 1,
                ..PassReport::default()
            }
        );
        let again = run_pass(
            &db,
            RetentionDays {
                payload: 30,
                delete: 0,
                no_op_hours: 0,
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(again, PassReport::default());
    }

    #[tokio::test]
    async fn disabled_windows_touch_nothing() {
        let db = seeded(3).await;
        let report = run_pass(
            &db,
            RetentionDays {
                payload: 0,
                delete: 0,
                no_op_hours: 0,
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(report, PassReport::default());
        assert_eq!(trimmed(&db).await, 0);
    }

    #[tokio::test]
    async fn other_requests_are_served_between_two_chunks() {
        // KT-984 DAT-3 — one statement held the write connection for the
        // whole purge.
        let total = CHUNK_ROWS * 3;
        let db = Arc::new(seeded(total).await);
        let pass = {
            let db = db.clone();
            tokio::spawn(async move {
                run_pass(
                    &db,
                    RetentionDays {
                        payload: 30,
                        delete: 0,
                        no_op_hours: 0,
                    },
                    Duration::from_millis(80),
                )
                .await
            })
        };
        let mut seen = Vec::new();
        while !pass.is_finished() {
            seen.push(trimmed(&db).await);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        pass.await.unwrap().unwrap();
        assert!(
            seen.iter().any(|&count| count > 0 && count < total as i64),
            "a request ran between chunks: {seen:?}"
        );
        assert_eq!(trimmed(&db).await, total as i64);
    }

    // ─── KT-1100 — per-class and per-workflow deletion ───────────────────

    const GLOBAL: RetentionDays = RetentionDays {
        payload: 0,
        delete: 0,
        no_op_hours: DEFAULT_NO_OP_RETENTION_HOURS,
    };

    /// Runs of `workflow` finished `age_hours` ago; `outcome` NULL when `None`.
    async fn runs(
        db: &Database,
        workflow: &str,
        retention: Option<WorkflowRetention>,
        rows: &[(&str, &str, Option<&str>, i64)],
    ) {
        let workflow = workflow.to_string();
        let rows: Vec<(String, String, Option<String>, i64)> = rows
            .iter()
            .map(|(id, status, outcome, age)| {
                (
                    id.to_string(),
                    status.to_string(),
                    outcome.map(str::to_string),
                    *age,
                )
            })
            .collect();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at,
                     retention_json)
                 VALUES (?1, ?1, '\"Manual\"', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?2)",
                rusqlite::params![
                    workflow,
                    retention.map(|r| serde_json::to_string(&r).unwrap())
                ],
            )?;
            for (id, status, outcome, age) in rows {
                let finished = (chrono::Utc::now() - chrono::Duration::hours(age)).to_rfc3339();
                conn.execute(
                    "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json,
                         started_at, finished_at, run_type, outcome)
                     VALUES (?1, ?2, ?3, '[]', ?4, ?4, 'linear', ?5)",
                    rusqlite::params![id, workflow, status, finished, outcome],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    }

    async fn left(db: &Database) -> Vec<String> {
        db.with_conn(|conn| {
            Ok(conn
                .prepare("SELECT id FROM workflow_runs ORDER BY id")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn an_old_no_op_run_goes_while_a_success_of_the_same_age_stays() {
        let db = Database::open_in_memory().unwrap();
        runs(
            &db,
            "wf",
            None,
            &[
                ("noop-old", "Success", Some("no_op"), 30),
                ("noop-recent", "Success", Some("no_op"), 2),
                ("success-old", "Success", Some("changed"), 30),
                ("unclassified-old", "Success", None, 30),
                ("failed-old", "Failed", None, 30),
            ],
        )
        .await;
        // Every run has its pinned definitions: they go with it, never hold it.
        db.with_conn(|conn| {
            for run in ["noop-old", "success-old"] {
                conn.execute(
                    "INSERT INTO workflow_run_pins (run_id, kind, resource_id, content_json)
                     VALUES (?1, 'run', 'wf', '{}')",
                    [run],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let report = run_pass(&db, GLOBAL, Duration::ZERO).await.unwrap();
        assert_eq!(report.deleted_no_op, 1);
        assert_eq!(report.deleted, 1);
        assert_eq!(
            left(&db).await,
            [
                "failed-old",
                "noop-recent",
                "success-old",
                "unclassified-old"
            ]
        );
        let pinned: Vec<String> = db
            .with_conn(|conn| {
                Ok(conn
                    .prepare("SELECT run_id FROM workflow_run_pins")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .await
            .unwrap();
        assert_eq!(pinned, ["success-old"]);
    }

    #[tokio::test]
    async fn a_workflow_retention_overrides_the_global_one() {
        let db = Database::open_in_memory().unwrap();
        // Global: no-op 24 h, everything else kept (KT-984 deletion off).
        runs(
            &db,
            "short",
            Some(WorkflowRetention {
                no_op_hours: Some(1),
                success_days: Some(1),
                failure_days: None,
            }),
            &[
                ("short-noop", "Success", Some("no_op"), 3),
                ("short-success", "Success", Some("changed"), 30),
                ("short-failed", "Failed", None, 30),
            ],
        )
        .await;
        runs(
            &db,
            "keep",
            Some(WorkflowRetention {
                no_op_hours: Some(0),
                success_days: None,
                failure_days: None,
            }),
            &[("keep-noop", "Success", Some("no_op"), 24 * 400)],
        )
        .await;
        runs(
            &db,
            "plain",
            None,
            &[
                ("plain-noop", "Success", Some("no_op"), 3),
                ("plain-success", "Success", Some("changed"), 30),
            ],
        )
        .await;
        run_pass(&db, GLOBAL, Duration::ZERO).await.unwrap();
        assert_eq!(
            left(&db).await,
            ["keep-noop", "plain-noop", "plain-success", "short-failed"]
        );

        // With a global deletion window, a workflow's `0` still keeps its runs.
        let db = Database::open_in_memory().unwrap();
        runs(
            &db,
            "forever",
            Some(WorkflowRetention {
                no_op_hours: None,
                success_days: Some(0),
                failure_days: Some(0),
            }),
            &[
                ("forever-success", "Success", Some("changed"), 24 * 40),
                ("forever-failed", "Failed", None, 24 * 40),
            ],
        )
        .await;
        runs(
            &db,
            "global",
            None,
            &[("global-success", "Success", None, 24 * 40)],
        )
        .await;
        let global = RetentionDays {
            delete: 30,
            ..GLOBAL
        };
        run_pass(&db, global, Duration::ZERO).await.unwrap();
        assert_eq!(left(&db).await, ["forever-failed", "forever-success"]);
    }

    #[tokio::test]
    async fn a_kept_worktree_or_a_changed_publication_keeps_a_no_op_run() {
        let db = Database::open_in_memory().unwrap();
        runs(
            &db,
            "wf",
            None,
            &[
                ("worktree", "Success", Some("no_op"), 48),
                ("published-same", "Success", Some("no_op"), 48),
                ("published-changed", "Success", Some("no_op"), 48),
            ],
        )
        .await;
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_runs SET workspace_path = '/repo/.kronn/worktrees/kept'
                  WHERE id = 'worktree'",
                [],
            )?;
            conn.execute(
                "INSERT INTO live_pages(id, title, slug, created_at, updated_at)
                 VALUES ('page', 'Page', 'page', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            for (revision, id, run, changed) in [
                (1, "pub-same", "published-same", "[]"),
                (2, "pub-changed", "published-changed", "[\"etat\"]"),
            ] {
                conn.execute(
                    "INSERT INTO live_page_publications (id, page_id, data_revision, workflow_id,
                         workflow_run_id, datasets_json, changed_datasets_json,
                         unchanged_datasets_json, points_added, points_removed, published_at)
                     VALUES (?1, 'page', ?2, 'wf', ?3, '[\"etat\"]', ?4, '[]', 0, 0,
                             '2026-01-01T00:00:00Z')",
                    rusqlite::params![id, revision, run, changed],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
        run_pass(&db, GLOBAL, Duration::ZERO).await.unwrap();
        assert_eq!(left(&db).await, ["published-changed", "worktree"]);
        let orphaned: Option<String> = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT workflow_run_id FROM live_page_publications WHERE id = 'pub-same'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            orphaned, None,
            "the publication stays, unlinked from the run"
        );
    }

    #[tokio::test]
    async fn an_unreadable_retention_keeps_every_run() {
        let db = Database::open_in_memory().unwrap();
        runs(
            &db,
            "corrupt",
            None,
            &[("corrupt-noop", "Success", Some("no_op"), 48)],
        )
        .await;
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflows SET retention_json = '{not json' WHERE id = 'corrupt'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        run_pass(&db, GLOBAL, Duration::ZERO).await.unwrap();
        assert_eq!(left(&db).await, ["corrupt-noop"]);
    }
}
