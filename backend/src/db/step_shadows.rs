//! What each execution of a step with shadows was and produced (KT-1459): one
//! row per shadow, and one identity-only row for the main execution, which
//! keeps its output and metrics in the run's own step result.

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::workflows::shadow::{ExecutionIdentity, ShadowStatus};

/// `shadow_id` of the row that describes the main execution.
pub const MAIN: &str = "";

/// One stored execution.
#[derive(Debug, Clone)]
pub struct StepShadowRow {
    pub run_id: String,
    pub result_index: u32,
    pub step_name: String,
    pub shadow_id: String,
    pub label: String,
    pub status: ShadowStatus,
    pub identity: ExecutionIdentity,
    pub output: String,
    pub tokens_used: Option<u64>,
    pub duration_ms: Option<u64>,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
    pub stopped: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

fn status_of(raw: &str) -> ShadowStatus {
    serde_json::from_value(serde_json::Value::String(raw.to_string()))
        .unwrap_or(ShadowStatus::Interrupted)
}

fn status_str(status: ShadowStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Records the start of an execution; a rerun of the same step (a Gate asking
/// for changes), or the execution's own row after the placeholder the step
/// planned it with (KT-1544), replaces the earlier row of the same shadow and
/// keeps its place in the list.
pub fn insert(conn: &Connection, row: &StepShadowRow) -> Result<()> {
    conn.execute(
        "INSERT INTO step_shadow_runs
            (id, run_id, result_index, step_name, shadow_id, label, status, identity_json,
             output, tokens_used, duration_ms, cost_usd, model, stopped, started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(run_id, result_index, shadow_id) DO UPDATE SET
            step_name = excluded.step_name, label = excluded.label, status = excluded.status,
            identity_json = excluded.identity_json, output = excluded.output,
            tokens_used = excluded.tokens_used, duration_ms = excluded.duration_ms,
            cost_usd = excluded.cost_usd, model = excluded.model, stopped = excluded.stopped,
            finished_at = excluded.finished_at",
        params![
            uuid::Uuid::new_v4().to_string(),
            row.run_id,
            row.result_index,
            row.step_name,
            row.shadow_id,
            row.label,
            status_str(row.status),
            serde_json::to_string(&row.identity)?,
            row.output,
            row.tokens_used.map(|n| n as i64),
            row.duration_ms.map(|n| n as i64),
            row.cost_usd,
            row.model,
            row.stopped,
            row.started_at,
            row.finished_at,
        ],
    )?;
    Ok(())
}

/// Settles a running execution with what it produced.
pub fn finish(conn: &Connection, row: &StepShadowRow) -> Result<()> {
    conn.execute(
        "UPDATE step_shadow_runs
            SET status = ?4, identity_json = ?5, output = ?6, tokens_used = ?7, duration_ms = ?8,
                cost_usd = ?9, model = ?10, stopped = ?11, finished_at = ?12
          WHERE run_id = ?1 AND result_index = ?2 AND shadow_id = ?3",
        params![
            row.run_id,
            row.result_index,
            row.shadow_id,
            status_str(row.status),
            serde_json::to_string(&row.identity)?,
            row.output,
            row.tokens_used.map(|n| n as i64),
            row.duration_ms.map(|n| n as i64),
            row.cost_usd,
            row.model,
            row.stopped,
            row.finished_at,
        ],
    )?;
    // The run's total of its shadows, recounted whole: never an increment a
    // second finish of the same row would add twice.
    if row.shadow_id != MAIN {
        conn.execute(
            "UPDATE workflow_runs
                SET shadow_tokens_used = (
                    SELECT COALESCE(SUM(tokens_used), 0) FROM step_shadow_runs
                     WHERE run_id = ?1 AND shadow_id != ?2)
              WHERE id = ?1",
            params![row.run_id, MAIN],
        )?;
    }
    Ok(())
}

/// Every execution recorded for `run_id`: step by step in the order the steps
/// ran, the main row first, the shadows in the order they were started.
pub fn list_for_run(conn: &Connection, run_id: &str) -> Result<Vec<StepShadowRow>> {
    let mut stmt = conn.prepare(
        "SELECT run_id, result_index, step_name, shadow_id, label, status, identity_json, output,
                tokens_used, duration_ms, cost_usd, model, stopped, started_at, finished_at
           FROM step_shadow_runs WHERE run_id = ?1
          ORDER BY result_index, (shadow_id <> ''), started_at, rowid",
    )?;
    let rows = stmt
        .query_map(params![run_id], |row| {
            let identity: String = row.get(6)?;
            Ok(StepShadowRow {
                run_id: row.get(0)?,
                result_index: row.get(1)?,
                step_name: row.get(2)?,
                shadow_id: row.get(3)?,
                label: row.get(4)?,
                status: status_of(&row.get::<_, String>(5)?),
                identity: serde_json::from_str(&identity).unwrap_or_default(),
                output: row.get(7)?,
                tokens_used: row.get::<_, Option<i64>>(8)?.map(|n| n.max(0) as u64),
                duration_ms: row.get::<_, Option<i64>>(9)?.map(|n| n.max(0) as u64),
                cost_usd: row.get(10)?,
                model: row.get(11)?,
                stopped: row.get(12)?,
                started_at: row.get(13)?,
                finished_at: row.get(14)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// At boot no shadow is still running: the ones a previous process left behind
/// are marked interrupted. The main execution's row is left alone (KT-2029):
/// it is identity only until a merge closes it, and its status, output and
/// metrics come from the run's step result, finished or not.
pub fn interrupt_running(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE step_shadow_runs
            SET status = 'interrupted', finished_at = ?1,
                stopped = 'Kronn restarted while this shadow was running'
          WHERE status = 'running' AND shadow_id <> ?2",
        params![chrono::Utc::now().to_rfc3339(), MAIN],
    )?)
}

fn boot_interrupt(conn: &Connection, _: &mut super::BootReport) -> Result<()> {
    match interrupt_running(conn) {
        Ok(0) => {}
        Ok(n) => tracing::info!("Marked {n} step shadow(s) interrupted by the restart"),
        Err(e) => tracing::warn!("Failed to settle running step shadows: {e}"),
    }
    Ok(())
}
inventory::submit! { super::BootStep { order: 70, run: boot_interrupt } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_round_trips_through_its_column() {
        for status in [
            ShadowStatus::Running,
            ShadowStatus::Success,
            ShadowStatus::Failed,
            ShadowStatus::Skipped,
            ShadowStatus::OverBudget,
            ShadowStatus::TimedOut,
            ShadowStatus::Interrupted,
        ] {
            assert_eq!(status_of(&status_str(status)), status);
        }
        assert_eq!(status_of("nonsense"), ShadowStatus::Interrupted);
    }

    fn shadow(shadow_id: &str, tokens: Option<u64>) -> StepShadowRow {
        StepShadowRow {
            run_id: "run-1".into(),
            result_index: 0,
            step_name: "review".into(),
            shadow_id: shadow_id.into(),
            label: shadow_id.into(),
            status: ShadowStatus::Success,
            identity: ExecutionIdentity::default(),
            output: String::new(),
            tokens_used: tokens,
            duration_ms: None,
            cost_usd: None,
            model: None,
            stopped: None,
            started_at: "2026-11-19T00:00:00Z".into(),
            finished_at: Some("2026-11-19T00:01:00Z".into()),
        }
    }

    /// KT-1957 — the run carries what its shadows spent, so the run list, the
    /// workflow card and the statistics count them without the shadow rows.
    #[test]
    fn a_finished_shadow_is_counted_once_in_its_runs_total() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                 VALUES ('wf-1', 'Flux', '\"Manual\"', '[]', '2026-11-19T00:00:00Z', '2026-11-19T00:00:00Z');
             INSERT INTO workflow_runs (id, workflow_id, status, tokens_used, started_at)
                 VALUES ('run-1', 'wf-1', 'Success', 100, '2026-11-19T00:00:00Z');",
        )
        .unwrap();
        let total = |conn: &Connection| -> (i64, i64) {
            conn.query_row(
                "SELECT tokens_used, shadow_tokens_used FROM workflow_runs WHERE id = 'run-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        for row in [
            shadow(MAIN, Some(100)),
            shadow("a", None),
            shadow("b", None),
        ] {
            insert(&conn, &row).unwrap();
        }
        // The main row repeats the step's own spend: it is never a shadow's.
        finish(&conn, &shadow(MAIN, Some(100))).unwrap();
        assert_eq!(total(&conn), (100, 0));
        finish(&conn, &shadow("a", Some(40))).unwrap();
        finish(&conn, &shadow("b", Some(7))).unwrap();
        assert_eq!(total(&conn), (100, 47));
        // Finishing a row again recounts, it does not add.
        finish(&conn, &shadow("a", Some(40))).unwrap();
        assert_eq!(total(&conn), (100, 47));
        // A shadow that reported nothing counts for nothing.
        finish(&conn, &shadow("b", None)).unwrap();
        assert_eq!(total(&conn), (100, 40));

        let summary: i64 = conn
            .query_row(crate::db::workflows::LAST_RUN_SUMMARIES_SQL, [], |row| {
                row.get(5)
            })
            .unwrap();
        assert_eq!(summary, 140, "the workflow card shows steps and shadows");
        let run = crate::db::workflows::get_run(&conn, "run-1")
            .unwrap()
            .unwrap();
        assert_eq!((run.tokens_used, run.shadow_tokens_used), (100, 40));
    }
}
