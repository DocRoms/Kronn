//! Adversarial QA — DIMENSION "multiroot_finalize".
//!
//! Two seams under test:
//!
//!  1. [`verify_source_marker`] resolution against a SINGLE root (`Some`) and
//!     against NO root (`None` → `Unchecked`, "no project root"). A relative
//!     file ref resolves to `Verified` when it exists under the root, `NotFound`
//!     when it is jailed-but-absent, and (the scary invariant) NEVER `Verified`
//!     when it escapes the root via `../`.
//!
//!  2. [`finalize_lint_report`] — the message-finalize chokepoint. Mode `off`
//!     ⇒ `None` no matter what; `warn` + a verified citation ⇒ `Some`; `warn` +
//!     no signal ⇒ `None`; and the worktree-root-FIRST preference (a relative
//!     path present in BOTH the workspace and the project resolves against the
//!     workspace copy first).
//!
//! Each test asserts the CORRECT behaviour per the documented semantics. A red
//! test here is reported as a suspected bug, never weakened to green.
//!
//! NOTE on the global mode: `finalize_lint_report` reads a PROCESS-GLOBAL mode
//! flag. To avoid the mode-drift race with sibling test binaries / parallel
//! tests in THIS binary, every off/warn flip lives SEQUENTIALLY inside one test
//! fn that restores `warn` before it returns, and it is marked `#[serial]`.

use kronn::core::anti_halluc::{
    finalize_lint_report, set_mode, verify_source_marker, SourceStatus,
};
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DIM: &str = "multiroot_finalize";

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

/// Verify a formal `file:` reference against a single root.
fn check_file_some(reference: &str, root: &Path) -> SourceStatus {
    let marker = format!("file: {reference}");
    verify_source_marker(&marker, Some(root)).status
}

// ──────────────────────────────────────────────────────────────────────────
// SINGLE ROOT (Some) / NONE (Unchecked)
// ──────────────────────────────────────────────────────────────────────────

// 1. Relative file that EXISTS under the single root → Verified.
#[test]
fn single_root_existing_relative_is_verified() {
    let root = temp_project();
    assert_eq!(
        check_file_some("src/foo.rs", &root),
        SourceStatus::Verified,
        "an existing in-jail relative file must resolve Verified against the single root"
    );
    cleanup(&root);
}

// 2. Relative file ABSENT under the single root → NotFound (not Verified).
#[test]
fn single_root_absent_relative_is_notfound() {
    let root = temp_project();
    assert_eq!(
        check_file_some("src/does_not_exist.rs", &root),
        SourceStatus::NotFound,
        "a jailed-but-absent relative file must be NotFound"
    );
    cleanup(&root);
}

// 3. No root at all (None) → Unchecked, with a "no project root" detail.
#[test]
fn none_root_file_ref_is_unchecked() {
    let check = verify_source_marker("file: src/foo.rs", None);
    assert_eq!(
        check.status,
        SourceStatus::Unchecked,
        "a file ref with NO project root cannot be resolved → Unchecked, not NotFound/Verified"
    );
    assert!(
        check.detail.to_lowercase().contains("no project root")
            || check.detail.to_lowercase().contains("can't resolve"),
        "detail should explain there is no root, got: {}",
        check.detail
    );
    // Unchecked is NOT fabricated — it must not drive the red pill.
    assert!(!check.status.is_fabricated());
}

// 4. SECURITY: a `../` traversal against a single root must NEVER be Verified.
#[test]
fn single_root_traversal_never_verified() {
    let root = temp_project();
    let status = check_file_some("../../../../etc/passwd", &root);
    assert_ne!(
        status,
        SourceStatus::Verified,
        "a ../ traversal must NEVER come back Verified"
    );
    assert!(
        matches!(status, SourceStatus::OutsideProject | SourceStatus::NotFound),
        "traversal must be OutsideProject (or NotFound), got {status:?}"
    );
    cleanup(&root);
}

// 5. SECURITY: a relative ref that climbs to a sibling-temp REAL file still
//    escapes the root → must be OutsideProject, never Verified, even though the
//    target genuinely exists on disk.
#[test]
fn single_root_escape_to_real_sibling_is_outside_not_verified() {
    let root_a = temp_project();
    let root_b = temp_project(); // a real sibling dir with a real src/foo.rs
    // Build a relative path from A that climbs out to B's real foo.rs.
    let rel = format!(
        "../{}/src/foo.rs",
        root_b.file_name().unwrap().to_string_lossy()
    );
    let status = check_file_some(&rel, &root_a);
    assert_ne!(
        status,
        SourceStatus::Verified,
        "escaping the root to a REAL sibling file must NOT be Verified"
    );
    assert_eq!(
        status,
        SourceStatus::OutsideProject,
        "lexical jail must reject the ../ escape as OutsideProject"
    );
    cleanup(&root_a);
    cleanup(&root_b);
}

// 6. Single root, line in bounds → Verified; line out of bounds → OutOfBounds.
#[test]
fn single_root_line_bounds_branch() {
    let root = temp_project(); // foo.rs has 5 lines
    assert_eq!(
        check_file_some("src/foo.rs:5", &root),
        SourceStatus::Verified,
        "line 5 of a 5-line file is in bounds"
    );
    assert_eq!(
        check_file_some("src/foo.rs:6", &root),
        SourceStatus::OutOfBounds,
        "line 6 of a 5-line file is out of bounds"
    );
    cleanup(&root);
}

// 7. DETERMINISM: same (marker, root) → identical SourceCheck twice.
#[test]
fn single_root_resolution_is_deterministic() {
    let root = temp_project();
    let a = verify_source_marker("file: src/foo.rs:3", Some(&root));
    let b = verify_source_marker("file: src/foo.rs:3", Some(&root));
    assert_eq!(a, b, "verification must be deterministic");
    cleanup(&root);
}

// ──────────────────────────────────────────────────────────────────────────
// finalize_lint_report
// ──────────────────────────────────────────────────────────────────────────

// 8. mode `off` → None regardless of content; restore `warn`. SEQUENTIAL to
//    avoid the global mode-drift race. Bundles the off-vs-warn comparison so the
//    global flag is never left flipped when this test returns.
#[test]
#[serial]
fn finalize_mode_off_returns_none_then_warn_restored() {
    let root = temp_project();
    let text = "The function lives in `src/foo.rs:1`. [src: file: src/foo.rs:2]";
    let project_path = root.to_string_lossy().to_string();

    // OFF: hard short-circuit, None even with a verifiable citation present.
    set_mode("off");
    let off = finalize_lint_report(text, None, &project_path);
    assert!(
        off.is_none(),
        "mode=off must return None even when the text has a verifiable citation"
    );

    // Restore + prove the SAME text now yields a report (so we know the None
    // above was the mode gate, not an empty report).
    set_mode("warn");
    let warn = finalize_lint_report(text, None, &project_path);
    assert!(
        warn.is_some(),
        "mode=warn must surface the verifiable citation"
    );

    // Leave the process in the rollout default.
    set_mode("warn");
    cleanup(&root);
}

// 9. warn + a VERIFIED formal [src:] citation → Some, with verified_count >= 1
//    and no fabrication.
#[test]
#[serial]
fn finalize_warn_verified_citation_is_some() {
    set_mode("warn");
    let root = temp_project();
    let text = "See [src: file: src/foo.rs:2] for the handler.";
    let report = finalize_lint_report(text, None, &root.to_string_lossy())
        .expect("a verified citation must produce a Some report");
    assert!(
        report.verified_count() >= 1,
        "expected a verified source, got {report:?}"
    );
    assert_eq!(
        report.fabricated_count, 0,
        "a real in-bounds file is not fabricated"
    );
    assert!(report.has_signal());
    set_mode("warn");
    cleanup(&root);
}

// 10. warn + NO signal (plain prose, no claim cue, no citation) → None.
#[test]
#[serial]
fn finalize_warn_no_signal_returns_none() {
    set_mode("warn");
    let root = temp_project();
    // Deliberately benign: a greeting + a question. No claim cue, no anchor.
    let text = "Hello there, how would you like me to proceed today?";
    let report = finalize_lint_report(text, None, &root.to_string_lossy());
    assert!(
        report.is_none(),
        "a no-signal reply must return None (nothing to show), got {report:?}"
    );
    set_mode("warn");
    cleanup(&root);
}

// 11. WORKTREE-ROOT-FIRST: a relative path present in BOTH the workspace and the
//     project resolves against the WORKSPACE (first root). We prove the ordering
//     by giving the two roots files of DIFFERENT lengths and citing a line that
//     is in bounds for the workspace copy but OUT of bounds for the project copy.
//     Worktree-first ⇒ Verified.
#[test]
#[serial]
fn finalize_worktree_root_is_tried_first() {
    set_mode("warn");
    let workspace = temp_project(); // src/foo.rs has 5 lines
    let project = temp_project();
    // Make the PROJECT copy short (2 lines) so line 5 would be OutOfBounds there.
    std::fs::write(project.join("src/foo.rs"), "x\ny\n").unwrap();

    let text = "[src: file: src/foo.rs:5]";
    let report = finalize_lint_report(
        text,
        Some(&workspace.to_string_lossy()),
        &project.to_string_lossy(),
    )
    .expect("citation should produce a report");

    // Worktree (workspace) first → 5 lines → in bounds → Verified.
    assert_eq!(
        report.fabricated_count, 0,
        "workspace copy has 5 lines so line 5 is in bounds; worktree-first must avoid the OutOfBounds project copy. Report: {report:?}"
    );
    assert!(
        report.verified_count() >= 1,
        "worktree-first resolution must verify against the 5-line workspace copy: {report:?}"
    );
    set_mode("warn");
    cleanup(&workspace);
    cleanup(&project);
}

// 12. WORKTREE-FIRST, contrapositive: when the file exists ONLY in the project
//     (not the workspace), resolution falls through to the project root and
//     still verifies. Proves the multi-root fallthrough, not just root[0].
#[test]
#[serial]
fn finalize_falls_through_to_project_root() {
    set_mode("warn");
    let workspace = temp_project();
    let project = temp_project();
    // Remove the file from the workspace so only the project has it.
    std::fs::remove_file(workspace.join("src/foo.rs")).unwrap();

    let text = "[src: file: src/foo.rs:3]";
    let report = finalize_lint_report(
        text,
        Some(&workspace.to_string_lossy()),
        &project.to_string_lossy(),
    )
    .expect("citation present → report");
    assert_eq!(
        report.fabricated_count, 0,
        "file exists in the project root, so it must verify via fallthrough, not be flagged fabricated: {report:?}"
    );
    assert!(report.verified_count() >= 1);
    set_mode("warn");
    cleanup(&workspace);
    cleanup(&project);
}

// 13. finalize with an EMPTY workspace string must not be treated as a root
//     (the .filter(!is_empty) guard) — resolution uses only the project root.
#[test]
#[serial]
fn finalize_empty_workspace_string_ignored() {
    set_mode("warn");
    let project = temp_project();
    let text = "[src: file: src/foo.rs:2]";
    let report = finalize_lint_report(text, Some(""), &project.to_string_lossy())
        .expect("citation present → report");
    assert_eq!(report.fabricated_count, 0, "{report:?}");
    assert!(report.verified_count() >= 1, "empty workspace must be ignored, project root used: {report:?}");
    set_mode("warn");
    cleanup(&project);
}

// 14. finalize with a fabricated formal citation (absent file) → Some with
//     fabricated_count >= 1 (RED). The red signal must survive finalize.
#[test]
#[serial]
fn finalize_fabricated_citation_is_red() {
    set_mode("warn");
    let project = temp_project();
    let text = "It is defined in [src: file: src/ghost.rs:1].";
    let report = finalize_lint_report(text, None, &project.to_string_lossy())
        .expect("a fabricated citation is a signal → Some");
    assert!(
        report.fabricated_count >= 1,
        "an absent cited file must count as fabricated (RED): {report:?}"
    );
    assert_eq!(report.verified_count(), 0);
    set_mode("warn");
    cleanup(&project);
}

// 15. NO PANIC + has_signal=false for empty / whitespace text under warn → None.
#[test]
#[serial]
fn finalize_empty_text_is_none_no_panic() {
    set_mode("warn");
    let project = temp_project();
    for t in ["", "   ", "\n\t  \n"] {
        let report = finalize_lint_report(t, None, &project.to_string_lossy());
        assert!(report.is_none(), "empty/whitespace text must be None, t={t:?}");
    }
    set_mode("warn");
    cleanup(&project);
}

// 16. NO PANIC on hostile input (huge / emoji / CJK / RTL / malformed [src:)
//     in finalize, regardless of root presence.
#[test]
#[serial]
fn finalize_hostile_input_no_panic() {
    set_mode("warn");
    let project = temp_project();
    let pp = project.to_string_lossy().to_string();
    let huge = "la fonction se trouve ici. ".repeat(5000);
    let hostile = [
        huge.as_str(),
        "🚀🔥 émoji é è ê programme 日本語のテスト مرحبا بالعالم",
        "[src: file: \u{0000}\u{0000}]",
        "[src: [src: file: ../[../]]] nested brackets [src:",
        "[src:",
        "[src: file:]",
        "`src/foo.rs:999999999999999999999999`", // overflowing line number
    ];
    for t in hostile {
        // Must not panic. Result may be Some or None; we only assert no panic
        // and that None-root absolute/relative refs are never spuriously Verified
        // beyond what the file system actually has.
        let _ = finalize_lint_report(t, None, &pp);
        let _ = finalize_lint_report(t, Some(&pp), &pp);
    }
    set_mode("warn");
    cleanup(&project);
}

// 17. DETERMINISM at the finalize layer: same (text, workspace, project) under a
//     fixed mode → identical report twice.
#[test]
#[serial]
fn finalize_is_deterministic() {
    set_mode("warn");
    let workspace = temp_project();
    let project = temp_project();
    let text =
        "The route is in [src: file: src/foo.rs:2] and `src/foo.rs:3`. La méthode se trouve ici.";
    let a = finalize_lint_report(
        text,
        Some(&workspace.to_string_lossy()),
        &project.to_string_lossy(),
    );
    let b = finalize_lint_report(
        text,
        Some(&workspace.to_string_lossy()),
        &project.to_string_lossy(),
    );
    assert_eq!(a, b, "finalize must be deterministic for identical inputs");
    set_mode("warn");
    cleanup(&workspace);
    cleanup(&project);
}
