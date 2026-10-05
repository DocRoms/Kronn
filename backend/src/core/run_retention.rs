//! KT-984 — periodic run retention. A pass trims the step outputs of old runs
//! (`server.run_payload_retention_days`, 30 on a new install, off on an existing one) and, when opted in,
//! deletes old runs (`server.run_retention_days`). Both go through
//! `db::run_retention`, in chunks, pausing between them so the shared write
//! connection keeps serving requests. Never at boot: the first pass waits.
//!
//! Spawned in BOTH binaries (`backend/src/main.rs` and
//! `desktop/src-tauri/src/main.rs`) through [`spawn`].
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::db::run_retention::{compact_run_payloads_chunk, cutoff, delete_runs_chunk, CHUNK_ROWS};
use crate::db::Database;
use crate::models::AppConfig;

const FIRST_PASS_DELAY: Duration = Duration::from_secs(10 * 60);
const PASS_INTERVAL: Duration = Duration::from_secs(6 * 3600);
const CHUNK_PAUSE: Duration = Duration::from_millis(200);

/// The two retention windows, in days; zero disables one.
#[derive(Debug, Clone, Copy)]
pub struct RetentionDays {
    pub payload: u32,
    pub delete: u32,
}

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassReport {
    pub trimmed: usize,
    pub deleted: usize,
}

/// Repeat `chunk` until it returns fewer than [`CHUNK_ROWS`], releasing the
/// write connection for `pause` between two chunks.
async fn drain(
    db: &Database,
    cutoff: String,
    pause: Duration,
    chunk: fn(&rusqlite::Connection, &str, usize) -> anyhow::Result<usize>,
) -> anyhow::Result<usize> {
    let mut total = 0;
    loop {
        let cutoff = cutoff.clone();
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
    if days.delete > 0 {
        report.deleted = drain(db, cutoff(now, days.delete), pause, delete_runs_chunk).await?;
    }
    Ok(report)
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
                }
            };
            match run_pass(&db, days, CHUNK_PAUSE).await {
                Ok(PassReport {
                    trimmed: 0,
                    deleted: 0,
                }) => {}
                Ok(report) => tracing::info!(
                    target: "run_retention",
                    "run retention: trimmed the step outputs of {} run(s) older than {} days, deleted {} run(s)",
                    report.trimmed,
                    days.payload,
                    report.deleted
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
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            report,
            PassReport {
                trimmed: CHUNK_ROWS * 2 + 1,
                deleted: 0
            }
        );
        let again = run_pass(
            &db,
            RetentionDays {
                payload: 30,
                delete: 0,
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
}
