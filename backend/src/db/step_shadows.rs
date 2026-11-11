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
/// are marked interrupted.
pub fn interrupt_running(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE step_shadow_runs
            SET status = 'interrupted', finished_at = ?1,
                stopped = 'Kronn restarted while this shadow was running'
          WHERE status = 'running'",
        params![chrono::Utc::now().to_rfc3339()],
    )?)
}

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
}
