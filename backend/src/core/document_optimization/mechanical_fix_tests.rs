use super::*;
use std::fs;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// The repository the apollo and africanews evidence describes, reduced.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "docs/AGENTS.md", "# P\n");
    write(root, "docs/operations/http.md", "# Http\n");
    write(root, "docs/repo-map.md", &"line\n".repeat(60));
    write(
        root,
        ".ssh/vr-v2njs01-s",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n",
    );
    write(
        root,
        "docs/tech-debt/TD-20260828-apache-root-user.md",
        "# TD\n\nProblem.\n",
    );
    write(root, "notes.md", "one\ntwo\n");
    // 26 lines, no closing newline: line 27 does not exist.
    write(
        root,
        "tests/galleries.test.js",
        &vec!["it();"; 26].join("\n"),
    );
    dir
}

fn repair(root: &Path, rel: &str, text: &str) -> Option<Repaired> {
    mechanical_repair(root, rel, text)
}

#[test]
fn a_link_resolved_from_the_repository_root_is_rewritten_from_the_documents_folder() {
    // Apollo step 10: "broken link target `docs/operations/` … write `operations`".
    let dir = project();
    let text = "See [ops](docs/operations/) and [page](docs/operations/http.md#top).\n";
    let fixed = repair(dir.path(), "docs/inconsistencies-security.md", text).unwrap();
    assert_eq!(
        fixed.content,
        "See [ops](operations) and [page](operations/http.md#top).\n"
    );
    assert_eq!(fixed.fixes.len(), 2);
    assert!(fixed.fixes.iter().all(|f| f.kind == "link_base"));
    // The link that resolves already, and an external one, are left alone.
    assert!(repair(
        dir.path(),
        "docs/inconsistencies-security.md",
        "[ok](operations/http.md) [web](https://example.com/docs/operations/)\n"
    )
    .is_none());
}

#[test]
fn a_secret_file_cited_bare_gets_its_presence_form() {
    // Apollo step 10: "this file may hold secrets … write [src: file: .ssh/x:exists]".
    let dir = project();
    let text = "Key [src: file: .ssh/vr-v2njs01-s] is tracked.\n";
    let fixed = repair(
        dir.path(),
        "docs/tech-debt/TD-20261018-ssh-key-tracked.md",
        text,
    )
    .unwrap();
    assert_eq!(
        fixed.content,
        "Key [src: file: .ssh/vr-v2njs01-s:exists] is tracked.\n"
    );
    assert_eq!(fixed.fixes[0].kind, "secret_presence");
    assert_eq!(fixed.fixes[0].from, ".ssh/vr-v2njs01-s");
}

#[test]
fn a_tech_debt_sheet_cited_bare_is_a_presence_citation() {
    // Africanews steps 8 and 10: the index cites the sheets by bare path.
    let dir = project();
    let text = "Root user: [src: file: docs/tech-debt/TD-20260828-apache-root-user.md] \
                and [src: file: docs/tech-debt/TD-20260828-apache-root-user.md:2].\n";
    let fixed = repair(dir.path(), "docs/inconsistencies-tech-debt.md", text).unwrap();
    assert_eq!(
        fixed.content,
        "Root user: [src: file: docs/tech-debt/TD-20260828-apache-root-user.md:exists] \
         and [src: file: docs/tech-debt/TD-20260828-apache-root-user.md:2].\n"
    );
    // A text file that is not a sheet has several readings: not applied.
    assert!(repair(dir.path(), "docs/AGENTS.md", "[src: file: notes.md]\n").is_none());
}

#[test]
fn a_single_did_you_mean_path_is_applied_and_several_are_not() {
    // Apollo step 5: "citation path does not exist `repo-map.md` … did you mean
    // `docs/repo-map.md:36-50`?".
    let dir = project();
    let fixed = repair(
        dir.path(),
        "docs/testing-quality.md",
        "[src: file: repo-map.md:36-50]\n",
    )
    .unwrap();
    assert_eq!(fixed.content, "[src: file: docs/repo-map.md:36-50]\n");
    assert_eq!(fixed.fixes[0].kind, "did_you_mean_path");

    // Two files could be meant (docs root and the citing folder): ambiguous.
    write(dir.path(), "docs/sub/map.md", &"x\n".repeat(10));
    write(dir.path(), "docs/map.md", &"y\n".repeat(10));
    assert!(repair(dir.path(), "docs/sub/page.md", "[src: file: map.md:3]\n").is_none());
    // No candidate holds the lines: nothing to propose.
    assert!(repair(
        dir.path(),
        "docs/testing-quality.md",
        "[src: file: repo-map.md:80]\n"
    )
    .is_none());
}

#[test]
fn a_range_one_line_past_the_end_is_clamped_and_a_far_one_is_not() {
    let dir = project();
    let doc = "docs/coding-rules.md";
    let fixed = repair(
        dir.path(),
        doc,
        "[src: file: tests/galleries.test.js:20-27] [src: file: tests/galleries.test.js:27]\n",
    )
    .unwrap();
    // Line 27 alone starts past the end: nothing to clamp it to.
    assert_eq!(
        fixed.content,
        "[src: file: tests/galleries.test.js:20-26] [src: file: tests/galleries.test.js:27]\n"
    );
    assert_eq!(fixed.fixes.len(), 1);
    assert_eq!(fixed.fixes[0].kind, "line_range_clamp");
    assert!(repair(
        dir.path(),
        doc,
        "[src: file: tests/galleries.test.js:1-30]\n"
    )
    .is_none());
    // A newline-terminated file reads one more, empty, line: that is valid.
    write(dir.path(), "tests/closed.js", "a\nb\n");
    assert!(repair(dir.path(), doc, "[src: file: tests/closed.js:1-3]\n").is_none());
}

#[test]
fn ambiguous_or_quoted_references_are_never_touched() {
    let dir = project();
    let doc = "docs/AGENTS.md";
    for text in [
        "[src: file: notes.md:1, .ssh/vr-v2njs01-s]\n",
        "[src: file: .ssh/vr-v2njs01-s — the tracked key]\n",
        "```md\n[src: file: .ssh/vr-v2njs01-s]\n[bad](docs/operations/)\n```\n",
        "Write `[src: file: .ssh/vr-v2njs01-s]` and `[x](docs/operations/)`.\n",
        "[src: file: docs/operations:1]\n",
        "[src: file: invented.rs:1]\n",
    ] {
        assert!(repair(dir.path(), doc, text).is_none(), "{text}");
    }
    // A quoted example next to a live reference: only the live one changes.
    let fixed = repair(
        dir.path(),
        doc,
        "Write `[src: file: .ssh/vr-v2njs01-s]`; here [src: file: .ssh/vr-v2njs01-s].\n",
    )
    .unwrap();
    assert_eq!(
        fixed.content,
        "Write `[src: file: .ssh/vr-v2njs01-s]`; here [src: file: .ssh/vr-v2njs01-s:exists].\n"
    );
}
