use super::*;
use crate::core::document_optimization::analyze;
use std::fs;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// docroms, reduced: a text file the step cited without a line, three attempts running.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "docs/AGENTS.md", "# P\n");
    write(
        root,
        "config/packages/prod/monolog.yaml",
        "monolog:\n    handlers: {}\n",
    );
    write(root, "src/lib.rs", "one\ntwo\n");
    write(root, ".env", "TOKEN=hunter2\n");
    dir
}

const DOC: &str = "docs/inconsistencies-tech-debt.md";

#[test]
fn a_text_file_cited_without_a_line_becomes_a_flagged_backticked_path() {
    let dir = project();
    let text = "Logs go to stderr [src: file: config/packages/prod/monolog.yaml] in prod.\n";
    let demoted = demote_unverifiable(dir.path(), DOC, text).unwrap();
    assert_eq!(
        demoted.content,
        "Logs go to stderr `config/packages/prod/monolog.yaml` (unverified reference) in prod.\n"
    );
    assert_eq!(
        demoted.references,
        vec![DemotedReference {
            doc: DOC.into(),
            marker: "[src: file: config/packages/prod/monolog.yaml]".into(),
        }]
    );
}

#[test]
fn a_line_outside_the_file_and_a_missing_path_lose_their_line() {
    let dir = project();
    let text = "A [src: file: src/lib.rs:99] and B [src: file: src/gone.rs:3-4].\n";
    let demoted = demote_unverifiable(dir.path(), DOC, text).unwrap();
    assert_eq!(
        demoted.content,
        "A `src/lib.rs` (unverified reference) and B `src/gone.rs` (unverified reference).\n"
    );
    assert_eq!(demoted.references.len(), 2);
}

#[test]
fn an_accepted_marker_inline_code_and_fences_are_left_as_they_are() {
    let dir = project();
    let text = concat!(
        "Fine [src: file: src/lib.rs:2]. Quoted `[src: file: nowhere.rs]`.\n",
        "```\n[src: file: config/packages/prod/monolog.yaml]\n```\n"
    );
    assert!(demote_unverifiable(dir.path(), DOC, text).is_none());
}

#[test]
fn a_comment_inside_the_marker_follows_the_flag() {
    let dir = project();
    let text = "X [src: file: config/packages/prod/monolog.yaml — handlers are empty]\n";
    let demoted = demote_unverifiable(dir.path(), DOC, text).unwrap();
    assert_eq!(
        demoted.content,
        "X `config/packages/prod/monolog.yaml` (unverified reference) — handlers are empty\n"
    );
}

#[test]
fn a_secret_file_leaves_no_path_and_no_line_in_the_text() {
    let dir = project();
    for raw in [".env", ".env:3", ".env:3 TOKEN=hunter2"] {
        let text = format!("Set [src: file: {raw}] first.\n");
        let demoted = demote_unverifiable(dir.path(), DOC, &text).unwrap();
        assert_eq!(demoted.content, "Set  first.\n", "{raw}");
        assert!(!demoted.content.contains(".env"));
        assert!(!demoted.content.contains("hunter2"));
        assert!(!demoted.references[0].marker.contains(".env"));
    }
}

#[test]
fn a_demoted_document_is_no_longer_judged_as_holding_a_citation() {
    let dir = project();
    let text = "# T\n\nLogs [src: file: config/packages/prod/monolog.yaml] and [src: file: src/gone.rs:1].\n";
    write(dir.path(), DOC, text);
    let before = analyze(dir.path()).unwrap();
    assert!(before.blocking_diagnostics().any(|d| d.path == DOC));
    let demoted = demote_unverifiable(dir.path(), DOC, text).unwrap();
    write(dir.path(), DOC, &demoted.content);
    let after = analyze(dir.path()).unwrap();
    assert!(
        !after.blocking_diagnostics().any(|d| d.path == DOC),
        "{:?}",
        after.diagnostics
    );
}

const SYNTHETIC: &str = "SYNTHETIC_COMMENT";

/// A marker that names a secret file, in whatever shape, leaves nothing of it.
fn assert_secret_marker_removed(dir: &tempfile::TempDir, raw: &str) {
    let text = format!("Set [src: file: {raw}] first.\n");
    let demoted = demote_unverifiable(dir.path(), DOC, &text)
        .unwrap_or_else(|| panic!("{raw} is not refused by the gate"));
    assert_eq!(demoted.content, "Set  first.\n", "{raw}");
    assert_eq!(demoted.references.len(), 1, "{raw}");
    assert_eq!(demoted.references[0].marker, SECRET_MARKER, "{raw}");
    assert!(!demoted.content.contains(SYNTHETIC), "{raw}");
}

#[test]
fn a_marker_naming_a_secret_in_any_piece_or_encoding_is_removed_entirely() {
    let dir = project();
    for raw in [
        ".env:1 safe_word, SYNTHETIC_COMMENT",
        "src/lib.rs:999, .env:1 SYNTHETIC_COMMENT",
        "src/gone.rs:1, SYNTHETIC_COMMENT, .env",
        "%2eenv:1 SYNTHETIC_COMMENT",
        "%252eenv:1 SYNTHETIC_COMMENT",
        "src/%2e%2e/.env:1 SYNTHETIC_COMMENT",
        "src/lib.rs:999 SYNTHETIC_COMMENT .env",
    ] {
        assert_secret_marker_removed(&dir, raw);
    }
}

#[cfg(unix)]
#[test]
fn a_marker_resolving_to_a_secret_through_a_link_is_removed_entirely() {
    let dir = project();
    write(dir.path(), ".ssh/synthetic-key", "SYNTHETIC_KEY_BODY\n");
    std::os::unix::fs::symlink(
        "../.ssh/synthetic-key",
        dir.path().join("docs/key-view.txt"),
    )
    .unwrap();
    assert_secret_marker_removed(&dir, "docs/key-view.txt:1 SYNTHETIC_COMMENT");
    assert_secret_marker_removed(&dir, "docs/key-view.txt");

    // `keys -> .ssh -> vault`: the canonical path no longer says `.ssh`.
    let dir = project();
    write(dir.path(), "vault/review-key", "SYNTHETIC_KEY_BODY\n");
    std::os::unix::fs::symlink("vault", dir.path().join(".ssh")).unwrap();
    std::os::unix::fs::symlink(".ssh", dir.path().join("keys")).unwrap();
    assert_secret_marker_removed(&dir, "keys/review-key:1 SYNTHETIC_COMMENT");
}

#[test]
fn a_name_that_only_looks_like_a_secret_is_demoted_as_any_path() {
    let dir = project();
    write(dir.path(), "config/env.md", "# env\n");
    let demoted = demote_unverifiable(
        dir.path(),
        DOC,
        "See [src: file: config/env.md:1 the notes, src/lib.rs:999].\n",
    )
    .unwrap();
    assert!(
        demoted.content.contains("`config/env.md`"),
        "{}",
        demoted.content
    );
    assert_ne!(demoted.references[0].marker, SECRET_MARKER);
}

#[test]
fn the_copy_kept_in_kronns_state_has_no_secret_marker() {
    let dir = project();
    let text = "A [src: file: .env:1 safe_word, SYNTHETIC_COMMENT] B [src: file: src/lib.rs:1] \
                C `[src: file: .env:1 SYNTHETIC_COMMENT]`\n";
    assert_eq!(
        scrub_secret_markers(dir.path(), text),
        "A [src: file: <secret file>] B [src: file: src/lib.rs:1] \
         C `[src: file: .env:1 SYNTHETIC_COMMENT]`\n"
    );
    // A document without a secret marker is copied as it is.
    let plain = "[src: file: src/lib.rs:1]\n";
    assert_eq!(scrub_secret_markers(dir.path(), plain), plain);
}
