//! Tests for recorded Quick Exec runs — KT-195.
//!
//! Reuse is a correctness question here, not a caching one: handing back a stored
//! result skips executing the command, so a result that did not actually answer
//! the question would turn "nobody checked" into "checked, nothing found". Most of
//! what follows is that one rule seen from different sides.

use super::*;
use crate::core::quick_exec::{ArtifactRef, Diagnostic};
use crate::db::{migrations, review_ledger};

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    conn
}

fn result(status: QuickExecStatus, findings_complete: bool) -> QuickExecResult {
    QuickExecResult {
        status,
        exit_code: match status {
            QuickExecStatus::Passed => Some(0),
            QuickExecStatus::Failed => Some(101),
            _ => None,
        },
        summary: "Passed\ntest result: ok. 12 passed".to_string(),
        failed_tests: Vec::new(),
        diagnostics: Vec::new(),
        artifact: Some(ArtifactRef {
            path: "/tmp/absent.log".to_string(),
            bytes: 42,
            truncated: false,
        }),
        duration_ms: 1_200,
        stdout_bytes: 900,
        stderr_bytes: 0,
        findings_complete,
    }
}

const FP: &str = "abcdef1234567890";
const SHA: &str = "aaaa111";

// ── idempotence, and its limits ─────────────────────────────────────

#[test]
fn a_conclusive_run_answers_for_the_same_work_on_the_same_tree() {
    let conn = test_db();
    let id = record_run(
        &conn,
        Some("backend-tests-full"),
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    let reused = reusable_run(&conn, FP, SHA).unwrap().expect("not reusable");
    assert_eq!(reused.id, id);
    assert_eq!(reused.template_id.as_deref(), Some("backend-tests-full"));
}

#[test]
fn a_failing_run_is_just_as_reusable_as_a_passing_one() {
    // A conclusive failure is an answer. Re-running it would burn the work to
    // learn what is already on record.
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Failed, true),
    )
    .unwrap();
    let reused = reusable_run(&conn, FP, SHA).unwrap().expect("not reusable");
    assert_eq!(reused.status, QuickExecStatus::Failed);
    assert_eq!(reused.exit_code, Some(101));
}

#[test]
fn a_timed_out_run_does_not_answer_for_the_work() {
    // THE rule. It found nothing because it was killed, not because there was
    // nothing to find.
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::TimedOut, true),
    )
    .unwrap();
    assert!(reusable_run(&conn, FP, SHA).unwrap().is_none());
}

#[test]
fn a_cancelled_or_rejected_run_does_not_answer_either() {
    for status in [QuickExecStatus::Cancelled, QuickExecStatus::Rejected] {
        let conn = test_db();
        record_run(&conn, None, FP, Some(SHA), &result(status, true)).unwrap();
        assert!(
            reusable_run(&conn, FP, SHA).unwrap().is_none(),
            "{status:?} was reused"
        );
    }
}

#[test]
fn a_run_with_a_partial_log_does_not_answer_for_the_work() {
    // It exited 0, so status alone would call it a pass — but its lists are not
    // exhaustive, so the absence of failures in them is not evidence.
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, false),
    )
    .unwrap();
    assert!(reusable_run(&conn, FP, SHA).unwrap().is_none());
}

#[test]
fn an_unpinned_run_is_never_reused() {
    // Without a head SHA we cannot say what tree the result was true of. Here the
    // exclusion comes from the SQL match itself — NULL never equals a SHA. The
    // conclusiveness rule is what covers the paths that do not query by SHA; see
    // `an_unpinned_run_cannot_back_anything`.
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        None,
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    assert!(reusable_run(&conn, FP, SHA).unwrap().is_none());
}

#[test]
fn a_result_from_another_tree_does_not_answer_for_this_one() {
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some("bbbb222"),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    assert!(reusable_run(&conn, FP, SHA).unwrap().is_none());
}

#[test]
fn different_work_on_the_same_tree_is_not_the_same_answer() {
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    assert!(reusable_run(&conn, "0000000000000000", SHA)
        .unwrap()
        .is_none());
}

#[test]
fn the_most_recent_run_wins() {
    // A conclusive run after an inconclusive one must not be shadowed by it.
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::TimedOut, true),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let latest = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    assert_eq!(reusable_run(&conn, FP, SHA).unwrap().unwrap().id, latest);
}

#[test]
fn an_inconclusive_run_is_still_recorded() {
    // It is a record of an attempt, and losing it would hide that the command was
    // tried and could not finish.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::TimedOut, true),
    )
    .unwrap();
    assert!(get_run(&conn, &id).unwrap().is_some());
}

#[test]
fn an_unknown_status_from_a_newer_writer_is_not_reusable() {
    // Forward compatibility that fails safe.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    conn.execute(
        "UPDATE quick_exec_runs SET status = 'quantum_superposition' WHERE id = ?1",
        params![id],
    )
    .unwrap();
    assert!(reusable_run(&conn, FP, SHA).unwrap().is_none());
    assert_eq!(
        get_run(&conn, &id).unwrap().unwrap().status,
        QuickExecStatus::Rejected
    );
}

// ── evidence ────────────────────────────────────────────────────────

#[test]
fn a_conclusive_run_can_back_a_task() {
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    assert!(attach_evidence(&conn, &id, EvidenceTarget::Task, "task-1").unwrap());
    let backing = evidence_for(&conn, EvidenceTarget::Task, "task-1").unwrap();
    assert_eq!(backing.len(), 1);
    assert_eq!(backing[0].id, id);
}

#[test]
fn an_inconclusive_run_cannot_back_anything() {
    // A link is read as "this was verified, here is what by". A timed-out run
    // would make that sentence false.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::TimedOut, true),
    )
    .unwrap();
    assert!(!attach_evidence(&conn, &id, EvidenceTarget::Task, "task-1").unwrap());
    assert!(evidence_for(&conn, EvidenceTarget::Task, "task-1")
        .unwrap()
        .is_empty());
}

#[test]
fn an_unpinned_run_cannot_back_anything() {
    // The evidence path does not query by SHA, so this is where the "pinned to a
    // tree" requirement actually has to hold. A proof that cannot say which tree
    // it was gathered against is not checkable.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        None,
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    assert!(!attach_evidence(&conn, &id, EvidenceTarget::Task, "task-1").unwrap());
    assert!(evidence_for(&conn, EvidenceTarget::Task, "task-1")
        .unwrap()
        .is_empty());
}

#[test]
fn attaching_the_same_run_twice_does_not_double_the_evidence() {
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    attach_evidence(&conn, &id, EvidenceTarget::Task, "task-1").unwrap();
    attach_evidence(&conn, &id, EvidenceTarget::Task, "task-1").unwrap();
    assert_eq!(
        evidence_for(&conn, EvidenceTarget::Task, "task-1")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_task_and_a_finding_with_the_same_id_do_not_share_evidence() {
    // The two targets live in different tables, so their ids can collide.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    attach_evidence(&conn, &id, EvidenceTarget::Task, "shared-id").unwrap();
    assert!(
        evidence_for(&conn, EvidenceTarget::ReviewFinding, "shared-id")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn attaching_an_unknown_run_is_an_error_not_a_silent_no_op() {
    let conn = test_db();
    assert!(attach_evidence(&conn, "absent", EvidenceTarget::Task, "task-1").is_err());
}

// ── the join with the review ledger ─────────────────────────────────

#[test]
fn a_passing_run_settles_a_finding_and_leaves_a_checkable_proof() {
    let conn = test_db();
    let finding = review_ledger::record_finding(
        &conn,
        "DocRoms/Kronn",
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "unwrapped error escapes",
        review_ledger::FindingStatus::Open,
        None,
    )
    .unwrap();
    let run = record_run(
        &conn,
        Some("backend-tests-filtered"),
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();

    assert!(settle_finding_with_run(&conn, &run, &finding).unwrap());
    let stored = &review_ledger::findings_for_pr(&conn, "DocRoms/Kronn", 42).unwrap()[0];
    assert_eq!(stored.status, review_ledger::FindingStatus::Fixed);
    let evidence = stored.evidence.as_deref().expect("no evidence recorded");
    assert!(
        evidence.contains("backend-tests-filtered"),
        "the proof does not name the command: {evidence}"
    );
    // And the ledger no longer counts it as blocking.
    assert!(review_ledger::blocking_findings(&conn, "DocRoms/Kronn", 42)
        .unwrap()
        .is_empty());
}

#[test]
fn a_failing_run_does_not_settle_a_finding() {
    // A failing run proves the finding is still live. Marking it fixed would
    // invert what the run showed.
    let conn = test_db();
    let finding = review_ledger::record_finding(
        &conn,
        "DocRoms/Kronn",
        42,
        SHA,
        Some("src/a.rs"),
        Some(10),
        None,
        "cause",
        review_ledger::FindingStatus::Open,
        None,
    )
    .unwrap();
    let run = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Failed, true),
    )
    .unwrap();
    assert!(!settle_finding_with_run(&conn, &run, &finding).unwrap());
    let stored = &review_ledger::findings_for_pr(&conn, "DocRoms/Kronn", 42).unwrap()[0];
    assert_eq!(stored.status, review_ledger::FindingStatus::Open);
    // The run is still attached: that the check was made and failed is worth
    // recording.
    assert_eq!(
        evidence_for(&conn, EvidenceTarget::ReviewFinding, &finding)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_timed_out_run_cannot_settle_a_finding() {
    let conn = test_db();
    let finding = review_ledger::record_finding(
        &conn,
        "DocRoms/Kronn",
        42,
        SHA,
        None,
        None,
        None,
        "cause",
        review_ledger::FindingStatus::Unproven,
        None,
    )
    .unwrap();
    let run = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::TimedOut, true),
    )
    .unwrap();
    assert!(!settle_finding_with_run(&conn, &run, &finding).unwrap());
    assert_eq!(
        review_ledger::blocking_findings(&conn, "DocRoms/Kronn", 42)
            .unwrap()
            .len(),
        1,
        "a timeout closed a finding nobody verified"
    );
}

#[test]
fn a_second_run_cannot_blank_the_proof_the_first_established() {
    // The ledger's rule, held across the Quick Exec join: evidence is only
    // overwritten by evidence.
    let conn = test_db();
    let finding = review_ledger::record_finding(
        &conn,
        "DocRoms/Kronn",
        42,
        SHA,
        None,
        None,
        None,
        "cause",
        review_ledger::FindingStatus::Open,
        None,
    )
    .unwrap();
    let first = record_run(
        &conn,
        Some("backend-tests-filtered"),
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    settle_finding_with_run(&conn, &first, &finding).unwrap();
    let second = record_run(
        &conn,
        Some("frontend-typecheck"),
        "0000000000000000",
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    settle_finding_with_run(&conn, &second, &finding).unwrap();
    let stored = &review_ledger::findings_for_pr(&conn, "DocRoms/Kronn", 42).unwrap()[0];
    assert!(stored.evidence.is_some());
    assert_eq!(
        evidence_for(&conn, EvidenceTarget::ReviewFinding, &finding)
            .unwrap()
            .len(),
        2,
        "both runs should remain attached"
    );
}

// ── retention ───────────────────────────────────────────────────────

#[test]
fn a_run_whose_artifact_is_gone_and_that_backs_nothing_is_pruned() {
    let conn = test_db();
    record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    // The fixture points at a path that does not exist.
    assert_eq!(prune_orphan_runs(&conn).unwrap(), 1);
}

#[test]
fn a_run_that_backs_a_finding_survives_the_loss_of_its_artifact() {
    // The summary is still the record of what was verified. Deleting the row
    // would silently un-verify the finding it settled.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::Passed, true),
    )
    .unwrap();
    attach_evidence(&conn, &id, EvidenceTarget::ReviewFinding, "finding-1").unwrap();
    assert_eq!(prune_orphan_runs(&conn).unwrap(), 0);
    assert!(get_run(&conn, &id).unwrap().is_some());
}

#[test]
fn a_run_whose_artifact_still_exists_is_kept() {
    let conn = test_db();
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut passing = result(QuickExecStatus::Passed, true);
    passing.artifact = Some(ArtifactRef {
        path: file.path().to_string_lossy().into_owned(),
        bytes: 10,
        truncated: false,
    });
    record_run(&conn, None, FP, Some(SHA), &passing).unwrap();
    assert_eq!(prune_orphan_runs(&conn).unwrap(), 0);
}

// ── what is stored ──────────────────────────────────────────────────

#[test]
fn the_findings_survive_the_round_trip() {
    let conn = test_db();
    let mut failing = result(QuickExecStatus::Failed, true);
    failing.failed_tests = vec!["db::a::breaks".to_string(), "db::b::breaks".to_string()];
    failing.diagnostics = vec![Diagnostic {
        path: Some("src/a.rs".to_string()),
        line: Some(12),
        message: "unused variable".to_string(),
    }];
    let id = record_run(&conn, None, FP, Some(SHA), &failing).unwrap();
    let stored = get_run(&conn, &id).unwrap().unwrap();
    assert_eq!(stored.failed_tests.len(), 2);
    assert_eq!(stored.failed_tests[0], "db::a::breaks");
}

#[test]
fn an_unknown_exit_code_stays_unknown() {
    // Not 0. A signal death whose code became 0 would read as a clean run.
    let conn = test_db();
    let id = record_run(
        &conn,
        None,
        FP,
        Some(SHA),
        &result(QuickExecStatus::TimedOut, true),
    )
    .unwrap();
    assert_eq!(get_run(&conn, &id).unwrap().unwrap().exit_code, None);
}
