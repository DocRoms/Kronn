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

/// The finding a cross-agent dispute card decides, with its project.
#[derive(Debug, Clone)]
pub struct DisputeTarget {
    pub audit_run_id: String,
    pub step_index: u32,
    pub file_label: String,
    pub project_id: String,
    pub project_path: String,
    pub record: CrossFindingRecord,
}

/// The finding behind `question_key`, only when `discussion_id` is the dispute
/// discussion Kronn recorded for that step: no other room can move a sheet.
pub fn dispute_target(
    conn: &Connection,
    discussion_id: &str,
    question_key: &str,
) -> Result<Option<DisputeTarget>> {
    let row = conn
        .query_row(
            "SELECT f.audit_run_id, f.step_index, s.file_label, r.project_id, p.path, f.record_json
               FROM audit_cross_steps s
               JOIN audit_cross_findings f
                 ON f.audit_run_id = s.audit_run_id AND f.step_index = s.step_index
               JOIN audit_runs r ON r.id = s.audit_run_id
               JOIN projects p ON p.id = r.project_id
               JOIN discussions d ON d.id = s.dispute_discussion_id
              WHERE s.dispute_discussion_id = ?1 AND d.project_id = r.project_id
                AND json_extract(f.record_json, '$.question_key') = ?2
              LIMIT 1",
            params![discussion_id, question_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((audit_run_id, step_index, file_label, project_id, project_path, json)) = row else {
        return Ok(None);
    };
    Ok(Some(DisputeTarget {
        audit_run_id,
        step_index: step_index as u32,
        file_label,
        project_id,
        project_path,
        record: serde_json::from_str(&json)?,
    }))
}

/// Stores a finding after a human decided its dispute.
pub fn update_record(conn: &Connection, target: &DisputeTarget) -> Result<()> {
    conn.execute(
        "UPDATE audit_cross_findings SET record_json = ?4
          WHERE audit_run_id = ?1 AND step_index = ?2 AND group_id = ?3",
        params![
            target.audit_run_id,
            target.step_index as i64,
            target.record.group_id,
            serde_json::to_string(&target.record)?,
        ],
    )?;
    Ok(())
}

/// A dispute card still waiting for a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDispute {
    pub td_id: String,
    pub discussion_id: String,
    pub question_key: String,
}

/// The project's unanswered dispute cards, oldest first.
pub fn pending_disputes(conn: &Connection, project_id: &str) -> Result<Vec<PendingDispute>> {
    let mut statement = conn.prepare(
        "SELECT f.td_id, q.discussion_id, q.question_key
           FROM discussion_questions q
           JOIN audit_cross_steps s ON s.dispute_discussion_id = q.discussion_id
           JOIN audit_cross_findings f
             ON f.audit_run_id = s.audit_run_id AND f.step_index = s.step_index
            AND json_extract(f.record_json, '$.question_key') = q.question_key
           JOIN audit_runs r ON r.id = s.audit_run_id
          WHERE r.project_id = ?1 AND q.state = 'pending' AND f.td_id IS NOT NULL
          ORDER BY q.created_at, q.id",
    )?;
    let rows = statement
        .query_map(params![project_id], |row| {
            Ok(PendingDispute {
                td_id: row.get(0)?,
                discussion_id: row.get(1)?,
                question_key: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The project of a dispute card, when `question_id` is one.
pub fn dispute_project_of_question(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
) -> Result<Option<String>> {
    let key: Option<String> = conn
        .query_row(
            "SELECT question_key FROM discussion_questions WHERE id = ?1 AND discussion_id = ?2",
            params![question_id, discussion_id],
            |row| row.get(0),
        )
        .optional()?;
    match key.filter(|k| k.starts_with(crate::core::cross_audit::QUESTION_KEY_PREFIX)) {
        Some(key) => Ok(dispute_target(conn, discussion_id, &key)?.map(|t| t.project_id)),
        None => Ok(None),
    }
}
