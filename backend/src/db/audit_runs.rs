//! CRUD for the `audit_runs` table (migration 050, 0.8.2).
//!
//! Used by:
//!   - the audit pipeline to insert at start + update at end;
//!   - the API to read latest N runs for a project (powers the
//!     health-badge sparkline + the audit-history doc generator);
//!   - the cluster detector to read previous recommendations.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

use crate::core::adr::StartOfRun;
use crate::models::{AuditRun, AuditRunStep};

/// Insert a new audit run with `status = Running` and zeroed counts.
/// Returns the row id for later update calls.
pub fn insert_running(
    conn: &Connection,
    id: &str,
    project_id: &str,
    kind: &str,
    agent_type: &str,
    started_at: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_runs (id, project_id, kind, agent_type, started_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, 'Running')",
        params![id, project_id, kind, agent_type, started_at.to_rfc3339()],
    )?;
    // KT-1211 — only the latest run of a project can be accepted from, so the
    // pre-step snapshots of the runs this one supersedes have nothing left to
    // protect or to diff against.
    conn.execute(
        "DELETE FROM audit_step_snapshots WHERE audit_run_id IN
            (SELECT id FROM audit_runs WHERE project_id = ?1 AND id <> ?2)",
        params![project_id, id],
    )?;
    Ok(())
}

/// Projects for which this instance recorded a Completed Full audit. One query
/// for the whole project list.
pub fn projects_with_completed_full(
    conn: &Connection,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT project_id FROM audit_runs WHERE kind = 'Full' AND status = 'Completed'",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn has_completed_full(conn: &Connection, project_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM audit_runs
         WHERE project_id = ?1 AND kind = 'Full' AND status = 'Completed'",
        [project_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Whether this instance ever ran a Full audit of the project, finished or not.
pub fn has_any_full(conn: &Connection, project_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM audit_runs WHERE project_id = ?1 AND kind = 'Full'",
        [project_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn has_running_for_project(conn: &Connection, project_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM audit_runs WHERE project_id = ?1 AND status = 'Running'",
        [project_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Final update for a completed run. Computes the duration from the
/// stored `started_at` so callers don't have to round-trip the value.
#[allow(clippy::too_many_arguments)]
pub fn complete(
    conn: &Connection,
    id: &str,
    ended_at: DateTime<Utc>,
    status: &str,
    td_critical: u32,
    td_high: u32,
    td_medium: u32,
    td_low: u32,
    td_resolved_since_last: u32,
    td_new_since_last: u32,
    td_carried_over: u32,
    health_score: u8,
    report_path: Option<&str>,
    recommendations_json: Option<&str>,
) -> Result<()> {
    let td_total = td_critical + td_high + td_medium + td_low;
    let ended_str = ended_at.to_rfc3339();

    // Pull started_at to compute the duration in the same statement,
    // avoiding a separate round-trip.
    let started_at: String = conn.query_row(
        "SELECT started_at FROM audit_runs WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let started: DateTime<Utc> = DateTime::parse_from_rfc3339(&started_at)?.with_timezone(&Utc);
    let duration_ms: i64 = (ended_at - started).num_milliseconds().max(0);

    let affected = conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = ?3,
            status = ?4,
            td_critical = ?5,
            td_high = ?6,
            td_medium = ?7,
            td_low = ?8,
            td_total = ?9,
            td_resolved_since_last = ?10,
            td_new_since_last = ?11,
            td_carried_over = ?12,
            health_score = ?13,
            report_path = ?14,
            recommendations_json = ?15
         WHERE id = ?1 AND status = 'Running'",
        params![
            id,
            ended_str,
            duration_ms as u32,
            status,
            td_critical,
            td_high,
            td_medium,
            td_low,
            td_total,
            td_resolved_since_last,
            td_new_since_last,
            td_carried_over,
            health_score,
            report_path,
            recommendations_json,
        ],
    )?;
    // A 0-row update means the row vanished OR was already terminal; the
    // caller treats this write as THE authoritative terminal transition, so
    // a non-Running row must be an error, never a silent overwrite.
    anyhow::ensure!(
        affected > 0,
        "complete: run row {id} not Running — terminal status not persisted"
    );
    Ok(())
}

/// Mark a running audit as Failed without computing counts (called when
/// the pipeline aborts early). Idempotent on already-terminal rows.
pub fn mark_failed(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    // We don't have a `reason` column; we encode it in the report_path
    // field with a `failure:` prefix for the (rare) inspection.
    conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Failed',
            report_path = COALESCE(report_path, ?3)
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339(), format!("failure: {reason}")],
    )?;
    Ok(())
}

/// Mark as Cancelled (Ctrl+C / cancel button), before any step. See
/// [`mark_cancelled_at_step`].
pub fn mark_cancelled(conn: &Connection, id: &str) -> Result<()> {
    mark_cancelled_at_step(conn, id, 0).map(|_| ())
}

/// KT-1211 — settle a run the user cancelled while `step` was in flight (0 =
/// before step 1). A run that already completed a step is settled
/// `Interrupted`, resumable, and carries the cancel in `cancelled_at_step`:
/// cancelling at step 12 must not mean running all 16 again. A run that
/// completed nothing is `Cancelled` (a fresh launch is its only next action).
/// Returns the status written, `None` when the row was not `Running`.
pub fn mark_cancelled_at_step(conn: &Connection, id: &str, step: u32) -> Result<Option<String>> {
    let affected = conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = CASE WHEN last_completed_step >= 1 THEN 'Interrupted' ELSE 'Cancelled' END,
            cancelled_at_step = ?3,
            report_path = CASE WHEN last_completed_step >= 1
                THEN COALESCE(report_path, 'cancelled: stopped by the user during step ' || ?3)
                ELSE report_path END
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339(), step as i64],
    )?;
    if affected == 0 {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT status FROM audit_runs WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?)
}

/// KT-1211 (M6) — what moved in the sources since the run this one resumes.
pub fn set_resume_drift(conn: &Connection, id: &str, message: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET resume_drift = ?2 WHERE id = ?1",
        params![id, message],
    )?;
    Ok(())
}

/// KT-1211 (M5) — the process group of the step's CLI agent, with the start
/// stamp of its leader, persisted so a boot after a crash can kill it.
pub fn set_agent_process(
    conn: &Connection,
    id: &str,
    pgid: i64,
    marker: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET agent_pgid = ?2, agent_started_marker = ?3 WHERE id = ?1",
        params![id, pgid, marker],
    )?;
    Ok(())
}

/// The agent of the run exited: nothing left to kill at boot.
pub fn clear_agent_process(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET agent_pgid = NULL, agent_started_marker = NULL WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// Agents a previous process left recorded: `(run id, pgid, start stamp)`.
pub fn recorded_agent_processes(conn: &Connection) -> Result<Vec<(String, i64, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_pgid, agent_started_marker FROM audit_runs WHERE agent_pgid IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// KT-1211 — what the step's target was before its agent ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSnapshot {
    pub step_index: u32,
    pub target_file: String,
    /// `None`: the file was absent.
    pub content: Option<String>,
    /// Fingerprint of the last content Kronn saw the step's agent write to the
    /// target (see `step_guard::TargetWatch`); `None` when it never saw it
    /// move. A crash settles the target only if what is on disk is this.
    pub observed_hash: Option<String>,
}

/// Largest target kept in a snapshot. An audit doc is a few KB; anything past
/// this is not a doc the agent should be rewriting, and keeping it would only
/// grow the database.
pub const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

/// Keep the target as it was before `step_index`'s agent ran. Replaces an
/// earlier snapshot of the same step. Refused past [`MAX_SNAPSHOT_BYTES`].
pub fn save_step_snapshot(
    conn: &Connection,
    run_id: &str,
    step_index: u32,
    target_file: &str,
    content: Option<&str>,
) -> Result<()> {
    if let Some(content) = content.filter(|c| c.len() > MAX_SNAPSHOT_BYTES) {
        anyhow::bail!(
            "`{target_file}` is {} bytes: past the {MAX_SNAPSHOT_BYTES}-byte snapshot cap",
            content.len()
        );
    }
    conn.execute(
        "INSERT OR REPLACE INTO audit_step_snapshots
            (audit_run_id, step_index, target_file, existed, content, observed_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
        params![
            run_id,
            step_index as i64,
            target_file,
            content.is_some() as i32,
            content
        ],
    )?;
    Ok(())
}

fn snapshot_of_row(row: &rusqlite::Row) -> rusqlite::Result<StepSnapshot> {
    let existed: i64 = row.get(2)?;
    Ok(StepSnapshot {
        step_index: row.get::<_, i64>(0)?.max(0) as u32,
        target_file: row.get(1)?,
        content: if existed != 0 { row.get(3)? } else { None },
        observed_hash: row.get(4)?,
    })
}

pub fn step_snapshot(
    conn: &Connection,
    run_id: &str,
    step_index: u32,
) -> Result<Option<StepSnapshot>> {
    Ok(conn
        .query_row(
            "SELECT step_index, target_file, existed, content, observed_hash FROM audit_step_snapshots
              WHERE audit_run_id = ?1 AND step_index = ?2",
            params![run_id, step_index as i64],
            snapshot_of_row,
        )
        .optional()?)
}

/// The agent of `step_index` was seen writing `observed_hash` to the target:
/// that is the content a crash may put back from. No-op for a step with no
/// snapshot.
pub fn record_observed_output(
    conn: &Connection,
    run_id: &str,
    step_index: u32,
    observed_hash: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_step_snapshots SET observed_hash = ?3
          WHERE audit_run_id = ?1 AND step_index = ?2",
        params![run_id, step_index as i64, observed_hash],
    )?;
    Ok(())
}

/// The step is done (or accepted): its snapshot has nothing left to protect.
pub fn delete_step_snapshot(conn: &Connection, run_id: &str, step_index: u32) -> Result<()> {
    conn.execute(
        "DELETE FROM audit_step_snapshots WHERE audit_run_id = ?1 AND step_index = ?2",
        params![run_id, step_index as i64],
    )?;
    Ok(())
}

/// Snapshots of the steps that never ended: an agent killed with its backend.
pub fn unfinished_step_snapshots(conn: &Connection, run_id: &str) -> Result<Vec<StepSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT p.step_index, p.target_file, p.existed, p.content, p.observed_hash
           FROM audit_step_snapshots p
           JOIN audit_run_steps s
             ON s.audit_run_id = p.audit_run_id AND s.step_index = p.step_index
          WHERE p.audit_run_id = ?1 AND s.ended_at IS NULL AND s.accepted_at IS NULL
          ORDER BY p.step_index",
    )?;
    let rows = stmt
        .query_map(params![run_id], snapshot_of_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Runs holding a step that never ended and has a snapshot: an agent killed
/// with its backend. `(run id, project id)`.
pub fn runs_with_unfinished_steps(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT r.id, r.project_id
           FROM audit_step_snapshots p
           JOIN audit_run_steps s
             ON s.audit_run_id = p.audit_run_id AND s.step_index = p.step_index
           JOIN audit_runs r ON r.id = p.audit_run_id
          WHERE s.ended_at IS NULL AND s.accepted_at IS NULL",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// KT-1211 — a step that never ended (its agent died with the backend) is
/// settled failed, with what became of its target. Its snapshot is dropped
/// unless `keep_snapshot`: a target that could not be put back keeps it for
/// the diff an acceptance shows.
pub fn settle_killed_step(
    conn: &Connection,
    run_id: &str,
    step_index: u32,
    reason: &str,
    target_state: Option<&str>,
    keep_snapshot: bool,
    at: DateTime<Utc>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE audit_run_steps SET ended_at = ?3, cli_success = 0, step_warning = ?4,
                target_state = ?5
          WHERE audit_run_id = ?1 AND step_index = ?2 AND ended_at IS NULL",
        params![
            run_id,
            step_index as i64,
            at.to_rfc3339(),
            reason,
            target_state
        ],
    )?;
    if !keep_snapshot {
        tx.execute(
            "DELETE FROM audit_step_snapshots WHERE audit_run_id = ?1 AND step_index = ?2",
            params![run_id, step_index as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// `restored` or `partial`: what became of the target of a step that did not
/// succeed.
pub fn set_step_target_state(
    conn: &Connection,
    run_id: &str,
    step_index: u32,
    state: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_run_steps SET target_state = ?3 WHERE audit_run_id = ?1 AND step_index = ?2",
        params![run_id, step_index as i64, state],
    )?;
    Ok(())
}

/// STRICT variants for the partial-run terminal transaction (Codex lot 3):
/// a 0-row update means the Running row was never transitioned (missing or
/// already terminal) — the server must NOT report a terminal status the DB
/// does not carry. The lenient versions above stay for reconcile paths
/// where idempotence on already-terminal rows is the point.
pub fn mark_failed_strict(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    let affected = conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Failed',
            report_path = COALESCE(report_path, ?3)
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339(), format!("failure: {reason}")],
    )?;
    anyhow::ensure!(
        affected > 0,
        "mark_failed_strict: run {id} not Running — terminal status not persisted"
    );
    Ok(())
}

/// See [`mark_failed_strict`] — own guarded UPDATE: a row ALREADY
/// Interrupted must refuse too (re-reading the status after a lenient call
/// would wrongly pass it).
pub fn mark_interrupted_strict(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    let affected = conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Interrupted',
            report_path = ?3
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339(), reason],
    )?;
    anyhow::ensure!(
        affected > 0,
        "mark_interrupted_strict: run {id} not Running — transition refused"
    );
    Ok(())
}

/// Latest N runs for a project, most recent first. Used by the health
/// badge to render the sparkline + delta chip.
pub fn list_recent(conn: &Connection, project_id: &str, limit: u32) -> Result<Vec<AuditRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, kind, agent_type, started_at, ended_at, duration_ms, status,
                td_critical, td_high, td_medium, td_low, td_total,
                td_resolved_since_last, td_new_since_last, td_carried_over,
                health_score, report_path, recommendations_json, last_completed_step,
                validation_discussion_id, step_outcomes_json,
                head_sha, branch, source_fingerprint, model, project_profile_draft,
                cancelled_at_step, resume_drift, tier, connection_id
         FROM audit_runs
         WHERE project_id = ?1
         ORDER BY started_at DESC, rowid DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project_id, limit], row_to_audit_run)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// The most recent COMPLETED audit run, used to bootstrap the health
/// badge in places that only need the latest snapshot (project list
/// page). Returns None when no run has finished yet.
pub fn latest_completed(conn: &Connection, project_id: &str) -> Result<Option<AuditRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, kind, agent_type, started_at, ended_at, duration_ms, status,
                td_critical, td_high, td_medium, td_low, td_total,
                td_resolved_since_last, td_new_since_last, td_carried_over,
                health_score, report_path, recommendations_json, last_completed_step,
                validation_discussion_id, step_outcomes_json,
                head_sha, branch, source_fingerprint, model, project_profile_draft,
                cancelled_at_step, resume_drift, tier, connection_id
         FROM audit_runs
         WHERE project_id = ?1 AND status = 'Completed'
         ORDER BY ended_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![project_id], row_to_audit_run)?;
    if let Some(r) = rows.next() {
        Ok(Some(r?))
    } else {
        Ok(None)
    }
}

/// Link the run to its validation discussion — executed INSIDE the same
/// transaction as `complete("Completed")` (076): the validate endpoint
/// trusts only this durable link, never title/date heuristics.
pub fn set_validation_discussion(conn: &Connection, run_id: &str, disc_id: &str) -> Result<()> {
    let affected = conn.execute(
        // A different discussion starts a fresh validation: the completion
        // stamped for the previous one (KT-1122) does not carry over.
        "UPDATE audit_runs SET
             validation_completed_at = CASE
                 WHEN validation_discussion_id IS ?2 THEN validation_completed_at END,
             validation_completed_message_id = CASE
                 WHEN validation_discussion_id IS ?2 THEN validation_completed_message_id END,
             validation_discussion_id = ?2
         WHERE id = ?1",
        params![run_id, disc_id],
    )?;
    anyhow::ensure!(
        affected > 0,
        "set_validation_discussion: run row {run_id} not found"
    );
    Ok(())
}

/// Whether `disc_id` is the durable validation discussion of an audit run for
/// `project_id`. The streaming path uses this relation instead of titles,
/// which users can edit and partial audits localize differently.
///
/// The run is `Completed`, or `Interrupted` (KT-931): a Full run some of whose
/// steps failed still gets a validation discussion for the steps that
/// succeeded, linked in the same transaction as its `Interrupted` status. Any
/// other status carries no link.
pub fn validation_discussion_belongs_to_project(
    conn: &Connection,
    disc_id: &str,
    project_id: &str,
) -> Result<bool> {
    let linked: bool = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM audit_runs
            WHERE validation_discussion_id = ?1
              AND project_id = ?2
              AND status IN ('Completed', 'Interrupted')
        )",
        params![disc_id, project_id],
        |row| row.get(0),
    )?;
    Ok(linked)
}

/// Persist the structured per-step outcomes of a (partial) run (076).
pub fn set_step_outcomes(conn: &Connection, run_id: &str, outcomes_json: &str) -> Result<()> {
    let affected = conn.execute(
        "UPDATE audit_runs SET step_outcomes_json = ?2 WHERE id = ?1",
        params![run_id, outcomes_json],
    )?;
    anyhow::ensure!(
        affected > 0,
        "set_step_outcomes: run row {run_id} not found"
    );
    Ok(())
}

/// Fetch a single run by id. Used by the resume path: the caller passes a
/// `resume_run_id` and the server derives the kind + checkpoint from the
/// authoritative row rather than trusting a client-supplied `resume_from`.
pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<AuditRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, kind, agent_type, started_at, ended_at, duration_ms, status,
                td_critical, td_high, td_medium, td_low, td_total,
                td_resolved_since_last, td_new_since_last, td_carried_over,
                health_score, report_path, recommendations_json, last_completed_step,
                validation_discussion_id, step_outcomes_json,
                head_sha, branch, source_fingerprint, model, project_profile_draft,
                cancelled_at_step, resume_drift, tier, connection_id
         FROM audit_runs
         WHERE id = ?1
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![id], row_to_audit_run)?;
    if let Some(r) = rows.next() {
        Ok(Some(r?))
    } else {
        Ok(None)
    }
}

fn row_to_audit_run(row: &rusqlite::Row) -> rusqlite::Result<AuditRun> {
    let started_str: String = row.get(4)?;
    let ended_str: Option<String> = row.get(5)?;
    let duration_ms: Option<i64> = row.get(6)?;
    Ok(AuditRun {
        id: row.get(0)?,
        project_id: row.get(1)?,
        kind: row.get(2)?,
        agent_type: row.get(3)?,
        started_at: DateTime::parse_from_rfc3339(&started_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        ended_at: ended_str.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }),
        duration_ms: duration_ms.map(|d| d.max(0) as u32),
        status: row.get(7)?,
        td_critical: row.get::<_, i64>(8)? as u32,
        td_high: row.get::<_, i64>(9)? as u32,
        td_medium: row.get::<_, i64>(10)? as u32,
        td_low: row.get::<_, i64>(11)? as u32,
        td_total: row.get::<_, i64>(12)? as u32,
        td_resolved_since_last: row.get::<_, i64>(13)? as u32,
        td_new_since_last: row.get::<_, i64>(14)? as u32,
        td_carried_over: row.get::<_, i64>(15)? as u32,
        health_score: row
            .get::<_, Option<i64>>(16)?
            .map(|v| v.clamp(0, 100) as u8),
        report_path: row.get(17)?,
        recommendations_json: row.get(18)?,
        last_completed_step: row.get::<_, i64>(19).unwrap_or(0).max(0) as u32,
        validation_discussion_id: row.get(20).unwrap_or(None),
        step_outcomes_json: row.get(21).unwrap_or(None),
        head_sha: row.get(22)?,
        branch: row.get(23)?,
        source_fingerprint: row.get(24)?,
        model: row.get(25)?,
        project_profile_draft: row.get(26)?,
        cancelled_at_step: row
            .get::<_, Option<i64>>(27)
            .unwrap_or(None)
            .map(|step| step.max(0) as u32),
        resume_drift: row.get(28).unwrap_or(None),
        tier: row.get(29).unwrap_or(None),
        connection_id: row.get(30).unwrap_or(None),
    })
}

/// The starter repository profile a Full audit drafted (KT-920).
pub fn set_project_profile_draft(conn: &Connection, id: &str, draft: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET project_profile_draft = ?2 WHERE id = ?1",
        params![id, draft],
    )?;
    Ok(())
}

/// What the run audits: commit, branch, source fingerprint and model at its
/// start. Each is NULL when unknown (outside git, model not resolved).
pub fn set_run_provenance(
    conn: &Connection,
    id: &str,
    head_sha: Option<&str>,
    branch: Option<&str>,
    source_fingerprint: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET head_sha = ?2, branch = ?3, source_fingerprint = ?4, model = ?5
         WHERE id = ?1",
        params![id, head_sha, branch, source_fingerprint, model],
    )?;
    Ok(())
}

/// Record the ADR baseline of run `id` and return it: its predecessor's when
/// it resumes one that recorded a baseline, else `found` (`docs/decisions.md`
/// and the TD sheets' `Decision record` lines as the run found them). A
/// resume must not trust ADRs or record lines its predecessor wrote.
pub fn record_adr_baseline(
    conn: &Connection,
    id: &str,
    resumed_from: Option<&str>,
    found: StartOfRun,
) -> Result<StartOfRun> {
    let inherited = match resumed_from {
        Some(previous) => adr_baseline(conn, previous)?,
        None => None,
    };
    let baseline = inherited.unwrap_or(found);
    conn.execute(
        "UPDATE audit_runs SET adr_baseline = ?2, adr_record_baseline = ?3 WHERE id = ?1",
        params![id, baseline.decisions, baseline.records],
    )?;
    Ok(baseline)
}

/// What a run started from, if it recorded it. Record lines missing from a
/// row that has a decisions baseline count as none (nothing honoured).
pub fn adr_baseline(conn: &Connection, id: &str) -> Result<Option<StartOfRun>> {
    Ok(conn
        .query_row(
            "SELECT adr_baseline, adr_record_baseline FROM audit_runs WHERE id = ?1",
            params![id],
            |row| {
                let decisions = row.get::<_, Option<String>>(0)?;
                let records = row.get::<_, Option<String>>(1)?.unwrap_or_default();
                Ok(decisions.map(|decisions| StartOfRun { decisions, records }))
            },
        )
        .optional()?
        .flatten())
}

/// Record how the run was launched: its model tier and, for an HTTP agent, the
/// named connection. A step retry or a resume reads them back (KT-1320).
pub fn set_run_launch(
    conn: &Connection,
    id: &str,
    tier: crate::models::ModelTier,
    connection_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET tier = ?2, connection_id = ?3 WHERE id = ?1",
        params![id, tier.as_str(), connection_id],
    )?;
    Ok(())
}

/// Record the model the run's agent served, or the configured one labelled as
/// such (KT-997). Updated as steps observe more of them.
pub fn set_run_model(conn: &Connection, id: &str, model: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET model = ?2 WHERE id = ?1",
        params![id, model],
    )?;
    Ok(())
}

/// Record a step's cost when its agent reported one (KT-997). Never called with
/// a guess: an unreported cost stays NULL.
pub fn set_step_cost(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    cost_usd_micros: u64,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_run_steps SET cost_usd_micros = ?3
         WHERE audit_run_id = ?1 AND step_index = ?2",
        params![audit_run_id, step_index as i64, cost_usd_micros as i64],
    )?;
    Ok(())
}

/// 0.8.3 (#311) — bump `last_completed_step` on every successful
/// `step_done` event so an interrupted run reports how far it got.
/// KT-931 — the value is the NUMBER of steps that succeeded in the run
/// (steps inherited from the resumed predecessor included), not the index
/// of the last one: a step that failed in the middle of the chain must not
/// make the progress read as if everything up to the end had succeeded.
/// Which steps are left is read from `audit_run_steps`, never from this
/// scalar.
/// Idempotent: if the new value isn't greater than the current one
/// (rare race where two updates land out of order), the existing
/// value wins. No-op on terminal rows.
pub fn update_last_completed_step(conn: &Connection, id: &str, step: u32) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET last_completed_step = ?2
         WHERE id = ?1 AND status = 'Running' AND last_completed_step < ?2",
        params![id, step as i64],
    )?;
    Ok(())
}

/// 0.8.3 (#311) — mark an audit run as `Interrupted`. Different from
/// `Cancelled` (explicit user action) and `Failed` (terminal error
/// the pipeline can't recover from). `Interrupted` means the SSE
/// stream ended before reaching step 10 without an explicit signal,
/// most often a CLI rate-limit, OOM, or network blip. The frontend
/// surfaces these specifically as resumable: "Reprendre Step N/10".
/// Stamp the TD counters computed at end-of-stream onto a row regardless of
/// its terminal status. Interrupted runs used to keep td_* at 0 while 14 TD
/// files sat on disk — the history chip strip under-reported real findings.
#[allow(clippy::too_many_arguments)]
pub fn update_td_counts(
    conn: &Connection,
    id: &str,
    td_critical: u32,
    td_high: u32,
    td_medium: u32,
    td_low: u32,
    td_resolved_since_last: u32,
    td_new_since_last: u32,
    td_carried_over: u32,
) -> Result<()> {
    // resolved/new are persisted too (benchmark run 1, 2026-07-21): the
    // Interrupted path computed them but left the row at its 0 defaults,
    // and the UI presented "0 new" as a KNOWN value while a genuinely new
    // TD existed on disk.
    conn.execute(
        "UPDATE audit_runs SET
            td_critical = ?2, td_high = ?3, td_medium = ?4, td_low = ?5,
            td_total = ?6, td_resolved_since_last = ?7,
            td_new_since_last = ?8, td_carried_over = ?9
         WHERE id = ?1",
        params![
            id,
            td_critical,
            td_high,
            td_medium,
            td_low,
            td_critical + td_high + td_medium + td_low,
            td_resolved_since_last,
            td_new_since_last,
            td_carried_over
        ],
    )?;
    Ok(())
}

pub fn mark_interrupted(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Interrupted',
            report_path = COALESCE(report_path, ?3)
         WHERE id = ?1 AND status = 'Running'",
        params![
            id,
            Utc::now().to_rfc3339(),
            format!("interrupted: {reason}")
        ],
    )?;
    Ok(())
}

/// 0.8.4 (#317 / B1) — reconcile stale `Running` rows that were never
/// terminated. A backend crash, container restart, kill -9, or any
/// other path that interrupts the SSE loop *without* the cancel flag
/// being set will leave a row stuck in `status = 'Running'` forever.
/// On a project that's been audited many times these accumulate and
/// pollute the recap-panel chip strip (and the "active audits" badge,
/// which trusts `status = 'Running'`).
///
/// Strategy: at boot, any run that's been `Running` for more than
/// `stale_after_secs` seconds is marked as `Interrupted` with a
/// `report_path` prefix `stale: ` for forensic clarity.
///
/// Returns the count of reconciled rows so the caller can log it.
///
/// The threshold defaults to 30 minutes (1800s) which is well above
/// the longest realistic Full-audit duration (~25 min on a 50k-line
/// repo). Callers can override for testing.
pub fn reconcile_stale_runs(conn: &Connection, stale_after_secs: i64) -> Result<u64> {
    let now = Utc::now();
    let cutoff = (now - chrono::Duration::seconds(stale_after_secs)).to_rfc3339();
    let now_rfc = now.to_rfc3339();
    let affected = conn.execute(
        "UPDATE audit_runs SET
            status = 'Interrupted',
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            report_path = COALESCE(report_path, 'stale: backend restarted while audit was Running')
         WHERE status = 'Running' AND started_at < ?1",
        params![cutoff, now_rfc],
    )?;
    Ok(affected as u64)
}

/// 0.8.4 (#317 / B1) — admin endpoint companion. Sister of
/// `reconcile_stale_runs` but force-flags ALL `Running` rows
/// regardless of age. Used by the UI cleanup button when the user
/// knows nothing is actually running (e.g. just rebuilt the
/// containers and wants a clean slate). Returns the count.
pub fn reconcile_all_running(conn: &Connection) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let affected = conn.execute(
        "UPDATE audit_runs SET
            status = 'Interrupted',
            ended_at = ?1,
            duration_ms = CAST(((julianday(?1) - julianday(started_at)) * 86400000) AS INTEGER),
            report_path = COALESCE(report_path, 'stale: cleared by user via admin cleanup')
         WHERE status = 'Running'",
        params![now],
    )?;
    Ok(affected as u64)
}

/// KT-1134 — `reconcile_all_running`, sparing the projects whose audit is
/// live in this process: the cleanup action never settles a run under its
/// own worker. Returns the projects whose rows it settled, one per row.
pub fn reconcile_running_except(
    conn: &Connection,
    live_projects: &[String],
) -> Result<Vec<String>> {
    let tx = conn.unchecked_transaction()?;
    let running: Vec<(String, String)> = {
        let mut statement =
            tx.prepare("SELECT id, project_id FROM audit_runs WHERE status = 'Running'")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let now = Utc::now().to_rfc3339();
    let mut settled = Vec::new();
    for (id, project_id) in running {
        if live_projects.contains(&project_id) {
            continue;
        }
        tx.execute(
            "UPDATE audit_runs SET
                status = 'Interrupted',
                ended_at = ?2,
                duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
                report_path = COALESCE(report_path, 'stale: cleared by user via admin cleanup')
             WHERE id = ?1 AND status = 'Running'",
            params![id, now],
        )?;
        settled.push(project_id);
    }
    tx.commit()?;
    Ok(settled)
}

/// 0.8.3 (#311) — the project's resumable run, if any. Mirrors the launch
/// rule EXACTLY: only the project's most recent run (any status, same
/// ordering as `list_recent`) is eligible, and it must be `Interrupted`
/// with at least one completed step (step 0 produced nothing — resume =
/// restart). Anything the launch would refuse must never be offered here:
/// a stale Interrupted run behind ANY newer attempt (Completed, Running,
/// Cancelled, a fresher Interrupted) is not resumable.
pub fn latest_resumable(conn: &Connection, project_id: &str) -> Result<Option<AuditRun>> {
    let newest = list_recent(conn, project_id, 1)?.into_iter().next();
    Ok(newest.filter(|r| r.status == "Interrupted" && r.last_completed_step >= 1))
}

// ─── 0.8.4 (#298) audit_run_steps helpers ─────────────────────────────

/// Insert a fresh step row at `step_start`. The metrics fields are
/// filled in by `finalize_step` at `step_done`. Idempotent on
/// `(audit_run_id, step_index)` so a resumed audit (#311) that skips
/// already-completed steps doesn't crash on UNIQUE violation — we
/// just ignore the conflict.
pub fn insert_audit_step_start(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    file_label: &str,
    started_at: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO audit_run_steps (audit_run_id, step_index, file_label, started_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            audit_run_id,
            step_index as i64,
            file_label,
            started_at.to_rfc3339()
        ],
    )?;
    Ok(())
}

/// KT-931 — a resumed run inherits the steps its predecessor completed
/// cleanly: one finished row per inherited step and the progress count seeded
/// from them. Without those rows a second resume — of a run that skipped
/// them — would find none of them succeeded and replay the whole chain, not
/// just the step that failed again. Each row keeps the dates, duration, tokens
/// and cost of the run that really ran it (`from_run_id`, or the run it was
/// itself carried from), named in `carried_from_run_id` (KT-1021). Idempotent
/// on `(audit_run_id, step_index)`, all-or-nothing.
pub fn carry_over_steps(
    conn: &Connection,
    audit_run_id: &str,
    from_run_id: &str,
    steps: &[(u32, String)],
    at: DateTime<Utc>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let at = at.to_rfc3339();
    for (step_index, file_label) in steps {
        let copied = tx.execute(
            "INSERT OR IGNORE INTO audit_run_steps
                (audit_run_id, step_index, file_label, started_at, ended_at, duration_ms,
                 step_tokens, cli_success, input_tokens, output_tokens, cache_read_tokens,
                 cache_write_tokens, cost_usd_micros, carried_from_run_id, token_accounting,
                 unchanged, step_warning, accepted_at, accepted_by, accepted_reason)
             SELECT ?1, step_index, ?3, started_at, COALESCE(ended_at, ?4), duration_ms,
                    step_tokens, CASE WHEN accepted_at IS NULL THEN 1 ELSE cli_success END,
                    input_tokens, output_tokens, cache_read_tokens,
                    cache_write_tokens, cost_usd_micros, COALESCE(carried_from_run_id, ?5),
                    token_accounting, unchanged,
                    CASE WHEN accepted_at IS NULL THEN NULL ELSE step_warning END,
                    accepted_at, accepted_by, accepted_reason
               FROM audit_run_steps WHERE audit_run_id = ?5 AND step_index = ?2",
            params![
                audit_run_id,
                *step_index as i64,
                file_label,
                at,
                from_run_id
            ],
        )?;
        if copied == 0 {
            tx.execute(
                "INSERT OR IGNORE INTO audit_run_steps
                    (audit_run_id, step_index, file_label, started_at, ended_at, cli_success,
                     carried_from_run_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5)",
                params![
                    audit_run_id,
                    *step_index as i64,
                    file_label,
                    at,
                    from_run_id
                ],
            )?;
        }
    }
    update_last_completed_step(&tx, audit_run_id, steps.len() as u32)?;
    tx.commit()?;
    Ok(())
}

/// KT-1131 — a resume that retries only some steps keeps the other failed
/// ones as they ended: their rows are copied verbatim (failure and reason
/// included) so the timeline still shows why, and the next resume still finds
/// them to redo. A step that never ran has no row to copy.
pub fn carry_over_failed_steps(
    conn: &Connection,
    audit_run_id: &str,
    from_run_id: &str,
    steps: &[u32],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for step_index in steps {
        tx.execute(
            "INSERT OR IGNORE INTO audit_run_steps
                (audit_run_id, step_index, file_label, started_at, ended_at, duration_ms,
                 step_tokens, cli_success, step_warning, step_repaired_from_template,
                 input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                 cost_usd_micros, carried_from_run_id, token_accounting, unchanged)
             SELECT ?1, step_index, file_label, started_at, ended_at, duration_ms,
                    step_tokens, cli_success, step_warning, step_repaired_from_template,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_usd_micros, COALESCE(carried_from_run_id, ?3), token_accounting, 0
               FROM audit_run_steps
              WHERE audit_run_id = ?3 AND step_index = ?2 AND accepted_at IS NULL",
            params![audit_run_id, *step_index as i64, from_run_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// KT-1131 — record a human's acceptance of a failed step. The step keeps its
/// failure (`cli_success`, `step_warning`) and gains who, when and why; a step
/// that never ran gets a finished row carrying only the acceptance. The run's
/// count of done steps includes it. Returns `false` when the step is already
/// done (succeeded or accepted): nothing is written.
pub fn accept_step(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    file_label: &str,
    by: &str,
    reason: Option<&str>,
    at: DateTime<Utc>,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let at = at.to_rfc3339();
    let done: bool = tx
        .query_row(
            "SELECT (cli_success = 1 AND ended_at IS NOT NULL) OR accepted_at IS NOT NULL
               FROM audit_run_steps WHERE audit_run_id = ?1 AND step_index = ?2",
            params![audit_run_id, step_index as i64],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if done {
        return Ok(false);
    }
    let updated = tx.execute(
        "UPDATE audit_run_steps
            SET accepted_at = ?3, accepted_by = ?4, accepted_reason = ?5,
                ended_at = COALESCE(ended_at, ?3), cli_success = CASE
                    WHEN ended_at IS NULL THEN 0 ELSE cli_success END
          WHERE audit_run_id = ?1 AND step_index = ?2",
        params![audit_run_id, step_index as i64, at, by, reason],
    )?;
    if updated == 0 {
        tx.execute(
            "INSERT INTO audit_run_steps
                (audit_run_id, step_index, file_label, started_at, ended_at, cli_success,
                 accepted_at, accepted_by, accepted_reason)
             VALUES (?1, ?2, ?3, ?4, ?4, 0, ?4, ?5, ?6)",
            params![audit_run_id, step_index as i64, file_label, at, by, reason],
        )?;
    }
    tx.execute(
        "UPDATE audit_runs SET last_completed_step = (
            SELECT COUNT(*) FROM audit_run_steps WHERE audit_run_id = ?1 AND ended_at IS NOT NULL
               AND (cli_success = 1 OR accepted_at IS NOT NULL)
        ) WHERE id = ?1",
        params![audit_run_id],
    )?;
    tx.commit()?;
    Ok(true)
}

/// A step a run rebuilt from the repository state inherits (KT-1199).
pub struct AdoptedStep {
    pub index: u32,
    pub label: String,
    pub unchanged: bool,
    /// Who accepted the step as it stands, for an accepted failure.
    pub accepted_by: Option<String>,
}

/// KT-1199 — rebuild, on a clone, the local row of a run another machine
/// committed: an `Interrupted` run under the repository's run id, the steps
/// whose produced file still matches (finished rows, no cost: this machine
/// did not run them) and the KT-1174 baseline as that run recorded it.
/// All-or-nothing; refuses an id already used.
#[allow(clippy::too_many_arguments)]
pub fn adopt_repo_run(
    conn: &Connection,
    id: &str,
    project_id: &str,
    kind: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    reason: &str,
    steps: &[AdoptedStep],
    baseline: Option<&StartOfRun>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO audit_runs (id, project_id, kind, agent_type, started_at, ended_at,
                                 duration_ms, status, report_path, last_completed_step)
         VALUES (?1, ?2, ?3, 'repository', ?4, ?5, ?6, 'Interrupted', ?7, ?8)",
        params![
            id,
            project_id,
            kind,
            started_at.to_rfc3339(),
            ended_at.to_rfc3339(),
            (ended_at - started_at).num_milliseconds().max(0),
            format!("interrupted: {reason}"),
            steps.len() as i64,
        ],
    )?;
    let at = ended_at.to_rfc3339();
    for step in steps {
        match &step.accepted_by {
            Some(by) => tx.execute(
                "INSERT INTO audit_run_steps
                    (audit_run_id, step_index, file_label, started_at, ended_at, cli_success,
                     accepted_at, accepted_by, carried_from_run_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 0, ?4, ?5, ?1)",
                params![id, step.index as i64, step.label, at, by],
            )?,
            None => tx.execute(
                "INSERT INTO audit_run_steps
                    (audit_run_id, step_index, file_label, started_at, ended_at, cli_success,
                     unchanged, carried_from_run_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5, ?1)",
                params![id, step.index as i64, step.label, at, step.unchanged as i32],
            )?,
        };
    }
    if let Some(baseline) = baseline {
        tx.execute(
            "UPDATE audit_runs SET adr_baseline = ?2, adr_record_baseline = ?3 WHERE id = ?1",
            params![id, baseline.decisions, baseline.records],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// KT-1134 — the project's run still recorded `Running`, if any: the run a
/// cancel targets.
pub fn running_run_id(conn: &Connection, project_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM audit_runs WHERE project_id = ?1 AND status = 'Running'
             ORDER BY started_at DESC, rowid DESC LIMIT 1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// KT-1134 — validation discussions linked to the project's audit runs: the
/// ones an audit agent may still be answering in.
pub fn validation_discussion_ids(conn: &Connection, project_id: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT validation_discussion_id FROM audit_runs
          WHERE project_id = ?1 AND validation_discussion_id IS NOT NULL",
    )?;
    let ids = statement
        .query_map(params![project_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

/// KT-1134 — the project an audit validation discussion belongs to, when the
/// discussion is linked to one of its runs.
pub fn project_of_validation_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT project_id FROM audit_runs WHERE validation_discussion_id = ?1 LIMIT 1",
            params![discussion_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// What one audit step consumed, as its agent reported it (KT-927). Every part
/// is `None` when the runtime did not report it — which is not zero: a step that
/// reported nothing is of unknown cost, never free. Input is as the agent states
/// it; whether it already contains the cached share depends on the agent (see
/// `core::pricing::TokenCounters::from_agent_report`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepTokens {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    /// What `input` contains, set by [`Self::inclusive_for`] and stored with the
    /// row so the headline and its breakdown are read back the same way.
    pub accounting: TokenAccounting,
}

/// How a step's stored `input` relates to its cache, as `core::pricing` knows
/// each agent's report (`TokenCounters::from_agent_report`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TokenAccounting {
    /// As the agent reported it: whether it includes the cache is unknown.
    #[default]
    AsReported,
    /// Includes cache reads and writes (Claude, made inclusive here).
    InclusiveReadWrite,
    /// Includes cache reads (Codex).
    InclusiveRead,
}

impl TokenAccounting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AsReported => "as_reported",
            Self::InclusiveReadWrite => "inclusive_read_write",
            Self::InclusiveRead => "inclusive_read",
        }
    }

    /// `None` for a row stored before the marker existed, or an unknown value.
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "as_reported" => Some(Self::AsReported),
            "inclusive_read_write" => Some(Self::InclusiveReadWrite),
            "inclusive_read" => Some(Self::InclusiveRead),
            _ => None,
        }
    }
}

impl StepTokens {
    /// Sum reported usage across two executed attempts. A component either
    /// attempt did not report stays unknown: a partial sum would read as
    /// complete and be priced (KT-1094).
    pub(crate) fn plus(self, other: Self) -> Self {
        let sum = |a: Option<u64>, b: Option<u64>| Some(a?.saturating_add(b?));
        let reported = |tokens: &Self| tokens.input.is_some() || tokens.output.is_some();
        Self {
            input: sum(self.input, other.input),
            output: sum(self.output, other.output),
            cache_read: sum(self.cache_read, other.cache_read),
            cache_write: sum(self.cache_write, other.cache_write),
            accounting: if reported(&self) {
                self.accounting
            } else {
                other.accounting
            },
        }
    }
    /// The usage of a step's attempts so far, `previous` being `None` before
    /// the first one: no attempt yet is not an attempt that reported nothing.
    pub(crate) fn after_attempt(previous: Option<Self>, attempt: Self) -> Self {
        previous.map_or(attempt, |previous| previous.plus(attempt))
    }

    /// Nothing was reported.
    pub const UNKNOWN: Self = Self {
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
        accounting: TokenAccounting::AsReported,
    };

    /// The usage a structured transport (an ACP session, an HTTP provider)
    /// reported for the run. `None` when it reported nothing.
    pub fn from_reported(usage: Option<crate::agents::runner::ReportedUsage>) -> Self {
        let Some(usage) = usage else {
            return Self::UNKNOWN;
        };
        Self {
            input: Some(usage.input_tokens),
            output: Some(usage.output_tokens),
            cache_read: usage.prompt_cache.cached_prompt_tokens,
            cache_write: usage.prompt_cache.cache_write_prompt_tokens,
            accounting: TokenAccounting::AsReported,
        }
    }

    /// The agent's report, marked with what its input contains (KT-1021).
    /// Claude reports input without its cache reads and writes: they are added,
    /// so the stored input includes the cache as Codex's does. Any other agent's
    /// input is kept as reported, its cache split unknown.
    pub fn inclusive_for(self, agent_type: &crate::models::AgentType) -> Self {
        match agent_type {
            crate::models::AgentType::ClaudeCode => Self {
                input: self.input.map(|input| {
                    input
                        .saturating_add(self.cache_read.unwrap_or(0))
                        .saturating_add(self.cache_write.unwrap_or(0))
                }),
                accounting: TokenAccounting::InclusiveReadWrite,
                ..self
            },
            crate::models::AgentType::Codex => Self {
                accounting: TokenAccounting::InclusiveRead,
                ..self
            },
            _ => Self {
                accounting: TokenAccounting::AsReported,
                ..self
            },
        }
    }

    /// The four disjoint counters `core::pricing` bills on, through its own
    /// per-agent rule. `None` when the cache split is unknown: an agent whose
    /// accounting it does not know, or a missing part.
    pub fn counters(&self) -> Option<crate::core::pricing::TokenCounters> {
        let input = self.input?;
        let output = self.output?;
        match self.accounting {
            TokenAccounting::InclusiveReadWrite => {
                let cached = self
                    .cache_read
                    .unwrap_or(0)
                    .saturating_add(self.cache_write.unwrap_or(0));
                crate::core::pricing::TokenCounters::from_agent_report(
                    "ClaudeCode",
                    input.checked_sub(cached)?,
                    output,
                    self.cache_read,
                    self.cache_write,
                )
            }
            TokenAccounting::InclusiveRead => {
                crate::core::pricing::TokenCounters::from_agent_report(
                    "Codex",
                    input,
                    output,
                    self.cache_read,
                    self.cache_write,
                )
            }
            TokenAccounting::AsReported => None,
        }
    }

    /// The step's headline figure: the fresh traffic, uncached input plus
    /// output, when the cache split is known; otherwise input plus output as
    /// reported. `None` — unknown — unless at least one of them was reported.
    pub fn total(&self) -> Option<u64> {
        if let Some(counters) = self.counters() {
            return Some(counters.input_tokens.saturating_add(counters.output_tokens));
        }
        match (self.input, self.output) {
            (None, None) => None,
            (input, output) => Some(input.unwrap_or(0).saturating_add(output.unwrap_or(0))),
        }
    }

    /// The headline's parts, shown on hover.
    pub fn breakdown(&self) -> crate::models::AuditTokenBreakdown {
        match self.counters() {
            Some(counters) => crate::models::AuditTokenBreakdown {
                uncached_input: Some(counters.input_tokens),
                input_as_reported: None,
                output: self.output,
                cache_read: self.cache_read,
                cache_write: self.cache_write,
                total_with_cache: Some(counters.traffic_tokens()),
            },
            None => crate::models::AuditTokenBreakdown {
                uncached_input: None,
                input_as_reported: self.input,
                output: self.output,
                cache_read: self.cache_read,
                cache_write: self.cache_write,
                total_with_cache: None,
            },
        }
    }
}

/// A later documentary gate can invalidate an earlier successful agent step.
/// Preserve its timing/usage and original warning; resume must not skip it.
pub(crate) fn invalidate_documentary_steps(
    conn: &Connection,
    run_id: &str,
    warnings: &[(u32, String)],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (step, warning) in warnings {
        tx.execute("UPDATE audit_run_steps SET cli_success = 0, unchanged = 0,
            step_warning = CASE WHEN step_warning IS NULL THEN ?3 ELSE step_warning || char(10) || ?3 END
            WHERE audit_run_id = ?1 AND step_index = ?2", params![run_id, step, warning])?;
    }
    tx.execute("UPDATE audit_runs SET last_completed_step = (
        SELECT COUNT(*) FROM audit_run_steps WHERE audit_run_id = ?1 AND cli_success = 1 AND ended_at IS NOT NULL
    ) WHERE id = ?1", params![run_id])?;
    tx.commit()?;
    Ok(())
}

/// Finalize a step row at `step_done` (or step_warning). `success`
/// is `false` when `validate_step_output` (#292) emitted
/// a warning OR when the CLI exited non-zero. `step_warning` is the
/// reason string (None on success). `cumulative_tokens` is the run's total so
/// far, `None` until a step has reported something.
#[allow(clippy::too_many_arguments)]
pub fn finalize_audit_step(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    ended_at: DateTime<Utc>,
    duration_ms: u64,
    tokens: &StepTokens,
    cumulative_tokens: Option<u64>,
    cli_success: bool,
    step_warning: Option<&str>,
    repaired: bool,
) -> Result<()> {
    finalize_audit_step_with_outcome(
        conn,
        audit_run_id,
        step_index,
        ended_at,
        duration_ms,
        tokens,
        cumulative_tokens,
        cli_success,
        step_warning,
        repaired,
        false,
    )
}

/// `finalize_audit_step`, also recording whether the step ended `unchanged`:
/// its target was already up to date and every gate passed (KT-1130).
#[allow(clippy::too_many_arguments)]
pub fn finalize_audit_step_with_outcome(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    ended_at: DateTime<Utc>,
    duration_ms: u64,
    tokens: &StepTokens,
    cumulative_tokens: Option<u64>,
    cli_success: bool,
    step_warning: Option<&str>,
    repaired: bool,
    unchanged: bool,
) -> Result<()> {
    let as_sql = |value: Option<u64>| value.map(|value| value as i64);
    conn.execute(
        "UPDATE audit_run_steps SET
            ended_at = ?3,
            duration_ms = ?4,
            step_tokens = ?5,
            cumulative_tokens = ?6,
            cli_success = ?7,
            step_warning = ?8,
            step_repaired_from_template = ?9,
            input_tokens = ?10,
            output_tokens = ?11,
            cache_read_tokens = ?12,
            cache_write_tokens = ?13,
            token_accounting = ?14,
            unchanged = ?15
         WHERE audit_run_id = ?1 AND step_index = ?2",
        params![
            audit_run_id,
            step_index as i64,
            ended_at.to_rfc3339(),
            duration_ms as i64,
            as_sql(tokens.total()),
            as_sql(cumulative_tokens),
            cli_success as i32,
            step_warning,
            repaired as i32,
            as_sql(tokens.input),
            as_sql(tokens.output),
            as_sql(tokens.cache_read),
            as_sql(tokens.cache_write),
            tokens.accounting.as_str(),
            unchanged as i32,
        ],
    )?;
    Ok(())
}

/// All steps of an audit run, ordered by step_index. Used by the
/// ProjectCard recap panel.
pub fn list_audit_steps(conn: &Connection, audit_run_id: &str) -> Result<Vec<AuditRunStep>> {
    let mut stmt = conn.prepare(
        "SELECT audit_run_id, step_index, file_label, started_at, ended_at,
                duration_ms, step_tokens, cumulative_tokens, cli_success,
                step_warning, step_repaired_from_template,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                cost_usd_micros, carried_from_run_id, token_accounting, unchanged,
                accepted_at, accepted_by, accepted_reason, target_state
         FROM audit_run_steps
         WHERE audit_run_id = ?1
         ORDER BY step_index ASC",
    )?;
    let rows = stmt.query_map(params![audit_run_id], |row| {
        let started_str: String = row.get(3)?;
        let ended_str: Option<String> = row.get(4)?;
        Ok(AuditRunStep {
            audit_run_id: row.get(0)?,
            step_index: row.get::<_, i64>(1)? as u32,
            file_label: row.get(2)?,
            started_at: DateTime::parse_from_rfc3339(&started_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            ended_at: ended_str.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            duration_ms: row.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as u64),
            step_tokens: row.get::<_, Option<i64>>(6)?.map(|v| v.max(0) as u64),
            cumulative_tokens: row.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as u64),
            input_tokens: row.get::<_, Option<i64>>(11)?.map(|v| v.max(0) as u64),
            output_tokens: row.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
            cache_read_tokens: row.get::<_, Option<i64>>(13)?.map(|v| v.max(0) as u64),
            cache_write_tokens: row.get::<_, Option<i64>>(14)?.map(|v| v.max(0) as u64),
            cli_success: row.get::<_, i64>(8)? != 0,
            step_warning: row.get(9)?,
            step_repaired_from_template: row.get::<_, i64>(10)? != 0,
            cost_usd_micros: row.get::<_, Option<i64>>(15)?.map(|v| v.max(0) as u64),
            carried_from_run_id: row.get(16)?,
            estimated_cost_usd_micros: None,
            cost_unknown_reason: None,
            breakdown: None,
            token_accounting: row.get(17)?,
            unchanged: row.get::<_, i64>(18)? != 0,
            accepted_at: row.get::<_, Option<String>>(19)?.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            accepted_by: row.get(20)?,
            accepted_reason: row.get(21)?,
            target_state: row.get(22)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        let mut step = r?;
        step.breakdown = step
            .step_tokens
            .and_then(|_| stored_tokens(&step))
            .map(|tokens| tokens.breakdown());
        out.push(step);
    }
    Ok(out)
}

/// A row's parts read back with its stored accounting; `None` for a row
/// stored before the accounting was recorded.
fn stored_tokens(step: &AuditRunStep) -> Option<StepTokens> {
    let accounting = TokenAccounting::parse(step.token_accounting.as_deref()?)?;
    Some(StepTokens {
        input: step.input_tokens,
        output: step.output_tokens,
        cache_read: step.cache_read_tokens,
        cache_write: step.cache_write_tokens,
        accounting,
    })
}

/// Fill the cost Kronn can estimate for each finished step whose agent reported
/// none, from its counters and the model its run served, priced at Kronn's
/// rates; or why it cannot. A configured model, or several, is not a served one.
pub fn price_steps(conn: &Connection, steps: &mut [AuditRunStep]) -> Result<()> {
    let mut runs: HashMap<String, (String, Option<String>)> = HashMap::new();
    for step in steps.iter_mut() {
        if step.ended_at.is_none() || step.cost_usd_micros.is_some() {
            continue;
        }
        // A row stored before its accounting was recorded is never estimated.
        let Some(tokens) = stored_tokens(step) else {
            continue;
        };
        let run_id = step
            .carried_from_run_id
            .clone()
            .unwrap_or_else(|| step.audit_run_id.clone());
        if !runs.contains_key(&run_id) {
            let found = conn
                .query_row(
                    "SELECT agent_type, model FROM audit_runs WHERE id = ?1",
                    params![run_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            runs.insert(run_id.clone(), found.unwrap_or_default());
        }
        let (agent_type, model) = &runs[&run_id];
        let served = model
            .as_deref()
            .filter(|model| !model.ends_with("(configured)") && !model.contains(" / "));
        // `counters` follows `core::pricing`'s per-agent rule: an agent whose
        // cache accounting it does not know has none, and stays unpriced.
        let priced = crate::core::pricing::price_reply(
            agent_type,
            served,
            step.step_tokens.unwrap_or(0),
            None,
            tokens.counters(),
        );
        step.estimated_cost_usd_micros = priced
            .cost_usd
            .and_then(crate::agents::chat_codec::usd_to_micros);
        step.cost_unknown_reason = priced.cost_unknown.map(|reason| reason.reason().to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_keeps_the_adr_baseline_of_the_run_it_continues() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p', 'p', '/p', 'now', 'now')",
            [],
        )
        .unwrap();
        for id in ["first", "second", "third"] {
            insert_running(&conn, id, "p", "Full", "ClaudeCode", Utc::now()).unwrap();
        }
        let run_start = |decisions: &str, records: &str| StartOfRun {
            decisions: decisions.to_string(),
            records: records.to_string(),
        };
        let before = run_start(
            "## ADR-0001: before\n- **Status**: accepted\n",
            "TD-20261001-old ADR-0001\n",
        );
        assert_eq!(
            record_adr_baseline(&conn, "first", None, before.clone()).unwrap(),
            before
        );
        // The interrupted run's step 9 wrote an accepted ADR and its TD step a
        // record line: the resume ignores both.
        let after_step9 = run_start(
            &format!(
                "{}## ADR-0002: late\n- **Status**: accepted\n",
                before.decisions
            ),
            "TD-20261001-old ADR-0001\nTD-20261011-new ADR-0001\n",
        );
        assert_eq!(
            record_adr_baseline(&conn, "second", Some("first"), after_step9.clone()).unwrap(),
            before
        );
        assert_eq!(adr_baseline(&conn, "second").unwrap(), Some(before.clone()));
        // A predecessor without a baseline (older row): what the run finds.
        assert_eq!(
            record_adr_baseline(&conn, "third", Some("missing"), after_step9.clone()).unwrap(),
            after_step9
        );
        assert_eq!(adr_baseline(&conn, "third").unwrap(), Some(after_step9));
    }

    #[test]
    fn the_launch_tier_and_connection_are_kept_on_the_run() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "LiteLlm", Utc::now()).unwrap();
        let fresh = get_by_id(&conn, "r1").unwrap().unwrap();
        assert!(fresh.tier.is_none() && fresh.connection_id.is_none());
        set_run_launch(
            &conn,
            "r1",
            crate::models::ModelTier::Economy,
            Some("conn-a"),
        )
        .unwrap();
        let run = get_by_id(&conn, "r1").unwrap().unwrap();
        assert_eq!(run.tier.as_deref(), Some("economy"));
        assert_eq!(run.connection_id.as_deref(), Some("conn-a"));
        assert_eq!(
            crate::models::ModelTier::parse(run.tier.as_deref().unwrap()),
            Some(crate::models::ModelTier::Economy)
        );
    }

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Project FK is checked at row insert time, so spin up the bare
        // minimum tables.
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT, path TEXT);
             CREATE TABLE audit_runs (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                kind TEXT NOT NULL DEFAULT 'Full',
                agent_type TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                duration_ms INTEGER,
                status TEXT NOT NULL DEFAULT 'Running',
                td_critical INTEGER NOT NULL DEFAULT 0,
                td_high INTEGER NOT NULL DEFAULT 0,
                td_medium INTEGER NOT NULL DEFAULT 0,
                td_low INTEGER NOT NULL DEFAULT 0,
                td_total INTEGER NOT NULL DEFAULT 0,
                td_resolved_since_last INTEGER NOT NULL DEFAULT 0,
                td_new_since_last INTEGER NOT NULL DEFAULT 0,
                td_carried_over INTEGER NOT NULL DEFAULT 0,
                health_score INTEGER,
                report_path TEXT,
                recommendations_json TEXT,
                last_completed_step INTEGER NOT NULL DEFAULT 0,
                validation_discussion_id TEXT,
                step_outcomes_json TEXT,
                head_sha TEXT,
                branch TEXT,
                source_fingerprint TEXT,
                model TEXT,
                project_profile_draft TEXT,
                validation_completed_at TEXT,
                validation_completed_message_id TEXT,
                cancelled_at_step INTEGER,
                resume_drift TEXT,
                tier TEXT,
                connection_id TEXT,
                agent_pgid INTEGER,
                agent_started_marker TEXT
            );
            CREATE TABLE audit_step_snapshots (
                audit_run_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                target_file TEXT NOT NULL,
                existed INTEGER NOT NULL,
                content TEXT,
                observed_hash TEXT,
                PRIMARY KEY (audit_run_id, step_index)
            );
            CREATE TABLE audit_run_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                audit_run_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                file_label TEXT NOT NULL,
                started_at DATETIME NOT NULL,
                ended_at DATETIME,
                duration_ms INTEGER,
                step_tokens INTEGER,
                cumulative_tokens INTEGER,
                cli_success INTEGER NOT NULL DEFAULT 1,
                step_warning TEXT,
                step_repaired_from_template INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                cost_usd_micros INTEGER,
                carried_from_run_id TEXT,
                token_accounting TEXT,
                unchanged INTEGER NOT NULL DEFAULT 0,
                accepted_at TEXT,
                accepted_by TEXT,
                accepted_reason TEXT,
                target_state TEXT,
                FOREIGN KEY (audit_run_id) REFERENCES audit_runs(id) ON DELETE CASCADE
            );
            CREATE UNIQUE INDEX idx_audit_run_steps_run
                ON audit_run_steps(audit_run_id, step_index);
            INSERT INTO projects (id, name, path) VALUES ('p1', 'Test', '/tmp/test');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn an_accepted_step_keeps_its_failure_counts_done_and_is_carried_as_accepted() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        insert_audit_step_start(&conn, "r1", 1, "docs/a.md", Utc::now()).unwrap();
        finalize_audit_step(
            &conn,
            "r1",
            1,
            Utc::now(),
            5,
            &StepTokens::default(),
            None,
            true,
            None,
            false,
        )
        .unwrap();
        insert_audit_step_start(&conn, "r1", 2, "docs/b.md", Utc::now()).unwrap();
        finalize_audit_step(
            &conn,
            "r1",
            2,
            Utc::now(),
            5,
            &StepTokens::default(),
            None,
            false,
            Some("placeholders remain"),
            false,
        )
        .unwrap();
        // Step 3 never ran; step 1 succeeded and cannot be accepted.
        assert!(!accept_step(&conn, "r1", 1, "docs/a.md", "romu", None, Utc::now()).unwrap());
        assert!(accept_step(
            &conn,
            "r1",
            2,
            "docs/b.md",
            "romu",
            Some("by hand"),
            Utc::now()
        )
        .unwrap());
        assert!(accept_step(&conn, "r1", 3, "docs/c.md", "romu", None, Utc::now()).unwrap());
        assert!(!accept_step(&conn, "r1", 2, "docs/b.md", "romu", None, Utc::now()).unwrap());
        let steps = list_audit_steps(&conn, "r1").unwrap();
        assert!(!steps[1].cli_success);
        assert_eq!(
            steps[1].step_warning.as_deref(),
            Some("placeholders remain")
        );
        assert_eq!(steps[1].accepted_by.as_deref(), Some("romu"));
        assert_eq!(steps[1].accepted_reason.as_deref(), Some("by hand"));
        assert!(steps[2].accepted_at.is_some() && steps[2].ended_at.is_some());
        assert!(!steps[2].cli_success);
        assert_eq!(
            get_by_id(&conn, "r1").unwrap().unwrap().last_completed_step,
            3
        );

        // A resume carries the acceptance, the failure and who decided.
        insert_running(&conn, "r2", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        let carried: Vec<(u32, String)> = (1..=3).map(|i| (i, format!("docs/{i}.md"))).collect();
        carry_over_steps(&conn, "r2", "r1", &carried, Utc::now()).unwrap();
        let steps = list_audit_steps(&conn, "r2").unwrap();
        assert!(steps[0].cli_success && steps[0].accepted_at.is_none());
        assert!(steps[0].step_warning.is_none());
        assert!(!steps[1].cli_success);
        assert_eq!(
            steps[1].step_warning.as_deref(),
            Some("placeholders remain")
        );
        assert_eq!(steps[1].accepted_reason.as_deref(), Some("by hand"));
        assert_eq!(steps[1].carried_from_run_id.as_deref(), Some("r1"));
    }

    #[test]
    fn a_single_step_retry_carries_the_other_failures_verbatim() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        for (step, ok) in [(1, false), (2, false)] {
            insert_audit_step_start(&conn, "r1", step, "docs/x.md", Utc::now()).unwrap();
            finalize_audit_step(
                &conn,
                "r1",
                step,
                Utc::now(),
                5,
                &StepTokens::default(),
                None,
                ok,
                Some("rewrote nothing"),
                false,
            )
            .unwrap();
        }
        insert_running(&conn, "r2", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        carry_over_failed_steps(&conn, "r2", "r1", &[2, 3]).unwrap();
        let steps = list_audit_steps(&conn, "r2").unwrap();
        assert_eq!(steps.len(), 1, "a step that never ran has no row to carry");
        assert_eq!(steps[0].step_index, 2);
        assert!(!steps[0].cli_success);
        assert_eq!(steps[0].step_warning.as_deref(), Some("rewrote nothing"));
        assert_eq!(steps[0].carried_from_run_id.as_deref(), Some("r1"));
    }

    #[test]
    fn the_cleanup_spares_the_runs_of_live_audits() {
        let conn = fresh_conn();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p2', 'Other', '/tmp/other')",
            [],
        )
        .unwrap();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        insert_running(&conn, "r2", "p2", "Full", "ClaudeCode", Utc::now()).unwrap();
        let settled = reconcile_running_except(&conn, &["p2".to_string()]).unwrap();
        assert_eq!(settled, vec!["p1".to_string()]);
        assert_eq!(
            get_by_id(&conn, "r1").unwrap().unwrap().status,
            "Interrupted"
        );
        assert_eq!(get_by_id(&conn, "r2").unwrap().unwrap().status, "Running");
        assert_eq!(running_run_id(&conn, "p2").unwrap().as_deref(), Some("r2"));
        assert_eq!(running_run_id(&conn, "p1").unwrap(), None);
    }

    #[test]
    fn insert_and_complete_round_trip() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "run-1", "p1", "Full", "ClaudeCode", start).unwrap();
        assert!(has_running_for_project(&conn, "p1").unwrap());
        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "Running");
        assert!(runs[0].ended_at.is_none());
        assert_eq!(runs[0].td_total, 0);

        let end = start + chrono::Duration::seconds(120);
        complete(
            &conn, "run-1", end, "Completed",
            1, 6, 6, 5,    // 1C/6H/6M/5L → DOCROMS_WEB calibration
            3, 8, 7,       // resolved / new / carried
            53,            // health score
            Some("docs/tech-debt/_reconciliation-2026-05-13.md"),
            Some("[{\"kind\":\"Security\",\"reason\":\"3 findings touch secrets\",\"cluster_size\":3}]"),
        )
        .unwrap();

        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert!(!has_running_for_project(&conn, "p1").unwrap());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "Completed");
        assert_eq!(runs[0].td_total, 18);
        assert_eq!(runs[0].health_score, Some(53));
        // Duration ~120s = 120000 ms, allow a small clock-skew margin.
        let dur = runs[0].duration_ms.unwrap();
        assert!((119_500..=120_500).contains(&dur), "got duration_ms={dur}");
        assert_eq!(
            runs[0].report_path.as_deref(),
            Some("docs/tech-debt/_reconciliation-2026-05-13.md")
        );
        assert!(runs[0].recommendations_json.is_some());
    }

    #[test]
    fn a_resume_chain_keeps_each_step_s_tokens_duration_and_origin() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "run-a", "p1", "Full", "ClaudeCode", start).unwrap();
        insert_audit_step_start(&conn, "run-a", 1, "docs/AGENTS.md", start).unwrap();
        let tokens = StepTokens {
            input: Some(1_000),
            output: Some(200),
            cache_read: Some(50),
            cache_write: None,
            ..StepTokens::UNKNOWN
        };
        finalize_audit_step(
            &conn, "run-a", 1, start, 61_000, &tokens, None, true, None, false,
        )
        .unwrap();
        set_step_cost(&conn, "run-a", 1, 12_345).unwrap();

        insert_running(&conn, "run-b", "p1", "Full", "ClaudeCode", start).unwrap();
        carry_over_steps(
            &conn,
            "run-b",
            "run-a",
            &[(1, "docs/AGENTS.md".into())],
            Utc::now(),
        )
        .unwrap();
        insert_running(&conn, "run-c", "p1", "Full", "ClaudeCode", start).unwrap();
        carry_over_steps(
            &conn,
            "run-c",
            "run-b",
            &[(1, "docs/AGENTS.md".into())],
            Utc::now(),
        )
        .unwrap();

        let step = &list_audit_steps(&conn, "run-c").unwrap()[0];
        assert_eq!(step.duration_ms, Some(61_000));
        assert_eq!(step.step_tokens, Some(1_200));
        assert_eq!(step.input_tokens, Some(1_000));
        assert_eq!(step.cache_read_tokens, Some(50));
        assert_eq!(step.cost_usd_micros, Some(12_345));
        assert_eq!(
            step.carried_from_run_id.as_deref(),
            Some("run-a"),
            "the run that spent it"
        );
        assert!(step.cli_success);

        // A step the source run never recorded is still carried, of unknown cost.
        carry_over_steps(
            &conn,
            "run-c",
            "run-b",
            &[(2, "docs/x.md".into())],
            Utc::now(),
        )
        .unwrap();
        let missing = &list_audit_steps(&conn, "run-c").unwrap()[1];
        assert_eq!(missing.step_tokens, None);
        assert_eq!(missing.cost_usd_micros, None);
        assert_eq!(missing.carried_from_run_id.as_deref(), Some("run-b"));
    }

    #[test]
    fn an_unchanged_step_is_recorded_carried_and_cleared_by_a_documentary_reopen() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "run-a", "p1", "Full", "ClaudeCode", start).unwrap();
        for (step, unchanged) in [(1, true), (2, false)] {
            insert_audit_step_start(&conn, "run-a", step, "docs/x.md", start).unwrap();
            finalize_audit_step_with_outcome(
                &conn,
                "run-a",
                step,
                start,
                10,
                &StepTokens::UNKNOWN,
                None,
                true,
                None,
                false,
                unchanged,
            )
            .unwrap();
        }
        let steps = list_audit_steps(&conn, "run-a").unwrap();
        assert!(steps[0].unchanged && steps[0].cli_success);
        assert!(!steps[1].unchanged);

        // A resume carries the flag with the step.
        insert_running(&conn, "run-b", "p1", "Full", "ClaudeCode", start).unwrap();
        carry_over_steps(
            &conn,
            "run-b",
            "run-a",
            &[(1, "docs/x.md".into()), (2, "docs/x.md".into())],
            Utc::now(),
        )
        .unwrap();
        let carried = list_audit_steps(&conn, "run-b").unwrap();
        assert!(carried[0].unchanged);
        assert!(!carried[1].unchanged);

        // A later documentary blocker reopens it: failed, no longer unchanged.
        invalidate_documentary_steps(&conn, "run-b", &[(1, "broken link".into())]).unwrap();
        let reopened = &list_audit_steps(&conn, "run-b").unwrap()[0];
        assert!(!reopened.cli_success && !reopened.unchanged);
    }

    #[test]
    fn run_provenance_round_trips_and_stays_unknown_by_default() {
        let conn = fresh_conn();
        insert_running(&conn, "r", "p1", "Full", "Codex", Utc::now()).unwrap();
        let fresh = get_by_id(&conn, "r").unwrap().unwrap();
        assert!(fresh.head_sha.is_none() && fresh.model.is_none());
        set_run_provenance(
            &conn,
            "r",
            Some("abc123"),
            Some("feat/é"),
            Some("fp"),
            Some("gpt-x"),
        )
        .unwrap();
        let run = get_by_id(&conn, "r").unwrap().unwrap();
        assert_eq!(run.head_sha.as_deref(), Some("abc123"));
        assert_eq!(run.branch.as_deref(), Some("feat/é"));
        assert_eq!(run.source_fingerprint.as_deref(), Some("fp"));
        assert_eq!(run.model.as_deref(), Some("gpt-x"));
    }

    #[test]
    fn token_totals_follow_the_pricing_modules_rule_for_each_agent() {
        use crate::models::AgentType;
        // The same real traffic: 100 fresh in, 900 read from and 50 written to
        // the cache, 10 out. Claude reports input without its cache, Codex
        // with its cache reads.
        let claude = StepTokens {
            input: Some(100),
            output: Some(10),
            cache_read: Some(900),
            cache_write: Some(50),
            ..StepTokens::UNKNOWN
        }
        .inclusive_for(&AgentType::ClaudeCode);
        let codex = StepTokens {
            input: Some(1_000),
            output: Some(10),
            cache_read: Some(900),
            cache_write: Some(50),
            ..StepTokens::UNKNOWN
        }
        .inclusive_for(&AgentType::Codex);
        assert_eq!(claude.total(), Some(110));
        assert_eq!(
            codex.total(),
            Some(110),
            "Codex: only the cache reads come out"
        );
        let breakdown = claude.breakdown();
        assert_eq!(breakdown, codex.breakdown());
        assert_eq!(
            (
                breakdown.uncached_input,
                breakdown.output,
                breakdown.cache_read,
                breakdown.cache_write
            ),
            (Some(100), Some(10), Some(900), Some(50))
        );
        assert_eq!(
            breakdown.total_with_cache,
            Some(1_060),
            "the vendor's full traffic"
        );
        assert_eq!(breakdown.input_as_reported, None);

        // An agent whose cache accounting pricing does not know: input as
        // reported, the split unknown, never a subtraction.
        for agent in [
            AgentType::OpenCode,
            AgentType::CopilotCli,
            AgentType::LiteLlm,
            AgentType::GeminiCli,
        ] {
            let tokens = StepTokens {
                input: Some(1_000),
                output: Some(500),
                cache_read: Some(900),
                cache_write: None,
                ..StepTokens::UNKNOWN
            }
            .inclusive_for(&agent);
            assert_eq!(tokens.total(), Some(1_500), "{agent:?}");
            assert_eq!(tokens.counters(), None, "{agent:?}");
            let breakdown = tokens.breakdown();
            assert_eq!(
                (
                    breakdown.uncached_input,
                    breakdown.input_as_reported,
                    breakdown.total_with_cache
                ),
                (None, Some(1_000), None),
                "{agent:?}"
            );
        }
        // Codex with more cache reads than its input: the split is not trusted.
        let unsplit = StepTokens {
            input: Some(10),
            output: Some(5),
            cache_read: Some(900),
            ..StepTokens::UNKNOWN
        }
        .inclusive_for(&AgentType::Codex);
        assert_eq!((unsplit.total(), unsplit.counters()), (Some(15), None));
        assert_eq!(
            StepTokens::UNKNOWN
                .inclusive_for(&AgentType::ClaudeCode)
                .total(),
            None,
            "unknown stays unknown"
        );
    }

    fn finished_step(conn: &Connection, run: &str, tokens: StepTokens) {
        let start = Utc::now();
        insert_audit_step_start(conn, run, 1, "docs/a.md", start).unwrap();
        finalize_audit_step(
            conn,
            run,
            1,
            start,
            1_000,
            &tokens,
            tokens.total(),
            true,
            None,
            false,
        )
        .unwrap();
    }

    fn codex_traffic() -> StepTokens {
        StepTokens {
            input: Some(1_000_000),
            output: Some(100_000),
            cache_read: Some(600_000),
            cache_write: None,
            ..StepTokens::UNKNOWN
        }
        .inclusive_for(&crate::models::AgentType::Codex)
    }

    #[test]
    fn a_step_without_a_reported_cost_is_estimated_at_the_served_models_rates() {
        let conn = fresh_conn();
        insert_running(&conn, "r", "p1", "Full", "Codex", Utc::now()).unwrap();
        set_run_model(&conn, "r", "gpt-5").unwrap();
        finished_step(&conn, "r", codex_traffic());
        let mut steps = list_audit_steps(&conn, "r").unwrap();
        price_steps(&conn, &mut steps).unwrap();
        let expected = crate::core::pricing::message_cost(
            "Codex",
            Some("gpt-5"),
            Some(&crate::core::pricing::TokenCounters::from_disjoint_input(
                400_000,
                100_000,
                Some(600_000),
                None,
            )),
        )
        .usd()
        .and_then(crate::agents::chat_codec::usd_to_micros);
        assert!(expected.is_some_and(|micros| micros > 0));
        assert_eq!(steps[0].estimated_cost_usd_micros, expected);
        assert_eq!(
            steps[0].cost_usd_micros, None,
            "an estimate is never stored as reported"
        );
        assert_eq!(steps[0].cost_unknown_reason, None);
        assert_eq!(steps[0].step_tokens, Some(500_000), "fresh headline");

        // A reported cost is the cost: nothing to estimate.
        set_step_cost(&conn, "r", 1, 42).unwrap();
        let mut steps = list_audit_steps(&conn, "r").unwrap();
        price_steps(&conn, &mut steps).unwrap();
        assert_eq!(
            (steps[0].cost_usd_micros, steps[0].estimated_cost_usd_micros),
            (Some(42), None)
        );
    }

    fn codex_attempt(cache_read: Option<u64>) -> StepTokens {
        StepTokens {
            input: Some(1_000),
            output: Some(100),
            cache_read,
            ..StepTokens::UNKNOWN
        }
        .inclusive_for(&crate::models::AgentType::Codex)
    }

    #[test]
    fn a_counter_missing_from_one_attempt_leaves_the_sum_unknown() {
        let (absent, present) = (codex_attempt(None), codex_attempt(Some(900)));
        for (first, second) in [(absent, present), (present, absent)] {
            let merged = StepTokens::after_attempt(Some(first), second);
            assert_eq!(merged.cache_read, None);
            assert_eq!(
                merged.input,
                Some(2_000),
                "fully reported parts still add up"
            );
            assert!(
                merged.counters().is_none(),
                "never a complete-looking split"
            );
        }
        let merged = StepTokens::after_attempt(Some(present), present);
        assert_eq!(merged.cache_read, Some(1_800));
        assert!(merged.counters().is_some());

        // An attempt that reported nothing at all makes every part unknown.
        let merged = StepTokens::after_attempt(Some(StepTokens::UNKNOWN), present);
        assert_eq!(
            (merged.input, merged.output, merged.cache_read),
            (None, None, None)
        );
    }

    #[test]
    fn the_first_attempt_is_not_merged_with_the_empty_accumulator() {
        let present = codex_attempt(Some(900));
        assert_eq!(StepTokens::after_attempt(None, present), present);
        assert_eq!(
            StepTokens::after_attempt(None, StepTokens::UNKNOWN),
            StepTokens::UNKNOWN
        );
    }

    #[test]
    fn a_retried_step_with_an_incomplete_attempt_is_never_estimated() {
        let conn = fresh_conn();
        insert_running(&conn, "r", "p1", "Full", "Codex", Utc::now()).unwrap();
        set_run_model(&conn, "r", "gpt-5").unwrap();
        let first = StepTokens::after_attempt(None, codex_attempt(None));
        finished_step(
            &conn,
            "r",
            StepTokens::after_attempt(Some(first), codex_attempt(Some(900))),
        );
        let mut steps = list_audit_steps(&conn, "r").unwrap();
        price_steps(&conn, &mut steps).unwrap();
        assert_eq!(steps[0].cache_read_tokens, None);
        assert_eq!(steps[0].estimated_cost_usd_micros, None);
        assert!(steps[0].cost_unknown_reason.is_some());

        // Both attempts complete: the cumulative step is still priced.
        let conn = fresh_conn();
        insert_running(&conn, "r", "p1", "Full", "Codex", Utc::now()).unwrap();
        set_run_model(&conn, "r", "gpt-5").unwrap();
        let first = StepTokens::after_attempt(None, codex_attempt(Some(900)));
        finished_step(
            &conn,
            "r",
            StepTokens::after_attempt(Some(first), codex_attempt(Some(900))),
        );
        let mut steps = list_audit_steps(&conn, "r").unwrap();
        price_steps(&conn, &mut steps).unwrap();
        assert!(steps[0]
            .estimated_cost_usd_micros
            .is_some_and(|micros| micros > 0));
    }

    #[test]
    fn an_agent_whose_cache_accounting_is_unknown_is_never_estimated() {
        let conn = fresh_conn();
        for agent in [
            crate::models::AgentType::OpenCode,
            crate::models::AgentType::CopilotCli,
        ] {
            let run = format!("r-{agent:?}");
            insert_running(&conn, &run, "p1", "Full", &format!("{agent:?}"), Utc::now()).unwrap();
            set_run_model(&conn, &run, "gpt-5").unwrap();
            let tokens = StepTokens {
                input: Some(1_000),
                output: Some(500),
                cache_read: Some(300),
                ..StepTokens::UNKNOWN
            }
            .inclusive_for(&agent);
            finished_step(&conn, &run, tokens);
            let mut steps = list_audit_steps(&conn, &run).unwrap();
            price_steps(&conn, &mut steps).unwrap();
            let step = &steps[0];
            assert_eq!(
                step.step_tokens,
                Some(1_500),
                "{agent:?}: input as reported"
            );
            assert_eq!(step.estimated_cost_usd_micros, None, "{agent:?}");
            assert_eq!(
                step.cost_unknown_reason.as_deref(),
                Some(crate::core::pricing::UnknownCost::NoTokenBreakdown.reason()),
                "{agent:?}"
            );
            let breakdown = step.breakdown.unwrap();
            assert_eq!(
                (breakdown.uncached_input, breakdown.input_as_reported),
                (None, Some(1_000))
            );
        }
    }

    /// A row stored before its accounting was: its headline included the
    /// cache. It keeps that headline, with no breakdown to count the cache
    /// twice and no estimate.
    #[test]
    fn a_row_stored_before_its_accounting_keeps_its_headline_without_breakdown_or_estimate() {
        let conn = fresh_conn();
        insert_running(&conn, "old", "p1", "Full", "Codex", Utc::now()).unwrap();
        set_run_model(&conn, "old", "gpt-5").unwrap();
        finished_step(&conn, "old", codex_traffic());
        conn.execute(
            "UPDATE audit_run_steps SET step_tokens = 1100000, token_accounting = NULL",
            [],
        )
        .unwrap();
        let mut steps = list_audit_steps(&conn, "old").unwrap();
        price_steps(&conn, &mut steps).unwrap();
        let step = &steps[0];
        assert_eq!(step.step_tokens, Some(1_100_000), "shown as stored");
        assert_eq!(step.breakdown, None);
        assert_eq!(
            (
                step.estimated_cost_usd_micros,
                step.cost_unknown_reason.as_deref()
            ),
            (None, None)
        );

        // Carried into a resume, it stays an old row.
        insert_running(&conn, "resume", "p1", "Full", "Codex", Utc::now()).unwrap();
        carry_over_steps(
            &conn,
            "resume",
            "old",
            &[(1, "docs/a.md".into())],
            Utc::now(),
        )
        .unwrap();
        assert_eq!(
            list_audit_steps(&conn, "resume").unwrap()[0].breakdown,
            None
        );
    }

    #[test]
    fn an_unknown_or_unserved_model_keeps_the_cost_unknown_with_its_reason() {
        let conn = fresh_conn();
        for (run, model, reason) in [
            (
                "r-unknown",
                Some("mystery-model-9"),
                crate::core::pricing::UnknownCost::NoRateForModel,
            ),
            (
                "r-configured",
                Some("gpt-5 (configured)"),
                crate::core::pricing::UnknownCost::ModelNotReported,
            ),
            (
                "r-none",
                None,
                crate::core::pricing::UnknownCost::ModelNotReported,
            ),
        ] {
            insert_running(&conn, run, "p1", "Full", "Codex", Utc::now()).unwrap();
            if let Some(model) = model {
                set_run_model(&conn, run, model).unwrap();
            }
            finished_step(&conn, run, codex_traffic());
            let mut steps = list_audit_steps(&conn, run).unwrap();
            price_steps(&conn, &mut steps).unwrap();
            assert_eq!(steps[0].estimated_cost_usd_micros, None, "{run}");
            assert_eq!(
                steps[0].cost_unknown_reason.as_deref(),
                Some(reason.reason()),
                "{run}"
            );
        }
    }

    #[test]
    fn get_by_id_returns_the_row_or_none() {
        // Backs the resume path: the handler loads a run by id then derives
        // kind + checkpoint from it. Must return the exact row, and None for
        // an unknown id (never someone else's row).
        let conn = fresh_conn();
        insert_running(&conn, "run-x", "p1", "Security", "ClaudeCode", Utc::now()).unwrap();
        update_last_completed_step(&conn, "run-x", 1).unwrap();
        mark_interrupted(&conn, "run-x", "test").unwrap();

        let row = get_by_id(&conn, "run-x").unwrap().expect("row exists");
        assert_eq!(row.id, "run-x");
        assert_eq!(row.project_id, "p1");
        assert_eq!(row.kind, "Security");
        assert_eq!(row.status, "Interrupted");
        assert_eq!(row.last_completed_step, 1);

        assert!(get_by_id(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn has_running_for_project_is_project_scoped() {
        let conn = fresh_conn();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p2', 'Test 2', '/tmp/test2')",
            [],
        )
        .unwrap();
        let start = Utc::now();
        insert_running(&conn, "run-1", "p1", "Full", "ClaudeCode", start).unwrap();
        insert_running(&conn, "run-2", "p2", "Full", "ClaudeCode", start).unwrap();
        complete(
            &conn,
            "run-1",
            start + chrono::Duration::seconds(1),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            100,
            None,
            None,
        )
        .unwrap();

        assert!(!has_running_for_project(&conn, "p1").unwrap());
        assert!(has_running_for_project(&conn, "p2").unwrap());
    }

    #[test]
    fn validation_discussion_link_is_completed_and_project_scoped() {
        let conn = fresh_conn();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p2', 'Test 2', '/tmp/test2')",
            [],
        )
        .unwrap();
        let start = Utc::now();
        insert_running(&conn, "run-v", "p1", "Full", "ClaudeCode", start).unwrap();
        set_validation_discussion(&conn, "run-v", "disc-v").unwrap();

        assert!(!validation_discussion_belongs_to_project(&conn, "disc-v", "p1").unwrap());
        complete(
            &conn,
            "run-v",
            start + chrono::Duration::seconds(1),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            100,
            None,
            None,
        )
        .unwrap();

        assert!(validation_discussion_belongs_to_project(&conn, "disc-v", "p1").unwrap());
        assert!(!validation_discussion_belongs_to_project(&conn, "disc-v", "p2").unwrap());
        assert!(!validation_discussion_belongs_to_project(&conn, "other", "p1").unwrap());
    }

    #[test]
    fn latest_completed_skips_running_rows() {
        let conn = fresh_conn();
        // First run completes.
        let t1 = Utc::now();
        insert_running(&conn, "r1", "p1", "Full", "Codex", t1).unwrap();
        complete(
            &conn,
            "r1",
            t1 + chrono::Duration::seconds(60),
            "Completed",
            0,
            1,
            2,
            3,
            0,
            6,
            0,
            84,
            None,
            None,
        )
        .unwrap();
        // Second run is still Running.
        let t2 = Utc::now();
        insert_running(&conn, "r2", "p1", "Full", "Kiro", t2).unwrap();

        let latest = latest_completed(&conn, "p1").unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(
            latest.id, "r1",
            "must be the completed one, not the running one"
        );
        assert_eq!(latest.health_score, Some(84));
    }

    #[test]
    fn update_last_completed_step_bumps_only_forward_on_running_rows() {
        // 0.8.3 (#311) — the bump must be strictly monotonic
        // (out-of-order updates from a glitchy SSE shouldn't rewind
        // progress) AND scoped to Running rows (a terminal status
        // shouldn't accept new step updates).
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "rr-1", "p1", "Full", "ClaudeCode", start).unwrap();
        // Bumps forward.
        update_last_completed_step(&conn, "rr-1", 3).unwrap();
        let after1 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(after1[0].last_completed_step, 3);
        update_last_completed_step(&conn, "rr-1", 5).unwrap();
        let after2 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(after2[0].last_completed_step, 5);
        // Out-of-order: lower value must NOT win.
        update_last_completed_step(&conn, "rr-1", 2).unwrap();
        let after3 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(
            after3[0].last_completed_step, 5,
            "monotonic — lower step ignored"
        );
        // Terminal: mark complete then try to bump — must not move.
        complete(
            &conn,
            "rr-1",
            start + chrono::Duration::seconds(30),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            100,
            None,
            None,
        )
        .unwrap();
        update_last_completed_step(&conn, "rr-1", 9).unwrap();
        let after4 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(
            after4[0].last_completed_step, 5,
            "terminal rows must not accept new bumps"
        );
    }

    #[test]
    fn mark_interrupted_writes_status_and_preserves_last_completed_step() {
        // 0.8.3 (#311) — interrupted run keeps last_completed_step
        // so the resume mechanism knows where to pick up.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "ri-1", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "ri-1", 5).unwrap();
        mark_interrupted(&conn, "ri-1", "rate-limit hit").unwrap();

        let runs = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(runs[0].status, "Interrupted");
        assert_eq!(runs[0].last_completed_step, 5,
            "last_completed_step must survive the mark_interrupted call so resume knows where to restart");
        assert!(
            runs[0]
                .report_path
                .as_deref()
                .unwrap_or("")
                .contains("rate-limit"),
            "interruption reason must be captured for forensics"
        );
    }

    #[test]
    fn latest_resumable_only_returns_interrupted_partial_runs() {
        // Eligible: Interrupted AND last_completed_step in 1..=9.
        let conn = fresh_conn();
        let start = Utc::now();
        // Completed run — not resumable.
        insert_running(&conn, "r-done", "p1", "Full", "Codex", start).unwrap();
        complete(
            &conn,
            "r-done",
            start + chrono::Duration::seconds(10),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            90,
            None,
            None,
        )
        .unwrap();
        // Interrupted but no step done — restart, not resume.
        insert_running(&conn, "r-empty", "p1", "Full", "Codex", start).unwrap();
        mark_interrupted(&conn, "r-empty", "crashed at step 1").unwrap();
        // Interrupted with step 5 done — resumable!
        insert_running(&conn, "r-good", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "r-good", 5).unwrap();
        mark_interrupted(&conn, "r-good", "rate-limit").unwrap();

        let res = latest_resumable(&conn, "p1").unwrap();
        let row = res.expect("must find r-good as resumable");
        assert_eq!(row.id, "r-good");
        assert_eq!(row.last_completed_step, 5);
    }

    #[test]
    fn update_td_counts_stamps_interrupted_runs() {
        // The TD files exist on disk even when the run ends Interrupted —
        // the history row must not report 0 findings. Benchmark run 1
        // (2026-07-21): resolved/new were computed but never persisted on
        // this path, so a genuinely new TD read as "0 new" — the exact
        // reconciliation partition (13 carried + 1 new, 2 resolved) must
        // land in the row.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", start).unwrap();
        mark_interrupted(&conn, "r1", "warned steps: [1]").unwrap();
        update_td_counts(&conn, "r1", 0, 2, 8, 4, 2, 1, 13).unwrap();
        let row = &list_recent(&conn, "p1", 1).unwrap()[0];
        assert_eq!(row.status, "Interrupted");
        assert_eq!(row.td_total, 14);
        assert_eq!(row.td_high, 2);
        assert_eq!(row.td_resolved_since_last, 2);
        assert_eq!(
            row.td_new_since_last, 1,
            "a new TD on an interrupted run must not read as 0"
        );
        assert_eq!(row.td_carried_over, 13);
    }

    #[test]
    fn latest_resumable_covers_chained_steps_past_nine() {
        // The chained Full pipeline runs 16 steps: a hardcoded 1..=9 window
        // silently excluded late-interrupted runs from the resume offer.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r-chain", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "r-chain", 12).unwrap();
        mark_interrupted(&conn, "r-chain", "warned steps: [12]").unwrap();
        let row = latest_resumable(&conn, "p1")
            .unwrap()
            .expect("step-12 run must be resumable");
        assert_eq!(row.last_completed_step, 12);
    }

    #[test]
    fn latest_resumable_superseded_by_newer_completed_run() {
        // An Interrupted run older than a Completed one must NOT be offered:
        // resuming it would replay stale steps over the fresh docs.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r-old", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "r-old", 1).unwrap();
        mark_interrupted(&conn, "r-old", "backend restarted").unwrap();
        // A later run completes the whole pipeline.
        insert_running(
            &conn,
            "r-new",
            "p1",
            "Full",
            "ClaudeCode",
            start + chrono::Duration::minutes(30),
        )
        .unwrap();
        complete(
            &conn,
            "r-new",
            start + chrono::Duration::minutes(40),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            80,
            None,
            None,
        )
        .unwrap();

        assert!(
            latest_resumable(&conn, "p1").unwrap().is_none(),
            "a newer Completed run supersedes the old Interrupted one"
        );
    }

    #[test]
    fn latest_resumable_requires_being_the_projects_newest_run() {
        // Codex round 4 — the UI must never offer a resume the launch would
        // refuse: an Interrupted run behind ANY newer attempt (here a
        // step-0 Interrupted, then a Cancelled) is not resumable.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r-a", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "r-a", 5).unwrap();
        mark_interrupted(&conn, "r-a", "cut").unwrap();
        // Newer attempt that produced nothing (step 0, itself not resumable).
        insert_running(
            &conn,
            "r-b",
            "p1",
            "Full",
            "ClaudeCode",
            start + chrono::Duration::minutes(5),
        )
        .unwrap();
        mark_interrupted(&conn, "r-b", "cut early").unwrap();
        assert!(
            latest_resumable(&conn, "p1").unwrap().is_none(),
            "r-a is stale behind r-b; r-b has no completed step — nothing resumable"
        );
        // And a newer Cancelled attempt blocks the old one just the same.
        insert_running(
            &conn,
            "r-c",
            "p1",
            "Full",
            "ClaudeCode",
            start + chrono::Duration::minutes(10),
        )
        .unwrap();
        mark_cancelled(&conn, "r-c").unwrap();
        assert!(latest_resumable(&conn, "p1").unwrap().is_none());
    }

    #[test]
    fn old_completed_stays_latest_completed_after_a_no_change_failed_run() {
        // Matrix v2 #7 — a no_change refresh ends Failed: the health
        // snapshot (latest_completed) must keep pointing at the real last
        // Completed audit, not lose it.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r-full", "p1", "Full", "ClaudeCode", start).unwrap();
        complete(
            &conn,
            "r-full",
            start + chrono::Duration::minutes(30),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            90,
            None,
            None,
        )
        .unwrap();
        insert_running(
            &conn,
            "r-nc",
            "p1",
            "Partial",
            "ClaudeCode",
            start + chrono::Duration::hours(1),
        )
        .unwrap();
        mark_failed_strict(&conn, "r-nc", "partial refresh wrote nothing").unwrap();
        let latest = latest_completed(&conn, "p1").unwrap().unwrap();
        assert_eq!(
            latest.id, "r-full",
            "the Failed no_change run must not displace it"
        );
    }

    #[test]
    fn strict_marks_refuse_non_running_rows() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r1", "p1", "Partial", "ClaudeCode", start).unwrap();
        mark_cancelled(&conn, "r1").unwrap();
        assert!(mark_failed_strict(&conn, "r1", "x").is_err());
        assert!(mark_interrupted_strict(&conn, "r1", "x").is_err());
        assert!(mark_failed_strict(&conn, "ghost", "x").is_err());
    }

    #[test]
    fn mark_failed_is_idempotent_on_terminal() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r1", "p1", "Full", "Codex", start).unwrap();
        // First completion sets status=Completed.
        complete(
            &conn,
            "r1",
            start + chrono::Duration::seconds(30),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            100,
            None,
            None,
        )
        .unwrap();
        // mark_failed must NOT downgrade an already-completed run.
        mark_failed(&conn, "r1", "spurious").unwrap();
        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(
            runs[0].status, "Completed",
            "terminal row must not be overwritten"
        );
    }

    // ─── 0.8.4 (#298) audit_run_steps ────────────────────────────────

    #[test]
    fn insert_step_start_then_finalize_round_trip() {
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-x", "p1", "Full", "ClaudeCode", t0).unwrap();

        let started = t0;
        insert_audit_step_start(&conn, "run-x", 1, "docs/glossary.md", started).unwrap();

        // Half-finalized state: a step that started but didn't end yet
        // should be visible in the recap with `ended_at = None`.
        let steps = list_audit_steps(&conn, "run-x").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_index, 1);
        assert_eq!(steps[0].file_label, "docs/glossary.md");
        assert!(steps[0].ended_at.is_none());
        assert!(steps[0].duration_ms.is_none());
        assert!(steps[0].step_tokens.is_none());
        assert!(
            steps[0].cli_success,
            "default cli_success=true while running"
        );

        // Finalize with success.
        let ended = started + chrono::Duration::seconds(42);
        finalize_audit_step(
            &conn,
            "run-x",
            1,
            ended,
            42_000,
            &StepTokens {
                input: Some(1_000),
                output: Some(234),
                cache_read: Some(50),
                cache_write: None,
                ..StepTokens::UNKNOWN
            },
            Some(1_234),
            true,
            None,
            false,
        )
        .unwrap();
        let steps = list_audit_steps(&conn, "run-x").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].duration_ms, Some(42_000));
        assert_eq!(steps[0].step_tokens, Some(1_234));
        assert_eq!(steps[0].cumulative_tokens, Some(1_234));
        assert_eq!(steps[0].input_tokens, Some(1_000));
        assert_eq!(steps[0].output_tokens, Some(234));
        assert_eq!(steps[0].cache_read_tokens, Some(50));
        assert_eq!(
            steps[0].cache_write_tokens, None,
            "a cache figure the runtime did not give stays absent, never 0"
        );
        assert!(steps[0].cli_success);
        assert!(steps[0].step_warning.is_none());
        assert!(!steps[0].step_repaired_from_template);
    }

    #[test]
    fn a_step_whose_agent_reported_no_usage_is_stored_unknown_never_zero() {
        // KT-927 — every ACP step used to land here as 0 tokens. A runtime that
        // reports nothing leaves the figures NULL, in the step and in the run.
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-u", "p1", "Full", "OpenCode", t0).unwrap();
        insert_audit_step_start(&conn, "run-u", 1, "docs/glossary.md", t0).unwrap();
        finalize_audit_step(
            &conn,
            "run-u",
            1,
            t0 + chrono::Duration::seconds(3),
            3_000,
            &StepTokens::UNKNOWN,
            None,
            true,
            None,
            false,
        )
        .unwrap();

        let step = &list_audit_steps(&conn, "run-u").unwrap()[0];
        assert_eq!(step.step_tokens, None);
        assert_eq!(step.cumulative_tokens, None);
        assert_eq!(step.input_tokens, None);
        assert_eq!(step.output_tokens, None);
        assert_eq!(step.cache_read_tokens, None);
        assert_eq!(step.cache_write_tokens, None);
        assert!(step.cli_success, "an unmeasured step is not a failed one");
    }

    #[test]
    fn a_reported_usage_becomes_parts_and_a_headline_total() {
        use crate::agents::runner::{PromptCacheUsage, ReportedUsage};
        let tokens = StepTokens::from_reported(Some(ReportedUsage {
            input_tokens: 6_126,
            output_tokens: 28,
            prompt_cache: PromptCacheUsage {
                cached_prompt_tokens: Some(1_800),
                cache_write_prompt_tokens: None,
            },
        }));
        // Its accounting not set yet: input as reported.
        assert_eq!(tokens.total(), Some(6_154));
        assert_eq!(tokens.cache_read, Some(1_800));
        assert_eq!(tokens.cache_write, None);
        assert_eq!(StepTokens::from_reported(None).total(), None);
        // Only the output reported: still a figure, not an unknown.
        assert_eq!(
            StepTokens {
                output: Some(9),
                ..StepTokens::UNKNOWN
            }
            .total(),
            Some(9)
        );
    }

    #[test]
    fn insert_step_start_is_idempotent_on_resume() {
        // 0.8.4 (#298) — on a resumed audit (#311), the SSE pipeline
        // re-fires step_start for the FIRST replayed step (we resume
        // at last_completed_step + 1 but `for step in 1..=N { ... }`
        // re-iterates from 1). The DB row already exists, so the
        // second insert must be a no-op — `INSERT OR IGNORE` over the
        // UNIQUE (audit_run_id, step_index) index handles this.
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-r", "p1", "Full", "ClaudeCode", t0).unwrap();

        insert_audit_step_start(&conn, "run-r", 3, "docs/repo-map.md", t0).unwrap();
        // Same step fired again — must NOT crash, must NOT overwrite started_at.
        let later = t0 + chrono::Duration::seconds(60);
        insert_audit_step_start(&conn, "run-r", 3, "docs/repo-map.md", later).unwrap();

        let steps = list_audit_steps(&conn, "run-r").unwrap();
        assert_eq!(steps.len(), 1, "second insert must be a no-op");
        // started_at must keep the first value (not the re-fire timestamp).
        let drift = (steps[0].started_at - t0).num_seconds().abs();
        assert!(
            drift < 2,
            "started_at must NOT be rewritten by the second insert (drift={drift}s)"
        );
    }

    #[test]
    fn finalize_step_with_warning_marks_failure_and_repaired() {
        // 0.8.4 (#298) — a step that triggered #292 validation must
        // surface in the recap with cli_success=false AND the warning
        // reason AND the repaired flag, so the UI can paint the row
        // red + show the warning text.
        // `repaired_from_template` is LEGACY: the auto-repair path was
        // removed (new runs always write false) but historical rows
        // carry true and must keep rendering — this fixture pins that.
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-w", "p1", "Full", "ClaudeCode", t0).unwrap();
        insert_audit_step_start(&conn, "run-w", 5, "docs/tech-debt/inventory.md", t0).unwrap();
        finalize_audit_step(
            &conn,
            "run-w",
            5,
            t0 + chrono::Duration::seconds(7),
            7_000,
            &StepTokens {
                input: Some(400),
                output: Some(100),
                ..StepTokens::UNKNOWN
            },
            Some(12_000),
            false,
            Some("target file is empty — repaired from template"),
            true,
        )
        .unwrap();

        let steps = list_audit_steps(&conn, "run-w").unwrap();
        assert_eq!(steps.len(), 1);
        assert!(!steps[0].cli_success);
        assert_eq!(
            steps[0].step_warning.as_deref(),
            Some("target file is empty — repaired from template")
        );
        assert!(steps[0].step_repaired_from_template);
    }

    #[test]
    fn list_audit_steps_is_ordered_by_step_index() {
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-o", "p1", "Full", "ClaudeCode", t0).unwrap();
        // Insert out of order — list must still return 1, 2, 3, 4.
        insert_audit_step_start(&conn, "run-o", 3, "docs/architecture.md", t0).unwrap();
        insert_audit_step_start(&conn, "run-o", 1, "docs/glossary.md", t0).unwrap();
        insert_audit_step_start(&conn, "run-o", 4, "docs/api.md", t0).unwrap();
        insert_audit_step_start(&conn, "run-o", 2, "docs/repo-map.md", t0).unwrap();

        let steps = list_audit_steps(&conn, "run-o").unwrap();
        let indexes: Vec<u32> = steps.iter().map(|s| s.step_index).collect();
        assert_eq!(
            indexes,
            vec![1, 2, 3, 4],
            "must be sorted ASC by step_index"
        );
    }

    #[test]
    fn list_audit_steps_returns_empty_for_unknown_run() {
        // Legacy case — pre-0.8.4 runs that have no audit_run_steps
        // rows. The frontend must render the panel as "no per-step
        // data" instead of crashing.
        let conn = fresh_conn();
        let steps = list_audit_steps(&conn, "nonexistent").unwrap();
        assert!(steps.is_empty());
    }

    // ─── 0.8.4 (#317 / B1) reconcile_stale_runs ─────────────────────

    #[test]
    fn reconcile_stale_runs_marks_old_running_rows_as_interrupted() {
        // Two stale runs (3h + 1h old) + one fresh (5 min old) + one
        // already terminal. Cutoff is 30 min → first two should flip,
        // third stays Running, fourth is unchanged.
        let conn = fresh_conn();
        let now = Utc::now();
        insert_running(
            &conn,
            "stale-3h",
            "p1",
            "Full",
            "ClaudeCode",
            now - chrono::Duration::hours(3),
        )
        .unwrap();
        insert_running(
            &conn,
            "stale-1h",
            "p1",
            "Full",
            "ClaudeCode",
            now - chrono::Duration::hours(1),
        )
        .unwrap();
        insert_running(
            &conn,
            "fresh",
            "p1",
            "Full",
            "ClaudeCode",
            now - chrono::Duration::minutes(5),
        )
        .unwrap();
        insert_running(
            &conn,
            "terminal",
            "p1",
            "Full",
            "ClaudeCode",
            now - chrono::Duration::hours(2),
        )
        .unwrap();
        complete(
            &conn,
            "terminal",
            now - chrono::Duration::hours(1),
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            100,
            None,
            None,
        )
        .unwrap();

        let affected = reconcile_stale_runs(&conn, 30 * 60).unwrap();
        assert_eq!(
            affected, 2,
            "exactly 2 stale Running rows should have been flipped"
        );

        let runs = list_recent(&conn, "p1", 10).unwrap();
        let by_id: std::collections::HashMap<_, _> = runs
            .iter()
            .map(|r| (r.id.clone(), r.status.clone()))
            .collect();
        assert_eq!(by_id["stale-3h"], "Interrupted");
        assert_eq!(by_id["stale-1h"], "Interrupted");
        assert_eq!(by_id["fresh"], "Running", "fresh run must NOT be touched");
        assert_eq!(
            by_id["terminal"], "Completed",
            "terminal rows must NEVER be touched"
        );

        // Idempotent: a second call does nothing.
        let again = reconcile_stale_runs(&conn, 30 * 60).unwrap();
        assert_eq!(again, 0, "second reconcile run must be a no-op");
    }

    #[test]
    fn reconcile_stale_runs_preserves_last_completed_step() {
        // A run can be Running with partial progress (last_completed_step=5).
        // After reconcile it becomes Interrupted AND keeps the step
        // pointer so the resume mechanism (#311) can pick up from
        // where it left off. Without this guard, a backend restart
        // mid-audit would lose all progress and force a from-scratch
        // re-run on resume — burning 30k+ tokens.
        let conn = fresh_conn();
        let old = Utc::now() - chrono::Duration::hours(2);
        insert_running(&conn, "partial", "p1", "Full", "ClaudeCode", old).unwrap();
        update_last_completed_step(&conn, "partial", 5).unwrap();

        reconcile_stale_runs(&conn, 30 * 60).unwrap();

        // The run is now Interrupted AND resumable.
        let resumable = latest_resumable(&conn, "p1")
            .unwrap()
            .expect("must be resumable post-reconcile");
        assert_eq!(resumable.id, "partial");
        assert_eq!(
            resumable.last_completed_step, 5,
            "last_completed_step must survive reconcile so resume picks up at step 6"
        );
    }

    #[test]
    fn reconcile_all_running_flips_everything_regardless_of_age() {
        // For the admin "force cleanup" button: even a 5-second-old
        // Running row gets flipped. Used when the operator KNOWS
        // nothing is running (just rebuilt docker, etc).
        let conn = fresh_conn();
        let now = Utc::now();
        insert_running(
            &conn,
            "fresh-1",
            "p1",
            "Full",
            "ClaudeCode",
            now - chrono::Duration::seconds(5),
        )
        .unwrap();
        insert_running(
            &conn,
            "fresh-2",
            "p1",
            "Full",
            "ClaudeCode",
            now - chrono::Duration::seconds(30),
        )
        .unwrap();

        let affected = reconcile_all_running(&conn).unwrap();
        assert_eq!(affected, 2);

        let runs = list_recent(&conn, "p1", 10).unwrap();
        for r in &runs {
            assert_eq!(
                r.status, "Interrupted",
                "every Running row must be flipped (got {} for {})",
                r.status, r.id
            );
        }
    }

    #[test]
    fn list_recent_returns_newest_first() {
        let conn = fresh_conn();
        let t0 = Utc::now() - chrono::Duration::hours(2);
        let t1 = Utc::now() - chrono::Duration::hours(1);
        let t2 = Utc::now();
        insert_running(&conn, "old", "p1", "Full", "Codex", t0).unwrap();
        insert_running(&conn, "mid", "p1", "Security", "Codex", t1).unwrap();
        insert_running(&conn, "new", "p1", "Docker", "ClaudeCode", t2).unwrap();
        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(runs[0].id, "new");
        assert_eq!(runs[1].id, "mid");
        assert_eq!(runs[2].id, "old");
    }

    #[test]
    fn documentary_failure_invalidates_prior_success_without_losing_usage() {
        let conn = fresh_conn();
        let now = Utc::now();
        insert_running(&conn, "repair", "p1", "Full", "LiteLlm", now).unwrap();
        let attempt = StepTokens {
            input: Some(100),
            output: Some(10),
            cache_read: Some(50),
            cache_write: None,
            ..StepTokens::UNKNOWN
        };
        let combined = attempt.plus(attempt);
        assert_eq!(combined.total(), Some(220), "input as reported");
        assert_eq!(combined.cache_write, None);
        for step in 1..=3 {
            insert_audit_step_start(&conn, "repair", step, "docs/a.md", now).unwrap();
            finalize_audit_step(
                &conn,
                "repair",
                step,
                now,
                500,
                &combined,
                Some(660),
                true,
                None,
                false,
            )
            .unwrap();
        }
        invalidate_documentary_steps(
            &conn,
            "repair",
            &[
                (1, "docs/a.md: invalid line".into()),
                (3, "docs/c.md: invented path".into()),
            ],
        )
        .unwrap();
        let steps = list_audit_steps(&conn, "repair").unwrap();
        assert_eq!(
            crate::api::audit::full::already_succeeded_step_indices(&steps),
            [2].into_iter().collect()
        );
        assert_eq!(steps[0].step_tokens, Some(220));
        assert_eq!(steps[0].duration_ms, Some(500));
        assert!(steps[2]
            .step_warning
            .as_ref()
            .unwrap()
            .contains("invented path"));
        assert_eq!(
            get_by_id(&conn, "repair")
                .unwrap()
                .unwrap()
                .last_completed_step,
            1
        );
    }
    // ---- KT-1211: cancel and failed steps keep the docs whole ----

    #[test]
    fn a_cancel_after_a_completed_step_settles_interrupted_and_stays_resumable() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        update_last_completed_step(&conn, "r1", 11).unwrap();

        let status = mark_cancelled_at_step(&conn, "r1", 12).unwrap();
        assert_eq!(status.as_deref(), Some("Interrupted"));

        let row = get_by_id(&conn, "r1").unwrap().unwrap();
        assert_eq!(row.status, "Interrupted");
        assert_eq!(
            row.cancelled_at_step,
            Some(12),
            "the cancel is flagged on the run"
        );
        assert_eq!(
            row.last_completed_step, 11,
            "the checkpoint survives the cancel"
        );
        let resumable = latest_resumable(&conn, "p1")
            .unwrap()
            .expect("resumable after a cancel");
        assert_eq!(resumable.id, "r1");
    }

    #[test]
    fn a_cancel_before_any_step_is_cancelled_and_not_resumable() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();

        let status = mark_cancelled_at_step(&conn, "r1", 1).unwrap();
        assert_eq!(status.as_deref(), Some("Cancelled"));

        let row = get_by_id(&conn, "r1").unwrap().unwrap();
        assert_eq!(row.status, "Cancelled");
        assert_eq!(row.cancelled_at_step, Some(1));
        assert!(latest_resumable(&conn, "p1").unwrap().is_none());
    }

    #[test]
    fn cancelling_a_run_that_is_not_running_changes_nothing() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        update_last_completed_step(&conn, "r1", 3).unwrap();
        mark_failed(&conn, "r1", "boom").unwrap();

        assert_eq!(mark_cancelled_at_step(&conn, "r1", 4).unwrap(), None);
        let row = get_by_id(&conn, "r1").unwrap().unwrap();
        assert_eq!(row.status, "Failed");
        assert_eq!(row.cancelled_at_step, None);
    }

    #[test]
    fn a_step_snapshot_round_trips_absent_and_present_content() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        save_step_snapshot(&conn, "r1", 2, "docs/a.md", Some("before\n")).unwrap();
        save_step_snapshot(&conn, "r1", 3, "docs/b.md", None).unwrap();

        let a = step_snapshot(&conn, "r1", 2).unwrap().unwrap();
        assert_eq!(a.content.as_deref(), Some("before\n"));
        let b = step_snapshot(&conn, "r1", 3).unwrap().unwrap();
        assert_eq!(b.content, None, "an absent file is not an empty file");

        // The later snapshot of the same step replaces the earlier one.
        save_step_snapshot(&conn, "r1", 2, "docs/a.md", Some("again")).unwrap();
        assert_eq!(
            step_snapshot(&conn, "r1", 2)
                .unwrap()
                .unwrap()
                .content
                .as_deref(),
            Some("again")
        );
        delete_step_snapshot(&conn, "r1", 2).unwrap();
        assert!(step_snapshot(&conn, "r1", 2).unwrap().is_none());
    }

    #[test]
    fn a_target_past_the_snapshot_cap_is_refused_not_stored() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        let huge = "x".repeat(MAX_SNAPSHOT_BYTES + 1);
        let refused = save_step_snapshot(&conn, "r1", 2, "docs/a.md", Some(&huge)).unwrap_err();
        assert!(refused.to_string().contains("snapshot cap"), "{refused}");
        assert!(step_snapshot(&conn, "r1", 2).unwrap().is_none());
        // The cap itself is still kept whole.
        let at_cap = "y".repeat(MAX_SNAPSHOT_BYTES);
        save_step_snapshot(&conn, "r1", 2, "docs/a.md", Some(&at_cap)).unwrap();
        assert_eq!(
            step_snapshot(&conn, "r1", 2)
                .unwrap()
                .unwrap()
                .content
                .map(|c| c.len()),
            Some(MAX_SNAPSHOT_BYTES)
        );
    }

    #[test]
    fn the_observed_output_is_kept_with_the_snapshot_and_forgotten_when_it_is_retaken() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        // No snapshot (a target past the cap): recording is a harmless no-op.
        record_observed_output(&conn, "r1", 2, "abc").unwrap();
        assert!(step_snapshot(&conn, "r1", 2).unwrap().is_none());

        save_step_snapshot(&conn, "r1", 2, "docs/a.md", Some("before")).unwrap();
        assert_eq!(
            step_snapshot(&conn, "r1", 2)
                .unwrap()
                .unwrap()
                .observed_hash,
            None
        );
        record_observed_output(&conn, "r1", 2, "abc").unwrap();
        record_observed_output(&conn, "r1", 2, "def").unwrap();
        assert_eq!(
            step_snapshot(&conn, "r1", 2)
                .unwrap()
                .unwrap()
                .observed_hash
                .as_deref(),
            Some("def"),
            "the latest sighting wins"
        );
        // A snapshot taken again (a new attempt) does not inherit a sighting
        // of an earlier one.
        save_step_snapshot(&conn, "r1", 2, "docs/a.md", Some("before")).unwrap();
        assert_eq!(
            step_snapshot(&conn, "r1", 2)
                .unwrap()
                .unwrap()
                .observed_hash,
            None
        );
    }

    #[test]
    fn a_new_run_drops_the_snapshots_of_the_runs_it_supersedes() {
        let conn = fresh_conn();
        insert_running(&conn, "old", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        save_step_snapshot(&conn, "old", 2, "docs/a.md", Some("a")).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p2', 'Other', '/tmp/p2')",
            [],
        )
        .unwrap();
        insert_running(
            &conn,
            "other-project",
            "p2",
            "Full",
            "ClaudeCode",
            Utc::now(),
        )
        .unwrap();
        save_step_snapshot(&conn, "other-project", 2, "docs/a.md", Some("a")).unwrap();

        insert_running(&conn, "new", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        assert!(
            step_snapshot(&conn, "old", 2).unwrap().is_none(),
            "superseded"
        );
        assert!(
            step_snapshot(&conn, "other-project", 2).unwrap().is_some(),
            "another project's run is not superseded"
        );
    }

    #[test]
    fn deleting_a_run_deletes_its_snapshots() {
        // The real schema: the cascade is the migration's, not the fixture's.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p9', 'P', '/tmp/p9', ?1, ?1)",
            params![now],
        )
        .unwrap();
        insert_running(&conn, "r9", "p9", "Full", "ClaudeCode", Utc::now()).unwrap();
        save_step_snapshot(&conn, "r9", 1, "docs/a.md", Some("a")).unwrap();
        assert!(step_snapshot(&conn, "r9", 1).unwrap().is_some());
        conn.execute("DELETE FROM audit_runs WHERE id = 'r9'", [])
            .unwrap();
        assert!(step_snapshot(&conn, "r9", 1).unwrap().is_none());
    }

    #[test]
    fn a_killed_step_is_settled_failed_with_its_target_state_and_loses_its_snapshot() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        insert_audit_step_start(&conn, "r1", 1, "docs/a.md", Utc::now()).unwrap();
        finalize_audit_step(
            &conn,
            "r1",
            1,
            Utc::now(),
            5,
            &StepTokens::default(),
            None,
            true,
            None,
            false,
        )
        .unwrap();
        insert_audit_step_start(&conn, "r1", 2, "docs/b.md", Utc::now()).unwrap();
        save_step_snapshot(&conn, "r1", 1, "docs/a.md", Some("a")).unwrap();
        save_step_snapshot(&conn, "r1", 2, "docs/b.md", Some("b")).unwrap();

        // Only the step that never ended is "unfinished".
        let unfinished = unfinished_step_snapshots(&conn, "r1").unwrap();
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].step_index, 2);

        settle_killed_step(
            &conn,
            "r1",
            2,
            "stopped during step 2",
            Some("restored"),
            false,
            Utc::now(),
        )
        .unwrap();
        let steps = list_audit_steps(&conn, "r1").unwrap();
        let killed = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert!(killed.ended_at.is_some());
        assert!(!killed.cli_success);
        assert_eq!(
            killed.step_warning.as_deref(),
            Some("stopped during step 2")
        );
        assert_eq!(killed.target_state.as_deref(), Some("restored"));
        assert!(step_snapshot(&conn, "r1", 2).unwrap().is_none());
        assert!(
            step_snapshot(&conn, "r1", 1).unwrap().is_some(),
            "a finished step's snapshot is not this function's to drop"
        );
    }

    #[test]
    fn the_drift_and_the_agent_process_are_recorded_and_cleared() {
        let conn = fresh_conn();
        insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", Utc::now()).unwrap();
        set_resume_drift(&conn, "r1", "sources changed since step 4").unwrap();
        assert_eq!(
            get_by_id(&conn, "r1")
                .unwrap()
                .unwrap()
                .resume_drift
                .as_deref(),
            Some("sources changed since step 4")
        );

        set_agent_process(&conn, "r1", 4242, Some("Mon Oct 12 10:00:00 2026")).unwrap();
        let recorded = recorded_agent_processes(&conn).unwrap();
        assert_eq!(
            recorded,
            vec![(
                "r1".to_string(),
                4242,
                Some("Mon Oct 12 10:00:00 2026".to_string())
            )]
        );
        clear_agent_process(&conn, "r1").unwrap();
        assert!(recorded_agent_processes(&conn).unwrap().is_empty());
    }
}
