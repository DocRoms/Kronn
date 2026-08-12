//! Tests for the bounded resume bundle — KT-193 DoD 2.
//!
//! The failure to guard against is a bundle that grows with the project: that
//! would move the cost from the transcript to the handover and change nothing.
//! So most of these tests push oversized input at it and check the ceiling
//! holds AND that what was cut is named.

use super::*;

fn task(reference: &str) -> BundleTask {
    BundleTask {
        reference: reference.to_string(),
        title: "Do the thing".to_string(),
        status: "in_progress".to_string(),
        dod_progress: Some("2/5".to_string()),
    }
}

#[test]
fn a_small_plan_survives_whole() {
    let bundle = build(
        Some("Ship 0.9.4"),
        Some("Reduce token consumption"),
        &["Benchmark is green".to_string()],
        &[task("KT-1")],
        &["KT-2 blocked on review".to_string()],
    );
    assert!(bundle.objective.unwrap().contains("Ship 0.9.4"));
    assert_eq!(bundle.open_dod.len(), 1);
    assert_eq!(bundle.active_tasks.len(), 1);
    assert_eq!(bundle.blockers.len(), 1);
    assert!(bundle.omitted.is_empty());
}

#[test]
fn the_bundle_never_exceeds_its_ceiling() {
    // THE property. An unbounded handover recreates the problem it solves: this
    // session reached 4 143 787 451 tokens because nothing ever said "enough".
    let huge = "x".repeat(100_000);
    let many: Vec<String> = (0..500).map(|n| format!("item {n} {huge}")).collect();
    let tasks: Vec<BundleTask> = (0..500).map(|n| task(&format!("KT-{n}"))).collect();
    let bundle = build(Some(&huge), Some(&huge), &many, &tasks, &many);
    // Hard-coded on purpose. Asserting against BUNDLE_MAX_BYTES made this test
    // tautological: raising the constant raised the assertion with it, so the
    // test could not fail. A negative control caught that.
    assert!(
        bundle.bytes <= 20_000,
        "{} B from a ~50 MB input — the ceiling is not holding",
        bundle.bytes,
    );
}

#[test]
fn what_was_cut_is_named() {
    // A bundle that silently dropped blockers would be worse than one admitting
    // it: the reader would believe they had the whole picture.
    let long = "y".repeat(5_000);
    let many: Vec<String> = (0..100).map(|n| format!("blocker {n}: {long}")).collect();
    let bundle = build(Some("Obj"), None, &[], &[], &many);
    assert!(!bundle.omitted.is_empty());
    assert!(
        bundle.omitted.iter().any(|note| note.contains("blocker")),
        "{:?}",
        bundle.omitted
    );
}

#[test]
fn a_byte_driven_cut_is_also_named() {
    // The earlier test reached the notice through the ITEM cap, so removing the
    // byte-path notice left it green — a negative control found that.
    //
    // Reaching the byte path takes arithmetic, not just one huge item: every
    // line is clipped to TASK_LINE_MAX_BYTES first, so one section alone can
    // never overflow. Fully loaded, the worst case is
    //   objective 4 096 + blockers 20x240 + dod 20x240 + tasks 20x~270 ≈ 19 096
    // against a 16 384 ceiling — so the cut lands in the LAST section assembled,
    // which is `active_tasks`.
    let line = "q".repeat(TASK_LINE_MAX_BYTES * 2);
    let items: Vec<String> = (0..SECTION_MAX_ITEMS)
        .map(|n| format!("{n} {line}"))
        .collect();
    let tasks: Vec<BundleTask> = (0..SECTION_MAX_ITEMS)
        .map(|n| BundleTask {
            reference: format!("KT-{n}"),
            title: line.clone(),
            status: "in_progress".to_string(),
            dod_progress: Some("1/2".to_string()),
        })
        .collect();
    let bundle = build(
        Some(&"o".repeat(OBJECTIVE_MAX_BYTES * 2)),
        None,
        &items,
        &tasks,
        &items,
    );
    assert!(
        bundle.active_tasks.len() < SECTION_MAX_ITEMS,
        "the byte ceiling did not engage: {} tasks kept",
        bundle.active_tasks.len()
    );
    assert!(
        bundle
            .omitted
            .iter()
            .any(|note| note.contains("active task")),
        "a byte-driven cut went unnamed: {:?}",
        bundle.omitted
    );
    assert!(bundle.bytes <= BUNDLE_MAX_BYTES);
}

#[test]
fn blockers_are_kept_before_the_checklist() {
    // Re-attempting a blocked task costs far more than the bytes needed to say
    // it is blocked, so blockers win when the budget is tight.
    let filler = "z".repeat(200);
    let dod: Vec<String> = (0..SECTION_MAX_ITEMS)
        .map(|n| format!("dod {n} {filler}"))
        .collect();
    let blockers = vec!["KT-9 blocked: waiting on API key".to_string()];
    let bundle = build(Some("Obj"), None, &dod, &[], &blockers);
    assert_eq!(
        bundle.blockers.len(),
        1,
        "a blocker was dropped for a checklist"
    );
}

#[test]
fn a_truncated_objective_says_so() {
    let huge = "w".repeat(50_000);
    let bundle = build(Some(&huge), None, &[], &[], &[]);
    assert!(bundle.objective.unwrap().ends_with('…'));
    assert!(bundle.omitted.iter().any(|note| note.contains("objective")));
}

#[test]
fn truncation_lands_on_a_char_boundary() {
    // Cutting mid-codepoint would panic or emit invalid UTF-8 — and the plan is
    // written in French, so accents are the normal case, not the edge one.
    let accented = "éàüçñ".repeat(10_000);
    let bundle = build(Some(&accented), None, &[], &[], &[]);
    let objective = bundle.objective.unwrap();
    assert!(objective.len() <= OBJECTIVE_MAX_BYTES);
    // If the cut were wrong this string would not exist at all.
    assert!(objective.ends_with('…'));
}

#[test]
fn no_objective_yields_none_not_an_empty_string() {
    // An empty string would render as a heading with nothing under it; None
    // lets the caller say "no objective recorded".
    let bundle = build(None, Some("orphan description"), &[], &[], &[]);
    assert!(bundle.objective.is_none());
}

#[test]
fn an_objective_with_a_blank_description_keeps_just_the_title() {
    let bundle = build(Some("Ship it"), Some("   "), &[], &[], &[]);
    assert_eq!(bundle.objective.unwrap(), "Ship it");
}

#[test]
fn item_counts_are_capped_even_when_each_item_is_tiny() {
    // Bytes are not the only axis: 500 one-word blockers would fit the ceiling
    // and still be unreadable.
    let tiny: Vec<String> = (0..500).map(|n| n.to_string()).collect();
    let bundle = build(Some("Obj"), None, &tiny, &[], &tiny);
    assert_eq!(bundle.blockers.len(), SECTION_MAX_ITEMS);
    assert_eq!(bundle.open_dod.len(), SECTION_MAX_ITEMS);
    assert!(bundle.omitted.iter().any(|note| note.contains("blocker")));
    assert!(bundle.omitted.iter().any(|note| note.contains("DoD")));
}

#[test]
fn an_empty_plan_produces_an_empty_bundle_not_an_error() {
    let bundle = build(None, None, &[], &[], &[]);
    assert!(bundle.objective.is_none());
    assert_eq!(bundle.bytes, 0);
    assert!(bundle.omitted.is_empty());
}

#[test]
fn the_reported_byte_count_matches_what_is_in_the_bundle() {
    // A count that drifted from the content would make the ceiling unverifiable.
    let bundle = build(
        Some("Ship 0.9.4"),
        Some("body"),
        &["a".to_string(), "b".to_string()],
        &[task("KT-1")],
        &["c".to_string()],
    );
    let actual: usize = bundle.objective.as_ref().map(|o| o.len()).unwrap_or(0)
        + bundle.open_dod.iter().map(|s| s.len()).sum::<usize>()
        + bundle.blockers.iter().map(|s| s.len()).sum::<usize>()
        + bundle
            .active_tasks
            .iter()
            .map(|t| t.reference.len() + t.title.len() + t.status.len() + 8)
            .sum::<usize>();
    assert_eq!(bundle.bytes, actual);
}

#[test]
fn no_message_history_is_ever_included() {
    // Structural, not incidental: the bundle has no field for it. A fresh session
    // that needs an old message asks for that one message (DoD 4) instead of
    // re-reading everything.
    let bundle = build(Some("Obj"), None, &[], &[], &[]);
    let json = serde_json::to_string(&bundle).unwrap();
    for forbidden in ["messages", "transcript", "history"] {
        assert!(!json.contains(forbidden), "bundle carries {forbidden}");
    }
}
