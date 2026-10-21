//! Cross-agent audit provenance (KT-1183, migration 257): one row per
//! cross-checked step, one per finding after the cross-check.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::core::cross_audit::{CrossFindingRecord, DroppedFinding};

pub struct CrossStepRow<'a> {
    pub audit_run_id: &'a str,
    pub step_index: u32,
    pub file_label: &'a str,
    pub agents: &'a [String],
    pub dropped: &'a [DroppedFinding],
    pub failed_agents: &'a [(String, String)],
    pub calls: u32,
    pub step_tokens: Option<u64>,
    pub cost_usd_micros: Option<u64>,
    pub dispute_discussion_id: Option<&'a str>,
}

/// Records the step and its findings; a resumed step replaces what an
/// earlier attempt stored. The caller holds the transaction.
pub fn record_step(
    conn: &Connection,
    step: &CrossStepRow<'_>,
    records: &[CrossFindingRecord],
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let as_sql = |v: Option<u64>| v.map(|v| v as i64);
    conn.execute(
        "DELETE FROM audit_cross_findings WHERE audit_run_id = ?1 AND step_index = ?2",
        params![step.audit_run_id, step.step_index as i64],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO audit_cross_steps (audit_run_id, step_index, file_label,
             agents_json, dropped_json, failed_agents_json, calls, step_tokens,
             cost_usd_micros, dispute_discussion_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            step.audit_run_id,
            step.step_index as i64,
            step.file_label,
            serde_json::to_string(step.agents)?,
            serde_json::to_string(step.dropped)?,
            serde_json::to_string(step.failed_agents)?,
            step.calls as i64,
            as_sql(step.step_tokens),
            as_sql(step.cost_usd_micros),
            step.dispute_discussion_id,
            now,
        ],
    )?;
    for record in records {
        conn.execute(
            "INSERT INTO audit_cross_findings (audit_run_id, step_index, group_id, td_id,
                 outcome, detection_count, record_json, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                step.audit_run_id,
                step.step_index as i64,
                record.group_id,
                record.td_id,
                serde_json::to_value(record.outcome)?
                    .as_str()
                    .unwrap_or_default(),
                record.detection_count as i64,
                serde_json::to_string(record)?,
                now,
            ],
        )?;
    }
    Ok(())
}

/// The findings of a run, by step.
pub fn list_findings(
    conn: &Connection,
    audit_run_id: &str,
) -> Result<Vec<(u32, CrossFindingRecord)>> {
    let mut statement = conn.prepare(
        "SELECT step_index, record_json FROM audit_cross_findings
          WHERE audit_run_id = ?1 ORDER BY step_index, rowid",
    )?;
    let rows = statement
        .query_map(params![audit_run_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(step, json)| Ok((step as u32, serde_json::from_str(&json)?)))
        .collect()
}

/// What a step cost when it ran: tokens and cost, either may be unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredStep {
    pub audit_run_id: String,
    pub step_tokens: Option<u64>,
    pub cost_usd_micros: Option<u64>,
    /// For a cross-checked step: how many agents ran it.
    pub agents: Option<usize>,
}

/// The project's latest single-agent run of `file_label` that reported a
/// figure, never a carried row nor a cross-checked one.
pub fn latest_single_agent_step(
    conn: &Connection,
    project_id: &str,
    file_label: &str,
) -> Result<Option<MeasuredStep>> {
    Ok(conn
        .query_row(
            "SELECT s.audit_run_id, s.step_tokens, s.cost_usd_micros
               FROM audit_run_steps s
               JOIN audit_runs r ON r.id = s.audit_run_id
               LEFT JOIN audit_cross_steps c
                 ON c.audit_run_id = s.audit_run_id AND c.step_index = s.step_index
              WHERE r.project_id = ?1 AND s.file_label = ?2 AND s.ended_at IS NOT NULL
                AND s.carried_from_run_id IS NULL AND c.audit_run_id IS NULL
                AND (s.step_tokens IS NOT NULL OR s.cost_usd_micros IS NOT NULL)
              ORDER BY julianday(s.started_at) DESC LIMIT 1",
            params![project_id, file_label],
            measured_row,
        )
        .optional()?)
}

/// The project's latest cross-checked run of `file_label`.
pub fn latest_cross_step(
    conn: &Connection,
    project_id: &str,
    file_label: &str,
) -> Result<Option<MeasuredStep>> {
    let row = conn
        .query_row(
            "SELECT c.audit_run_id, c.step_tokens, c.cost_usd_micros, c.agents_json
               FROM audit_cross_steps c
               JOIN audit_runs r ON r.id = c.audit_run_id
              WHERE r.project_id = ?1 AND c.file_label = ?2
                AND (c.step_tokens IS NOT NULL OR c.cost_usd_micros IS NOT NULL)
              ORDER BY julianday(c.created_at) DESC LIMIT 1",
            params![project_id, file_label],
            |row| Ok((measured_row(row)?, row.get::<_, String>(3)?)),
        )
        .optional()?;
    Ok(row.map(|(mut step, agents)| {
        step.agents = serde_json::from_str::<Vec<String>>(&agents)
            .ok()
            .map(|a| a.len());
        step
    }))
}

fn measured_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeasuredStep> {
    Ok(MeasuredStep {
        audit_run_id: row.get(0)?,
        step_tokens: row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
        cost_usd_micros: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
        agents: None,
    })
}
