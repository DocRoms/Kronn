//! Adversarial QA — DIMENSION "perf_caps".
//!
//! The anti-hallucination core bounds its work with hard caps so a pathological
//! agent message can't blow up the JSON it attaches or burn unbounded CPU:
//!   - `MAX_SOURCES_VERIFIED = 50`  — at most 50 `[src:]` markers (and 50 inline
//!     anchors) are verified per message.
//!   - `MAX_FLAGGED_SPANS = 25`     — the unsourced count keeps growing past 25,
//!     but only 25 excerpt spans are stored.
//!   - `LINE_COUNT_SIZE_CAP_BYTES = 2 MiB` — files above this skip the bounds
//!     check (existence is enough), so a huge file never gets fully read.
//!
//! Every test asserts the CORRECT documented behaviour. A red test here is a
//! real defect (a cap that doesn't hold, a panic on a huge input, or a
//! non-deterministic report).

use kronn::core::anti_halluc::{analyze, lint_assertions, SourceStatus};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DIM: &str = "perf_caps";

/// Mirror of the private `MAX_SOURCES_VERIFIED` constant (not exported).
const MAX_SOURCES_VERIFIED: usize = 50;
/// Mirror of the private `MAX_FLAGGED_SPANS` constant (not exported).
const MAX_FLAGGED_SPANS: usize = 25;
/// Mirror of the private `LINE_COUNT_SIZE_CAP_BYTES` constant (not exported).
const LINE_COUNT_SIZE_CAP_BYTES: u64 = 2 * 1024 * 1024;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique temp project with a known file `src/foo.rs` (5 lines).
fn temp_project() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!(
        "ahbh_{}_{}_{}_{}",
        DIM,
        std::process::id(),
        nanos,
        n
    ));
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
    d
}

fn cleanup(d: &Path) {
    let _ = std::fs::remove_dir_all(d);
}

// ─────────────────────────────────────────────────────────────────────────
// MAX_SOURCES_VERIFIED — formal [src:] markers
// ─────────────────────────────────────────────────────────────────────────

/// 60 DISTINCT existing-file `[src:]` markers → only 50 are verified/stored.
/// The `.take(MAX_SOURCES_VERIFIED)` happens on extraction, BEFORE dedup, and
/// every extracted marker is pushed into `sources` (dedup only guards the
/// inline-anchor pass), so `sources.len()` must be exactly 50.
#[test]
fn sixty_distinct_file_markers_cap_sources_at_fifty() {
    let root = temp_project();
    // 60 markers pointing at the SAME existing file but with DISTINCT line
    // numbers (1..=5 cycled) plus a distinct trailing comment so the raw text
    // differs — all resolve to a real path, all distinct raw strings.
    let mut txt = String::new();
    for i in 0..60 {
        let line = (i % 5) + 1; // 1..=5, always in bounds for the 5-line file
        txt.push_str(&format!(
            "Fact {i} stands here [src: file: src/foo.rs:{line}]. "
        ));
    }
    let r = analyze(&txt, Some(&root));
    assert_eq!(
        r.sources.len(),
        MAX_SOURCES_VERIFIED,
        "60 distinct [src:] markers must cap sources at {MAX_SOURCES_VERIFIED}, got {}",
        r.sources.len()
    );
    cleanup(&root);
}

/// Exactly 50 markers → all 50 verified (boundary, no truncation).
#[test]
fn exactly_fifty_markers_all_verified() {
    let root = temp_project();
    let mut txt = String::new();
    for i in 0..50 {
        let line = (i % 5) + 1;
        txt.push_str(&format!("Item {i} [src: file: src/foo.rs:{line}]. "));
    }
    let r = analyze(&txt, Some(&root));
    assert_eq!(r.sources.len(), 50, "50 markers → 50 sources, none dropped");
    // All point at a real, in-bounds line → all Verified.
    assert_eq!(
        r.verified_count(),
        50,
        "all 50 in-bounds file refs must be Verified, got {}",
        r.verified_count()
    );
    cleanup(&root);
}

/// 60 FABRICATED markers (non-existent path) → sources capped at 50 AND every
/// surfaced one counts as fabricated (the cap must not silently hide red).
#[test]
fn sixty_fabricated_markers_cap_and_all_red() {
    let root = temp_project();
    let mut txt = String::new();
    for i in 0..60 {
        txt.push_str(&format!("Claim {i} [src: file: src/ghost_{i}.rs:1]. "));
    }
    let r = analyze(&txt, Some(&root));
    assert_eq!(
        r.sources.len(),
        MAX_SOURCES_VERIFIED,
        "fabricated markers must still cap at {MAX_SOURCES_VERIFIED}"
    );
    // Each of the 50 surfaced points at a missing file → NotFound → fabricated.
    assert_eq!(
        r.fabricated_count, MAX_SOURCES_VERIFIED as u32,
        "every surfaced fabricated marker must count, got {}",
        r.fabricated_count
    );
    cleanup(&root);
}

// ─────────────────────────────────────────────────────────────────────────
// MAX_SOURCES_VERIFIED — inline backtick anchors
// ─────────────────────────────────────────────────────────────────────────

/// 60 DISTINCT inline backtick anchors that don't resolve → the inline-anchor
/// pass is ALSO `.take(MAX_SOURCES_VERIFIED)`, so at most 50 inline anchors are
/// surfaced. With zero [src:] markers, total sources must be ≤ 50.
#[test]
fn sixty_inline_anchors_cap_at_fifty() {
    let root = temp_project();
    let mut txt = String::new();
    for i in 0..60 {
        // distinct non-existent path, known ext + slash → recognised anchor
        txt.push_str(&format!("see `src/missing_{i}.rs:1` for detail. "));
    }
    let r = analyze(&txt, Some(&root));
    assert!(
        r.sources.len() <= MAX_SOURCES_VERIFIED,
        "inline anchors must be capped at {MAX_SOURCES_VERIFIED}, got {}",
        r.sources.len()
    );
    // All 50 surfaced are non-resolving → soft amber (Unchecked), not red.
    assert_eq!(
        r.fabricated_count, 0,
        "non-resolving inline anchors are amber, never red"
    );
    assert!(
        r.unverified_count >= 1,
        "non-resolving inline anchors must bump unverified_count"
    );
    cleanup(&root);
}

/// The two caps are INDEPENDENT: 60 [src:] markers + 60 inline anchors can
/// together surface up to 100 sources (50 from each pass), not capped jointly.
/// Probes that the inline pass isn't accidentally sharing the marker budget.
#[test]
fn marker_and_anchor_caps_are_independent() {
    let root = temp_project();
    let mut txt = String::new();
    for i in 0..60 {
        txt.push_str(&format!("M{i} [src: file: src/ghost_m{i}.rs:1]. "));
    }
    for i in 0..60 {
        txt.push_str(&format!("see `src/ghost_a{i}.rs:1`. "));
    }
    let r = analyze(&txt, Some(&root));
    // 50 markers + 50 anchors (distinct path keys, no dedup collision).
    assert_eq!(
        r.sources.len(),
        2 * MAX_SOURCES_VERIFIED,
        "independent caps must allow up to 100 sources, got {}",
        r.sources.len()
    );
    cleanup(&root);
}

// ─────────────────────────────────────────────────────────────────────────
// MAX_FLAGGED_SPANS
// ─────────────────────────────────────────────────────────────────────────

/// 60 flaggable unsourced claims → count tracks ALL 60, stored spans cap at 25.
#[test]
fn sixty_unsourced_claims_count_grows_spans_cap() {
    // Each sentence carries the "is configured" cue, no anchor, no hedge,
    // long enough (> MIN_SENTENCE_CHARS), not a question/heading.
    let mut txt = String::new();
    for i in 0..60 {
        txt.push_str(&format!(
            "The parameter number {i} is configured for the entire fleet here. "
        ));
    }
    let r = lint_assertions(&txt);
    assert_eq!(
        r.unsourced_count, 60,
        "count must track all 60 unsourced claims, got {}",
        r.unsourced_count
    );
    assert_eq!(
        r.flagged_spans.len(),
        MAX_FLAGGED_SPANS,
        "stored spans must cap at {MAX_FLAGGED_SPANS}, got {}",
        r.flagged_spans.len()
    );
    cleanup(Path::new("/nonexistent_noop")); // no-op, keeps shape uniform
}

/// Exactly 25 claims → all 25 stored, none dropped (boundary).
#[test]
fn exactly_twenty_five_claims_all_spans_stored() {
    let mut txt = String::new();
    for i in 0..25 {
        txt.push_str(&format!(
            "The parameter number {i} is configured across the cluster today. "
        ));
    }
    let r = lint_assertions(&txt);
    assert_eq!(r.unsourced_count, 25);
    assert_eq!(
        r.flagged_spans.len(),
        25,
        "exactly 25 claims → 25 spans, none dropped, got {}",
        r.flagged_spans.len()
    );
}

/// 26 claims → count 26, spans capped at 25 (just-over boundary).
#[test]
fn twenty_six_claims_drops_one_span() {
    let mut txt = String::new();
    for i in 0..26 {
        txt.push_str(&format!(
            "The parameter number {i} is configured across the cluster today. "
        ));
    }
    let r = lint_assertions(&txt);
    assert_eq!(r.unsourced_count, 26);
    assert_eq!(r.flagged_spans.len(), MAX_FLAGGED_SPANS);
}

/// The span cap surfaces through the full `analyze` path too, not just the bare
/// heuristic — `analyze` copies the lint report's spans verbatim.
#[test]
fn span_cap_survives_full_analyze() {
    let root = temp_project();
    let mut txt = String::new();
    for i in 0..40 {
        txt.push_str(&format!(
            "The parameter number {i} is configured for the whole region here. "
        ));
    }
    let r = analyze(&txt, Some(&root));
    assert_eq!(r.unsourced_count, 40);
    assert_eq!(
        r.flagged_spans.len(),
        MAX_FLAGGED_SPANS,
        "analyze must preserve the span cap, got {}",
        r.flagged_spans.len()
    );
    cleanup(&root);
}

// ─────────────────────────────────────────────────────────────────────────
// Huge / pathological inputs — NO PANIC
// ─────────────────────────────────────────────────────────────────────────

/// ~100k chars of prose → no panic, returns a report. (Sanity perf bound.)
#[test]
fn very_long_text_no_panic() {
    let unit = "The endpoint returns a 500 error on a null payload here today. ";
    let mut txt = String::with_capacity(110_000);
    while txt.len() < 100_000 {
        txt.push_str(unit);
    }
    let root = temp_project();
    let r = analyze(&txt, Some(&root));
    // Spans must still be capped even on a 100k input.
    assert!(
        r.flagged_spans.len() <= MAX_FLAGGED_SPANS,
        "spans capped on huge input, got {}",
        r.flagged_spans.len()
    );
    // The huge text has no [src:] markers and no inline anchors → no sources.
    assert!(r.sources.is_empty(), "no citations → no sources");
    cleanup(&root);
}

/// ~100k chars stuffed with 1000 `[src:]` markers → no panic, sources capped.
#[test]
fn very_long_text_many_markers_no_panic_capped() {
    let root = temp_project();
    let mut txt = String::with_capacity(120_000);
    let mut i = 0u32;
    while txt.len() < 100_000 {
        let line = (i % 5) + 1;
        txt.push_str(&format!("Fact {i} [src: file: src/foo.rs:{line}]. "));
        i += 1;
    }
    let r = analyze(&txt, Some(&root));
    assert_eq!(
        r.sources.len(),
        MAX_SOURCES_VERIFIED,
        "huge text with 1000s of markers still caps at {MAX_SOURCES_VERIFIED}, got {}",
        r.sources.len()
    );
    cleanup(&root);
}

// ─────────────────────────────────────────────────────────────────────────
// LINE_COUNT_SIZE_CAP_BYTES — file size boundary
// ─────────────────────────────────────────────────────────────────────────

/// A file JUST UNDER 2 MiB → line bounds ARE checked: an out-of-bounds line
/// must come back OutOfBounds (fabricated), an in-bounds line Verified.
#[test]
fn file_just_under_size_cap_does_bounds_check() {
    let root = temp_project();
    // ~1.5 MiB file, one char + newline per line → known small line count.
    let lines = 100usize;
    let mut content = String::new();
    // Pad to ~1.5 MiB but keep a known line count: first `lines` real lines,
    // then a single very long final line (no newline) padded with spaces.
    for i in 0..lines {
        content.push_str(&format!("line{i}\n"));
    }
    let pad = (1_500_000usize).saturating_sub(content.len());
    content.push_str(&" ".repeat(pad)); // long trailing line, no '\n'
    std::fs::write(root.join("src/big_under.txt"), &content).unwrap();
    let meta = std::fs::metadata(root.join("src/big_under.txt")).unwrap();
    assert!(
        meta.len() < LINE_COUNT_SIZE_CAP_BYTES,
        "fixture must be under the cap"
    );
    // total lines = `lines` full lines + 1 trailing padded line = lines+1.
    let total = lines + 1;
    // In-bounds line → Verified.
    let ok = analyze(
        &format!("ref [src: file: src/big_under.txt:{}]", total),
        Some(&root),
    );
    assert_eq!(
        ok.verified_count(),
        1,
        "in-bounds line in an under-cap file must verify"
    );
    // Out-of-bounds line → OutOfBounds → fabricated (bounds check ran).
    let bad = analyze(
        &format!("ref [src: file: src/big_under.txt:{}]", total + 5000),
        Some(&root),
    );
    assert_eq!(
        bad.fabricated_count, 1,
        "out-of-bounds line in an under-cap file must be flagged fabricated"
    );
    assert_eq!(bad.sources.len(), 1);
    assert_eq!(bad.sources[0].status, SourceStatus::OutOfBounds);
    cleanup(&root);
}

/// A file JUST OVER 2 MiB → the line-count check is SKIPPED (existence only).
/// An "out-of-bounds" line that would fail in a small file must now VERIFY,
/// because bounds aren't checked above the size cap.
#[test]
fn file_over_size_cap_skips_bounds_check_verifies() {
    let root = temp_project();
    // > 2 MiB of content, few lines → a huge line number can't be validated.
    let big = "x".repeat((LINE_COUNT_SIZE_CAP_BYTES as usize) + 1024);
    std::fs::write(root.join("src/big_over.txt"), &big).unwrap();
    let meta = std::fs::metadata(root.join("src/big_over.txt")).unwrap();
    assert!(
        meta.len() > LINE_COUNT_SIZE_CAP_BYTES,
        "fixture must exceed the cap, got {}",
        meta.len()
    );
    // A line number far beyond the real count → still Verified (bounds skipped).
    let r = analyze("ref [src: file: src/big_over.txt:9999999]", Some(&root));
    assert_eq!(
        r.verified_count(),
        1,
        "above the size cap, an out-of-bounds line still verifies (existence-only)"
    );
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.sources[0].status, SourceStatus::Verified);
    cleanup(&root);
}

// ─────────────────────────────────────────────────────────────────────────
// Determinism
// ─────────────────────────────────────────────────────────────────────────

/// Same (text, root) over a capped, mixed input → byte-identical report.
#[test]
fn capped_report_is_deterministic() {
    let root = temp_project();
    let mut txt = String::new();
    for i in 0..60 {
        let line = (i % 5) + 1;
        txt.push_str(&format!(
            "Param {i} is configured here [src: file: src/foo.rs:{line}]. "
        ));
    }
    for i in 0..60 {
        txt.push_str(&format!("see `src/ghost_{i}.rs:1`. "));
    }
    let a = analyze(&txt, Some(&root));
    let b = analyze(&txt, Some(&root));
    assert_eq!(a.unsourced_count, b.unsourced_count);
    assert_eq!(a.fabricated_count, b.fabricated_count);
    assert_eq!(a.unverified_count, b.unverified_count);
    assert_eq!(a.flagged_spans.len(), b.flagged_spans.len());
    assert_eq!(a.sources.len(), b.sources.len());
    // Field-by-field source equality (order + content must be stable).
    let key = |r: &kronn::core::anti_halluc::LintReport| {
        r.sources
            .iter()
            .map(|s| (s.raw.clone(), s.kind, s.status))
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&a), key(&b), "source list must be deterministic");
    // Span text+reason equality.
    let spans = |r: &kronn::core::anti_halluc::LintReport| {
        r.flagged_spans
            .iter()
            .map(|s| (s.text.clone(), s.reason.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(spans(&a), spans(&b), "spans must be deterministic");
    cleanup(&root);
}
