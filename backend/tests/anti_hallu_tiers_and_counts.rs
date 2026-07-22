//! Adversarial QA — DIMENSION "tiers_and_counts".
//!
//! The five pill tiers as they fall out of `analyze()` + the exact counts that
//! drive each one:
//!   - GREEN   verified-only      → verified_count() > 0, everything else 0
//!   - AMBER   unsourced-only     → unsourced_count > 0, no sources
//!   - RED     fabricated-only    → fabricated_count > 0 (formal [src:] NotFound)
//!   - AMBER-  unverified-only    → unverified_count > 0 (inline anchor that fails)
//!   - NEUTRAL unverifiable-only  → has_signal()==true via an Unchecked URL/user citation, with fabricated/verified/unverified all 0
//!   - MIXED   all of the above coexist → each count exact
//!
//! Plus the dedup invariant: the same path cited BOTH as `[src: file: …]` and as
//! an inline backtick anchor must count as ONE verified source, not two.
//!
//! Every test asserts the CORRECT behaviour per the documented semantics. A red
//! test here is a real defect, not a weakened expectation.

use kronn::core::anti_halluc::{
    analyze, lint_assertions, verify_source_marker, SourceKind, SourceStatus,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DIM: &str = "tiers_and_counts";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp project with a known file `src/foo.rs` of exactly 5 lines
/// (`.lines().count()` == 5). Line 3 is in bounds; line 99 is out of bounds.
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

// ──────────────────────────────────────────────────────────────────────────
// 1. VERIFIED-ONLY (green). A formal [src: file:] that resolves in bounds.
//    verified_count()==1; fabricated/unverified/unsourced all 0.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_verified_only_formal_marker() {
    let root = temp_project();
    let text = "See the handler [src: file: src/foo.rs:3] for details.";
    let r = analyze(text, Some(&root));
    assert_eq!(r.verified_count(), 1, "one resolved formal file source");
    assert_eq!(r.fabricated_count, 0, "nothing fabricated");
    assert_eq!(r.unverified_count, 0, "nothing unverified");
    assert_eq!(r.unsourced_count, 0, "the sentence carries an anchor");
    assert!(r.has_signal(), "a citation is always surfaced");
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].status, SourceStatus::Verified);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 2. VERIFIED-ONLY via a NATURAL inline backtick anchor that resolves.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_verified_only_inline_anchor() {
    let root = temp_project();
    // Backticked path with slash + known ext + in-bounds line → auto-verified.
    let text = "The logic lives in `src/foo.rs:2` right there.";
    let r = analyze(text, Some(&root));
    assert_eq!(r.verified_count(), 1, "inline anchor auto-verifies green");
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.unverified_count, 0);
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].status, SourceStatus::Verified);
    assert_eq!(r.sources[0].kind, SourceKind::File);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 3. UNSOURCED-ONLY (amber). A confident technical claim with no anchor.
//    unsourced_count>0; sources empty; verified/fabricated/unverified all 0.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_unsourced_only_no_citation() {
    let root = temp_project();
    let text = "The function authenticate_user is defined in the auth layer.";
    let r = analyze(text, Some(&root));
    assert!(r.unsourced_count > 0, "an unanchored claim must be flagged");
    assert!(
        r.sources.is_empty(),
        "no [src:] and no inline anchor → no sources"
    );
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.unverified_count, 0);
    assert!(r.has_signal());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 4. FABRICATED-ONLY (red). A formal [src: file:] pointing at a missing file.
//    NotFound → fabricated_count==1; verified/unverified 0.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_fabricated_only_notfound() {
    let root = temp_project();
    let text = "Refer to [src: file: src/ghost.rs:1].";
    let r = analyze(text, Some(&root));
    assert_eq!(
        r.fabricated_count, 1,
        "a missing formal ref is fabricated (red)"
    );
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.unverified_count, 0);
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].status, SourceStatus::NotFound);
    assert!(r.sources[0].status.is_fabricated());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 4b. FABRICATED-ONLY via OutOfBounds — line beyond file length is still red.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_fabricated_only_out_of_bounds() {
    let root = temp_project();
    // File exists (5 lines) but line 99 is past the end.
    let text = "Refer to [src: file: src/foo.rs:99].";
    let r = analyze(text, Some(&root));
    assert_eq!(
        r.fabricated_count, 1,
        "out-of-bounds line is fabricated (red)"
    );
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.sources[0].status, SourceStatus::OutOfBounds);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 4c. FABRICATED-ONLY via training-data — Rejected counts as fabricated (red).
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_fabricated_only_training_data_rejected() {
    let root = temp_project();
    let text = "I recall that [src: training-data: GPT pretraining corpus] says so.";
    let r = analyze(text, Some(&root));
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].kind, SourceKind::TrainingData);
    assert_eq!(r.sources[0].status, SourceStatus::Rejected);
    assert_eq!(r.fabricated_count, 1, "training-data is rejected → red");
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.unverified_count, 0);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 5. UNVERIFIED-ONLY (soft amber). An inline backtick anchor that does NOT
//    resolve → Unchecked + unverified_count++; NOT fabricated, NOT verified.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_unverified_only_inline_anchor_fails() {
    let root = temp_project();
    let text = "It is wired up in `src/missing.rs:3` somewhere.";
    let r = analyze(text, Some(&root));
    assert_eq!(
        r.unverified_count, 1,
        "non-resolving inline anchor → soft amber"
    );
    assert_eq!(
        r.fabricated_count, 0,
        "inline failure is NOT red fabrication"
    );
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
    assert!(
        r.sources[0].detail.contains("couldn't verify"),
        "detail should honestly say couldn't verify, got: {}",
        r.sources[0].detail
    );
    assert!(r.has_signal());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 6. UNVERIFIABLE-ONLY (neutral) via URL. has_signal()==true, but
//    fabricated/verified/unverified all 0 — Option B surfaces it anyway.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_unverifiable_only_url() {
    let root = temp_project();
    let text = "Per the spec [src: url: https://example.com/spec].";
    let r = analyze(text, Some(&root));
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].kind, SourceKind::Url);
    assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
    assert_eq!(r.fabricated_count, 0, "an unchecked URL is not fabricated");
    assert_eq!(r.verified_count(), 0, "an unchecked URL is not verified");
    assert_eq!(
        r.unverified_count, 0,
        "unverified_count is for inline anchors only"
    );
    assert!(
        r.has_signal(),
        "any citation, even unverifiable, is surfaced (Option B)"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 6b. UNVERIFIABLE-ONLY via user-confirmed → User/Unchecked, all counts 0.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_unverifiable_only_user_confirmed() {
    let root = temp_project();
    let text = "This was [src: user-confirmed] during the call.";
    let r = analyze(text, Some(&root));
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].kind, SourceKind::User);
    assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.unverified_count, 0);
    assert!(r.has_signal());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 6c. UNVERIFIABLE-ONLY via inferred → Inferred/Unchecked, all counts 0.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_unverifiable_only_inferred() {
    let root = temp_project();
    let text = "Probably the cache [src: inferred: from the access pattern].";
    let r = analyze(text, Some(&root));
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].kind, SourceKind::Inferred);
    assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.unverified_count, 0);
    assert!(r.has_signal());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 7. MIXED — all five tiers in one message, each count exact.
//    verified=1 (foo.rs:2), fabricated=1 (ghost.rs), unverified=1 (missing.rs),
//    unverifiable=1 (url, Unchecked), and an unanchored claim sentence (amber).
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn tier_mixed_all_counts_exact() {
    let root = temp_project();
    let text = "\
The verified bit is [src: file: src/foo.rs:2].
The bogus one is [src: file: src/ghost.rs:1].
The inline guess is `src/missing.rs:4`.
The external ref is [src: url: https://example.com].
The function authenticate_user is defined in the core module.";
    let r = analyze(text, Some(&root));

    assert_eq!(r.verified_count(), 1, "exactly one verified source");
    assert_eq!(r.fabricated_count, 1, "exactly one fabricated source");
    assert_eq!(
        r.unverified_count, 1,
        "exactly one unverified inline anchor"
    );
    assert!(
        r.unsourced_count >= 1,
        "the unanchored claim sentence must flag at least once, got {}",
        r.unsourced_count
    );

    // Three formal [src:] markers + one inline anchor = four listed sources.
    assert_eq!(
        r.sources.len(),
        4,
        "3 formal markers + 1 inline anchor listed, got {:?}",
        r.sources
    );
    // One of the four is the neutral Unchecked URL (not fabricated/verified).
    let url_unchecked = r
        .sources
        .iter()
        .filter(|s| s.kind == SourceKind::Url && s.status == SourceStatus::Unchecked)
        .count();
    assert_eq!(url_unchecked, 1, "exactly one neutral unchecked URL source");
    assert!(r.has_signal());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 8. DEDUP — the SAME path cited as a formal [src: file:] AND inline backtick
//    must count as ONE verified source, not two.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn dedup_formal_and_inline_same_path_counts_once() {
    let root = temp_project();
    let text = "Defined in [src: file: src/foo.rs:2], see also `src/foo.rs:3`.";
    let r = analyze(text, Some(&root));
    assert_eq!(
        r.verified_count(),
        1,
        "same path cited twice (formal + inline) must dedup to ONE verified, got {} sources: {:?}",
        r.verified_count(),
        r.sources
    );
    // The inline anchor must be dropped entirely (the formal one already covers
    // the path), so exactly one source is listed.
    assert_eq!(
        r.sources.len(),
        1,
        "the inline duplicate must not be appended, got {:?}",
        r.sources
    );
    assert_eq!(
        r.unverified_count, 0,
        "the inline dup is not surfaced as unverified"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 8b. DEDUP must NOT collapse distinct paths — two different verified files
//     count as two.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn dedup_distinct_paths_not_collapsed() {
    let root = temp_project();
    std::fs::write(root.join("src/bar.rs"), "x\ny\nz\n").unwrap();
    let text = "See [src: file: src/foo.rs:1] and `src/bar.rs:2`.";
    let r = analyze(text, Some(&root));
    assert_eq!(
        r.verified_count(),
        2,
        "two distinct verified paths must NOT dedup, got {:?}",
        r.sources
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 9. SILENT — a reply with no claim cue, no anchor, no citation has NO signal.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn no_signal_when_nothing_to_say() {
    let root = temp_project();
    let text = "Hello, thanks for the update. Talk soon.";
    let r = analyze(text, Some(&root));
    assert_eq!(r.unsourced_count, 0);
    assert!(r.sources.is_empty());
    assert!(!r.has_signal(), "a chatty no-claim reply is silent");
    assert!(r.is_empty(), "is_empty mirrors !has_signal");
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 10. DETERMINISM — same (text, root) yields an identical report twice.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn determinism_same_input_same_report() {
    let root = temp_project();
    let text = "\
Verified [src: file: src/foo.rs:1]; bogus [src: file: src/ghost.rs:9];
inline `src/missing.rs:7`; url [src: url: https://example.com];
the endpoint /v1/users is handled by the router.";
    let a = analyze(text, Some(&root));
    let b = analyze(text, Some(&root));
    assert_eq!(a, b, "analyze must be deterministic for identical inputs");
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 11. NO PANIC on hostile input: empty, huge, emoji/CJK/RTL, malformed/nested
//     brackets, null-ish chars. analyze must always return a report.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn no_panic_on_hostile_input() {
    let root = temp_project();
    let huge = "[src: file: ".to_string() + &"a/".repeat(50_000) + "x.rs:1]";
    let cases: Vec<String> = vec![
        String::new(),
        " ".to_string(),
        "[src:".to_string(),                     // unterminated
        "[src: file: ".to_string(),              // unterminated + no path
        "[src: [src: file: a]]".to_string(),     // nested brackets
        "[src: file: a[0][1].rs:2]".to_string(), // balanced nested
        "émojis 🚀🔥 and CJK 中文 and RTL \u{202E}reversed".to_string(),
        "null\u{0000}byte [src: file: \u{0000}.rs:1]".to_string(),
        "[src: file: src/foo.rs:99999999999999999999]".to_string(), // overflow line
        "[src: file: src/foo.rs:-5]".to_string(),                   // negative-ish
        huge,
    ];
    for c in &cases {
        let r = analyze(c, Some(&root));
        // Determinism even on hostile input.
        let r2 = analyze(c, Some(&root));
        assert_eq!(r, r2, "hostile input must still be deterministic");
        // Counts are coherent (verified ≤ total sources).
        assert!(
            r.verified_count() as usize <= r.sources.len(),
            "verified_count cannot exceed listed sources"
        );
    }
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 12. has_signal() contract — unsourced-only and sources-only each trigger it,
//     and verify_source_marker is consistent with analyze's per-source verdict.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn has_signal_and_marker_consistency() {
    let root = temp_project();

    // unsourced-only path (no sources, but a flag) → has_signal true.
    let lint = lint_assertions("The route /api/foo is handled by the dispatcher.");
    assert!(lint.unsourced_count > 0);
    assert!(lint.has_signal());
    assert!(lint.sources.is_empty());

    // A standalone marker check matches analyze's classification.
    let direct = verify_source_marker("file: src/foo.rs:3", Some(&root));
    assert_eq!(direct.kind, SourceKind::File);
    assert_eq!(direct.status, SourceStatus::Verified);

    let direct_bad = verify_source_marker("file: src/ghost.rs:1", Some(&root));
    assert_eq!(direct_bad.status, SourceStatus::NotFound);
    assert!(direct_bad.status.is_fabricated());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 13. UNVERIFIABLE tier without a project root — file refs fall to Unchecked,
//     so they surface (has_signal) but never count as verified/fabricated.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn rootless_file_ref_is_unchecked_not_fabricated() {
    let text = "See [src: file: src/foo.rs:2] in the codebase.";
    let r = analyze(text, None);
    assert_eq!(r.sources.len(), 1);
    assert_eq!(
        r.sources[0].status,
        SourceStatus::Unchecked,
        "no root → can't resolve → Unchecked, not fabricated"
    );
    assert_eq!(r.fabricated_count, 0, "rootless file ref is NOT red");
    assert_eq!(r.verified_count(), 0);
    assert_eq!(r.unverified_count, 0);
    assert!(r.has_signal());
}
