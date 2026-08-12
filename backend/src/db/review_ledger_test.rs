//! Tests for the review ledger — KT-196.
//!
//! The loop this replaces: a verdict is declared, one more comment lands, and the
//! whole review is redone. So the properties that matter are dedup (five comments
//! about one cause are one finding) and delta replay (a SHA change only reopens
//! what the diff touched). Both are tested from the outcome, not the mechanism.

use super::*;
use crate::db::migrations;

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    conn
}

const SHA: &str = "aaaa111";
const REPO: &str = "DocRoms/Kronn";

// ── dedup: a finding is a CAUSE, not a comment ──────────────────────

#[test]
fn two_comments_about_one_cause_become_one_finding() {
    // THE point of the ledger. Keyed on the comment id these were two items, so
    // the same thing kept coming back after every verdict.
    let conn = test_db();
    let first = record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        Some(12),
        "unwrapped error escapes",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    let second = record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(11),
        Some(12),
        "Unwrapped   error   escapes",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    assert_eq!(first, second, "one cause produced two findings");
    assert_eq!(findings_for_pr(&conn, REPO, 42).unwrap().len(), 1);
}

#[test]
fn the_symptom_count_survives_the_dedup() {
    // Folding must not erase that five people reported it — that count is a
    // signal about the review, and losing it would hide why dedup was needed.
    let conn = test_db();
    let id = record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        Some(12),
        "same cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    for comment in ["c1", "c2", "c3"] {
        attach_symptom(&conn, &id, comment, SHA).unwrap();
    }
    let findings = findings_for_pr(&conn, REPO, 42).unwrap();
    assert_eq!(findings[0].symptom_count, 3);
}

#[test]
fn replaying_the_same_comment_does_not_count_it_twice() {
    // Webhooks are redelivered. A count that drifted upward on redelivery would
    // make the signal useless.
    let conn = test_db();
    let id = record_finding(
        &conn,
        REPO,
        42,
        SHA,
        None,
        None,
        None,
        "cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    attach_symptom(&conn, &id, "c1", SHA).unwrap();
    attach_symptom(&conn, &id, "c1", SHA).unwrap();
    assert_eq!(
        findings_for_pr(&conn, REPO, 42).unwrap()[0].symptom_count,
        1
    );
}

#[test]
fn different_causes_in_one_file_stay_separate() {
    // Dedup must not become a merge: two real defects in one file are two
    // findings, and collapsing them would lose one.
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "unwrapped error",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "off by one in the loop",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    assert_eq!(findings_for_pr(&conn, REPO, 42).unwrap().len(), 2);
}

#[test]
fn distant_lines_in_one_file_are_different_causes() {
    // The bucket is 10 lines: same block, same cause. Two hundred lines apart is
    // a different site, and merging them would attribute evidence to the wrong
    // code.
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(210),
        None,
        "cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    assert_eq!(findings_for_pr(&conn, REPO, 42).unwrap().len(), 2);
}

#[test]
fn the_same_cause_in_another_pr_is_a_separate_finding() {
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    record_finding(
        &conn,
        REPO,
        99,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    assert_eq!(findings_for_pr(&conn, REPO, 42).unwrap().len(), 1);
    assert_eq!(findings_for_pr(&conn, REPO, 99).unwrap().len(), 1);
}

// ── evidence is never lost ──────────────────────────────────────────

#[test]
fn a_later_run_that_proves_nothing_does_not_erase_evidence() {
    // The whole value of the ledger is not redoing settled work. A run that
    // re-reports a finding without evidence must not wipe the proof an earlier
    // run established.
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        FindingStatus::Fixed,
        Some("cargo test tests::regression passes"),
    )
    .unwrap();
    record_finding(
        &conn,
        REPO,
        42,
        "bbbb222",
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    let finding = &findings_for_pr(&conn, REPO, 42).unwrap()[0];
    assert_eq!(
        finding.evidence.as_deref(),
        Some("cargo test tests::regression passes")
    );
    // The status DID move — a finding reopened at a new SHA is open again.
    assert_eq!(finding.status, FindingStatus::Open);
    assert_eq!(finding.settled_at_sha, "bbbb222");
}

// ── delta replay: a SHA change reopens only what moved ──────────────

#[test]
fn only_findings_in_changed_files_are_replayed() {
    // This is what makes a re-review a delta instead of a repeat.
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/touched.rs"),
        Some(10),
        None,
        "a",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/untouched.rs"),
        Some(10),
        None,
        "b",
        FindingStatus::Open,
        None,
    )
    .unwrap();

    let replay = findings_needing_replay(&conn, REPO, 42, &["src/touched.rs".to_string()]).unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].path.as_deref(), Some("src/touched.rs"));
}

#[test]
fn a_finding_with_no_path_is_always_replayed() {
    // It cannot be shown to be unaffected, and assuming it is unaffected is how
    // a stale verdict survives a change that invalidated it.
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        None,
        None,
        None,
        "architectural concern",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    let replay =
        findings_needing_replay(&conn, REPO, 42, &["src/elsewhere.rs".to_string()]).unwrap();
    assert_eq!(replay.len(), 1);
}

#[test]
fn an_empty_diff_replays_only_the_pathless_findings() {
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "a",
        FindingStatus::Open,
        None,
    )
    .unwrap();
    assert!(findings_needing_replay(&conn, REPO, 42, &[])
        .unwrap()
        .is_empty());
}

// ── the gate: unproven never counts as clean ────────────────────────

#[test]
fn an_unproven_finding_blocks_the_verdict() {
    // "Nobody checked" is not "fine". Treating it as settled is what let a final
    // verdict be declared over an unexamined finding.
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        FindingStatus::Unproven,
        None,
    )
    .unwrap();
    assert_eq!(blocking_findings(&conn, REPO, 42).unwrap().len(), 1);
}

#[test]
fn fixed_and_dismissed_do_not_block() {
    let conn = test_db();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "fixed one",
        FindingStatus::Fixed,
        Some("test passes"),
    )
    .unwrap();
    record_finding(
        &conn,
        REPO,
        42,
        SHA,
        Some("src/b.rs"),
        Some(10),
        None,
        "not a defect",
        FindingStatus::Dismissed,
        Some("by design, see ADR-001"),
    )
    .unwrap();
    assert!(blocking_findings(&conn, REPO, 42).unwrap().is_empty());
}

#[test]
fn an_unknown_status_from_a_newer_writer_blocks_rather_than_passes() {
    // Forward compatibility that fails safe: a status this build does not know
    // must not be read as settled.
    let conn = test_db();
    let id = record_finding(
        &conn,
        REPO,
        42,
        SHA,
        None,
        None,
        None,
        "cause",
        FindingStatus::Fixed,
        Some("proof"),
    )
    .unwrap();
    conn.execute(
        "UPDATE review_findings SET status = 'quantum_superposition' WHERE id = ?1",
        params![id],
    )
    .unwrap();
    assert_eq!(blocking_findings(&conn, REPO, 42).unwrap().len(), 1);
}

// ── fingerprint properties ──────────────────────────────────────────

#[test]
fn the_fingerprint_ignores_case_and_whitespace_but_not_meaning() {
    assert_eq!(
        fingerprint(Some("a.rs"), Some(1), "Unwrapped  ERROR"),
        fingerprint(Some("a.rs"), Some(1), "unwrapped error"),
    );
    assert_ne!(
        fingerprint(Some("a.rs"), Some(1), "unwrapped error"),
        fingerprint(Some("a.rs"), Some(1), "off by one"),
    );
}

#[test]
fn the_fingerprint_is_stable_across_calls() {
    // A fingerprint that varied per run would dedup nothing — it must not carry
    // a timestamp or a random component.
    assert_eq!(
        fingerprint(Some("a.rs"), Some(1), "cause"),
        fingerprint(Some("a.rs"), Some(1), "cause"),
    );
}

#[test]
fn a_pathless_finding_still_gets_a_fingerprint() {
    assert_eq!(fingerprint(None, None, "cause").len(), 16);
}
