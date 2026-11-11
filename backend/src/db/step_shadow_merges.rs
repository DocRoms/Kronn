//! The optional merge of a step's main execution with its shadows (KT-1544):
//! one row per merged step of a run, holding what the merge selected, waited
//! for and condensed, and the merged result that became the step's output.

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::workflows::shadow_merge::{MergeStatus, ShadowMergeReport};

/// A stored merge: where it sits in the run, and its report.
#[derive(Debug, Clone)]
pub struct StepMergeRow {
    pub result_index: u32,
    pub step_name: String,
    pub report: ShadowMergeReport,
}

fn status_str(status: MergeStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Records the merge of the step at `result_index`, replacing the row an earlier
/// stage of the same merge (waiting, condensing) wrote. The merged output has
/// its own column: run retention blanks it with the run's outputs.
pub fn upsert(
    conn: &Connection,
    run_id: &str,
    result_index: u32,
    step_name: &str,
    report: &ShadowMergeReport,
    started_at: &str,
    finished_at: Option<&str>,
) -> Result<()> {
    let mut stored = report.clone();
    stored.merged_output = String::new();
    conn.execute(
        "INSERT INTO step_shadow_merges
            (id, run_id, result_index, step_name, status, report_json, merged_output,
             started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(run_id, result_index) DO UPDATE SET
            step_name = excluded.step_name, status = excluded.status,
            report_json = excluded.report_json, merged_output = excluded.merged_output,
            started_at = excluded.started_at, finished_at = excluded.finished_at",
        params![
            uuid::Uuid::new_v4().to_string(),
            run_id,
            result_index,
            step_name,
            status_str(report.status),
            serde_json::to_string(&stored)?,
            report.merged_output,
            started_at,
            finished_at,
        ],
    )?;
    Ok(())
}

/// The merges of `run_id`, in the order the steps ran.
pub fn list_for_run(conn: &Connection, run_id: &str) -> Result<Vec<StepMergeRow>> {
    let mut stmt = conn.prepare(
        "SELECT result_index, step_name, status, report_json, merged_output
           FROM step_shadow_merges WHERE run_id = ?1 ORDER BY result_index",
    )?;
    let rows = stmt
        .query_map(params![run_id], |row| {
            let status: String = row.get(2)?;
            let raw: String = row.get(3)?;
            let mut report: ShadowMergeReport = serde_json::from_str(&raw).unwrap_or_default();
            report.status = serde_json::from_value(serde_json::Value::String(status))
                .unwrap_or(MergeStatus::Failed);
            report.merged_output = row.get(4)?;
            Ok(StepMergeRow {
                result_index: row.get(0)?,
                step_name: row.get(1)?,
                report,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// At boot no merge is still waiting: one a previous process left behind ends
/// as failed, and the step it belonged to kept its main output.
pub fn interrupt_running(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE step_shadow_merges SET status = 'failed', finished_at = ?1
          WHERE status IN ('waiting', 'condensing')",
        params![chrono::Utc::now().to_rfc3339()],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_serialises_to_the_value_the_column_filters_on() {
        assert_eq!(status_str(MergeStatus::Waiting), "waiting");
        assert_eq!(status_str(MergeStatus::Condensing), "condensing");
        assert_eq!(status_str(MergeStatus::Merged), "merged");
    }
}
