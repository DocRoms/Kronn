use super::*;
use crate::core::citation_drift::CitationState;
use crate::core::doc_provenance::{self, DocWriter, ValidationReason};
use std::fs;

const RULES: &str = "docs/coding-rules.md";

/// A project validated this morning: one doc citing the code, the fingerprints,
/// the provenance and `validated_at` all recorded.
fn validated() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries [src: file: src/lib.rs:2].\nTimeout [src: file: src/lib.rs:6].\n",
    )
    .unwrap();
    fs::write(dir.join("docs/AGENTS.md"), "# Project\nSee the rules.\n").unwrap();
    citation_drift::record_all(dir).unwrap();
    doc_provenance::record_all(dir, ValidationReason::AuditValidation).unwrap();
    kronn_state::mark_validated_by(dir, "morning", Some("opener")).unwrap();
    tmp
}

fn edit_outside_kronn(dir: &FsPath) {
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries [src: file: src/lib.rs:2].\nTimeout [src: file: src/lib.rs:6].\nNew rule added by hand.\n",
    )
    .unwrap();
}

#[test]
fn a_doc_edited_outside_kronn_is_detected_and_a_kronn_write_is_not() {
    let tmp = validated();
    let dir = tmp.path();
    assert!(doc_provenance::outside_kronn(dir).unwrap().is_empty());

    // Kronn's own write explains the difference: not "outside".
    let before = fs::read_to_string(dir.join(RULES)).unwrap();
    let after = format!("{before}Written by an audit.\n");
    fs::write(dir.join(RULES), &after).unwrap();
    doc_provenance::note_write(dir, RULES, &before, &after, DocWriter::PartialAudit).unwrap();
    assert!(doc_provenance::outside_kronn(dir).unwrap().is_empty());

    // A person edits it afterwards, and adds a doc, and removes another.
    edit_outside_kronn(dir);
    fs::write(dir.join("docs/new-td.md"), "# TD\n").unwrap();
    fs::remove_file(dir.join("docs/AGENTS.md")).unwrap();
    let outside = doc_provenance::outside_kronn(dir).unwrap();
    assert_eq!(
        outside,
        vec![
            "docs/AGENTS.md".to_string(),
            RULES.to_string(),
            "docs/new-td.md".to_string()
        ]
    );
    let report = doc_provenance::report(dir, false).unwrap();
    assert_eq!(report.outside_kronn, outside);
}

#[test]
fn a_project_with_no_provenance_record_has_nothing_outside_kronn() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs")).unwrap();
    fs::write(tmp.path().join("docs/AGENTS.md"), "# Legacy\n").unwrap();
    assert!(doc_provenance::outside_kronn(tmp.path())
        .unwrap()
        .is_empty());
}

#[test]
fn validating_this_version_records_everything_without_an_agent() {
    let tmp = validated();
    let dir = tmp.path();
    edit_outside_kronn(dir);
    fs::write(
        dir.join("docs/new-td.md"),
        "# TD\nTimeout [src: file: src/lib.rs:6].\n",
    )
    .unwrap();
    assert!(!doc_provenance::outside_kronn(dir).unwrap().is_empty());

    let outcome = validate_current_version(dir, "afternoon").unwrap();
    assert!(outcome.validated, "{outcome:?}");
    assert!(outcome.broken.is_empty());
    assert_eq!(outcome.docs, 3);

    let state = kronn_state::read(dir).unwrap();
    assert_eq!(state.validated_by.as_deref(), Some("afternoon"));
    assert_eq!(state.validation_started_by, None);
    assert_ne!(state.validated_at.as_deref(), Some("1970-01-01"));
    assert_eq!(state.validation_history.len(), 1);
    assert_eq!(
        state.validation_history[0].validated_by.as_deref(),
        Some("morning")
    );

    // The fingerprints and the provenance follow the docs as they read now.
    let citations = citation_drift::read_strict(dir).unwrap().unwrap();
    assert!(citations.docs.contains_key("docs/new-td.md"));
    let report = citation_drift::check(dir);
    assert_eq!(report.broken_count, 0);
    assert!(report.sections.iter().all(|s| !s.doc_changed));
    assert!(doc_provenance::outside_kronn(dir).unwrap().is_empty());
    assert_eq!(doc_provenance::report(dir, false).unwrap().to_reconfirm, 0);
}

#[test]
fn a_removed_doc_leaves_the_record_when_this_version_is_validated() {
    let tmp = validated();
    let dir = tmp.path();
    fs::remove_file(dir.join("docs/AGENTS.md")).unwrap();
    assert!(validate_current_version(dir, "me").unwrap().validated);
    assert!(doc_provenance::outside_kronn(dir).unwrap().is_empty());
    assert!(!doc_provenance::read_strict(dir)
        .unwrap()
        .docs
        .contains_key("docs/AGENTS.md"));
}

#[test]
fn a_moved_line_is_re_anchored_then_the_version_is_validated() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("src/lib.rs"),
        "// header\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    let outcome = validate_current_version(dir, "me").unwrap();
    assert!(outcome.validated, "{outcome:?}");
    assert_eq!(outcome.reanchored, 2);
    let rules = fs::read_to_string(dir.join(RULES)).unwrap();
    assert!(rules.contains("src/lib.rs:3]") && rules.contains("src/lib.rs:7]"));
    assert_eq!(citation_drift::check(dir).moved_count, 0);
}

#[test]
fn broken_citations_refuse_the_validation_and_list_them() {
    let tmp = validated();
    let dir = tmp.path();
    // The cited line changed, a new citation names a missing file, another is
    // past the end of the file.
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 5;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("docs/new-td.md"),
        "# TD\nGhost [src: file: src/ghost.rs:1].\nFar [src: file: src/lib.rs:90].\n",
    )
    .unwrap();
    let before = kronn_state::read(dir).unwrap();

    let outcome = validate_current_version(dir, "me").unwrap();
    assert!(!outcome.validated);
    let found: Vec<(&str, &str, CitationState)> = outcome
        .broken
        .iter()
        .map(|b| (b.doc.as_str(), b.citation.cite.as_str(), b.citation.state))
        .collect();
    assert_eq!(
        found,
        vec![
            (RULES, "src/lib.rs:2", CitationState::Changed),
            (
                "docs/new-td.md",
                "src/ghost.rs:1",
                CitationState::FileMissing
            ),
            ("docs/new-td.md", "src/lib.rs:90", CitationState::Removed),
        ]
    );

    // Nothing was recorded: the validation is exactly what it was.
    assert_eq!(kronn_state::read(dir).unwrap(), before);
}

#[test]
fn nothing_validated_means_no_validation_to_renew() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs")).unwrap();
    fs::write(tmp.path().join("docs/AGENTS.md"), "# P\n").unwrap();
    let err = validate_current_version(tmp.path(), "me").unwrap_err();
    assert!(err.contains("not validated"), "{err}");
    assert!(kronn_state::revalidate_current(tmp.path(), "me").is_err());
}

#[test]
fn an_unresolved_placeholder_refuses_the_validation() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(dir.join("docs/AGENTS.md"), "# Project\n{{TO_FILL}}\n").unwrap();
    let before = kronn_state::read(dir).unwrap();
    let err = validate_current_version(dir, "me").unwrap_err();
    assert!(err.contains("Unresolved unknowns"), "{err}");
    assert_eq!(kronn_state::read(dir).unwrap(), before);
}

#[test]
fn the_syntax_example_in_a_template_header_is_no_citation() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("docs/AGENTS.md"),
        "# Project\nEvery claim carries a [src: …] marker, or [src: ...], or [src: file: <path>:<line>].\n",
    )
    .unwrap();
    assert!(
        broken_citations(dir).is_empty(),
        "{:?}",
        broken_citations(dir)
    );
    // Recorded too: the template text is no fingerprint to go missing later.
    citation_drift::record_all(dir).unwrap();
    let recorded = citation_drift::read_strict(dir).unwrap().unwrap();
    assert!(recorded.docs["docs/AGENTS.md"].citations.is_empty());
    assert_eq!(citation_drift::check(dir).broken_count, 0);
}

#[test]
fn a_claim_that_says_a_file_does_not_exist_is_not_a_missing_file() {
    let tmp = validated();
    let dir = tmp.path();
    for phrase in [
        "The Jenkinsfile copies `config/config.int_com.yml`, which n'existe pas [src: file: config/config.int_com.yml].",
        "The Jenkinsfile copies config/config.int_com.yml, qui n\u{2019}existe pas [src: file: config/config.int_com.yml].",
        "The file does not exist: [src: file: config/config.int_com.yml].",
    ] {
        fs::write(dir.join(RULES), format!("# Rules\n{phrase}\n")).unwrap();
        // Not recorded yet: the probe of a new citation.
        assert!(broken_citations(dir).is_empty(), "{phrase}");
        // Recorded while the file was missing, then checked again.
        citation_drift::record_all(dir).unwrap();
        assert_eq!(citation_drift::check(dir).broken_count, 0, "{phrase}");
    }
    // The same citation with no such statement is still broken.
    fs::write(
        dir.join(RULES),
        "# Rules\nThe Jenkinsfile copies the settings [src: file: config/config.int_com.yml].\n",
    )
    .unwrap();
    let broken = broken_citations(dir);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0].citation.state, CitationState::FileMissing);
}

#[test]
fn the_curly_apostrophe_no_longer_turns_an_absence_into_an_outdated_claim() {
    let tmp = validated();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("config")).unwrap();
    fs::write(dir.join("config/app.yml"), "a: 1\n").unwrap();
    fs::write(
        dir.join(RULES),
        "# Rules\nThe Jenkinsfile copie `config/config.int_com.yml`, qui n\u{2019}existe pas.\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    let report = citation_drift::check(dir);
    let relevance = crate::core::doc_relevance::analyze(dir, &report);
    assert!(
        relevance.sections.iter().all(|s| s.doc != RULES),
        "{:?}",
        relevance.sections
    );
}

#[test]
fn an_internal_citation_is_checked_on_its_content_not_only_its_bounds() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("docs/target.md"),
        "# Target\n\n## Alpha\nalpha body\n\n## Beta\nbeta body\n",
    )
    .unwrap();
    fs::write(
        dir.join("docs/source.md"),
        "# Source\nBeta is described [src: file: docs/target.md:6].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    assert!(broken_citations(dir).is_empty());

    // The cited doc is edited: the heading the claim is about moved down by
    // sections. Line 6 is still inside the file; its content is not the same.
    // The content is what is compared, so the citation follows the heading.
    fs::write(
        dir.join("docs/target.md"),
        "# Target\n\n## Alpha\nalpha body\nmore alpha\nand more alpha\n\n## Gamma\ngamma body\n\n## Beta\nbeta body\n",
    )
    .unwrap();
    assert!(broken_citations(dir).is_empty());
    let moved = citation_drift::check(dir);
    assert_eq!(moved.moved_count, 1);
    assert!(validate_current_version(dir, "me").unwrap().validated);
    assert!(fs::read_to_string(dir.join("docs/source.md"))
        .unwrap()
        .contains("docs/target.md:11]"));

    // The heading it is about is gone and line 11 now says something else:
    // still in bounds, no longer the same content.
    fs::write(
        dir.join("docs/target.md"),
        "# Target\n\n## Alpha\nalpha body\nmore alpha\nand more alpha\n\n## Gamma\ngamma body\n\n## Delta\ndelta body\n",
    )
    .unwrap();
    let broken = broken_citations(dir);
    assert_eq!(
        broken
            .iter()
            .map(|b| (b.doc.as_str(), b.citation.cite.as_str(), b.citation.state))
            .collect::<Vec<_>>(),
        vec![(
            "docs/source.md",
            "docs/target.md:11",
            CitationState::Changed
        )],
        "{broken:?}"
    );

    // A citation added by hand carries no fingerprint: its anchor decides.
    fs::write(
        dir.join("docs/source.md"),
        "# Source\nBeta is described [src: file: docs/target.md:6#Beta].\n",
    )
    .unwrap();
    let unrecorded = citation_drift::unrecorded_problems(dir);
    assert!(
        unrecorded
            .iter()
            .any(|(doc, c)| doc == "docs/source.md" && c.state == CitationState::Changed),
        "{unrecorded:?}"
    );
}

#[test]
fn a_claim_that_a_file_is_used_to_do_something_does_not_hide_its_deletion() {
    let tmp = validated();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("config")).unwrap();
    for phrase in [
        "`config/auth.rs` is used to check tokens [src: file: config/auth.rs:1].",
        "`config/auth.rs` no longer needs a cache, it used to read one [src: file: config/auth.rs:1].",
        "We could split `config/auth.rs` if we add a provider [src: file: config/auth.rs:1].",
    ] {
        fs::write(dir.join("config/auth.rs"), "fn check() {}\n").unwrap();
        fs::write(dir.join(RULES), format!("# Rules\n{phrase}\n")).unwrap();
        citation_drift::record_all(dir).unwrap();
        assert!(broken_citations(dir).is_empty(), "{phrase}");
        fs::remove_file(dir.join("config/auth.rs")).unwrap();
        // Recorded, then deleted: broken for « Vérifier la doc » and here.
        assert_eq!(citation_drift::check(dir).broken_count, 1, "{phrase}");
        let broken = broken_citations(dir);
        assert_eq!(broken.len(), 1, "{phrase}: {broken:?}");
        assert_eq!(broken[0].citation.state, CitationState::FileMissing);
        // Not recorded (a citation written by hand): the probe of a new one.
        fs::remove_file(citation_drift::citations_path(dir)).unwrap();
        let unrecorded = citation_drift::unrecorded_problems(dir);
        assert_eq!(unrecorded.len(), 1, "{phrase}: {unrecorded:?}");
        assert!(!validate_current_version(dir, "me").unwrap().validated);
    }
}

#[test]
fn an_absence_said_outside_the_marker_sentence_counts_for_nothing() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join(RULES),
        "# Rules\nThe Jenkinsfile copies `config/gone.yml`. The cache does not exist. [src: file: config/gone.yml].\n",
    )
    .unwrap();
    assert_eq!(broken_citations(dir).len(), 1);
}

/// The doc texts of a present-tense citation of `src/auth.rs:3` and a statement
/// that the file does not exist, in both orders, on separate lines and in one
/// paragraph, with the statement citing the same span and another one.
fn present_and_absent_texts() -> Vec<String> {
    let present = "Runtime loads tokens from `src/auth.rs` [src: file: src/auth.rs:3].";
    let mut texts = Vec::new();
    for absent in [
        "The file does not exist [src: file: src/auth.rs].",
        "The file does not exist [src: file: src/auth.rs:3].",
        "The file does not exist [src: file: src/auth.rs:1-2].",
    ] {
        for separator in ["\n", " "] {
            texts.push(format!("# Rules\n{present}{separator}{absent}\n"));
            texts.push(format!("# Rules\n{absent}{separator}{present}\n"));
        }
    }
    texts
}

fn assert_present_tense_citation_refused(dir: &FsPath, text: &str) {
    let outcome = validate_current_version(dir, "me").unwrap();
    assert!(!outcome.validated, "{text}");
    assert!(
        outcome.broken.iter().any(|b| b.doc == RULES
            && b.citation.cite == "src/auth.rs:3"
            && b.citation.state == CitationState::FileMissing),
        "{text}: {:?}",
        outcome.broken
    );
}

#[test]
fn an_absence_sentence_does_not_exempt_a_present_tense_citation_of_the_same_file() {
    // (a) A recorded citation: its fingerprint was taken while the file existed.
    for text in present_and_absent_texts() {
        let tmp = validated();
        let dir = tmp.path();
        fs::write(dir.join("src/auth.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        fs::write(dir.join(RULES), &text).unwrap();
        citation_drift::record_all(dir).unwrap();
        fs::remove_file(dir.join("src/auth.rs")).unwrap();
        assert_present_tense_citation_refused(dir, &text);
        assert!(citation_drift::check(dir).broken_count >= 1, "{text}");
    }
    // (b) A citation added by hand, in no fingerprint record: the file is gone.
    for text in present_and_absent_texts() {
        let tmp = validated();
        let dir = tmp.path();
        fs::write(dir.join(RULES), &text).unwrap();
        assert!(
            !citation_drift::unrecorded_problems(dir).is_empty(),
            "{text}"
        );
        assert_present_tense_citation_refused(dir, &text);
    }
}

#[test]
fn every_occurrence_of_a_citation_must_say_the_file_is_gone() {
    use crate::core::doc_relevance::says_cite_gone;
    let absent = "The file does not exist [src: file: src/auth.rs:3].";
    let present = "Runtime loads tokens from `src/auth.rs` [src: file: src/auth.rs:3].";
    assert!(says_cite_gone(absent, "src/auth.rs:3"));
    assert!(says_cite_gone(
        &format!("{absent}\nThe file n'existe pas [src: file: src/auth.rs:3]."),
        "src/auth.rs:3"
    ));
    assert!(!says_cite_gone(
        &format!("{present}\n{absent}"),
        "src/auth.rs:3"
    ));
    assert!(!says_cite_gone(
        &format!("{absent}\n{present}"),
        "src/auth.rs:3"
    ));
    // The statement of another span does not speak for this one.
    assert!(!says_cite_gone(
        "The file does not exist [src: file: src/auth.rs].",
        "src/auth.rs:3"
    ));
    // The statement alone still exempts a doc that only says the file is gone.
    let tmp = validated();
    let dir = tmp.path();
    fs::write(dir.join(RULES), format!("# Rules\n{absent}\n")).unwrap();
    assert!(broken_citations(dir).is_empty());
}

#[test]
fn an_absence_about_something_else_does_not_hide_the_deletion_of_the_cited_file() {
    let tmp = validated();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    for phrase in [
        "`src/a.rs` lit la config, le cache n'existe pas encore [src: file: src/a.rs:1].",
        "`src/a.rs` reads the config, the cache does not exist yet [src: file: src/a.rs:1].",
        "The loader in `src/a.rs` drops entries that do not exist [src: file: src/a.rs:1].",
    ] {
        fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        fs::write(dir.join(RULES), format!("# Rules\n{phrase}\n")).unwrap();
        citation_drift::record_all(dir).unwrap();
        assert!(broken_citations(dir).is_empty(), "{phrase}");
        fs::remove_file(dir.join("src/a.rs")).unwrap();
        let broken = broken_citations(dir);
        assert_eq!(broken.len(), 1, "{phrase}: {broken:?}");
        assert_eq!(broken[0].citation.state, CitationState::FileMissing);
        fs::remove_file(citation_drift::citations_path(dir)).unwrap();
        assert_eq!(
            citation_drift::unrecorded_problems(dir).len(),
            1,
            "{phrase}"
        );
    }
    // The statement about the cited path itself stays filtered, also in a list.
    for phrase in [
        "The Jenkinsfile copies `src/a.rs`, which does not exist [src: file: src/a.rs].",
        "`src/b.rs` and `src/a.rs` n\u{2019}existent pas [src: file: src/a.rs].",
    ] {
        fs::write(dir.join(RULES), format!("# Rules\n{phrase}\n")).unwrap();
        assert!(broken_citations(dir).is_empty(), "{phrase}");
    }
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_read_refuses_the_validation_before_any_write() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = validated();
    let dir = tmp.path();
    let provenance = doc_provenance::provenance_path(dir);
    fs::set_permissions(&provenance, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read(&provenance).is_ok();
    let rules = fs::read(dir.join(RULES)).unwrap();
    let result = Snapshot::take(dir);
    fs::set_permissions(&provenance, fs::Permissions::from_mode(0o644)).unwrap();
    if readable {
        return; // running with a privilege that ignores the mode
    }
    let err = result.err().expect("an unreadable file fails the snapshot");
    assert!(err.contains("provenance"), "{err}");
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), rules);
}

#[test]
fn a_rollback_that_cannot_restore_a_file_names_it() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("src/lib.rs"),
        "// header\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    let err = validate_with(dir, "me", |project, _| {
        // The doc is replaced by a directory: the rollback cannot write it back.
        fs::remove_file(project.join(RULES)).unwrap();
        fs::create_dir(project.join(RULES)).unwrap();
        Err("disk full".to_string())
    })
    .unwrap_err();
    assert!(err.starts_with("disk full"), "{err}");
    assert!(err.contains("could not restore"), "{err}");
    assert!(err.contains(RULES), "{err}");
    // No temp file is left next to the doc by the failed restore.
    let leftovers: Vec<_> = fs::read_dir(dir.join("docs"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_rollback_restores_the_doc_with_its_mode() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("src/lib.rs"),
        "// header\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join(RULES), fs::Permissions::from_mode(0o640)).unwrap();
    }
    let before = fs::read(dir.join(RULES)).unwrap();
    assert!(validate_with(dir, "me", |_, _| Err("disk full".to_string())).is_err());
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join(RULES)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }
}

#[test]
fn a_failed_write_leaves_the_docs_and_the_records_as_they_were() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("src/lib.rs"),
        "// header\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    let files = [
        dir.join(RULES),
        citation_drift::citations_path(dir),
        doc_provenance::provenance_path(dir),
        kronn_state::state_path(dir),
    ];
    let before: Vec<Vec<u8>> = files.iter().map(|f| fs::read(f).unwrap()).collect();

    // The last write fails, after the docs were re-anchored and the
    // fingerprints and the provenance re-recorded.
    let err = validate_with(dir, "me", |_, _| Err("disk full".to_string())).unwrap_err();
    assert_eq!(err, "disk full");
    let after: Vec<Vec<u8>> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    assert_eq!(before, after);
    // Still drifted for « Vérifier la doc »: nothing was laundered.
    assert_eq!(citation_drift::check(dir).moved_count, 2);
    assert_eq!(
        kronn_state::read(dir).unwrap().validated_by.as_deref(),
        Some("morning")
    );

    // Run again for real: it goes through.
    assert!(validate_current_version(dir, "me").unwrap().validated);
}

#[test]
fn an_unreadable_provenance_refuses_before_any_write() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join("src/lib.rs"),
        "// header\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    fs::write(doc_provenance::provenance_path(dir), "not json").unwrap();
    let rules = fs::read(dir.join(RULES)).unwrap();
    let citations = fs::read(citation_drift::citations_path(dir)).unwrap();
    let state = kronn_state::read(dir).unwrap();
    let err = validate_current_version(dir, "me").unwrap_err();
    assert!(err.contains("malformed"), "{err}");
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), rules);
    assert_eq!(
        fs::read(citation_drift::citations_path(dir)).unwrap(),
        citations
    );
    assert_eq!(kronn_state::read(dir).unwrap(), state);
}

#[test]
fn a_malformed_citation_record_refuses_before_any_write() {
    let tmp = validated();
    let dir = tmp.path();
    // The cited line changed and still exists: with the record read as empty
    // it would be accepted without comparison to its former fingerprint.
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 9;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    fs::write(citation_drift::citations_path(dir), "not json").unwrap();
    let rules = fs::read(dir.join(RULES)).unwrap();
    let provenance = fs::read(doc_provenance::provenance_path(dir)).unwrap();
    let state = kronn_state::read(dir).unwrap();
    let err = validate_current_version(dir, "me").unwrap_err();
    assert!(err.contains("malformed"), "{err}");
    assert_eq!(
        fs::read(citation_drift::citations_path(dir)).unwrap(),
        b"not json"
    );
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), rules);
    assert_eq!(
        fs::read(doc_provenance::provenance_path(dir)).unwrap(),
        provenance
    );
    assert_eq!(kronn_state::read(dir).unwrap(), state);
}

#[test]
fn a_different_missing_path_with_the_same_ending_does_not_hide_the_deletion() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(dir.join("src/auth.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    let phrase = "Runtime reads tokens, while missing `old/src/auth.rs` is noted [src: file: src/auth.rs:3].";
    // The statement is about `old/src/auth.rs`, not about the cited file.
    assert!(!crate::core::doc_relevance::says_cite_gone(
        phrase,
        "src/auth.rs:3"
    ));
    assert!(crate::core::doc_relevance::says_cite_gone(
        "The tool reads missing `old/src/auth.rs` [src: file: old/src/auth.rs].",
        "old/src/auth.rs"
    ));
    fs::write(dir.join(RULES), format!("# Rules\n{phrase}\n")).unwrap();
    citation_drift::record_all(dir).unwrap();
    assert!(broken_citations(dir).is_empty());
    fs::remove_file(dir.join("src/auth.rs")).unwrap();
    let broken = broken_citations(dir);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0].citation.state, CitationState::FileMissing);
    fs::remove_file(citation_drift::citations_path(dir)).unwrap();
    assert_eq!(citation_drift::unrecorded_problems(dir).len(), 1);
    assert!(!validate_current_version(dir, "me").unwrap().validated);

    // A marker that names a longer path is not a marker of the shorter one.
    assert!(!crate::core::doc_relevance::says_cite_gone(
        "Missing `src/auth.rs`, which does not exist [src: file: old/src/auth.rs].",
        "src/auth.rs"
    ));
}

#[test]
fn a_changed_or_second_symbol_anchor_on_cited_lines_is_checked() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join(RULES),
        "# Rules\nAlpha [src: file: src/lib.rs:1#alpha].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    assert!(broken_citations(dir).is_empty());

    // The recorded citation keeps its lines; only its anchor is now wrong.
    fs::write(
        dir.join(RULES),
        "# Rules\nAlpha [src: file: src/lib.rs:1#gamma].\n",
    )
    .unwrap();
    let broken = broken_citations(dir);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0].citation.cite, "src/lib.rs:1");
    assert_eq!(broken[0].citation.state, CitationState::Changed);
    assert!(!validate_current_version(dir, "me").unwrap().validated);

    // The same lines cited under two anchors: each one is checked, whatever
    // the order, and one that holds does not stand for the other.
    for body in [
        "Alpha [src: file: src/lib.rs:1#alpha] and [src: file: src/lib.rs:1#gamma].",
        "Alpha [src: file: src/lib.rs:1#gamma] and [src: file: src/lib.rs:1#alpha].",
    ] {
        fs::write(dir.join(RULES), format!("# Rules\n{body}\n")).unwrap();
        assert_eq!(broken_citations(dir).len(), 1, "{body}");
    }
    fs::write(
        dir.join(RULES),
        "# Rules\nAlpha [src: file: src/lib.rs:1#alpha] and [src: file: src/lib.rs:1#alpha].\n",
    )
    .unwrap();
    assert!(broken_citations(dir).is_empty());
}

#[test]
fn a_moved_block_whose_symbol_anchor_is_missing_is_refused() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries [src: file: src/lib.rs:1-3#alpha].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    assert!(broken_citations(dir).is_empty());

    // The block only moved down by two lines, and the doc names a symbol the
    // new lines do not hold: the fingerprint says "moved", the anchor says no.
    fs::write(
        dir.join("src/lib.rs"),
        "// a\n// b\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
    assert_eq!(citation_drift::check(dir).moved_count, 1);
    assert!(
        broken_citations(dir).is_empty(),
        "the anchor holds on lines 3-5"
    );
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries [src: file: src/lib.rs:1-3#gamma].\n",
    )
    .unwrap();
    let broken = broken_citations(dir);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0].citation.cite, "src/lib.rs:1-3");
    assert_eq!(broken[0].citation.state, CitationState::Changed);
    let rules = fs::read(dir.join(RULES)).unwrap();
    let refused = validate_current_version(dir, "me").unwrap();
    assert!(!refused.validated);
    assert_eq!(refused.broken.len(), 1);
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), rules);
}

/// `alpha` moved down four lines AND its body changed: the claim "retries is 3"
/// is stale, whatever the unique-symbol re-anchor suggests.
fn move_and_change_alpha(dir: &FsPath) {
    fs::write(
        dir.join("src/lib.rs"),
        "// a\n// b\n// c\n// d\nfn alpha() {\n    let retries = 5;\n}\n\nfn beta() {\n    let timeout_ms = 2500;\n}\n",
    )
    .unwrap();
}

#[test]
fn a_moved_and_modified_block_is_not_accepted_as_a_move() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries equal 3 [src: file: src/lib.rs:1-3#alpha].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    assert!(broken_citations(dir).is_empty());

    move_and_change_alpha(dir);
    let broken = broken_citations(dir);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0].citation.cite, "src/lib.rs:1-3");
    assert_eq!(broken[0].citation.state, CitationState::Changed);

    // Refused with nothing rewritten, nothing recorded, the claim still reads 1-3.
    let rules = fs::read(dir.join(RULES)).unwrap();
    let before = kronn_state::read(dir).unwrap();
    let refused = validate_current_version(dir, "me").unwrap();
    assert!(!refused.validated);
    assert_eq!(refused.reanchored, 0);
    assert_eq!(refused.broken.len(), 1);
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), rules);
    assert_eq!(kronn_state::read(dir).unwrap(), before);
}

#[test]
fn a_second_invalid_anchor_on_a_moved_and_modified_block_is_refused_too() {
    let tmp = validated();
    let dir = tmp.path();
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries equal 3 [src: file: src/lib.rs:1-3#alpha].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();

    // The doc also cites the same range under an anchor the code never held.
    move_and_change_alpha(dir);
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries equal 3 [src: file: src/lib.rs:1-3#alpha] and [src: file: src/lib.rs:1-3#gamma].\n",
    )
    .unwrap();
    let rules = fs::read(dir.join(RULES)).unwrap();
    let before = kronn_state::read(dir).unwrap();
    let refused = validate_current_version(dir, "me").unwrap();
    assert!(!refused.validated, "{refused:?}");
    assert!(!refused.broken.is_empty());
    assert_eq!(fs::read(dir.join(RULES)).unwrap(), rules);
    assert_eq!(kronn_state::read(dir).unwrap(), before);

    // Once the claim is rewritten to the new content and the block is cited
    // where it now stands, with one anchor that holds, it is accepted.
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries equal 5 [src: file: src/lib.rs:5-7#alpha].\n",
    )
    .unwrap();
    let accepted = validate_current_version(dir, "me").unwrap();
    assert!(accepted.validated, "{accepted:?}");
}

#[test]
fn an_absence_names_the_cited_path_with_a_line_span_or_a_dot_slash() {
    use crate::core::doc_relevance::says_cite_gone;
    // The span after the path is part of the same name.
    assert!(says_cite_gone(
        "The loader reads `src/a.rs:3`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
    assert!(says_cite_gone(
        "The loader reads `src/a.rs:3-9`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
    assert!(says_cite_gone(
        "The loader reads `./src/a.rs`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
    assert!(says_cite_gone(
        "The loader reads `./src/a.rs:3`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
    // A longer path, with or without a span, is another file.
    assert!(!says_cite_gone(
        "The loader reads `old/src/a.rs:3`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
    assert!(!says_cite_gone(
        "The loader reads `./old/src/a.rs`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
    // A colon followed by something that is not a span is not a span.
    assert!(!says_cite_gone(
        "The loader reads `src/a.rs:x`, which does not exist [src: file: src/a.rs].",
        "src/a.rs"
    ));
}

#[test]
fn only_a_human_validates_a_version() {
    assert!(
        crate::core::bridge_token::route_for("POST", "/api/projects/{id}/validate-current-docs")
            .is_none(),
        "an agent token must not reach the validation of a version"
    );
}

#[tokio::test]
async fn a_bridge_caller_is_refused_the_validation_of_a_version() {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let caller = crate::core::bridge_token::BridgeCaller {
        token_id: "t".into(),
        project: None,
        own_discussions: vec![],
        own_run: None,
        agent: None,
    };
    let Json(response) = validate_current_docs(
        State(state),
        Path("p1".into()),
        Some(axum::Extension(caller)),
    )
    .await;
    assert!(!response.success);
    assert!(response.data.is_none());
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Only a human")),
        "{:?}",
        response.error
    );
}
