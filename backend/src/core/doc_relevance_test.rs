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
See `src/lib.rs` and `src/removed.rs`; never `path/to/file.rs`.
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
    assert_eq!(clean.counts.anchors_intact, 5);
    assert_eq!(clean.counts.unanchored, 1);
    assert_eq!(clean.counts.anchored_intact_pct, Some(100));

    move_code(dir);
    let report = analyze(dir, &citation_drift::check(dir));
    let counts = &report.counts;
    assert_eq!(counts.sections, 6);
    assert_eq!(counts.anchors_intact, 1, "only Retries still resolves");
    assert_eq!(counts.unanchored, 1);
    assert_eq!(report.unanchored[0].heading, "Architecture");
    assert_eq!(counts.outdated, 3, "Timeout, Flag and Layout");
    assert_eq!(counts.no_longer_relevant, 1, "Legacy");
    assert_eq!(counts.anchored_intact_pct, Some(20));
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
fn a_validation_does_not_accept_a_vanished_path_nobody_was_shown() {
    let tmp = project();
    let dir = tmp.path();
    let with_history = format!("{DOC}\n## History\nThe parser lives in `src/parser_v1.rs`.\n");
    fs::write(dir.join("docs/architecture.md"), &with_history).unwrap();
    // The first full validation sees the vanished path and absorbs nothing.
    citation_drift::record_all(dir).unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    let history = verdict_of(&report, "History").expect("still flagged after a validation");
    assert_eq!(history.verdict, SectionVerdict::NoLongerRelevant);
    assert_eq!(report.counts.prose_missing, 1);
    assert_eq!(
        crate::core::checksums::check_drift(dir).sources_state,
        SourcesState::NeedsReverification
    );
}

#[test]
fn an_acknowledged_path_is_accepted_per_section_and_survives_revalidation() {
    let tmp = project();
    let dir = tmp.path();
    let doc = "docs/architecture.md";
    let with_history = format!("{DOC}\n## History\nThe parser lives in `src/parser_v1.rs`.\n");
    fs::write(dir.join(doc), &with_history).unwrap();
    citation_drift::record_all(dir).unwrap();
    citation_drift::acknowledge(dir, doc).unwrap();

    // The acknowledgement was the human's decision: the mention is accepted.
    let report = analyze(dir, &citation_drift::check(dir));
    assert!(verdict_of(&report, "History").is_none());
    let accepted = &citation_drift::read_strict(dir).unwrap().unwrap().docs[doc].known_missing;
    assert_eq!(accepted["History"], ["src/parser_v1.rs"]);

    // A Revalidation record and a new full validation carry it over: the
    // accepted mention does not come back for the agent to remove.
    citation_drift::record_docs(
        dir,
        &[doc.to_string()],
        citation_drift::RecordReason::Revalidation,
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    assert!(verdict_of(&report, "History").is_none());
    assert_eq!(report.counts.prose_missing, 0);

    // The same path named in ANOTHER section was not shown: it is flagged.
    fs::write(
        dir.join(doc),
        format!(
            "{with_history}\n## Cache\nCached by `src/parser_v1.rs` and `src/cache_layer.rs`.\n"
        ),
    )
    .unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    let cache = verdict_of(&report, "Cache").unwrap();
    assert_eq!(cache.evidence.len(), 2);
    assert!(verdict_of(&report, "History").is_none());
}

#[test]
fn a_doc_added_after_validation_is_cleared_by_the_acknowledgement() {
    let tmp = project();
    let dir = tmp.path();
    fs::write(
        dir.join("docs/added.md"),
        "# Added\nRuns in `src/vanished.rs`.\n",
    )
    .unwrap();
    // Never recorded, yet judged: the badge is raised.
    assert_eq!(
        crate::core::checksums::check_drift(dir).sources_state,
        SourcesState::NeedsReverification
    );
    // Not in the drift baseline, not in citations.json: still clearable.
    crate::core::checksums::acknowledge_stale(dir, &["docs/added.md".to_string()]).unwrap();
    assert_eq!(
        crate::core::checksums::check_drift(dir).sources_state,
        SourcesState::Clean
    );
    assert!(crate::core::checksums::acknowledge_stale(dir, &[]).is_err());
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
    assert_eq!(result.broken_citations, 0, "citations are counted apart");
    assert_eq!(result.missing_paths, 1);

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

// ─── False positives: a path is gone only when it resolves nowhere ───────

fn flagged(report: &RelevanceReport) -> Vec<String> {
    report
        .sections
        .iter()
        .flat_map(|s| s.evidence.iter().map(|e| e.reference.clone()))
        .collect()
}

fn doc_with(dir: &Path, doc: &str, body: &str) -> RelevanceReport {
    let path = dir.join(doc);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    analyze(dir, &citation_drift::check(dir))
}

#[test]
fn a_path_written_relative_to_a_sub_package_is_present() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    // Kronn has a root `lib/` (shell) and a `frontend/src/lib/` (TypeScript).
    fs::create_dir_all(dir.join("lib")).unwrap();
    fs::write(dir.join("lib/kronn.sh"), "#!/bin/sh\n").unwrap();
    fs::create_dir_all(dir.join("frontend/src/lib")).unwrap();
    fs::write(dir.join("frontend/src/lib/routes.ts"), "export {};\n").unwrap();
    let report = doc_with(
        dir,
        "docs/coding-rules.md",
        "# TS\nRoutes live in `lib/routes.ts`; the shell in `lib/kronn.sh`; \
         a real miss is `lib/gone.ts`.\n",
    );
    assert_eq!(flagged(&report), ["lib/gone.ts"]);
}

#[test]
fn a_path_relative_to_the_docs_own_folder_is_present() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::create_dir_all(dir.join("docs/architecture/assets")).unwrap();
    fs::write(dir.join("docs/architecture/assets/flow.svg"), "<svg/>").unwrap();
    let report = doc_with(
        dir,
        "docs/architecture/overview.md",
        "# Flow\nSee `assets/flow.svg`.\n",
    );
    assert!(flagged(&report).is_empty());
    assert_eq!(report.counts.anchors_intact, 1);
}

#[test]
fn generated_and_git_ignored_outputs_are_not_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(init.status.success());
    fs::write(dir.join(".gitignore"), "out/\n").unwrap();
    fs::create_dir_all(dir.join("out")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    let report = doc_with(
        dir,
        "docs/ops.md",
        "# Outputs\nKronn writes `docs/briefing.md` and `docs/checksums.json`; \
         the build leaves `out/report.json`; the real miss is `docs/removed.md`.\n",
    );
    assert_eq!(flagged(&report), ["docs/removed.md"]);
}

#[test]
fn changelog_and_release_docs_are_history_not_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    let body = "# 0.3\nAdded `src/gone.rs`.\n";
    let report = doc_with(dir, "docs/releases/CHANGELOG-legacy.md", body);
    assert_eq!(report.counts.sections, 0, "not analyzed at all");
    assert!(report.sections.is_empty());
    assert!(!has_stale_prose_paths(
        dir,
        "docs/releases/CHANGELOG-legacy.md",
        body
    ));
    // The same text in an ordinary doc is a claim.
    assert!(has_stale_prose_paths(dir, "docs/design.md", body));
    for historical in [
        "docs/releases/0.14.1-fluidity-audit.md",
        "docs/CHANGELOG.md",
        "docs/release-notes-archive.md",
        "docs/architecture/layout-history.md",
    ] {
        assert!(is_historical_doc(historical), "{historical}");
    }
    for ordinary in ["docs/architecture/overview.md", "docs/repo-map.md"] {
        assert!(!is_historical_doc(ordinary), "{ordinary}");
    }
}

#[test]
fn a_line_that_tells_history_is_not_a_claim() {
    assert!(tells_history(
        "The former hot files `a/b.rs` are now modules."
    ));
    assert!(tells_history("`x/y.rs` was removed in 0.9."));
    assert!(tells_history(
        "`x/y.rs` n'existe plus, remplacé par `x/z.rs`."
    ));
    assert!(!tells_history(
        "The performer lives in `src/legacy/engine.rs`."
    ));
    assert!(!tells_history("Handled by `src/engine.rs`."));

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    let report = doc_with(
        dir,
        "docs/repo-map.md",
        "# Notes\nThe former `src/old_api.rs` is now a module.\nIt is `src/new_api.rs`.\n",
    );
    assert_eq!(flagged(&report), ["src/new_api.rs"]);
}

#[test]
fn fenced_paths_are_ignored_and_a_path_cited_and_named_counts_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
    fs::write(dir.join("src/gone.rs"), "fn b() {}\n").unwrap();
    fs::write(
        dir.join("docs/rules.md"),
        "# Fence\n```\nsee `src/fenced_gone.rs`\n```\nSee [src: file: src/lib.rs:1].\n\
         # Both\nUsed in `src/gone.rs` [src: file: src/gone.rs:1].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    fs::remove_file(dir.join("src/gone.rs")).unwrap();
    let report = analyze(dir, &citation_drift::check(dir));
    let both = verdict_of(&report, "Both").unwrap();
    assert_eq!(both.evidence.len(), 1, "cited and named: one anchor");
    assert_eq!(both.evidence[0].kind, EvidenceKind::Citation);
    assert_eq!(report.counts.missing_references, 1);
    assert_eq!(report.counts.prose_missing, 0);
    assert!(verdict_of(&report, "Fence").is_none());
}

#[test]
fn unanchored_sections_are_listed_with_a_check_or_anchor_proposal() {
    let tmp = project();
    let dir = tmp.path();
    let report = analyze(dir, &citation_drift::check(dir));
    assert_eq!(report.counts.unanchored, 1);
    let first = &report.unanchored[0];
    assert_eq!(first.verdict, SectionVerdict::Unanchored);
    assert_eq!(first.corrections[0].kind, CorrectionKind::CheckOrAnchor);
    // They cannot be measured, so they never raise the badge by themselves.
    assert!(!report.needs_attention());
    assert_eq!(report.escalate(SourcesState::Clean), SourcesState::Clean);
    // A heading with nothing under it is not a section to judge.
    let empty = doc_with(dir, "docs/empty.md", "# Only a title\n## Also empty\n");
    assert_eq!(empty.counts.sections, report.counts.sections);
}

#[test]
fn a_citation_the_diff_maps_to_a_new_line_is_not_stale() {
    let tmp = project();
    let dir = tmp.path();
    move_code(dir);
    let report = citation_drift::check(dir);
    // Without the diff mapping the changed line is outdated...
    let plain = analyze(dir, &report);
    assert_eq!(
        verdict_of(&plain, "Timeout").unwrap().verdict,
        SectionVerdict::Outdated
    );
    // ...with it, Kronn re-anchors the citation: nothing is stale.
    let mut moved = BTreeMap::new();
    moved.insert(
        "docs/architecture.md".to_string(),
        BTreeSet::from(["src/lib.rs:6".to_string()]),
    );
    let mapped = analyze_with_moves(dir, &report, &moved);
    assert!(verdict_of(&mapped, "Timeout").is_none());
    assert_eq!(mapped.counts.outdated, plain.counts.outdated - 1);
    assert_eq!(
        mapped.counts.anchors_intact,
        plain.counts.anchors_intact + 1
    );
}

#[test]
fn an_unfilled_template_doc_and_on_demand_docs_are_not_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    // The example paths of a template still holding its placeholders.
    let report = doc_with(
        dir,
        "docs/inconsistencies-tech-debt.md",
        "# Tech debt\nExample: `src/config.rs:42`.\n| {{COMPONENT}} | {{VERSION}} |\n",
    );
    assert!(flagged(&report).is_empty());
    // `docs/linked-repos.md` exists only once a repo is linked.
    let report = doc_with(
        dir,
        "docs/AGENTS.md",
        "# Agents\nRead `docs/linked-repos.md` only for cross-repo tasks.\n",
    );
    assert!(flagged(&report).is_empty());
}

#[test]
fn absence_proposals_and_naming_patterns_are_not_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::create_dir_all(dir.join("scripts")).unwrap();
    let report = doc_with(
        dir,
        "docs/tech-debt/TD-1.md",
        "# Problem\nThe cite `docs/missing_a.md` resolved to nothing.\n\
         Wrap it in `scripts/lint.sh` if we add one.\n\
         Detail files are `docs/tech-debt/TD-YYYYMMDD-slug.md`.\n\
         Budgets live in `docs/.kronn-document-budgets.json`.\n\
         ## Suggested direction\nTrack it in `docs/api-coverage.md`.\n\
         ## Current state\nTracked in `docs/real_gone.md`.\n",
    );
    assert_eq!(flagged(&report), ["docs/real_gone.md"]);
}

#[test]
fn an_example_and_the_list_under_it_illustrate_a_format_not_the_repo() {
    assert!(tells_example(
        "**Detail file example** (`tech-debt/TD-1.md`):"
    ));
    assert!(tells_example("Use paths like `src/a.rs`, e.g. `src/b.rs`."));
    assert!(!tells_example("The `src/examples/run.rs` binary."));

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    let report = doc_with(
        dir,
        "docs/inconsistencies-tech-debt.md",
        "# Tech debt\nTrack-only list.\n\n**Detail file example**:\n\
         - **Problem**: key in `src/config.rs:42`\n\
         - **Where**: `src/payments/client.rs:15`\n\n\
         After the blank line, `src/real_gone.rs` is a claim again.\n",
    );
    assert_eq!(flagged(&report), ["src/real_gone.rs"]);
}

/// Run on demand against the repository's own docs:
/// `cargo nextest run --offline --run-ignored only kronn_own_docs_report`.
/// It prints the remaining prose-path hits so false positives can be audited.
#[test]
#[ignore = "report over this repository's docs, not a regression test"]
fn kronn_own_docs_report() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let report = analyze(&root, &citation_drift::check(&root));
    let mut hits: Vec<String> = Vec::new();
    for s in report.sections.iter() {
        for e in s
            .evidence
            .iter()
            .filter(|e| e.kind == EvidenceKind::PathReference)
        {
            hits.push(format!("{} > {} > {}", s.doc, s.heading, e.reference));
        }
    }
    eprintln!("counts: {:?}", report.counts);
    eprintln!("prose path hits: {}", hits.len());
    for hit in &hits {
        eprintln!("  {hit}");
    }
}
