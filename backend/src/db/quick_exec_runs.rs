//! Recorded Quick Exec runs — KT-195.
//!
//! Two jobs. Idempotence: a mechanical result that has already been established
//! against a given tree is handed back instead of re-executed. And evidence: a
//! run can be attached to a task or to a review finding, so a claim in the plan
//! or in the ledger points at something a reader can check.
//!
//! The rule both jobs turn on is that only a CONCLUSIVE run counts. A timeout, a
//! cancellation, a rejection and a partial log all produced no findings, and
//! treating one of them as an answer is how "nobody checked" becomes "checked,
//! nothing found".

use crate::core::quick_exec::{QuickExecResult, QuickExecStatus};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What a run may be evidence for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTarget {
    Task,
    ReviewFinding,
}

impl EvidenceTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::ReviewFinding => "review_finding",
        }
    }
}

/// A stored run, as a later reader gets it back.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StoredRun {
    pub id: String,
    pub template_id: Option<String>,
    pub spec_fingerprint: String,
    pub head_sha: Option<String>,
    pub status: QuickExecStatus,
    pub exit_code: Option<i32>,
    pub summary: String,
    pub failed_tests: Vec<String>,
    pub artifact_path: Option<String>,
    pub findings_complete: bool,
    pub duration_ms: i64,
    pub created_at: String,
}

impl StoredRun {
    /// Whether this run answers the question it was run for.
    ///
    /// Requires all three: it passed or genuinely failed, its lists are
    /// exhaustive, and it is pinned to a tree. Any one of them missing makes the
    /// result a report about an attempt rather than about the code.
    pub fn is_conclusive(&self) -> bool {
        matches!(
            self.status,
            QuickExecStatus::Passed | QuickExecStatus::Failed
        ) && self.findings_complete
            && self.head_sha.is_some()
    }
}

/// Store a result. `head_sha` is what the run describes; passing `None` records
/// the run but makes it unreusable, which is the honest outcome for a command run
/// against an unpinned working tree.
pub fn record_run(
    conn: &Connection,
    template_id: Option<&str>,
    spec_fingerprint: &str,
    head_sha: Option<&str>,
    result: &QuickExecResult,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO quick_exec_runs (
             id, template_id, spec_fingerprint, head_sha, status, exit_code,
             summary, failed_tests, diagnostics, artifact_path, artifact_bytes,
             artifact_truncated, findings_complete, duration_ms, stdout_bytes,
             stderr_bytes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            id,
            template_id,
            spec_fingerprint,
            head_sha,
            status_as_str(result.status),
            result.exit_code,
            result.summary,
            serde_json::to_string(&result.failed_tests)?,
            serde_json::to_string(&result.diagnostics)?,
            result.artifact.as_ref().map(|a| a.path.clone()),
            result.artifact.as_ref().map(|a| a.bytes as i64),
            result.artifact.as_ref().is_some_and(|a| a.truncated) as i64,
            result.findings_complete as i64,
            result.duration_ms as i64,
            result.stdout_bytes as i64,
            result.stderr_bytes as i64,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(id)
}

/// The run that makes re-executing this work unnecessary, if there is one.
///
/// Matches on the work AND the tree, and returns only a conclusive run. An
/// inconclusive stored run is left in place — it is a record of an attempt, and
/// worth keeping — but it does not answer for the work.
pub fn reusable_run(
    conn: &Connection,
    spec_fingerprint: &str,
    head_sha: &str,
) -> Result<Option<StoredRun>> {
    let stored = conn
        .query_row(
            "SELECT id, template_id, spec_fingerprint, head_sha, status, exit_code,
                    summary, failed_tests, artifact_path, findings_complete,
                    duration_ms, created_at
               FROM quick_exec_runs
              WHERE spec_fingerprint = ?1 AND head_sha = ?2
              ORDER BY created_at DESC
              LIMIT 1",
            params![spec_fingerprint, head_sha],
            row_to_stored_run,
        )
        .optional()?;
    Ok(stored.filter(StoredRun::is_conclusive))
}

fn row_to_stored_run(row: &rusqlite::Row) -> rusqlite::Result<StoredRun> {
    Ok(StoredRun {
        id: row.get(0)?,
        template_id: row.get(1)?,
        spec_fingerprint: row.get(2)?,
        head_sha: row.get(3)?,
        status: parse_status(&row.get::<_, String>(4)?),
        exit_code: row.get(5)?,
        summary: row.get(6)?,
        failed_tests: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
        artifact_path: row.get(8)?,
        findings_complete: row.get::<_, i64>(9)? != 0,
        duration_ms: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn status_as_str(status: QuickExecStatus) -> &'static str {
    match status {
        QuickExecStatus::Passed => "passed",
        QuickExecStatus::Failed => "failed",
        QuickExecStatus::TimedOut => "timed_out",
        QuickExecStatus::Cancelled => "cancelled",
        QuickExecStatus::Rejected => "rejected",
    }
}

/// An unknown status from a newer writer is read as `Rejected`.
///
/// Fails safe: `Rejected` is the one variant that carries no claim about the code
/// at all, so a status this build does not understand cannot become a pass.
fn parse_status(value: &str) -> QuickExecStatus {
    match value {
        "passed" => QuickExecStatus::Passed,
        "failed" => QuickExecStatus::Failed,
        "timed_out" => QuickExecStatus::TimedOut,
        "cancelled" => QuickExecStatus::Cancelled,
        _ => QuickExecStatus::Rejected,
    }
}

/// Attach a run as evidence for a task or a finding.
///
/// Refuses an inconclusive run: a link is read as "this was verified, here is
/// what by", and a timed-out run would make that sentence false. Idempotent, so
/// replaying a publish cannot invent extra evidence.
pub fn attach_evidence(
    conn: &Connection,
    run_id: &str,
    target: EvidenceTarget,
    target_id: &str,
) -> Result<bool> {
    let run = get_run(conn, run_id)?;
    let Some(run) = run else {
        anyhow::bail!("no Quick Exec run {run_id}");
    };
    if !run.is_conclusive() {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO quick_exec_evidence
             (run_id, target_kind, target_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![run_id, target.as_str(), target_id, Utc::now().to_rfc3339()],
    )?;
    Ok(true)
}

pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<StoredRun>> {
    Ok(conn
        .query_row(
            "SELECT id, template_id, spec_fingerprint, head_sha, status, exit_code,
                    summary, failed_tests, artifact_path, findings_complete,
                    duration_ms, created_at
               FROM quick_exec_runs WHERE id = ?1",
            params![run_id],
            row_to_stored_run,
        )
        .optional()?)
}

/// The runs backing a task or a finding, newest first.
pub fn evidence_for(
    conn: &Connection,
    target: EvidenceTarget,
    target_id: &str,
) -> Result<Vec<StoredRun>> {
    let mut statement = conn.prepare(
        "SELECT r.id, r.template_id, r.spec_fingerprint, r.head_sha, r.status,
                r.exit_code, r.summary, r.failed_tests, r.artifact_path,
                r.findings_complete, r.duration_ms, r.created_at
           FROM quick_exec_evidence e
           JOIN quick_exec_runs r ON r.id = e.run_id
          WHERE e.target_kind = ?1 AND e.target_id = ?2
          ORDER BY r.created_at DESC",
    )?;
    let rows = statement
        .query_map(params![target.as_str(), target_id], row_to_stored_run)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Attach a run to a review finding and, if it passed, write its summary into the
/// finding's evidence column.
///
/// This is the join between Quick Exec and the ledger: a finding marked fixed now
/// points at a command whose output someone can re-read. Only a PASSING run is
/// allowed to write the evidence — a failing run proves the finding is still live,
/// which is a status change, not a proof of a fix.
pub fn settle_finding_with_run(conn: &Connection, run_id: &str, finding_id: &str) -> Result<bool> {
    if !attach_evidence(conn, run_id, EvidenceTarget::ReviewFinding, finding_id)? {
        return Ok(false);
    }
    let Some(run) = get_run(conn, run_id)? else {
        return Ok(false);
    };
    if run.status != QuickExecStatus::Passed {
        return Ok(false);
    }
    let proof = format!(
        "quick_exec {} — {}",
        run.template_id.as_deref().unwrap_or(&run.spec_fingerprint),
        run.summary.lines().next().unwrap_or("").trim()
    );
    conn.execute(
        // COALESCE keeps the ledger's rule that evidence is only overwritten BY
        // evidence, so a second run cannot blank what the first established.
        "UPDATE review_findings
            SET status = 'fixed',
                evidence = COALESCE(?2, evidence),
                proving_test = ?3,
                updated_at = ?4
          WHERE id = ?1",
        params![finding_id, proof, run.template_id, Utc::now().to_rfc3339()],
    )?;
    Ok(true)
}

/// Delete run rows whose artifact file is gone AND that are not evidence for
/// anything.
///
/// The artifact retention sweep removes files by age; this keeps the table from
/// pointing at them. A run that IS evidence is kept even without its artifact:
/// the summary is still the record of what was verified, and dropping the row
/// would silently un-verify a finding.
pub fn prune_orphan_runs(conn: &Connection) -> Result<usize> {
    let mut statement = conn.prepare(
        "SELECT r.id, r.artifact_path
           FROM quick_exec_runs r
          WHERE NOT EXISTS (
                SELECT 1 FROM quick_exec_evidence e WHERE e.run_id = r.id)",
    )?;
    let candidates = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut removed = 0;
    for (id, path) in candidates {
        let gone = match &path {
            None => true,
            Some(path) => !std::path::Path::new(path).exists(),
        };
        if gone {
            conn.execute("DELETE FROM quick_exec_runs WHERE id = ?1", params![id])?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
#[path = "quick_exec_runs_test.rs"]
mod quick_exec_runs_test;
