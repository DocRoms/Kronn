//! Adversarial QA — DIMENSION "malformed_markers".
//!
//! Hammers the `[src: …]` extraction + verification path with malformed,
//! degenerate, nested, multiplexed, fenced and over-cap markers. The contract
//! (tightened by the 0.8.11 pre-tag quick win — literal-mention false positives):
//!
//!  - extraction is bracket-balanced (`[src: a[0]]` → `a[0]`),
//!  - fenced ```code``` markers AND `inline code` markers are SKIPPED,
//!  - empty / whitespace-only markers are DROPPED (grammar mentions, not
//!    citations) — a typed-but-empty ref (`[src: file:]`) still verifies
//!    as `EmptyRef`,
//!  - an unclosed `[src: …` (no `]`) is DROPPED (partially-typed prose), no panic,
//!  - the verified-source list is capped (`MAX_SOURCES_VERIFIED = 50`),
//!  - NO INPUT EVER PANICS,
//!  - extraction/analysis is DETERMINISTIC.
//!
//! Each test asserts the CORRECT behaviour per the documented semantics. A red
//! test here = a real defect, not a weakened expectation.

use kronn::core::anti_halluc::{
    analyze, extract_source_markers, verify_source_marker, SourceStatus,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DIM: &str = "malformed_markers";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique temp project with a known file: `src/foo.rs` (5 lines).
fn temp_project() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "ahbh_{}_{}_{}_{}",
        DIM,
        std::process::id(),
        n,
        nanos
    ));
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
    d
}

fn cleanup(d: &Path) {
    let _ = std::fs::remove_dir_all(d);
}

// ── 1. Empty marker `[src:]` → DROPPED (grammar mention, not a citation) ───

#[test]
fn empty_marker_is_dropped_as_a_grammar_mention() {
    // 0.8.11 quick win: a bare `[src:]` in prose is the agent talking ABOUT
    // the grammar — it used to surface as EmptyRef/fabricated (4× live).
    let txt = "The default is X [src:].";
    assert!(
        extract_source_markers(txt).is_empty(),
        "an empty [src:] is a mention, not a citation"
    );

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 0, "no source surfaced");
    assert_eq!(report.fabricated_count, 0, "no fabrication from a mention");
    // The EmptyRef verdict still exists for TYPED empty refs reaching
    // verification directly.
    assert_eq!(
        verify_source_marker("file:", Some(&root)).status,
        SourceStatus::EmptyRef,
    );
    cleanup(&root);
}

// ── 2. Whitespace-only marker `[src:   ]` → DROPPED like the empty one ─────

#[test]
fn whitespace_only_marker_is_dropped() {
    let txt = "Trust me [src:    \t  ].";
    assert!(
        extract_source_markers(txt).is_empty(),
        "whitespace-only inner content trims to empty → dropped"
    );

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 0);
    assert_eq!(report.fabricated_count, 0);
    cleanup(&root);
}

// ── 3. Unclosed marker `[src: file: x` (no `]`) → DROPPED, no panic ────────

#[test]
fn unclosed_marker_is_dropped_no_panic() {
    // 0.8.11 quick win (Copilot): partially-typed prose must not become a
    // citation — only a marker with its closing `]` is extracted.
    let txt = "It lives in [src: file: src/foo.rs:1 and then we never close it";
    assert!(
        extract_source_markers(txt).is_empty(),
        "unclosed marker must not reach verification"
    );

    // Whole-pipeline must not panic on the unclosed input either.
    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 0, "nothing to verify");
    cleanup(&root);
}

// ── 4. Truly unclosed empty `[src:` at the very end → no panic, dropped ────

#[test]
fn dangling_open_marker_at_eof_no_panic() {
    let txt = "trailing open bracket [src:";
    assert!(
        extract_source_markers(txt).is_empty(),
        "dangling [src: with no body extracts nothing"
    );
    // The EmptyRef contract itself is untouched for direct verification.
    let check = verify_source_marker("", None::<&Path>);
    assert_eq!(
        check.status,
        SourceStatus::EmptyRef,
        "an empty ref still verifies as EmptyRef even with no root"
    );
}

// ── 5. Nested brackets `[src: file: arr[0].rs:1]` → balanced extraction ────

#[test]
fn nested_brackets_are_balanced() {
    let txt = "see [src: file: arr[0].rs:1] here";
    let markers = extract_source_markers(txt);
    assert_eq!(
        markers,
        vec!["file: arr[0].rs:1"],
        "nested [..] must be balanced, capturing the inner brackets verbatim"
    );
}

// ── 6. Deeply nested brackets `[src: a[b[c]]]` → still one balanced marker ─

#[test]
fn deeply_nested_brackets_balanced() {
    let txt = "x [src: file: a[b[c]].rs] y";
    let markers = extract_source_markers(txt);
    assert_eq!(
        markers,
        vec!["file: a[b[c]].rs"],
        "multi-level nesting must balance to one marker"
    );
}

// ── 7. Multiple markers in one text → all extracted, in order ──────────────

#[test]
fn multiple_markers_extracted_in_order() {
    let txt = "A [src: file: src/foo.rs:1] mid [src: url: https://x.example] \
               then [src: training-data: GPT lore] end.";
    let markers = extract_source_markers(txt);
    assert_eq!(markers.len(), 3, "all three markers extracted");
    assert_eq!(markers[0], "file: src/foo.rs:1");
    assert_eq!(markers[1], "url: https://x.example");
    assert_eq!(markers[2], "training-data: GPT lore");

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 3, "three verified-marker rows");
    // foo.rs:1 → Verified, url → Unchecked, training-data → Rejected (fabricated).
    assert_eq!(report.sources[0].status, SourceStatus::Verified);
    assert_eq!(report.sources[1].status, SourceStatus::Unchecked);
    assert_eq!(report.sources[2].status, SourceStatus::Rejected);
    assert_eq!(
        report.fabricated_count, 1,
        "only the training-data marker is fabricated"
    );
    cleanup(&root);
}

// ── 8. Marker inside a ```fenced``` block → MUST be skipped ────────────────

#[test]
fn marker_inside_fenced_block_is_skipped() {
    let txt = "Real one [src: file: src/foo.rs:2].\n\
               ```rust\n\
               // example: [src: file: does/not/exist.rs:99]\n\
               ```\n\
               Done.";
    let markers = extract_source_markers(txt);
    assert_eq!(
        markers,
        vec!["file: src/foo.rs:2"],
        "the fenced marker must NOT be extracted"
    );

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 1, "only the prose marker is verified");
    assert_eq!(report.sources[0].status, SourceStatus::Verified);
    assert_eq!(
        report.fabricated_count, 0,
        "the fake fenced marker must not inflate fabricated_count"
    );
    cleanup(&root);
}

// ── 9. Marker inside `inline code` → SKIPPED (0.8.11 quick win) ────────────

#[test]
fn marker_inside_inline_code_is_skipped() {
    // A marker quoted in backticks is the agent showing the syntax, not
    // citing — it false-positived as "fabricated" 4× in one live session.
    let txt = "Use this `[src: file: src/foo.rs:1]` syntax.";
    assert!(
        extract_source_markers(txt).is_empty(),
        "an inline-code marker is a quoted example, never extracted"
    );

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 0);
    assert_eq!(report.fabricated_count, 0);

    // The same marker OUTSIDE backticks still extracts and verifies.
    let real = "Use this [src: file: src/foo.rs:1] citation.";
    let report = analyze(real, Some(&root));
    assert_eq!(report.sources.len(), 1);
    assert_eq!(report.sources[0].status, SourceStatus::Verified);
    cleanup(&root);
}

// ── 10. Weird internal whitespace / tabs / newline inside the marker ───────

#[test]
fn weird_internal_whitespace_in_marker() {
    // Tabs and runs of spaces around the type prefix and reference. The marker
    // body is trimmed at the edges but the inner shape is preserved verbatim;
    // it must still classify + verify the file correctly.
    let txt = "ref [src:   file:    src/foo.rs:1   ] ok";
    let markers = extract_source_markers(txt);
    assert_eq!(markers.len(), 1, "one marker despite the messy whitespace");
    assert!(
        markers[0].starts_with("file:"),
        "edges trimmed, body preserved: got {:?}",
        markers[0]
    );

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    assert_eq!(report.sources.len(), 1);
    assert_eq!(
        report.sources[0].status,
        SourceStatus::Verified,
        "internal whitespace must not break file resolution"
    );
    cleanup(&root);
}

// ── 11. 60+ markers → verified-source list capped at MAX_SOURCES_VERIFIED ──

#[test]
fn over_cap_marker_count_is_bounded() {
    let mut txt = String::from("Many citations: ");
    for _ in 0..64 {
        txt.push_str("[src: file: src/foo.rs:1] ");
    }
    let markers = extract_source_markers(&txt);
    assert_eq!(
        markers.len(),
        64,
        "extraction itself is NOT capped — it returns every marker"
    );

    let root = temp_project();
    let report = analyze(&txt, Some(&root));
    // analyze() takes(MAX_SOURCES_VERIFIED) = 50.
    assert!(
        report.sources.len() <= 50,
        "verified sources must be capped at 50, got {}",
        report.sources.len()
    );
    assert_eq!(
        report.sources.len(),
        50,
        "exactly the cap (50) markers are verified from 64 identical refs"
    );
    cleanup(&root);
}

// ── 12. Marker spanning a newline → extraction is line-agnostic for content ─

#[test]
fn marker_body_spanning_newline_no_panic() {
    // A `[src:` opened on one line and closed on the next is NOT inside a fence,
    // so it is read across the newline up to the closing `]`. Must not panic.
    let txt = "open [src: file: src/foo.rs:1\nstill going] close";
    let markers = extract_source_markers(txt);
    assert_eq!(markers.len(), 1, "one marker spanning the newline");

    let root = temp_project();
    let report = analyze(txt, Some(&root));
    // Determinism + no panic are the contract here; status is incidental.
    assert!(report.sources.len() <= 1);
    cleanup(&root);
}

// ── 13. Garbage / null-ish / emoji / CJK / RTL near markers → no panic ─────

#[test]
fn adversarial_unicode_and_control_chars_no_panic() {
    let inputs = [
        "",
        "[src:",
        "[src:]",
        "[src: ]",
        "[[[[[src: file: a]]]]]",
        "[src: file: 日本語/ファイル.rs:1]",
        "[src: url: https://例え.example/\u{200F}rtl] mixed \u{0000} null",
        "🔥🔥 [src: file: emoji/🚀.rs:1] 🔥🔥",
        "[src: file: a].[src: file: b].[src: file: c]",
        "][src:][[src:[src:]]",
    ];
    let root = temp_project();
    for inp in inputs {
        // Both the raw extractor and the full pipeline must survive.
        let _ = extract_source_markers(inp);
        let _ = analyze(inp, Some(&root));
        let _ = analyze(inp, None::<&Path>);
    }
    cleanup(&root);
}

// ── 14. DETERMINISM — same (text, root) → byte-identical report ────────────

#[test]
fn analysis_is_deterministic() {
    let txt = "A [src: file: src/foo.rs:1] B [src: url: https://x.example] \
               C [src: file: missing/x.rs:9] `inline.rs:2` D [src:] E.";
    let root = temp_project();
    let r1 = analyze(txt, Some(&root));
    let r2 = analyze(txt, Some(&root));
    assert_eq!(r1, r2, "same input must produce an identical report");

    // Extraction alone is deterministic too.
    let m1 = extract_source_markers(txt);
    let m2 = extract_source_markers(txt);
    assert_eq!(m1, m2);
    cleanup(&root);
}
