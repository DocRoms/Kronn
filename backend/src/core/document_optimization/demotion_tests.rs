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
