use super::*;
use crate::core::citation_drift::{self, SourcesState};
use std::fs;

const LIB: &str = "fn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n\nfn gamma() {\n    let feature_flag = true;\n}\n";

const DOC: &str = "\
# Architecture
Intro with no anchor at all.

## Retries
Retries are three [src: file: src/lib.rs:2].

## Timeout
The timeout is 2.5 s [src: file: src/lib.rs:6].

## Flag
The flag lives at the end [src: file: src/lib.rs:10].

## Legacy
The old engine [src: file: src/old.rs:1] and `src/old_helpers.rs` did this.

## Layout
See `src/lib.rs` and `src/removed.rs`; an example is `path/to/file.rs`.
";

fn project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(dir.join("src/lib.rs"), LIB).unwrap();
    fs::write(dir.join("src/old.rs"), "pub fn engine() {}\n").unwrap();
    fs::write(dir.join("src/old_helpers.rs"), "pub fn help() {}\n").unwrap();
    fs::write(dir.join("src/removed.rs"), "pub fn x() {}\n").unwrap();
    fs::write(dir.join("docs/architecture.md"), DOC).unwrap();
    citation_drift::record_all(dir).unwrap();
    tmp
}

/// The code moves on: the timeout changes, the file shrinks past the flag
/// line, the legacy engine and two helpers are deleted.
fn move_code(dir: &Path) {
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 9000;\n}\n",
    )
    .unwrap();
    fs::remove_file(dir.join("src/old.rs")).unwrap();
    fs::remove_file(dir.join("src/old_helpers.rs")).unwrap();
    fs::remove_file(dir.join("src/removed.rs")).unwrap();
}

fn verdict_of<'a>(report: &'a RelevanceReport, heading: &str) -> Option<&'a SectionRelevance> {
    report.sections.iter().find(|s| s.heading == heading)
}

#[test]
fn each_section_gets_its_own_verdict_with_evidence_and_a_proposed_correction() {
    let tmp = project();
    let dir = tmp.path();
    let clean = analyze(dir, &citation_drift::check(dir));
    assert!(!clean.needs_attention());
    assert_eq!(clean.counts.still_true, 5);
    assert_eq!(clean.counts.unanchored, 1);
    assert_eq!(clean.counts.relevance_pct, Some(100));

    move_code(dir);
    let report = analyze(dir, &citation_drift::check(dir));
    let counts = &report.counts;
    assert_eq!(counts.sections, 6);
    assert_eq!(counts.still_true, 1, "only Retries still resolves");
    assert_eq!(counts.unanchored, 1);
    assert_eq!(counts.outdated, 3, "Timeout, Flag and Layout");
    assert_eq!(counts.no_longer_relevant, 1, "Legacy");
    assert_eq!(counts.relevance_pct, Some(20));
    assert_eq!(counts.untouched_stale, 4, "no one edited the doc");

    let timeout = verdict_of(&report, "Timeout").unwrap();
    assert_eq!(timeout.verdict, SectionVerdict::Outdated);
    assert_eq!(timeout.evidence[0].state, CitationState::Changed);
    assert_eq!(timeout.corrections[0].kind, CorrectionKind::Recheck);

    let flag = verdict_of(&report, "Flag").unwrap();
    assert_eq!(flag.evidence[0].state, CitationState::Removed);

    let legacy = verdict_of(&report, "Legacy").unwrap();
    assert_eq!(legacy.verdict, SectionVerdict::NoLongerRelevant);
    assert_eq!(legacy.evidence.len(), 2);
    assert!(legacy
        .corrections
        .iter()
        .any(|c| c.kind == CorrectionKind::RemoveSection));

    let layout = verdict_of(&report, "Layout").unwrap();
    assert_eq!(layout.verdict, SectionVerdict::Outdated);
    assert_eq!(layout.evidence.len(), 1);
    assert_eq!(layout.evidence[0].kind, EvidenceKind::PathReference);
    assert_eq!(layout.evidence[0].reference, "src/removed.rs");
}

#[test]
fn a_section_revised_since_validation_is_no_longer_counted_untouched() {
    let tmp = project();
    let dir = tmp.path();
    move_code(dir);
    let revised = DOC.replace(
        "The timeout is 2.5 s [src: file: src/lib.rs:6].",
        "The timeout is configurable [src: file: src/lib.rs:6].",
    );
    fs::write(dir.join("docs/architecture.md"), revised).unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    let timeout = verdict_of(&report, "Timeout").unwrap();
    assert!(!timeout.untouched_since_code_changed);
    assert!(
        verdict_of(&report, "Flag")
            .unwrap()
            .untouched_since_code_changed
    );
    assert_eq!(report.counts.untouched_stale, 3);
}

#[test]
fn a_path_a_human_validated_as_gone_is_a_deliberate_mention() {
    let tmp = project();
    let dir = tmp.path();
    let with_history =
        format!("{DOC}\n## History\nThe parser lived in `src/parser_v1.rs` until 0.3.\n");
    fs::write(dir.join("docs/architecture.md"), &with_history).unwrap();
    citation_drift::record_all(dir).unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    assert!(verdict_of(&report, "History").is_none());
    assert_eq!(report.counts.missing_references, 0);

    // A new stale mention, after the sign-off, is flagged.
    fs::write(
        dir.join("docs/architecture.md"),
        format!("{with_history}\n## Cache\nCached by `src/cache_layer.rs`.\n"),
    )
    .unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    assert_eq!(
        verdict_of(&report, "Cache").unwrap().verdict,
        SectionVerdict::NoLongerRelevant
    );
    assert!(verdict_of(&report, "History").is_none());
}

#[test]
fn the_badge_weighs_prose_paths_no_fingerprint_covers() {
    let tmp = project();
    let dir = tmp.path();
    fs::write(
        dir.join("docs/architecture.md"),
        format!("{DOC}\n## Worker\nRuns in `src/worker.rs` and `src/lib.rs`.\n"),
    )
    .unwrap();
    // Every citation still matches: the fingerprints alone call it clean.
    let drift = citation_drift::check(dir);
    assert_eq!(drift.sources_state, SourcesState::Clean);
    assert_eq!(drift.broken_count, 0);

    let result = crate::core::checksums::check_drift(dir);
    assert_eq!(result.sources_state, SourcesState::NeedsReverification);
    assert_eq!(result.broken_citations, 1);

    // Acknowledged by a person: the badge is clean again.
    citation_drift::acknowledge(dir, "docs/architecture.md").unwrap();
    let result = crate::core::checksums::check_drift(dir);
    assert_eq!(result.sources_state, SourcesState::Clean);
}

#[test]
fn headings_inside_fences_do_not_split_and_repeated_headings_stay_distinct() {
    let content = "# A\n```\n# not a heading\n```\ntext\n## Same\none\n## Same\ntwo\n";
    let keys: Vec<String> = split_sections(content).into_iter().map(|s| s.key).collect();
    assert_eq!(keys, ["A", "Same", "Same#2"]);
    assert_eq!(section_digests(content).len(), 3);
}

#[test]
fn only_repo_looking_paths_in_inline_code_are_claims() {
    assert_eq!(path_token("src/lib.rs"), Some("src/lib.rs".into()));
    assert_eq!(path_token("src/lib.rs:12-20"), Some("src/lib.rs".into()));
    for not_a_path in [
        "lib.rs",
        "/etc/hosts",
        "../x/y.rs",
        "https://x.io/a.rs",
        "src/*.rs",
        "src/dir/",
        "a b/c.rs",
        "src/v1.2",
        "~/x/y.rs",
    ] {
        assert_eq!(path_token(not_a_path), None, "{not_a_path}");
    }
}

#[test]
fn the_check_is_deterministic() {
    let tmp = project();
    let dir = tmp.path();
    move_code(dir);
    let first = analyze(dir, &citation_drift::check(dir));
    let second = analyze(dir, &citation_drift::check(dir));
    assert_eq!(first, second);
}
