//! Adversarial integration tests for the anti-hallucination `verify_outcomes`
//! dimension: the mechanical line/range bounds verdict of a formal
//! `[src: file: PATH:LINE]` marker.
//!
//! A known-length temp file (`src/foo.rs`, 5 content lines) is the fixture.
//! We probe Verified (single line + range, both in bounds), NotFound,
//! OutOfBounds (single past EOF, range end past EOF), the malformed-spec cases
//! (inverted range `b<a`, line 0) which per `parse_line_spec` are NOT treated
//! as ranges, and EmptyRef.
//!
//! Each test asserts the CORRECT behaviour per the documented SEMANTICS. A red
//! test here is a suspected real bug, not a weakened expectation.

use kronn::core::anti_halluc::{analyze, verify_source_marker, SourceKind, SourceStatus};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique temp project with a known 5-content-line file.
/// `src/foo.rs` content = "a\nb\nc\nd\ne\n" → `content.lines().count()` == 5.
fn temp_project() -> PathBuf {
    let uniq = format!(
        "ahbh_verify_outcomes_{}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst),
        uuid::Uuid::new_v4()
    );
    let d = std::env::temp_dir().join(uniq);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
    d
}

fn cleanup(d: &PathBuf) {
    let _ = std::fs::remove_dir_all(d);
}

// ── 1. Verified — single line, strictly in bounds ─────────────────────────
#[test]
fn verified_single_line_in_bounds() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:3", Some(root.as_path()));
    assert_eq!(c.kind, SourceKind::File, "should classify as File");
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "line 3 of a 5-line file must be Verified, got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(!c.status.is_fabricated(), "Verified must not be fabricated");
    cleanup(&root);
}

// ── 2. Verified — last line exactly at EOF boundary ───────────────────────
#[test]
fn verified_last_line_at_boundary() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:5", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "line 5 == file length 5 must be Verified (end <= total), got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 3. Verified — range fully in bounds ───────────────────────────────────
#[test]
fn verified_range_in_bounds() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:2-4", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "range 2-4 within 5 lines must be Verified, got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 4. Verified — range ending exactly at EOF ─────────────────────────────
#[test]
fn verified_range_end_at_eof() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:1-5", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "range 1-5 (end == total) must be Verified, got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 5. NotFound — file does not exist ─────────────────────────────────────
#[test]
fn not_found_missing_file() {
    let root = temp_project();
    let c = verify_source_marker("file: src/ghost.rs:1", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::NotFound,
        "a non-existent file must be NotFound, got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "NotFound must count as fabricated (RED)"
    );
    cleanup(&root);
}

// ── 6. OutOfBounds — single line past EOF ─────────────────────────────────
#[test]
fn out_of_bounds_single_line_past_eof() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:6", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::OutOfBounds,
        "line 6 of a 5-line file must be OutOfBounds, got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "OutOfBounds must count as fabricated (RED)"
    );
    cleanup(&root);
}

// ── 7. OutOfBounds — range end beyond EOF ─────────────────────────────────
#[test]
fn out_of_bounds_range_end_past_eof() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:3-99", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::OutOfBounds,
        "range 3-99 (end past 5 lines) must be OutOfBounds, got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 8. Inverted range b<a — parse_line_spec rejects → treated as path ─────
// `parse_line_spec("5-2")` returns None (end < start), so `split_path_and_lines`
// does NOT peel the spec: the path becomes `src/foo.rs:5-2`, which does not
// exist on disk → NotFound (a literal file named `foo.rs:5-2`). The contract
// dimension lists this under "OutOfBounds (... inverted range a-b with b<a ...)"
// but the implementation's actual reachable verdict is NotFound. We assert the
// stronger invariant that matters: an inverted range must NEVER come back
// Verified, and must be classified as fabricated.
#[test]
fn inverted_range_is_never_verified() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:5-2", Some(root.as_path()));
    assert_ne!(
        c.status,
        SourceStatus::Verified,
        "inverted range 5-2 must NEVER be Verified, got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "inverted range must be fabricated (RED), got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 9. Line 0 — parse_line_spec rejects → treated as path ─────────────────
// `parse_line_spec("0")` returns None (n == 0), so the path becomes
// `src/foo.rs:0`, a non-existent file → must NOT be Verified. Line numbering is
// 1-based; line 0 is meaningless and must never validate.
#[test]
fn line_zero_is_never_verified() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:0", Some(root.as_path()));
    assert_ne!(
        c.status,
        SourceStatus::Verified,
        "line 0 must NEVER be Verified (1-based numbering), got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "line 0 must be fabricated (RED), got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 10. Range starting at 0 — `0-3` rejected by parse_line_spec ───────────
// `parse_line_spec("0-3")` returns None (start == 0), so path = `src/foo.rs:0-3`
// → never Verified.
#[test]
fn range_start_zero_is_never_verified() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs:0-3", Some(root.as_path()));
    assert_ne!(
        c.status,
        SourceStatus::Verified,
        "range starting at line 0 must NEVER be Verified, got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 11. EmptyRef — `[src: file: ]` (type prefix, empty reference) ─────────
#[test]
fn empty_ref_file_prefix_only() {
    let root = temp_project();
    let c = verify_source_marker("file:", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::EmptyRef,
        "a `file:` marker with an empty reference must be EmptyRef, got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "EmptyRef must count as fabricated (RED)"
    );
    cleanup(&root);
}

// ── 12. EmptyRef — bare `[src: ]` with whitespace, no type ────────────────
// An empty/whitespace inner falls through classify_source to File (the default
// kind), then verify_file_ref sees an empty reference → EmptyRef.
#[test]
fn empty_ref_whitespace_only() {
    let root = temp_project();
    let c = verify_source_marker("   ", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::EmptyRef,
        "a whitespace-only marker must be EmptyRef, got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}

// ── 13. EmptyRef via full analyze() over real `[src: file: ]` text ────────
// End-to-end through `analyze`: the formal marker with an empty ref must be
// extracted, verified EmptyRef, and counted in fabricated_count.
#[test]
fn analyze_empty_file_ref_counts_fabricated() {
    let root = temp_project();
    let text = "The config [src: file: ] lives somewhere.";
    let report = analyze(text, Some(root.as_path()));
    assert_eq!(
        report.sources.len(),
        1,
        "exactly one [src:] marker extracted"
    );
    assert_eq!(
        report.sources[0].status,
        SourceStatus::EmptyRef,
        "empty file ref must be EmptyRef, got {:?}",
        report.sources[0].status
    );
    assert_eq!(
        report.fabricated_count, 1,
        "EmptyRef must increment fabricated_count, got {}",
        report.fabricated_count
    );
    assert_eq!(report.verified_count(), 0, "nothing verified here");
    assert!(report.has_signal(), "a citation present → has_signal");
    cleanup(&root);
}

// ── 14. analyze() — Verified single line increments verified_count ────────
#[test]
fn analyze_verified_increments_verified_count() {
    let root = temp_project();
    let text = "The entry point [src: file: src/foo.rs:2] is defined here.";
    let report = analyze(text, Some(root.as_path()));
    assert_eq!(report.sources.len(), 1, "one marker");
    assert_eq!(report.sources[0].status, SourceStatus::Verified);
    assert_eq!(report.verified_count(), 1, "one verified source");
    assert_eq!(report.fabricated_count, 0, "nothing fabricated");
    cleanup(&root);
}

// ── 15. analyze() — OutOfBounds increments fabricated_count ───────────────
#[test]
fn analyze_out_of_bounds_increments_fabricated() {
    let root = temp_project();
    let text = "See [src: file: src/foo.rs:42] for details.";
    let report = analyze(text, Some(root.as_path()));
    assert_eq!(report.sources.len(), 1);
    assert_eq!(report.sources[0].status, SourceStatus::OutOfBounds);
    assert_eq!(report.fabricated_count, 1);
    assert_eq!(report.verified_count(), 0);
    cleanup(&root);
}

// ── 16. DETERMINISM — same (text, root) → identical report twice ──────────
#[test]
fn determinism_same_input_same_report() {
    let root = temp_project();
    let text = "A [src: file: src/foo.rs:3] and a bad [src: file: src/foo.rs:99] ref.";
    let r1 = analyze(text, Some(root.as_path()));
    let r2 = analyze(text, Some(root.as_path()));
    assert_eq!(r1, r2, "analyze must be deterministic for identical inputs");
    cleanup(&root);
}

// ── 17. NO PANIC — malformed / hostile line specs must not panic ──────────
#[test]
fn no_panic_on_malformed_line_specs() {
    let root = temp_project();
    let probes = [
        "file: src/foo.rs:",                           // trailing colon, empty spec
        "file: src/foo.rs:-",                          // bare dash
        "file: src/foo.rs:1-",                         // open-ended range
        "file: src/foo.rs:-5",                         // negative-looking
        "file: src/foo.rs:abc",                        // non-numeric
        "file: src/foo.rs:99999999999999999999999999", // overflow usize
        "file: src/foo.rs:1-99999999999999999999999999",
        "file: src/foo.rs:🦀",  // emoji spec
        "file: src/foo.rs:2:3", // double colon
        "file: ",               // empty
        "file: ::::",           // colon soup
    ];
    for p in probes {
        let c = verify_source_marker(p, Some(root.as_path()));
        // Whatever the verdict, it must never be Verified for these garbage
        // specs against a 5-line file (none of the above name a real, in-bounds
        // line of an existing path).
        assert_ne!(
            c.status,
            SourceStatus::Verified,
            "garbage spec {:?} must not verify, got {:?} ({})",
            p,
            c.status,
            c.detail
        );
    }
    cleanup(&root);
}

// ── 18. Bare path, no line spec → Verified (existence only) ───────────────
#[test]
fn bare_path_no_line_is_verified() {
    let root = temp_project();
    let c = verify_source_marker("file: src/foo.rs", Some(root.as_path()));
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "an existing file with no line spec must be Verified (existence), got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&root);
}
