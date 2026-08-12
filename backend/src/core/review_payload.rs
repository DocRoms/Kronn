//! The bounded review payload and the final gate — KT-196.
//!
//! What an agent receives when a pull request is re-reviewed. The ledger already
//! records what was established; this decides what still has to be decided and
//! sends only that. The rest is stated as a COUNT, because not sending it is the
//! saving.
//!
//! And the gate: what must hold before a final verdict. Its shape follows from the
//! loop it exists to break — a verdict declared while something was still
//! unexamined — so "nobody checked" and "nothing was recorded" both block, and
//! neither can be mistaken for clean.

use crate::db::review_ledger::{
    blocking_findings, findings_for_pr, findings_needing_replay, FindingStatus, ReviewFinding,
};
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Cap on the rendered payload. The whole point is that a re-review costs a delta,
/// so this number is the budget the delta has to fit in.
pub const REVIEW_PAYLOAD_MAX_BYTES: usize = 24_576;

/// Compile-time ceiling, tightened when a gain is real and never raised to make a
/// build pass.
const _: () = assert!(
    REVIEW_PAYLOAD_MAX_BYTES <= 32_768,
    "a review payload above 32 KiB is a full re-review again, not a delta"
);

pub const MAX_PAYLOAD_FINDINGS: usize = 30;
pub const MAX_PAYLOAD_PATHS: usize = 60;
/// Enough to recognise a proof, not enough to reproduce a log.
pub const MAX_EVIDENCE_EXCERPT: usize = 200;

/// One finding as the agent sees it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PayloadFinding {
    pub id: String,
    pub path: Option<String>,
    pub line_start: Option<i64>,
    pub scenario: String,
    pub status: FindingStatus,
    /// Short excerpt, or `None` when nothing was ever established. The
    /// distinction is the whole reason `Unproven` exists.
    pub evidence_excerpt: Option<String>,
    pub symptom_count: i64,
}

/// What a CI check is known to be. `Unknown` is its own value: a check nobody
/// could read is not a passing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    Passing,
    Failing,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewPayload {
    pub repo: String,
    pub pr_number: i64,
    pub head_sha: String,
    /// The findings this head can have changed. Everything else keeps the
    /// evidence it already has.
    pub to_replay: Vec<PayloadFinding>,
    /// Settled and untouched, so deliberately absent. A count rather than a list:
    /// leaving them out IS the delta.
    pub settled_untouched: usize,
    pub changed_paths: Vec<String>,
    /// The real number, which may exceed what `changed_paths` shows.
    pub changed_paths_total: usize,
    /// One line per mechanical check already run, from Quick Exec.
    pub mechanical_evidence: Vec<String>,
    /// Everything left out, named. An omission a reader cannot see is
    /// indistinguishable from an absence.
    pub omissions: Vec<String>,
}

/// Assemble the payload for a head SHA.
pub fn build_payload(
    conn: &Connection,
    repo: &str,
    pr_number: i64,
    head_sha: &str,
    changed_paths: &[String],
    mechanical_evidence: Vec<String>,
) -> Result<ReviewPayload> {
    let all = findings_for_pr(conn, repo, pr_number)?;
    let replay = findings_needing_replay(conn, repo, pr_number, changed_paths)?;
    let replay_ids: Vec<&str> = replay.iter().map(|f| f.id.as_str()).collect();

    let mut omissions = Vec::new();

    // Settled AND not in the replay set. A finding still open is never counted
    // here, even if the diff missed its file: an open finding is work to do.
    let settled_untouched = all
        .iter()
        .filter(|f| f.status.is_settled() && !replay_ids.contains(&f.id.as_str()))
        .count();

    let mut to_replay: Vec<PayloadFinding> = replay.iter().map(to_payload_finding).collect();
    if to_replay.len() > MAX_PAYLOAD_FINDINGS {
        omissions.push(format!(
            "{} of {} findings needing replay are not listed — raise the cap or narrow the diff",
            to_replay.len() - MAX_PAYLOAD_FINDINGS,
            to_replay.len()
        ));
        // Unsettled first, so what is cut is what was already answered once.
        to_replay.sort_by_key(|f| f.status.is_settled());
        to_replay.truncate(MAX_PAYLOAD_FINDINGS);
    }

    let changed_paths_total = changed_paths.len();
    let shown: Vec<String> = changed_paths
        .iter()
        .take(MAX_PAYLOAD_PATHS)
        .cloned()
        .collect();
    if changed_paths_total > shown.len() {
        omissions.push(format!(
            "{} of {changed_paths_total} changed paths are not listed",
            changed_paths_total - shown.len()
        ));
    }
    if settled_untouched > 0 {
        omissions.push(format!(
            "{settled_untouched} settled finding(s) the diff did not touch are omitted on purpose"
        ));
    }

    Ok(ReviewPayload {
        repo: repo.to_string(),
        pr_number,
        head_sha: head_sha.to_string(),
        to_replay,
        settled_untouched,
        changed_paths: shown,
        changed_paths_total,
        mechanical_evidence,
        omissions,
    })
}

fn to_payload_finding(finding: &ReviewFinding) -> PayloadFinding {
    PayloadFinding {
        id: finding.id.clone(),
        path: finding.path.clone(),
        line_start: finding.line_start,
        scenario: truncate(&finding.scenario, 300),
        status: finding.status,
        // Kept as None when absent rather than becoming an empty string: "nothing
        // was established" and "an empty proof" must not look the same.
        evidence_excerpt: finding
            .evidence
            .as_deref()
            .map(|text| truncate(text, MAX_EVIDENCE_EXCERPT)),
        symptom_count: finding.symptom_count,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

/// Render the payload as the text an agent reads.
///
/// Order is deliberate: what must be decided, then what is missing, then context.
/// The cap cuts from the END, so a truncated payload loses background rather than
/// losing a finding or an omission notice.
pub fn render(payload: &ReviewPayload) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "REVIEW DELTA {}#{} at {}\n",
        payload.repo, payload.pr_number, payload.head_sha
    ));

    if payload.to_replay.is_empty() {
        out.push_str("\nNothing needs replaying at this head.\n");
    } else {
        out.push_str(&format!(
            "\nTO DECIDE ({} finding(s)):\n",
            payload.to_replay.len()
        ));
        for finding in &payload.to_replay {
            out.push_str(&format!(
                "- [{:?}] {}:{} {} (symptoms: {})\n",
                finding.status,
                finding.path.as_deref().unwrap_or("(no file)"),
                finding
                    .line_start
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                finding.scenario,
                finding.symptom_count
            ));
            match &finding.evidence_excerpt {
                Some(excerpt) => out.push_str(&format!("  established: {excerpt}\n")),
                // Said explicitly. A missing line would read as "no evidence
                // needed" rather than "nobody has checked".
                None => out.push_str("  established: nothing yet\n"),
            }
        }
    }

    if !payload.omissions.is_empty() {
        out.push_str("\nNOT IN THIS PAYLOAD:\n");
        for omission in &payload.omissions {
            out.push_str(&format!("- {omission}\n"));
        }
    }

    if !payload.mechanical_evidence.is_empty() {
        out.push_str("\nALREADY VERIFIED MECHANICALLY:\n");
        for line in &payload.mechanical_evidence {
            out.push_str(&format!("- {line}\n"));
        }
    }

    if !payload.changed_paths.is_empty() {
        out.push_str(&format!(
            "\nCHANGED PATHS ({} of {}):\n",
            payload.changed_paths.len(),
            payload.changed_paths_total
        ));
        for path in &payload.changed_paths {
            out.push_str(&format!("- {path}\n"));
        }
    }

    if out.len() > REVIEW_PAYLOAD_MAX_BYTES {
        let mut cut = REVIEW_PAYLOAD_MAX_BYTES;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n… payload truncated; the omissions above still apply\n");
    }
    out
}

/// Why a final verdict cannot be issued yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GateBlocker {
    /// No finding and no evidence. A gate that passes because nothing was
    /// recorded is the exact failure this ledger exists to prevent.
    NothingWasChecked,
    /// Findings known to be live.
    OpenFindings { count: usize },
    /// Findings nobody verified either way. Separate from open: "not looked at"
    /// and "looked at, still broken" call for different work.
    UnprovenFindings { count: usize },
    /// Settled, but against an older head whose diff touched them.
    StaleEvidence { finding_ids: Vec<String> },
    /// Marked settled with nothing to check.
    SettledWithoutEvidence { finding_ids: Vec<String> },
    /// Not passing — including not readable, which is not a pass.
    CiNotGreen { state: CiState },
}

/// Everything standing between the current state and a final verdict.
///
/// `changed_paths_since_evidence` is the union of paths touched since the evidence
/// on record was gathered. A finding settled at an older SHA is only stale if that
/// diff could have reached it — otherwise requiring a fresh run would undo the
/// delta the ledger exists to enable.
pub fn final_verdict_blockers(
    conn: &Connection,
    repo: &str,
    pr_number: i64,
    head_sha: &str,
    changed_paths_since_evidence: &[String],
    ci: CiState,
) -> Result<Vec<GateBlocker>> {
    let all = findings_for_pr(conn, repo, pr_number)?;
    let mut blockers = Vec::new();

    if all.is_empty() {
        blockers.push(GateBlocker::NothingWasChecked);
    }

    let unresolved = blocking_findings(conn, repo, pr_number)?;
    let open = unresolved
        .iter()
        .filter(|f| f.status == FindingStatus::Open)
        .count();
    // Anything not settled and not Open is unverified — including a status this
    // build does not recognise, which the ledger parses as Unproven.
    let unproven = unresolved.len() - open;
    if open > 0 {
        blockers.push(GateBlocker::OpenFindings { count: open });
    }
    if unproven > 0 {
        blockers.push(GateBlocker::UnprovenFindings { count: unproven });
    }

    let mut stale = Vec::new();
    let mut unproven_claims = Vec::new();
    for finding in all.iter().filter(|f| f.status.is_settled()) {
        if finding.evidence.is_none() {
            unproven_claims.push(finding.id.clone());
        }
        if finding.settled_at_sha != head_sha {
            let reachable = match &finding.path {
                // No path: we cannot show the diff missed it.
                None => true,
                Some(path) => changed_paths_since_evidence.iter().any(|p| p == path),
            };
            if reachable {
                stale.push(finding.id.clone());
            }
        }
    }
    if !stale.is_empty() {
        blockers.push(GateBlocker::StaleEvidence { finding_ids: stale });
    }
    if !unproven_claims.is_empty() {
        blockers.push(GateBlocker::SettledWithoutEvidence {
            finding_ids: unproven_claims,
        });
    }

    if ci != CiState::Passing {
        blockers.push(GateBlocker::CiNotGreen { state: ci });
    }

    Ok(blockers)
}

#[cfg(test)]
#[path = "review_payload_test.rs"]
mod review_payload_test;
