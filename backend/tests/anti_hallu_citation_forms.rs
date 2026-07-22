//! Adversarial QA for the anti-hallucination DIMENSION "citation_forms".
//!
//! Every `[src: <type>: …]` variant, inline backtick path anchors, and the
//! token-boundary case (a file literally named `user_service.rs` must classify
//! as File, NOT User). Each case asserts the (kind, status) contract from the
//! anti-halluc spec. We also probe the hard invariants: no traversal ever comes
//! back Verified, no panic on hostile input, determinism.

use kronn::core::anti_halluc::{analyze, verify_source_marker, SourceKind, SourceStatus};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DIM: &str = "citation_forms";
static SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique temp project with a few known files of known length.
///   src/foo.rs            → 5 lines (a,b,c,d,e)
///   src/user_service.rs   → 3 lines (the token-boundary trap)
///   composer.json         → 1 line  (root-level, line-cited anchor)
fn temp_project() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "ahcf_{}_{}_{}_{}",
        DIM,
        std::process::id(),
        n,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
    std::fs::write(d.join("src/user_service.rs"), "x\ny\nz\n").unwrap();
    std::fs::write(d.join("composer.json"), "{}\n").unwrap();
    d
}

fn cleanup(d: &Path) {
    let _ = std::fs::remove_dir_all(d);
}

/// Convenience: verify one marker's inner content against a root.
fn vsm(inner: &str, root: &Path) -> kronn::core::anti_halluc::SourceCheck {
    verify_source_marker(inner, Some(root))
}

// ─── 1. file: existing path + line in bounds → File / Verified ──────────────

#[test]
fn file_existing_inbounds_is_verified() {
    let p = temp_project();
    let c = vsm("file: src/foo.rs:3", &p);
    assert_eq!(c.kind, SourceKind::File, "kind for file ref");
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "src/foo.rs:3 exists with 5 lines → Verified, got {:?} ({})",
        c.status,
        c.detail
    );
    cleanup(&p);
}

// ─── 2. file: existing path, line PAST EOF → File / OutOfBounds (fabricated) ─

#[test]
fn file_line_out_of_bounds_is_fabricated() {
    let p = temp_project();
    let c = vsm("file: src/foo.rs:99", &p);
    assert_eq!(c.kind, SourceKind::File);
    assert_eq!(
        c.status,
        SourceStatus::OutOfBounds,
        "line 99 in a 5-line file → OutOfBounds, got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "OutOfBounds must count as fabricated"
    );
    cleanup(&p);
}

// ─── 3. file: nonexistent path → File / NotFound (fabricated) ───────────────

#[test]
fn file_missing_is_notfound() {
    let p = temp_project();
    let c = vsm("file: src/ghost.rs:1", &p);
    assert_eq!(c.kind, SourceKind::File);
    assert_eq!(c.status, SourceStatus::NotFound, "detail: {}", c.detail);
    assert!(c.status.is_fabricated());
    cleanup(&p);
}

// ─── 4. url: → Url / Unchecked (NOT fabricated, NOT verified) ───────────────

#[test]
fn url_is_unchecked() {
    let p = temp_project();
    let c = vsm("url: https://example.com/spec", &p);
    assert_eq!(c.kind, SourceKind::Url, "url prefix → Url");
    assert_eq!(c.status, SourceStatus::Unchecked);
    assert!(
        !c.status.is_fabricated(),
        "an unchecked URL is not fabricated"
    );
    cleanup(&p);
}

// ─── 5. bare URL with no type prefix → Url / Unchecked ──────────────────────

#[test]
fn bare_url_no_prefix_is_url_unchecked() {
    let p = temp_project();
    let c = vsm("https://example.com/path/file.rs", &p);
    assert_eq!(
        c.kind,
        SourceKind::Url,
        "a bare http(s) inner must classify Url even with no `url:` prefix"
    );
    assert_eq!(c.status, SourceStatus::Unchecked);
    cleanup(&p);
}

// ─── 6. user-confirmed (no-colon form) → User / Unchecked ───────────────────

#[test]
fn user_confirmed_phrase_is_user_unchecked() {
    let p = temp_project();
    let c = vsm("user-confirmed: peer told me in standup", &p);
    assert_eq!(c.kind, SourceKind::User, "user-confirmed → User");
    assert_eq!(c.status, SourceStatus::Unchecked);
    assert!(!c.status.is_fabricated());
    cleanup(&p);
}

// ─── 7. user:<id>:<date> typed form → User / Unchecked ──────────────────────

#[test]
fn user_typed_prefix_is_user_unchecked() {
    let p = temp_project();
    let c = vsm("user:TestUser:2026-05-30: confirmed verbally", &p);
    assert_eq!(c.kind, SourceKind::User);
    assert_eq!(c.status, SourceStatus::Unchecked);
    cleanup(&p);
}

// ─── 8. commit: → Commit / Unchecked ────────────────────────────────────────

#[test]
fn commit_is_unchecked() {
    let p = temp_project();
    let c = vsm("commit: d1662db", &p);
    assert_eq!(c.kind, SourceKind::Commit, "commit prefix → Commit");
    assert_eq!(c.status, SourceStatus::Unchecked);
    assert!(!c.status.is_fabricated());
    cleanup(&p);
}

// ─── 9. inferred: → Inferred / Unchecked (soft, never file-verified) ────────

#[test]
fn inferred_is_unchecked() {
    let p = temp_project();
    let c = vsm("inferred: probably handled by the router", &p);
    assert_eq!(c.kind, SourceKind::Inferred);
    assert_eq!(c.status, SourceStatus::Unchecked);
    assert!(!c.status.is_fabricated());
    cleanup(&p);
}

// ─── 10. hypothesis: → Hypothesis / Unchecked ───────────────────────────────

#[test]
fn hypothesis_is_unchecked() {
    let p = temp_project();
    let c = vsm("hypothesis: the cache TTL might be 60s", &p);
    assert_eq!(c.kind, SourceKind::Hypothesis);
    assert_eq!(c.status, SourceStatus::Unchecked);
    assert!(!c.status.is_fabricated());
    cleanup(&p);
}

// ─── 11. api: → Api / Unchecked (treated like a URL, no network) ────────────

#[test]
fn api_is_unchecked() {
    let p = temp_project();
    let c = vsm("api: GET /v1/users", &p);
    assert_eq!(c.kind, SourceKind::Api, "api prefix → Api");
    assert_eq!(c.status, SourceStatus::Unchecked);
    assert!(!c.status.is_fabricated());
    cleanup(&p);
}

// ─── 12. code-comment: pointing at a REAL file:line → CodeComment / Verified ─

#[test]
fn code_comment_existing_is_verified_low_trust() {
    let p = temp_project();
    let c = vsm("code-comment: src/foo.rs:2", &p);
    assert_eq!(
        c.kind,
        SourceKind::CodeComment,
        "code-comment → CodeComment"
    );
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "code-comment still existence-verifies; detail: {}",
        c.detail
    );
    assert!(
        c.detail.to_lowercase().contains("comment"),
        "detail should flag low-trust comment: {}",
        c.detail
    );
    cleanup(&p);
}

// ─── 13. training-data: → TrainingData / Rejected (the hallucination case) ──

#[test]
fn training_data_is_rejected() {
    let p = temp_project();
    let c = vsm("training-data: I recall the default is 8080", &p);
    assert_eq!(
        c.kind,
        SourceKind::TrainingData,
        "training-data → TrainingData"
    );
    assert_eq!(
        c.status,
        SourceStatus::Rejected,
        "model prior knowledge is refused as a source"
    );
    assert!(
        c.status.is_fabricated(),
        "Rejected must count as fabricated (RED)"
    );
    cleanup(&p);
}

// ─── 14. TOKEN-BOUNDARY TRAP: file named user_service.rs → File, NOT User ───

#[test]
fn user_service_rs_classifies_as_file_not_user() {
    let p = temp_project();
    // "user" is a prefix of "user_service.rs" but NOT at a token boundary
    // (next char is '_', not ':'/whitespace/end), so it must fall through to
    // File and be existence-verified — never silently treated as a User tier.
    let c = vsm("file: src/user_service.rs:2", &p);
    assert_eq!(
        c.kind,
        SourceKind::File,
        "user_service.rs must be File, not User"
    );
    assert_eq!(c.status, SourceStatus::Verified, "detail: {}", c.detail);
    cleanup(&p);
}

// ─── 15. TOKEN-BOUNDARY (no `file:` prefix): bare user_service.rs path ───────

#[test]
fn bare_user_service_path_is_file_not_user() {
    let p = temp_project();
    // No type prefix at all. The classifier must NOT see leading "user" as the
    // User keyword (it's "user_..."), and must default to File.
    let c = vsm("src/user_service.rs:1", &p);
    assert_eq!(
        c.kind,
        SourceKind::File,
        "bare user_service.rs path must classify File, not User"
    );
    assert_eq!(c.status, SourceStatus::Verified, "detail: {}", c.detail);
    cleanup(&p);
}

// ─── 16. TOKEN-BOUNDARY: file named api_client.rs → File, not Api ───────────

#[test]
fn api_client_rs_is_file_not_api() {
    let p = temp_project();
    std::fs::write(p.join("src/api_client.rs"), "1\n2\n").unwrap();
    let c = vsm("src/api_client.rs:1", &p);
    assert_eq!(
        c.kind,
        SourceKind::File,
        "api_client.rs must be File, not Api"
    );
    assert_eq!(c.status, SourceStatus::Verified, "detail: {}", c.detail);
    cleanup(&p);
}

// ─── 17. INLINE backtick anchor `path/file.ext:line` that RESOLVES → green ──

#[test]
fn inline_anchor_with_slash_resolves_verified() {
    let p = temp_project();
    let text = "The handler is in `src/foo.rs:4` per my read.";
    let r = analyze(text, Some(&p));
    let file_sources: Vec<_> = r
        .sources
        .iter()
        .filter(|s| s.kind == SourceKind::File)
        .collect();
    assert_eq!(
        file_sources.len(),
        1,
        "exactly one inline file anchor; got {:?}",
        r.sources
    );
    assert_eq!(
        file_sources[0].status,
        SourceStatus::Verified,
        "`src/foo.rs:4` resolves in a 5-line file → Verified; detail {}",
        file_sources[0].detail
    );
    assert_eq!(r.verified_count(), 1);
    assert_eq!(r.unverified_count, 0);
    assert!(r.has_signal(), "any citation → has_signal");
    cleanup(&p);
}

// ─── 18. INLINE bare root-level `composer.json:1` (line, no slash) resolves ─

#[test]
fn inline_anchor_root_level_with_line_resolves() {
    let p = temp_project();
    // No slash, but an explicit `:line` + known ext → must be treated as a
    // file anchor (the documented recall fix for root-level files).
    let text = "Bumped a dep at `composer.json:1` last week.";
    let r = analyze(text, Some(&p));
    let file_sources: Vec<_> = r
        .sources
        .iter()
        .filter(|s| s.kind == SourceKind::File)
        .collect();
    assert_eq!(
        file_sources.len(),
        1,
        "composer.json:1 (line, no slash) must be picked up as a file anchor; sources {:?}",
        r.sources
    );
    assert_eq!(
        file_sources[0].status,
        SourceStatus::Verified,
        "composer.json exists → Verified; detail {}",
        file_sources[0].detail
    );
    cleanup(&p);
}

// ─── 19. INLINE anchor that does NOT resolve → Unchecked + unverified++ ─────

#[test]
fn inline_anchor_unresolved_is_soft_unverified_not_red() {
    let p = temp_project();
    let text = "I edited `src/ghost.rs:2` for this change.";
    let r = analyze(text, Some(&p));
    let file_sources: Vec<_> = r
        .sources
        .iter()
        .filter(|s| s.kind == SourceKind::File)
        .collect();
    assert_eq!(file_sources.len(), 1, "sources {:?}", r.sources);
    assert_eq!(
        file_sources[0].status,
        SourceStatus::Unchecked,
        "unresolved inline anchor is soft Unchecked, NOT a fabricated red"
    );
    assert!(
        file_sources[0]
            .detail
            .to_lowercase()
            .contains("couldn't verify"),
        "detail should say couldn't verify: {}",
        file_sources[0].detail
    );
    assert_eq!(r.unverified_count, 1, "unverified_count bumps");
    assert_eq!(r.fabricated_count, 0, "must NOT be red/fabricated");
    cleanup(&p);
}

// ─── 20. INLINE bare extensionful prose `node.js` (no slash, no line) → none ─

#[test]
fn inline_bare_ext_prose_is_not_a_file_anchor() {
    let p = temp_project();
    let text = "We use `node.js` for the toolchain here today.";
    let r = analyze(text, Some(&p));
    let file_sources: Vec<_> = r
        .sources
        .iter()
        .filter(|s| s.kind == SourceKind::File)
        .collect();
    assert!(
        file_sources.is_empty(),
        "`node.js` (no slash, no :line) must NOT be treated as a file anchor; got {:?}",
        file_sources
    );
    cleanup(&p);
}

// ─── 21. SECURITY: relative `../` traversal must NEVER be Verified ──────────

#[test]
fn relative_traversal_never_verified() {
    let p = temp_project();
    // Plant a real file OUTSIDE the root so the only thing protecting us is the
    // jail, not non-existence.
    let outside = p
        .parent()
        .unwrap()
        .join(format!("ahcf_secret_{}.rs", std::process::id()));
    std::fs::write(&outside, "s\ne\nc\n").unwrap();
    let inner = format!(
        "file: ../{}:1",
        outside.file_name().unwrap().to_str().unwrap()
    );
    let c = vsm(&inner, &p);
    assert_ne!(
        c.status,
        SourceStatus::Verified,
        "a ../ traversal to a REAL file must never verify; got {:?} ({})",
        c.status,
        c.detail
    );
    assert!(
        matches!(
            c.status,
            SourceStatus::OutsideProject | SourceStatus::NotFound
        ),
        "traversal must be OutsideProject or NotFound; got {:?}",
        c.status
    );
    let _ = std::fs::remove_file(&outside);
    cleanup(&p);
}

// ─── 22. SECURITY: deep `../../../etc/passwd` probe → fabricated, never green ─

#[test]
fn deep_traversal_to_etc_passwd_is_outside_project() {
    let p = temp_project();
    let c = vsm("file: ../../../../../../etc/passwd:1", &p);
    assert_ne!(
        c.status,
        SourceStatus::Verified,
        "{:?} {}",
        c.status,
        c.detail
    );
    assert!(
        c.status.is_fabricated(),
        "a root-escaping probe must count as fabricated; got {:?}",
        c.status
    );
    cleanup(&p);
}

// ─── 23. EmptyRef: file: with no usable reference → File / EmptyRef ─────────

#[test]
fn empty_file_ref_is_emptyref() {
    let p = temp_project();
    let c = vsm("file:", &p);
    assert_eq!(c.kind, SourceKind::File);
    assert_eq!(
        c.status,
        SourceStatus::EmptyRef,
        "empty file ref → EmptyRef; detail {}",
        c.detail
    );
    assert!(c.status.is_fabricated());
    cleanup(&p);
}

// ─── 24. NO PANIC + determinism on hostile input ────────────────────────────

#[test]
fn no_panic_on_hostile_input_and_deterministic() {
    let p = temp_project();
    let hostile = [
        "",
        "[src:",                                   // malformed, unterminated
        "[src: file: ]",                           // empty after type
        "[src: [src: file: src/foo.rs ] ]",        // nested brackets
        "file: src/foo\0.rs:1",                    // null-ish
        "url: https://例え.テスト/路径/файл.rs:5", // CJK/RTL/cyrillic + emoji below
        "file: 😀/💥.rs:1",
        "training-data:",
        "code-comment:",
        "file: src/foo.rs:0",   // zero line → no line spec
        "file: src/foo.rs:2-1", // inverted range
    ];
    for h in hostile {
        // Marker-level: must not panic.
        let a = verify_source_marker(h, Some(&p));
        let b = verify_source_marker(h, Some(&p));
        assert_eq!(a, b, "verify_source_marker not deterministic for {:?}", h);
        assert_ne!(
            a.status,
            SourceStatus::Verified,
            "hostile {:?} should not be Verified ({:?})",
            h,
            a
        );
    }
    // Whole-text analysis over huge + multibyte input: no panic, deterministic.
    let huge = format!(
        "{} [src: file: src/foo.rs:1] `src/foo.rs:2` 😀漢字שלום {}",
        "x".repeat(50_000),
        "y".repeat(50_000)
    );
    let r1 = analyze(&huge, Some(&p));
    let r2 = analyze(&huge, Some(&p));
    assert_eq!(
        r1, r2,
        "analyze must be deterministic on huge multibyte input"
    );
    cleanup(&p);
}
