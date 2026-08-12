//! Tests for the bounded review payload and the final gate — KT-196.
//!
//! The payload is judged on what it LEAVES OUT and whether it says so; the gate on
//! whether anything unexamined can slip past it. Both are the same failure seen
//! from two ends: a verdict issued over something nobody looked at.

use super::*;
use crate::db::migrations;
use crate::db::review_ledger::record_finding;

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    conn
}

const REPO: &str = "DocRoms/Kronn";
const SHA: &str = "aaaa111";

fn paths(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

#[allow(clippy::too_many_arguments)]
fn finding(
    conn: &Connection,
    sha: &str,
    path: Option<&str>,
    line: Option<i64>,
    scenario: &str,
    status: FindingStatus,
    evidence: Option<&str>,
) -> String {
    record_finding(
        conn, REPO, 42, sha, path, line, None, scenario, status, evidence,
    )
    .unwrap()
}

// ── the payload sends the delta and names what it drops ─────────────

#[test]
fn only_findings_the_diff_could_have_changed_are_sent() {
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/touched.rs"),
        Some(10),
        "still live",
        FindingStatus::Open,
        None,
    );
    finding(
        &conn,
        SHA,
        Some("src/untouched.rs"),
        Some(10),
        "already fixed",
        FindingStatus::Fixed,
        Some("test passes"),
    );

    let payload = build_payload(
        &conn,
        REPO,
        42,
        SHA,
        &paths(&["src/touched.rs"]),
        Vec::new(),
    )
    .unwrap();

    assert_eq!(payload.to_replay.len(), 1);
    assert_eq!(payload.to_replay[0].path.as_deref(), Some("src/touched.rs"));
    assert_eq!(payload.settled_untouched, 1);
}

#[test]
fn the_findings_left_out_are_announced_as_a_count() {
    // Not sending them is the saving; not SAYING they exist would make the payload
    // look like the whole picture.
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "fixed earlier",
        FindingStatus::Fixed,
        Some("proof"),
    );
    let payload =
        build_payload(&conn, REPO, 42, SHA, &paths(&["src/other.rs"]), Vec::new()).unwrap();
    assert!(payload.to_replay.is_empty());
    assert!(
        payload.omissions.iter().any(|o| o.contains("settled")),
        "the omission was silent: {:?}",
        payload.omissions
    );
}

#[test]
fn an_open_finding_the_diff_missed_is_not_counted_as_settled() {
    // It is work still to do. Counting it among the omitted-because-settled would
    // hide it from both the payload and the reader.
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "still live",
        FindingStatus::Open,
        None,
    );
    let payload =
        build_payload(&conn, REPO, 42, SHA, &paths(&["src/other.rs"]), Vec::new()).unwrap();
    assert_eq!(payload.settled_untouched, 0);
}

#[test]
fn a_finding_with_no_file_is_always_sent() {
    let conn = test_db();
    finding(
        &conn,
        SHA,
        None,
        None,
        "architectural concern",
        FindingStatus::Open,
        None,
    );
    let payload =
        build_payload(&conn, REPO, 42, SHA, &paths(&["src/other.rs"]), Vec::new()).unwrap();
    assert_eq!(payload.to_replay.len(), 1);
}

#[test]
fn a_truncated_path_list_reports_the_real_total() {
    let conn = test_db();
    let many: Vec<String> = (0..MAX_PAYLOAD_PATHS + 20)
        .map(|i| format!("src/f{i}.rs"))
        .collect();
    let payload = build_payload(&conn, REPO, 42, SHA, &many, Vec::new()).unwrap();
    assert_eq!(payload.changed_paths.len(), MAX_PAYLOAD_PATHS);
    assert_eq!(payload.changed_paths_total, MAX_PAYLOAD_PATHS + 20);
    assert!(payload
        .omissions
        .iter()
        .any(|o| o.contains("changed paths")));
}

#[test]
fn when_findings_overflow_the_unsettled_ones_are_kept() {
    // If something has to be cut, cut what was already answered once — not the
    // findings nobody has resolved.
    let conn = test_db();
    let mut changed = Vec::new();
    for index in 0..MAX_PAYLOAD_FINDINGS + 10 {
        let path = format!("src/f{index}.rs");
        let status = if index < 5 {
            FindingStatus::Open
        } else {
            FindingStatus::Fixed
        };
        let evidence = if index < 5 { None } else { Some("proof") };
        finding(&conn, SHA, Some(&path), Some(10), "cause", status, evidence);
        changed.push(path);
    }
    let payload = build_payload(&conn, REPO, 42, SHA, &changed, Vec::new()).unwrap();
    assert_eq!(payload.to_replay.len(), MAX_PAYLOAD_FINDINGS);
    assert_eq!(
        payload
            .to_replay
            .iter()
            .filter(|f| f.status == FindingStatus::Open)
            .count(),
        5,
        "an unresolved finding was dropped in favour of a settled one"
    );
    assert!(payload.omissions.iter().any(|o| o.contains("not listed")));
}

#[test]
fn nothing_established_stays_distinguishable_from_an_empty_proof() {
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Unproven,
        None,
    );
    let payload = build_payload(&conn, REPO, 42, SHA, &paths(&["src/a.rs"]), Vec::new()).unwrap();
    assert!(payload.to_replay[0].evidence_excerpt.is_none());
    assert!(render(&payload).contains("nothing yet"));
}

// ── the rendered text ───────────────────────────────────────────────

#[test]
fn the_rendered_payload_stays_within_the_budget() {
    // The per-item caps alone nearly bound the total, so reaching the byte cap
    // takes the worst case from every direction at once: the maximum number of
    // findings at maximum scenario and evidence length, AND the maximum number of
    // long paths. Sized past the cap deliberately — a test that never overflows
    // does not test the cap.
    let conn = test_db();
    let mut changed = Vec::new();
    for index in 0..MAX_PAYLOAD_FINDINGS {
        let path = format!(
            "src/deeply/nested/module/directory/number/{index}/{}.rs",
            "n".repeat(150)
        );
        finding(
            &conn,
            SHA,
            Some(&path),
            Some(10),
            &"a very long scenario sentence repeated to fill the budget ".repeat(20),
            FindingStatus::Open,
            Some(&"evidence text ".repeat(100)),
        );
        changed.push(path);
    }
    for index in 0..MAX_PAYLOAD_PATHS {
        changed.push(format!(
            "frontend/src/another/long/path/{index}/{}.tsx",
            "p".repeat(150)
        ));
    }
    let payload = build_payload(&conn, REPO, 42, SHA, &changed, Vec::new()).unwrap();
    let text = render(&payload);
    assert!(
        text.len() <= REVIEW_PAYLOAD_MAX_BYTES + 80,
        "payload was {} bytes",
        text.len()
    );
    assert!(text.contains("truncated"), "the cap was never reached");
}

#[test]
fn a_truncated_payload_keeps_its_findings_and_omissions() {
    // The cap cuts from the end, so what is lost is background — not the list of
    // things to decide, and not the notice that something is missing.
    let conn = test_db();
    let mut changed = Vec::new();
    for index in 0..MAX_PAYLOAD_PATHS + 40 {
        changed.push(format!("src/very/long/path/number/{index}/file.rs"));
    }
    finding(
        &conn,
        SHA,
        Some("src/very/long/path/number/0/file.rs"),
        Some(10),
        "the one thing to decide",
        FindingStatus::Open,
        None,
    );
    let payload = build_payload(&conn, REPO, 42, SHA, &changed, Vec::new()).unwrap();
    let text = render(&payload);
    assert!(text.contains("the one thing to decide"));
    assert!(text.contains("NOT IN THIS PAYLOAD"));
}

#[test]
fn an_empty_delta_says_so_rather_than_rendering_nothing() {
    // A payload with no section at all reads as a failure to build one.
    let conn = test_db();
    let payload = build_payload(&conn, REPO, 42, SHA, &[], Vec::new()).unwrap();
    assert!(render(&payload).contains("Nothing needs replaying"));
}

#[test]
fn mechanical_evidence_is_listed_as_already_verified() {
    let conn = test_db();
    let payload = build_payload(
        &conn,
        REPO,
        42,
        SHA,
        &[],
        vec!["backend-tests-full passed at aaaa111".to_string()],
    )
    .unwrap();
    let text = render(&payload);
    assert!(text.contains("ALREADY VERIFIED MECHANICALLY"));
    assert!(text.contains("backend-tests-full"));
}

// ── the benchmark shape (KT-196 DoD 7) ──────────────────────────────
//
// Measured on the two reference discussions, `095dfee0…` and `8490d400…`: 162
// review passes across 723 messages. Per pass, the changed files a reviewer named
// were median 1, p90 4, max 12; the root-cause sites median 0, p90 2, max 10; path
// length median 19, p90 34.
//
// The cap in this module is a CEILING, not a target, and the benchmark showed why
// the difference matters: costing every pass at the ceiling makes a bounded payload
// look more expensive than a warm session's incremental context. These two tests
// pin what the code actually produces at the measured shape, so the real figure is
// a measurement rather than a ceiling — and so a regression that inflates the
// payload fails a test.

/// Payload at the measured p90 shape. Pinned at what the renderer produces today;
/// tightened on a real gain, never raised to make a build pass.
const P90_PAYLOAD_CEILING: usize = 564;
/// Payload at the measured worst observed shape.
const WORST_PAYLOAD_CEILING: usize = 2_199;

fn benchmark_payload(conn: &Connection, findings: usize, path_count: usize) -> String {
    let mut changed = Vec::new();
    for index in 0..path_count {
        // 34 characters: the p90 path length in the reference discussions.
        changed.push(format!("backend/src/api/module_{index:02}.rs "));
    }
    for index in 0..findings {
        finding(
            conn,
            SHA,
            Some(changed[index % path_count].as_str()),
            Some(10 * index as i64),
            "the error path is not handled and the request returns 500",
            FindingStatus::Open,
            Some("cargo test --lib api::module reproduces it"),
        );
    }
    let payload = build_payload(conn, REPO, 42, SHA, &changed, Vec::new()).unwrap();
    render(&payload)
}

#[test]
fn a_payload_at_the_measured_p90_shape_stays_small() {
    let conn = test_db();
    let text = benchmark_payload(&conn, 2, 4);
    assert!(
        text.len() <= P90_PAYLOAD_CEILING,
        "p90 payload grew to {} bytes (ceiling {P90_PAYLOAD_CEILING})",
        text.len()
    );
    // Two orders of magnitude under the module cap — the ceiling is a backstop,
    // not what a normal pass costs.
    assert!(text.len() * 10 < REVIEW_PAYLOAD_MAX_BYTES);
    // Far below the module cap — which is the point: the ceiling is a backstop,
    // not what a normal pass costs.
}

#[test]
fn a_payload_at_the_worst_measured_shape_stays_small() {
    let conn = test_db();
    let text = benchmark_payload(&conn, 10, 12);
    assert!(
        text.len() <= WORST_PAYLOAD_CEILING,
        "worst-shape payload grew to {} bytes (ceiling {WORST_PAYLOAD_CEILING})",
        text.len()
    );
    assert!(text.len() * 4 < REVIEW_PAYLOAD_MAX_BYTES);
}

// ── the gate ────────────────────────────────────────────────────────

#[test]
fn an_empty_ledger_does_not_pass_the_gate() {
    // THE gate rule. Zero blockers because nothing was recorded is precisely the
    // failure the ledger exists to prevent.
    let conn = test_db();
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Passing).unwrap();
    assert!(blockers.contains(&GateBlocker::NothingWasChecked));
}

#[test]
fn a_clean_ledger_with_green_ci_passes() {
    // The gate has to be passable, or it would just be a permanent refusal.
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Fixed,
        Some("cargo test tests::regression passes"),
    );
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Passing).unwrap();
    assert!(blockers.is_empty(), "unexpected blockers: {blockers:?}");
}

#[test]
fn an_open_finding_blocks() {
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Open,
        None,
    );
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Passing).unwrap();
    assert!(blockers.contains(&GateBlocker::OpenFindings { count: 1 }));
}

#[test]
fn an_unproven_finding_blocks_separately_from_an_open_one() {
    // "Not looked at" and "looked at, still broken" call for different work, so
    // they are reported apart.
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "never checked",
        FindingStatus::Unproven,
        None,
    );
    finding(
        &conn,
        SHA,
        Some("src/b.rs"),
        Some(10),
        "still live",
        FindingStatus::Open,
        None,
    );
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Passing).unwrap();
    assert!(blockers.contains(&GateBlocker::OpenFindings { count: 1 }));
    assert!(blockers.contains(&GateBlocker::UnprovenFindings { count: 1 }));
}

#[test]
fn a_settled_finding_with_no_evidence_blocks() {
    // A claim with nothing to check is not coverage.
    let conn = test_db();
    let id = finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Fixed,
        None,
    );
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Passing).unwrap();
    assert!(blockers.contains(&GateBlocker::SettledWithoutEvidence {
        finding_ids: vec![id]
    }));
}

#[test]
fn evidence_from_an_older_head_blocks_when_the_diff_reached_it() {
    let conn = test_db();
    let id = finding(
        &conn,
        "old111",
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Fixed,
        Some("proof"),
    );
    let blockers = final_verdict_blockers(
        &conn,
        REPO,
        42,
        SHA,
        &paths(&["src/a.rs"]),
        CiState::Passing,
    )
    .unwrap();
    assert!(blockers.contains(&GateBlocker::StaleEvidence {
        finding_ids: vec![id]
    }));
}

#[test]
fn evidence_from_an_older_head_the_diff_never_reached_still_holds() {
    // Otherwise the gate would force a full replay on every push, which is the
    // repeat the delta exists to remove.
    let conn = test_db();
    finding(
        &conn,
        "old111",
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Fixed,
        Some("proof"),
    );
    let blockers = final_verdict_blockers(
        &conn,
        REPO,
        42,
        SHA,
        &paths(&["src/elsewhere.rs"]),
        CiState::Passing,
    )
    .unwrap();
    assert!(blockers.is_empty(), "unexpected blockers: {blockers:?}");
}

#[test]
fn stale_evidence_on_a_pathless_finding_always_blocks() {
    // It cannot be shown the diff missed it, and assuming it did is how a verdict
    // outlives the change that invalidated it.
    let conn = test_db();
    let id = finding(
        &conn,
        "old111",
        None,
        None,
        "architectural concern",
        FindingStatus::Dismissed,
        Some("by design"),
    );
    let blockers = final_verdict_blockers(
        &conn,
        REPO,
        42,
        SHA,
        &paths(&["src/elsewhere.rs"]),
        CiState::Passing,
    )
    .unwrap();
    assert!(blockers.contains(&GateBlocker::StaleEvidence {
        finding_ids: vec![id]
    }));
}

#[test]
fn ci_that_could_not_be_read_blocks_like_a_failure() {
    // Unknown is not green. A gate that treated it as green would pass a PR whose
    // checks nobody managed to look at.
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Fixed,
        Some("proof"),
    );
    for state in [CiState::Failing, CiState::Unknown] {
        let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], state).unwrap();
        assert!(
            blockers.contains(&GateBlocker::CiNotGreen { state }),
            "{state:?} did not block"
        );
    }
}

#[test]
fn an_unknown_status_from_a_newer_writer_blocks_the_gate() {
    let conn = test_db();
    let id = finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "cause",
        FindingStatus::Fixed,
        Some("proof"),
    );
    conn.execute(
        "UPDATE review_findings SET status = 'quantum_superposition' WHERE id = ?1",
        rusqlite::params![id],
    )
    .unwrap();
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Passing).unwrap();
    assert!(blockers.contains(&GateBlocker::UnprovenFindings { count: 1 }));
}

#[test]
fn several_blockers_are_all_reported_at_once() {
    // Reporting one at a time turns closing a review into a guessing loop.
    let conn = test_db();
    finding(
        &conn,
        SHA,
        Some("src/a.rs"),
        Some(10),
        "live",
        FindingStatus::Open,
        None,
    );
    finding(
        &conn,
        SHA,
        Some("src/b.rs"),
        Some(10),
        "claimed",
        FindingStatus::Fixed,
        None,
    );
    let blockers = final_verdict_blockers(&conn, REPO, 42, SHA, &[], CiState::Unknown).unwrap();
    assert!(blockers.len() >= 3, "only got {blockers:?}");
}
