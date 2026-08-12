//! Review ledger — KT-196.
//!
//! Replaces the "final verdict, then a new symptom" loop: a reviewer declares a
//! PR clean, one more comment lands, and the whole review is redone from the
//! diff. The ledger makes a re-review a DELTA by recording, per cause, what was
//! established and at which SHA.
//!
//! The pivot is that a finding is keyed to a CAUSE, not to a comment. Five
//! comments about the same unwrapped error are one finding with five symptoms;
//! keying on the comment made them five separate items, which is precisely why
//! the same thing kept coming back.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a finding stands. `Open` and `Unproven` are distinct on purpose: a
/// finding nobody has evidence for is not the same as one known to be live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    /// Confirmed live at `settled_at_sha`.
    Open,
    /// Fixed, with evidence.
    Fixed,
    /// Judged not a defect, with a reason in `evidence`.
    Dismissed,
    /// Raised but never verified either way. Never treated as clean.
    Unproven,
}

impl FindingStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Fixed => "fixed",
            Self::Dismissed => "dismissed",
            Self::Unproven => "unproven",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "open" => Self::Open,
            "fixed" => Self::Fixed,
            "dismissed" => Self::Dismissed,
            // An unknown value from a newer writer must not be read as settled.
            _ => Self::Unproven,
        }
    }

    /// Whether this status closes the finding for gate purposes.
    ///
    /// `Unproven` does NOT close: treating "nobody checked" as "fine" is the
    /// failure that lets a verdict be declared over an unexamined finding.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Fixed | Self::Dismissed)
    }
}

/// One finding: a cause, where it lives, and what is known about it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewFinding {
    pub id: String,
    pub repo: String,
    pub pr_number: i64,
    /// The head the evidence was gathered against. A finding settled at one SHA
    /// is not automatically settled at the next.
    pub settled_at_sha: String,
    pub root_cause_fingerprint: String,
    pub path: Option<String>,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub scenario: String,
    pub status: FindingStatus,
    /// What makes the status checkable. `None` = unproven, which must stay
    /// distinguishable from proven-clean.
    pub evidence: Option<String>,
    pub proving_test: Option<String>,
    pub fixing_commit: Option<String>,
    /// How many comments described this one cause. A count above one is a signal
    /// about the review, not noise — it is why dedup was needed.
    pub symptom_count: i64,
}

/// Derive the identity of a cause from what a reviewer can state about it.
///
/// Normalised so trivial differences do not split one cause into two: case and
/// whitespace are folded, and the line RANGE is bucketed rather than exact —
/// two comments about the same block, three lines apart, are the same cause, and
/// an exact line would make them different findings.
///
/// Deliberately NOT derived from the comment text. Wording varies between
/// reviewers and between runs; a fingerprint that moved with the prose would
/// dedup nothing.
pub fn fingerprint(path: Option<&str>, line: Option<i64>, cause: &str) -> String {
    use std::fmt::Write as _;
    let normalised: String = cause
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // 10-line buckets: close enough to be the same block, far enough that two
    // genuinely different sites stay apart.
    let bucket = line.map(|value| value / 10);
    let mut seed = String::new();
    let _ = write!(
        seed,
        "{}\0{}\0{}",
        path.unwrap_or(""),
        bucket.map(|b| b.to_string()).unwrap_or_default(),
        normalised
    );
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    // 16 hex chars: enough that two causes will not collide in one PR, short
    // enough to read in a log or a comment.
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

/// Record a finding, or fold it into the one already on record for that cause.
///
/// The UNIQUE index on (repo, pr, fingerprint) is what makes this dedup rather
/// than accumulate. Returns the finding id, which is the SAME id when folding —
/// a caller can attach its symptom to it.
#[allow(clippy::too_many_arguments)]
pub fn record_finding(
    conn: &Connection,
    repo: &str,
    pr_number: i64,
    head_sha: &str,
    path: Option<&str>,
    line_start: Option<i64>,
    line_end: Option<i64>,
    scenario: &str,
    status: FindingStatus,
    evidence: Option<&str>,
) -> Result<String> {
    let cause = fingerprint(path, line_start, scenario);
    let now = Utc::now().to_rfc3339();

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM review_findings
              WHERE repo = ?1 AND pr_number = ?2 AND root_cause_fingerprint = ?3",
            params![repo, pr_number, cause],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        conn.execute(
            // Evidence is only overwritten by evidence: a later run that proves
            // nothing must not erase what an earlier one established.
            "UPDATE review_findings
                SET status = ?2,
                    settled_at_sha = ?3,
                    evidence = COALESCE(?4, evidence),
                    updated_at = ?5
              WHERE id = ?1",
            params![id, status.as_str(), head_sha, evidence, now],
        )?;
        return Ok(id);
    }

    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO review_findings (
             id, repo, pr_number, settled_at_sha, root_cause_fingerprint,
             path, line_start, line_end, scenario, status, evidence,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![
            id,
            repo,
            pr_number,
            head_sha,
            cause,
            path,
            line_start,
            line_end,
            scenario,
            status.as_str(),
            evidence,
            now,
        ],
    )?;
    Ok(id)
}

/// Attach the comment that reported a finding. Idempotent: replaying a webhook
/// cannot count one comment twice.
pub fn attach_symptom(
    conn: &Connection,
    finding_id: &str,
    source_comment_id: &str,
    observed_at_sha: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO review_finding_symptoms
             (finding_id, source_comment_id, observed_at_sha, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            finding_id,
            source_comment_id,
            observed_at_sha,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// Everything on record for a PR, newest first.
pub fn findings_for_pr(
    conn: &Connection,
    repo: &str,
    pr_number: i64,
) -> Result<Vec<ReviewFinding>> {
    let mut statement = conn.prepare(
        "SELECT f.id, f.repo, f.pr_number, f.settled_at_sha, f.root_cause_fingerprint,
                f.path, f.line_start, f.line_end, f.scenario, f.status, f.evidence,
                f.proving_test, f.fixing_commit,
                (SELECT COUNT(*) FROM review_finding_symptoms s
                  WHERE s.finding_id = f.id) AS symptoms
           FROM review_findings f
          WHERE f.repo = ?1 AND f.pr_number = ?2
          ORDER BY f.updated_at DESC",
    )?;
    let rows = statement
        .query_map(params![repo, pr_number], |row| {
            Ok(ReviewFinding {
                id: row.get(0)?,
                repo: row.get(1)?,
                pr_number: row.get(2)?,
                settled_at_sha: row.get(3)?,
                root_cause_fingerprint: row.get(4)?,
                path: row.get(5)?,
                line_start: row.get(6)?,
                line_end: row.get(7)?,
                scenario: row.get(8)?,
                status: FindingStatus::parse(&row.get::<_, String>(9)?),
                evidence: row.get(10)?,
                proving_test: row.get(11)?,
                fixing_commit: row.get(12)?,
                symptom_count: row.get(13)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Which findings a SHA change actually needs replaying.
///
/// This is what turns a re-review into a delta. A finding is replayed when the
/// new diff touched its file; everything else keeps the evidence it already has.
///
/// A finding with NO path is always replayed: it cannot be shown to be
/// unaffected, and assuming it is unaffected is how a stale verdict survives a
/// change that invalidated it.
pub fn findings_needing_replay(
    conn: &Connection,
    repo: &str,
    pr_number: i64,
    changed_paths: &[String],
) -> Result<Vec<ReviewFinding>> {
    let all = findings_for_pr(conn, repo, pr_number)?;
    Ok(all
        .into_iter()
        .filter(|finding| match &finding.path {
            None => true,
            Some(path) => changed_paths.iter().any(|changed| changed == path),
        })
        .collect())
}

/// Findings that must be closed before a final verdict.
///
/// `Unproven` counts as blocking: a verdict declared over a finding nobody
/// verified is the loop this ledger exists to break.
pub fn blocking_findings(
    conn: &Connection,
    repo: &str,
    pr_number: i64,
) -> Result<Vec<ReviewFinding>> {
    Ok(findings_for_pr(conn, repo, pr_number)?
        .into_iter()
        .filter(|finding| !finding.status.is_settled())
        .collect())
}

#[cfg(test)]
#[path = "review_ledger_test.rs"]
mod review_ledger_test;
