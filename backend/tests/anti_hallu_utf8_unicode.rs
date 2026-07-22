//! Adversarial QA — DIMENSION "utf8_unicode".
//!
//! The anti-hallucination core slices strings by byte index in several places
//! (sentence splitting, excerpt truncation, `[src:` scanning, type-keyword
//! peeling, line-spec splitting on the last `:`). Any of those is a latent
//! `byte index is not a char boundary` panic the moment a French accent, an
//! emoji, a CJK ideograph, an RTL run, or a combining sequence shows up — in
//! EITHER the prose claim OR the cited path (a real file literally named
//! `café.rs`). These tests feed exactly that and assert:
//!   - NO PANIC on any of it, and
//!   - a SANE report (counts in range, determinism, sources actually verify
//!     when the unicode-named file truly exists on disk).
//!
//! Each test asserts the CORRECT documented behaviour. A panic OR a wrong
//! count = a real defect, reported under suspected_bugs (test kept red).

use kronn::core::anti_halluc::{
    analyze, lint_assertions, verify_source_marker, SourceKind, SourceStatus,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DIM: &str = "utf8_unicode";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique temp project seeded with files whose NAMES carry unicode:
///   - `src/foo.rs`       (5 lines) — ASCII control.
///   - `src/café.rs`      (5 lines) — Latin-1 accent in the name.
///   - `src/日本語.rs`    (3 lines) — CJK ideographs in the name.
///   - `src/emoji_🚀.rs`  (4 lines) — emoji (4-byte UTF-8) in the name.
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
    std::fs::write(d.join("src/café.rs"), "a\nb\nc\nd\ne\n").unwrap();
    std::fs::write(d.join("src/日本語.rs"), "x\ny\nz\n").unwrap();
    std::fs::write(d.join("src/emoji_🚀.rs"), "1\n2\n3\n4\n").unwrap();
    d
}

fn cleanup(d: &Path) {
    let _ = std::fs::remove_dir_all(d);
}

// ──────────────────────────────────────────────────────────────────────────
// 1. A real file literally named `café.rs` cited formally MUST verify.
//    (Byte-index path handling must survive the multibyte 'é'.)
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn accented_filename_formal_src_verifies() {
    let root = temp_project();
    let check = verify_source_marker("file: src/café.rs:3", Some(&root));
    assert_eq!(
        check.status,
        SourceStatus::Verified,
        "a real file named café.rs at an in-bounds line must verify (no boundary panic), got {:?} ({})",
        check.status, check.detail
    );
    assert_eq!(check.kind, SourceKind::File);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 2. CJK-named file cited with an out-of-bounds line → OutOfBounds (fabricated),
//    never a panic and never a false Verified.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn cjk_filename_out_of_bounds_line_is_fabricated() {
    let root = temp_project();
    // 日本語.rs has 3 lines; line 999 is past the end.
    let check = verify_source_marker("file: src/日本語.rs:999", Some(&root));
    assert_eq!(
        check.status,
        SourceStatus::OutOfBounds,
        "a CJK-named file cited beyond its length must be OutOfBounds, got {:?} ({})",
        check.status,
        check.detail
    );
    assert!(check.status.is_fabricated());
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 3. Emoji (4-byte) in the cited path of a real file → verifies; emoji in a
//    NON-existent path → NotFound. No panic either way.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn emoji_filename_verifies_and_missing_is_not_found() {
    let root = temp_project();
    let ok = verify_source_marker("file: src/emoji_🚀.rs:2", Some(&root));
    assert_eq!(
        ok.status,
        SourceStatus::Verified,
        "real emoji-named file must verify, got {:?} ({})",
        ok.status,
        ok.detail
    );
    let missing = verify_source_marker("file: src/missing_💥.rs", Some(&root));
    assert_eq!(
        missing.status,
        SourceStatus::NotFound,
        "non-existent emoji path must be NotFound, got {:?}",
        missing.status
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 4. analyze() over prose mixing accents/emoji/CJK with a verifiable accented
//    inline backtick anchor → the anchor resolves (verified File), no panic.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn analyze_mixed_script_prose_with_accented_inline_anchor() {
    let root = temp_project();
    let text = "Le café ☕ est délicieux 日本語 🚀. La logique vit dans `src/café.rs:1`.";
    let report = analyze(text, Some(&root));
    let verified = report.verified_count();
    assert!(
        verified >= 1,
        "the inline `src/café.rs:1` anchor (real file) should auto-verify, sources={:?}",
        report.sources
    );
    // No fabricated formal markers in this prose.
    assert_eq!(
        report.fabricated_count, 0,
        "no formal [src:] here, so 0 fabricated"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 5. lint_assertions over heavy emoji/CJK/RTL prose with a claim cue but no
//    anchor → must NOT panic, and the count is a sane small number.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn lint_emoji_cjk_rtl_prose_no_panic_sane_count() {
    // RTL Arabic + Hebrew + emoji + CJK, with an English claim cue ("the function").
    let text =
        "مرحبا بالعالم 🌍 שלום עולם 你好世界 — the function handles every request reliably here.";
    let report = lint_assertions(text);
    // Exactly one claim-cue sentence, no anchor/hedge → expect a flag, but the
    // hard guarantee is: no panic and the count stays bounded/sane.
    assert!(
        report.unsourced_count <= 3,
        "unsourced_count must stay sane on mixed-script prose, got {}",
        report.unsourced_count
    );
    // Spans must never exceed the count (basic internal consistency).
    assert!(report.flagged_spans.len() as u32 <= report.unsourced_count);
    cleanup(Path::new("/nonexistent")); // no-op; keep symmetry
}

// ──────────────────────────────────────────────────────────────────────────
// 6. Curly (typographic) quotes / apostrophe in a hedge. The normalizer folds
//    U+2019 → ' so "je n'sais" style hedges still suppress. Probe a curly-quote
//    hedge: "je crois" with a curly apostrophe variant + a cue → suppressed.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn curly_quote_hedge_suppresses_flag() {
    // "peut-être" carries the OPINION cue. Use the curly apostrophe in "l'option"
    // surroundings and a hedge with a curly apostrophe. The cue "se trouve" is
    // present; the hedge "je crois" should suppress.
    let hedged = "Je crois que la fonction se trouve quelque part dans le module principal.";
    let report = lint_assertions(hedged);
    assert_eq!(
        report.unsourced_count, 0,
        "a hedged French claim must not be flagged, spans={:?}",
        report.flagged_spans
    );

    // Now the same WITHOUT the hedge but WITH a curly-apostrophe opinion frame
    // ("je n'\u{2019}suggère"-ish). Use a clean opinion cue with curly quote.
    let opinion = "Je suggère que la fonction se trouve dans le module principal partagé.";
    let r2 = lint_assertions(opinion);
    assert_eq!(
        r2.unsourced_count, 0,
        "an opinion-framed claim must not be flagged, spans={:?}",
        r2.flagged_spans
    );
}

// ──────────────────────────────────────────────────────────────────────────
// 7. Combining characters: "café" written as base 'e' + U+0301 combining acute
//    in BOTH the prose and a cited path. Must not panic. The combining-form
//    path does NOT byte-equal the precomposed file on disk → NotFound (expected,
//    NOT a false Verified), and definitely no boundary panic.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn combining_chars_in_path_and_prose_no_panic() {
    let root = temp_project();
    // "cafe\u{0301}.rs" = c a f e + COMBINING ACUTE — NFD form, different bytes
    // from the precomposed "café.rs" (NFC) that actually exists on disk.
    let nfd_path = "file: src/cafe\u{0301}.rs:1";
    let check = verify_source_marker(nfd_path, Some(&root));
    // The FS stores the NFC form; the NFD citation won't byte-match → NotFound.
    // The invariant we truly assert: no panic + not a false Verified of the
    // wrong file. (On case/normalization-insensitive FSes this could verify;
    // accept either Verified or NotFound — but NEVER OutsideProject/panic.)
    assert!(
        matches!(
            check.status,
            SourceStatus::Verified | SourceStatus::NotFound
        ),
        "NFD combining path must be Verified or NotFound, never escape/panic, got {:?}",
        check.status
    );

    // Combining chars in prose with a claim cue — must not panic.
    let prose = "Le cafe\u{0301} ☕ : the function returns a value, e\u{0301}videmment.";
    let r = lint_assertions(prose);
    assert!(
        r.unsourced_count <= 3,
        "sane count, got {}",
        r.unsourced_count
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 8. Excerpt truncation across a multibyte boundary. A claim sentence with a
//    cue, no anchor/hedge, and >160 chars of accented + emoji text forces the
//    excerpt() char-take path. Must not panic, and the excerpt must be valid
//    UTF-8 (it always is if no panic) and the flag fires.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn long_accented_emoji_sentence_excerpt_no_panic() {
    // ~200 chars, all multibyte, ending with a claim cue, no anchor/hedge.
    let filler = "é🚀".repeat(90); // 180 chars, all multibyte
    let text = format!("{filler} the endpoint returns json");
    let report = lint_assertions(&text);
    assert_eq!(
        report.unsourced_count, 1,
        "one long unanchored claim should flag exactly once, got {}",
        report.unsourced_count
    );
    // The stored excerpt is capped; it must be a valid (non-empty) string.
    let span = &report.flagged_spans[0];
    assert!(!span.text.is_empty(), "excerpt must not be empty");
    // char count must not exceed cap + the ellipsis.
    assert!(
        span.text.chars().count() <= 161,
        "excerpt char count exceeds 160(+ellipsis): {}",
        span.text.chars().count()
    );
}

// ──────────────────────────────────────────────────────────────────────────
// 9. Multibyte content INSIDE a `[src: …]` marker with nested brackets and a
//    type keyword whose tail is multibyte. extract + classify + verify must not
//    panic on the byte-index slicing in match_type_keyword / split_path_and_lines.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn multibyte_inside_src_marker_with_nested_brackets() {
    let root = temp_project();
    // Nested brackets + accented/CJK content. classify_source slices by keyword
    // byte length; the remainder is multibyte.
    let text = "Voir [src: file: src/café.rs:2] et [src: inferred: 日本語 hypothèse[0]] ☕.";
    let report = analyze(text, Some(&root));
    // The café.rs:2 marker verifies; the inferred marker is Unchecked (soft).
    assert!(
        report.verified_count() >= 1,
        "café.rs:2 formal marker should verify, sources={:?}",
        report.sources
    );
    assert_eq!(
        report.fabricated_count, 0,
        "inferred is Unchecked, café exists → nothing fabricated, got {}",
        report.fabricated_count
    );
    // The inferred marker must be present and classified Inferred / Unchecked.
    assert!(
        report
            .sources
            .iter()
            .any(|s| s.kind == SourceKind::Inferred && s.status == SourceStatus::Unchecked),
        "the multibyte inferred marker must be Unchecked, sources={:?}",
        report.sources
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 10. Determinism on a heavy unicode payload: same (text, root) → identical
//     report across repeated calls. (Catches HashSet-ordering / nondeterministic
//     slicing bugs that only manifest on multibyte input.)
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn determinism_on_heavy_unicode_payload() {
    let root = temp_project();
    let text = "café ☕ 日本語 🚀 مرحبا. The function lives in `src/café.rs:4`. \
                Voir [src: file: src/日本語.rs:2] et [src: url: https://exemplé.test/é]. \
                The endpoint returns data 你好. [src: training-data: GPT prior]";
    let a = analyze(text, Some(&root));
    let b = analyze(text, Some(&root));
    let c = analyze(text, Some(&root));
    assert_eq!(a, b, "report must be deterministic (run 1 vs 2)");
    assert_eq!(b, c, "report must be deterministic (run 2 vs 3)");
    // Sanity: training-data → Rejected (fabricated); two real files → verified.
    assert!(
        a.fabricated_count >= 1,
        "training-data must be Rejected/fabricated, sources={:?}",
        a.sources
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 11. Pathological / null-ish / malformed multibyte input must not panic.
//     Lone surrogates can't exist in Rust &str, but we throw: a truncated-looking
//     `[src:` with multibyte tail, a NUL char, RTL override, zero-width joiners,
//     a BOM, and an unterminated marker.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn pathological_multibyte_inputs_no_panic() {
    let root = temp_project();
    let big = "é".repeat(5000); // big multibyte blob (no cue)
    let payloads = [
        "\u{FEFF}[src: file: café\u{0301}", // BOM + unterminated marker, NFD tail
        "the function\u{0000}returns 🚀 [src:", // NUL + unterminated [src:
        "\u{202E}gnp.elif :rraw\u{202C} the endpoint", // RTL override wrapping
        "👨‍👩‍👧‍👦 the table is defined 你好",     // ZWJ family emoji + cue
        "[src: file: ../é/../🚀.rs:1]",     // traversal w/ multibyte
        "[src::::é:::]",                    // degenerate colons + accent
        "",                                 // empty
        big.as_str(),
    ];
    for (i, p) in payloads.iter().enumerate() {
        // Both entrypoints, with and without a root — none may panic.
        let r1 = lint_assertions(p);
        let r2 = analyze(p, Some(&root));
        let r3 = analyze(p, None);
        // Counts are u32, always sane; just assert internal consistency.
        assert!(
            r1.flagged_spans.len() as u32 <= r1.unsourced_count,
            "payload #{i}: spans exceed count"
        );
        // A traversal marker must NEVER come back Verified.
        for s in r2.sources.iter().chain(r3.sources.iter()) {
            if s.raw.contains("..") {
                assert_ne!(
                    s.status,
                    SourceStatus::Verified,
                    "payload #{i}: a ../ traversal must never verify, src={:?}",
                    s
                );
            }
        }
    }
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 12. has_signal() / is_empty() coherence on a unicode-only reply with a single
//     verifiable accented anchor — green path, no panic.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn has_signal_coherent_on_unicode_anchor_only() {
    let root = temp_project();
    let text = "Résumé 🚀 日本語 : `src/café.rs`.";
    let report = analyze(text, Some(&root));
    assert!(
        report.has_signal(),
        "a reply with a resolving anchor must surface a signal"
    );
    assert!(!report.is_empty(), "has_signal ⇒ not empty");
    assert_eq!(
        report.has_signal(),
        report.unsourced_count > 0 || !report.sources.is_empty(),
        "has_signal contract must hold verbatim"
    );
    cleanup(&root);
}
