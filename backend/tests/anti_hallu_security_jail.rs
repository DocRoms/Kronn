//! Adversarial QA — DIMENSION "security_jail".
//!
//! The single scariest invariant of the anti-hallucination core: a RELATIVE
//! source path that escapes the project root (`../`, deeply-nested `../`, or a
//! symlinked subdir pointing outside the root) must NEVER come back `Verified`.
//! A traversal must be `OutsideProject` (or, at worst, `NotFound`) — never a
//! green chip. Absolute paths are existence-only (no jail) and must NOT be
//! re-rooted under the project.
//!
//! These tests assert the CORRECT behaviour per the documented semantics. A
//! red test here = a real security defect.

use kronn::core::anti_halluc::{
    analyze, verify_source_marker, LintReport, SourceKind, SourceStatus,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DIM: &str = "security_jail";

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

/// Convenience: verify a formal `[src: file: …]` reference against a root.
fn check_file(reference: &str, root: &Path) -> SourceStatus {
    let marker = format!("file: {reference}");
    verify_source_marker(&marker, Some(root)).status
}

// ──────────────────────────────────────────────────────────────────────────
// 1. The control: a legitimate in-jail relative path IS verified.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn control_legit_relative_path_is_verified() {
    let root = temp_project();
    let st = check_file("src/foo.rs", &root);
    assert_eq!(
        st,
        SourceStatus::Verified,
        "a legit in-bounds relative path must verify (control)"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 2. Classic shallow traversal — ../../../../etc/passwd — must NOT verify.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn relative_etc_passwd_traversal_is_outside_project() {
    let root = temp_project();
    let st = check_file("../../../../etc/passwd", &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "../../../../etc/passwd must NEVER be Verified"
    );
    assert_eq!(
        st,
        SourceStatus::OutsideProject,
        "a ../ escape must be OutsideProject, got {st:?}"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 3. Deeply-nested ../ (way more than the depth of the root) — still jailed.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn deeply_nested_parent_dir_is_outside_project() {
    let root = temp_project();
    // 40 levels of ../ — guaranteed to climb above the filesystem root.
    let mut rel = String::new();
    for _ in 0..40 {
        rel.push_str("../");
    }
    rel.push_str("etc/passwd");
    let st = check_file(&rel, &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "deep ../ must never be Verified"
    );
    assert_eq!(
        st,
        SourceStatus::OutsideProject,
        "40x ../ must be OutsideProject, got {st:?}"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 4. Traversal that climbs out then dives back into the SAME root by name.
//    `../<rootname>/src/foo.rs` lexically escapes (pops to parent) then re-adds
//    the root dir — lexically it normalises back inside. The jail must still
//    not let an OUT-OF-ROOT spelling slip through if it does NOT canonicalise
//    inside. Here it genuinely points back at the real file, so the honest
//    expectation is: it either Verifies (it IS the real file) or is jailed.
//    The hard invariant: it must NOT verify a DIFFERENT escaping target.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn climb_out_and_back_to_sibling_escape_never_verifies_foreign_file() {
    let root = temp_project();
    // Create a SIBLING dir next to root holding a secret file.
    let parent = root.parent().unwrap();
    let sibling = parent.join(format!(
        "ahbh_{}_sibling_{}",
        DIM,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("secret.rs"), "x\ny\n").unwrap();

    let sibling_name = sibling.file_name().unwrap().to_str().unwrap();
    let rel = format!("../{sibling_name}/secret.rs");
    let st = check_file(&rel, &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "a ../sibling/ file outside the root must NEVER be Verified, got {st:?}"
    );
    assert_eq!(
        st,
        SourceStatus::OutsideProject,
        "../sibling/secret.rs escapes the root → OutsideProject, got {st:?}"
    );
    cleanup(&sibling);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 5. Symlinked subdir pointing OUTSIDE the root. Lexically in-jail, but
//    canonicalises out → must be OutsideProject, never Verified.
// ──────────────────────────────────────────────────────────────────────────
#[cfg(unix)]
#[test]
fn symlinked_subdir_escaping_root_is_outside_project() {
    let root = temp_project();
    // A real out-of-root target dir with a file.
    let parent = root.parent().unwrap();
    let outside = parent.join(format!(
        "ahbh_{}_outside_{}",
        DIM,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("loot.rs"), "1\n2\n3\n").unwrap();

    // root/escape -> outside
    let link = root.join("escape");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    // Cited path stays lexically under root: escape/loot.rs
    let st = check_file("escape/loot.rs", &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "a file reached via a symlink that escapes the root must NEVER be Verified, got {st:?}"
    );
    assert_eq!(
        st,
        SourceStatus::OutsideProject,
        "symlink escaping the root → OutsideProject, got {st:?}"
    );
    cleanup(&outside);
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 6. Symlink to /etc directly — the textbook attack.
// ──────────────────────────────────────────────────────────────────────────
#[cfg(unix)]
#[test]
fn symlink_to_etc_is_outside_project() {
    let root = temp_project();
    let link = root.join("sys");
    // Only meaningful if /etc/passwd exists (it does on Linux CI).
    if std::os::unix::fs::symlink("/etc", &link).is_ok() {
        let st = check_file("sys/passwd", &root);
        assert_ne!(
            st,
            SourceStatus::Verified,
            "sys/passwd via symlink-to-/etc must NEVER be Verified, got {st:?}"
        );
        assert_eq!(
            st,
            SourceStatus::OutsideProject,
            "symlink to /etc → OutsideProject, got {st:?}"
        );
    }
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 7. Absolute /etc/passwd — existence-only, NOT treated as project-relative.
//    The contract: absolute paths get NO jail. If /etc/passwd exists it may be
//    Verified (existence-only). The invariant we PROVE here is that it is NOT
//    re-rooted: it must NOT come back OutsideProject (that would mean it was
//    wrongly jailed), and the verdict must be identical whether the root is the
//    temp project or a totally unrelated root (because it's not project-relative).
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn absolute_etc_passwd_is_existence_only_not_jailed() {
    let root = temp_project();
    let st = check_file("/etc/passwd", &root);
    // Absolute → never jailed.
    assert_ne!(
        st,
        SourceStatus::OutsideProject,
        "an absolute path must NOT be jailed/treated as project-relative, got {st:?}"
    );
    // It's existence-only: Verified if it exists, NotFound otherwise.
    let exists = Path::new("/etc/passwd").exists();
    if exists {
        assert_eq!(
            st,
            SourceStatus::Verified,
            "/etc/passwd exists → existence-only Verified"
        );
    } else {
        assert_eq!(st, SourceStatus::NotFound, "/etc/passwd absent → NotFound");
    }
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 8. Absolute path verdict is root-INDEPENDENT (proves it isn't re-rooted).
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn absolute_path_verdict_is_root_independent() {
    let root_a = temp_project();
    let root_b = temp_project();
    let abs = "/etc/passwd";
    let st_a = check_file(abs, &root_a);
    let st_b = check_file(abs, &root_b);
    assert_eq!(
        st_a, st_b,
        "an absolute path is existence-only → same verdict regardless of project root"
    );
    cleanup(&root_a);
    cleanup(&root_b);
}

// ──────────────────────────────────────────────────────────────────────────
// 9. Absolute path to a NON-existent file → NotFound (existence-only), never
//    Verified and never OutsideProject.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn absolute_nonexistent_is_not_found_not_jailed() {
    let root = temp_project();
    let st = check_file("/this/surely/does/not/exist/anywhere_kronn_qa.rs", &root);
    assert_eq!(
        st,
        SourceStatus::NotFound,
        "absolute non-existent file → NotFound (existence-only), got {st:?}"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 10. Backtick-wrapped traversal inside a formal [src:] — clean_reference must
//     strip the backticks but NOT widen the escape surface. Still OutsideProject.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn backtick_wrapped_traversal_is_outside_project() {
    let root = temp_project();
    let st = check_file("`../../../../etc/passwd`", &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "backtick-wrapped ../ must NEVER be Verified, got {st:?}"
    );
    assert_eq!(
        st,
        SourceStatus::OutsideProject,
        "backtick-wrapped ../ still escapes → OutsideProject, got {st:?}"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 11. Traversal with a line-spec (`../../etc/passwd:1`) — the line peel must
//     not change the jail verdict.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn traversal_with_line_spec_is_outside_project() {
    let root = temp_project();
    let st = check_file("../../../../etc/passwd:1", &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "../ with :line must never be Verified"
    );
    assert_eq!(
        st,
        SourceStatus::OutsideProject,
        "../etc/passwd:1 escapes → OutsideProject, got {st:?}"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 12. Full analyze(): a [src: file: ../../etc/passwd] marker in prose drives
//     fabricated_count (OutsideProject is_fabricated), never verified_count.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn analyze_counts_traversal_as_fabricated_not_verified() {
    let root = temp_project();
    let text = "The secret lives in [src: file: ../../../../etc/passwd].";
    let report: LintReport = analyze(text, Some(&root));
    assert_eq!(
        report.verified_count(),
        0,
        "a traversal must contribute ZERO verified sources"
    );
    assert_eq!(
        report.fabricated_count, 1,
        "OutsideProject is fabricated → fabricated_count == 1, got {}",
        report.fabricated_count
    );
    assert_eq!(report.sources.len(), 1, "exactly one [src:] extracted");
    assert_eq!(report.sources[0].status, SourceStatus::OutsideProject);
    assert!(
        report.has_signal(),
        "a fabricated source must surface a signal"
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 13. Inline backtick anchor with traversal — `../../etc/passwd` resolves
//     OutsideProject → soft amber (unverified_count), NEVER Verified.
//     (needs a known ext; etc/passwd has none, so use a .rs traversal.)
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn analyze_inline_anchor_traversal_is_unverified_not_verified() {
    let root = temp_project();
    let text = "See `../../../../tmp/evil.rs` for the payload.";
    let report = analyze(text, Some(&root));
    assert_eq!(
        report.verified_count(),
        0,
        "an inline traversal anchor must never be Verified"
    );
    // Inline anchors that don't resolve are soft amber, not red fabricated.
    assert_eq!(
        report.fabricated_count, 0,
        "inline anchor (not a formal [src:]) must NOT be red fabricated"
    );
    assert_eq!(
        report.unverified_count, 1,
        "non-resolving inline anchor → unverified_count == 1, got {}",
        report.unverified_count
    );
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 14. No-root context: file refs are Unchecked, never Verified (can't jail
//     against nothing, must not optimistically verify).
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn no_root_file_ref_is_unchecked_never_verified() {
    let st = verify_source_marker("file: ../../etc/passwd", None).status;
    assert_ne!(st, SourceStatus::Verified, "no root → must not be Verified");
    assert_eq!(
        st,
        SourceStatus::Unchecked,
        "no project root → file ref is Unchecked, got {st:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// 15. NO PANIC + DETERMINISM on adversarial inputs (empty, huge, UTF-8/emoji/
//     CJK/RTL, malformed brackets, null-ish chars). Same (text, root) twice →
//     identical report.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn no_panic_and_deterministic_on_adversarial_inputs() {
    let root = temp_project();
    let huge = format!("[src: file: ../{}/x.rs] ", "../".repeat(5000));
    let inputs: Vec<String> = vec![
        String::new(),
        "   ".to_string(),
        "[src:".to_string(),        // unterminated
        "[src: file: ".to_string(), // empty ref, unterminated
        "[src: file: [nested[brackets]]]".to_string(),
        "[src: file: ../\0/etc/passwd]".to_string(), // null-ish
        "[src: file: ../../📁/emoji.rs]".to_string(),
        "[src: file: ../../日本語/ファイル.rs]".to_string(), // CJK
        "[src: file: ../../\u{202E}rtl_evil.rs]".to_string(), // RTL override
        "أهلا [src: file: ../../etc/passwd] مرحبا".to_string(), // arabic around
        huge,
    ];
    for inp in &inputs {
        let r1 = analyze(inp, Some(&root));
        let r2 = analyze(inp, Some(&root));
        assert_eq!(r1, r2, "determinism broken for input: {inp:?}");
        // Whatever it found, no traversal may be Verified-as-File.
        for s in &r1.sources {
            if s.kind == SourceKind::File && s.raw.contains("..") {
                assert_ne!(
                    s.status,
                    SourceStatus::Verified,
                    "traversal source wrongly Verified: {s:?}"
                );
            }
        }
    }
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// 16. Mixed-separator / Windows-style backslash traversal on a relative path.
//     `..\..\etc\passwd` — on unix backslash is a normal filename char, so this
//     does NOT escape lexically (it's one weird filename under the root) and
//     should be NotFound, never Verified, never a green chip on /etc/passwd.
// ──────────────────────────────────────────────────────────────────────────
#[test]
fn backslash_traversal_relative_never_verifies_etc() {
    let root = temp_project();
    let st = check_file("..\\..\\etc\\passwd", &root);
    assert_ne!(
        st,
        SourceStatus::Verified,
        "backslash 'traversal' must never be Verified, got {st:?}"
    );
    // On unix it's a literal (non-existent) filename inside root → NotFound.
    assert!(
        matches!(st, SourceStatus::NotFound | SourceStatus::OutsideProject),
        "backslash ref must be NotFound or OutsideProject, got {st:?}"
    );
    cleanup(&root);
}
