use super::*;
use std::fs;

const HUMAN_DOC: &str = "# AGENTS\n\
<!-- kronn:section name=\"stack\" curated=\"ai\" owner=\"audit\" -->\n\
Rust backend.\n\
<!-- kronn:section:end -->\n\
\n\
<!-- kronn:section name=\"team\" curated=\"human\" owner=\"human\" -->\n\
Ask Romu before merging.\n\
<!-- kronn:section:end -->\n";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();
    fs::write(dir.path().join("docs/AGENTS.md"), "# AGENTS\nIndex.\n").unwrap();
    fs::write(dir.path().join("docs/repo-map.md"), "# Map\nline two\n").unwrap();
    dir
}

fn state_of(root: &Path, doc: &str) -> ProvenanceState {
    report(root, false)
        .unwrap()
        .docs
        .into_iter()
        .find(|d| d.doc == doc)
        .unwrap_or_else(|| panic!("{doc} has no record"))
        .state
}

/// The automatic writer's own write, then its note: the write always lands.
fn kronn_writes(root: &Path, doc: &str, after: &str, writer: DocWriter) -> bool {
    let before = fs::read_to_string(root.join(doc)).unwrap_or_default();
    fs::write(root.join(doc), after).unwrap();
    note_write(root, doc, &before, after, writer).unwrap()
}

#[test]
fn audit_validation_marks_every_doc_human_validated() {
    let dir = project();
    record_all(dir.path(), ValidationReason::AuditValidation).unwrap();
    let report = report(dir.path(), false).unwrap();
    assert_eq!(report.to_reconfirm, 0);
    let docs: Vec<_> = report.docs.iter().map(|d| d.doc.as_str()).collect();
    assert_eq!(docs, ["docs/AGENTS.md", "docs/repo-map.md"]);
    assert!(report
        .docs
        .iter()
        .all(|d| d.state == ProvenanceState::HumanValidated
            && d.reason == ValidationReason::AuditValidation
            && d.scope == ProvenanceScope::Doc
            && !d.validated_at.is_empty()));
    // Sidecar only: the prose carries no marker.
    assert_eq!(
        fs::read_to_string(dir.path().join("docs/repo-map.md")).unwrap(),
        "# Map\nline two\n"
    );
    assert!(dir.path().join("docs").join(PROVENANCE_FILENAME).is_file());
}

#[test]
fn acknowledging_one_doc_validates_only_that_doc() {
    let dir = project();
    record_validation(
        dir.path(),
        &["docs/repo-map.md".to_string()],
        ValidationReason::Acknowledged,
    )
    .unwrap();
    let report = report(dir.path(), false).unwrap();
    assert_eq!(report.docs.len(), 1);
    assert_eq!(report.docs[0].reason, ValidationReason::Acknowledged);
}

#[test]
fn a_kronn_write_into_a_validated_doc_lands_and_is_marked_with_its_diff() {
    let dir = project();
    record_all(dir.path(), ValidationReason::AuditValidation).unwrap();
    let marked = kronn_writes(
        dir.path(),
        "docs/repo-map.md",
        "# Map\nline two, corrected\n",
        DocWriter::FullAudit,
    );
    assert!(marked);
    assert_eq!(
        fs::read_to_string(dir.path().join("docs/repo-map.md")).unwrap(),
        "# Map\nline two, corrected\n",
        "the write is applied, never reverted"
    );
    let report = report(dir.path(), true).unwrap();
    assert_eq!(report.to_reconfirm, 1);
    let doc = report
        .docs
        .iter()
        .find(|d| d.doc == "docs/repo-map.md")
        .unwrap();
    assert_eq!(doc.state, ProvenanceState::ToReconfirm);
    assert_eq!(doc.kronn_writes.len(), 1);
    assert_eq!(doc.kronn_writes[0].writer, DocWriter::FullAudit);
    assert_eq!(doc.changes.len(), 1);
    assert_eq!(doc.changes[0].before.as_deref(), Some("# Map\nline two\n"));
    assert_eq!(
        doc.changes[0].after.as_deref(),
        Some("# Map\nline two, corrected\n")
    );
    // The untouched doc stays human-validated.
    assert_eq!(
        state_of(dir.path(), "docs/AGENTS.md"),
        ProvenanceState::HumanValidated
    );
}

#[test]
fn an_edit_outside_kronn_also_asks_for_reconfirmation_without_a_writer() {
    let dir = project();
    record_all(dir.path(), ValidationReason::AuditValidation).unwrap();
    fs::write(dir.path().join("docs/repo-map.md"), "# Map\nby hand\n").unwrap();
    let report = report(dir.path(), false).unwrap();
    let doc = report
        .docs
        .iter()
        .find(|d| d.doc == "docs/repo-map.md")
        .unwrap();
    assert_eq!(doc.state, ProvenanceState::ToReconfirm);
    assert!(doc.kronn_writes.is_empty());
}

#[test]
fn reconfirmation_refuses_unresolved_templates_and_todos_atomically() {
    let dir = project();
    record_all(dir.path(), ValidationReason::AuditValidation).unwrap();
    for bad in [
        "# {{PROJECT_NAME}}\n",
        "# Map\n<!-- TODO: ask the owner -->\n",
    ] {
        fs::write(dir.path().join("docs/repo-map.md"), bad).unwrap();
        fs::write(
            dir.path().join("docs/AGENTS.md"),
            "# Valid changed content\n",
        )
        .unwrap();
        let before = fs::read(dir.path().join("docs").join(PROVENANCE_FILENAME)).unwrap();
        let error = confirm(dir.path(), &[]).unwrap_err();
        assert!(error.contains("docs/repo-map.md"), "{error}");
        assert_eq!(
            fs::read(dir.path().join("docs").join(PROVENANCE_FILENAME)).unwrap(),
            before
        );
        assert_eq!(report(dir.path(), false).unwrap().to_reconfirm, 2);
    }
    fs::write(
        dir.path().join("docs/repo-map.md"),
        "# Map\nThe example `{{PROJECT_NAME}}` is literal.\n",
    )
    .unwrap();
    assert_eq!(confirm(dir.path(), &[]).unwrap().len(), 2);
    assert_eq!(report(dir.path(), false).unwrap().to_reconfirm, 0);
}

#[test]
fn a_write_into_an_owner_human_section_lands_and_records_the_human_text_as_before() {
    let dir = project();
    fs::write(dir.path().join("docs/AGENTS.md"), HUMAN_DOC).unwrap();
    let after = HUMAN_DOC.replace("Ask Romu before merging.", "Ask the lead before merging.");
    assert!(kronn_writes(
        dir.path(),
        "docs/AGENTS.md",
        &after,
        DocWriter::PartialAudit
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("docs/AGENTS.md")).unwrap(),
        after
    );
    let report = report(dir.path(), true).unwrap();
    let doc = &report.docs[0];
    assert_eq!(doc.reason, ValidationReason::HumanOwned);
    assert_eq!(doc.scope, ProvenanceScope::Sections);
    assert_eq!(doc.sections, ["team"]);
    assert_eq!(doc.state, ProvenanceState::ToReconfirm);
    let change = &doc.changes[0];
    assert_eq!(change.section.as_deref(), Some("team"));
    assert!(change.before.as_deref().unwrap().contains("Ask Romu"));
    assert!(change.after.as_deref().unwrap().contains("Ask the lead"));
}

#[test]
fn a_write_outside_human_sections_of_an_unvalidated_doc_marks_nothing() {
    let dir = project();
    fs::write(dir.path().join("docs/AGENTS.md"), HUMAN_DOC).unwrap();
    let after = HUMAN_DOC.replace("Rust backend.", "Rust + axum backend.");
    assert!(!kronn_writes(
        dir.path(),
        "docs/AGENTS.md",
        &after,
        DocWriter::FullAudit
    ));
    assert!(report(dir.path(), false).unwrap().docs.is_empty());
}

#[test]
fn a_removed_human_section_is_marked_with_no_after() {
    let dir = project();
    fs::write(dir.path().join("docs/AGENTS.md"), HUMAN_DOC).unwrap();
    let after = "# AGENTS\n";
    assert!(kronn_writes(
        dir.path(),
        "docs/AGENTS.md",
        after,
        DocWriter::TemplateReinstall
    ));
    let doc = &report(dir.path(), true).unwrap().docs[0];
    assert_eq!(doc.changes[0].after, None);
    assert!(doc.changes[0].before.is_some());
}

#[test]
fn confirm_restores_the_provenance_and_clears_the_writes() {
    let dir = project();
    record_all(dir.path(), ValidationReason::AuditValidation).unwrap();
    kronn_writes(
        dir.path(),
        "docs/repo-map.md",
        "# Map\nnew\n",
        DocWriter::Revalidation,
    );
    assert_eq!(
        state_of(dir.path(), "docs/repo-map.md"),
        ProvenanceState::ToReconfirm
    );
    let confirmed = confirm(dir.path(), &["docs/repo-map.md".to_string()]).unwrap();
    assert_eq!(confirmed, ["docs/repo-map.md"]);
    let report = report(dir.path(), false).unwrap();
    let doc = report
        .docs
        .iter()
        .find(|d| d.doc == "docs/repo-map.md")
        .unwrap();
    assert_eq!(doc.state, ProvenanceState::HumanValidated);
    assert_eq!(doc.reason, ValidationReason::Confirmed);
    let file = read_strict(dir.path()).unwrap();
    assert!(file.docs["docs/repo-map.md"].kronn_writes.is_empty());
    assert_eq!(report.to_reconfirm, 0);
}

#[test]
fn confirm_with_no_doc_confirms_every_doc_to_reconfirm_in_its_scope() {
    let dir = project();
    fs::write(dir.path().join("docs/AGENTS.md"), HUMAN_DOC).unwrap();
    let after = HUMAN_DOC.replace("Ask Romu", "Ask Ana");
    kronn_writes(
        dir.path(),
        "docs/AGENTS.md",
        &after,
        DocWriter::DocumentRepair,
    );
    assert_eq!(confirm(dir.path(), &[]).unwrap(), ["docs/AGENTS.md"]);
    let doc = &report(dir.path(), false).unwrap().docs[0];
    assert_eq!(doc.state, ProvenanceState::HumanValidated);
    assert_eq!(doc.scope, ProvenanceScope::Sections);
}

#[test]
fn confirm_refuses_a_doc_never_validated() {
    let dir = project();
    assert!(confirm(dir.path(), &["docs/repo-map.md".to_string()]).is_err());
}

#[test]
fn learning_counts_a_source_by_its_doc_provenance() {
    let dir = project();
    let root = dir.path();
    assert_eq!(source_provenance(root, "src/main.rs:1"), None);
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::AiGenerated),
        "never validated"
    );
    record_all(root, ValidationReason::AuditValidation).unwrap();
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::HumanValidated)
    );
    let absolute = format!("{}:2", root.join("docs/repo-map.md").display());
    assert_eq!(
        source_provenance(root, &absolute),
        Some(SourceProvenance::HumanValidated)
    );
    kronn_writes(
        root,
        "docs/repo-map.md",
        "# Map\nline 2 rewritten\n",
        DocWriter::FullAudit,
    );
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::AiGenerated),
        "modified since validation"
    );
    confirm(root, &[]).unwrap();
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::HumanValidated),
        "reconfirmed"
    );
}

#[test]
fn a_validated_doc_naming_a_vanished_path_is_no_longer_human_validated() {
    let dir = project();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/engine.rs"), "pub fn run() {}\n").unwrap();
    fs::write(
        root.join("docs/repo-map.md"),
        "# Map\nThe engine is `src/engine.rs`.\n",
    )
    .unwrap();
    record_all(root, ValidationReason::AuditValidation).unwrap();
    crate::core::citation_drift::record_all(root).unwrap();
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::HumanValidated)
    );

    // The code the validated prose describes is gone: nothing was edited, no
    // citation broke, yet the human did not see this: AI-generated again.
    // No citation report is passed: the answer must be the learning gate's.
    fs::remove_file(root.join("src/engine.rs")).unwrap();
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::AiGenerated)
    );
    let report = crate::core::citation_drift::check(root);
    assert_eq!(
        source_provenance_with_citations(root, "docs/repo-map.md:2", Some(&report)),
        Some(SourceProvenance::AiGenerated)
    );

    // The human acknowledges the mention: trusted as validated again.
    crate::core::citation_drift::acknowledge(root, "docs/repo-map.md").unwrap();
    assert_eq!(
        source_provenance(root, "docs/repo-map.md:2"),
        Some(SourceProvenance::HumanValidated)
    );
}

#[test]
fn learning_reads_owner_human_lines_as_human_until_kronn_changes_them() {
    let dir = project();
    let root = dir.path();
    fs::write(root.join("docs/AGENTS.md"), HUMAN_DOC).unwrap();
    // Line 7 is in `team` (human), line 3 in `stack` (audit).
    assert_eq!(
        source_provenance(root, "docs/AGENTS.md:7"),
        Some(SourceProvenance::HumanValidated)
    );
    assert_eq!(
        source_provenance(root, "docs/AGENTS.md:3"),
        Some(SourceProvenance::AiGenerated)
    );
    let after = HUMAN_DOC.replace("Ask Romu", "Ask Ana");
    kronn_writes(root, "docs/AGENTS.md", &after, DocWriter::FullAudit);
    assert_eq!(
        source_provenance(root, "docs/AGENTS.md:7"),
        Some(SourceProvenance::AiGenerated)
    );
}

#[test]
fn a_malformed_record_is_an_error_not_a_silent_reset() {
    let dir = project();
    fs::write(
        dir.path().join("docs").join(PROVENANCE_FILENAME),
        "{not json",
    )
    .unwrap();
    let err = note_write(
        dir.path(),
        "docs/repo-map.md",
        "a",
        "b",
        DocWriter::FullAudit,
    )
    .unwrap_err();
    assert!(err.contains("malformed"), "{err}");
}

#[test]
fn snapshot_tracks_recorded_and_human_docs_and_notes_their_writes() {
    let dir = project();
    let root = dir.path();
    fs::write(root.join("docs/AGENTS.md"), HUMAN_DOC).unwrap();
    record_validation(
        root,
        &["docs/repo-map.md".to_string()],
        ValidationReason::ValidationCard,
    )
    .unwrap();
    fs::write(root.join("docs/glossary.md"), "# Glossary\n").unwrap();
    let snapshot = snapshot_tracked(root).unwrap();
    let docs: Vec<_> = snapshot.iter().map(|(d, _)| d.as_str()).collect();
    assert_eq!(
        docs,
        ["docs/AGENTS.md", "docs/glossary.md", "docs/repo-map.md"]
    );
    fs::write(root.join("docs/repo-map.md"), "# Map\nchanged\n").unwrap();
    fs::write(
        root.join("docs/AGENTS.md"),
        HUMAN_DOC.replace("Ask Romu", "Ask Ana"),
    )
    .unwrap();
    fs::write(root.join("docs/glossary.md"), "# Glossary\nmore\n").unwrap();
    assert_eq!(
        note_snapshot_writes(root, &snapshot, DocWriter::FullAudit).unwrap(),
        3
    );
    assert_eq!(report(root, false).unwrap().to_reconfirm, 3);
}

#[test]
fn originals_no_record_points_to_are_pruned() {
    let dir = project();
    let root = dir.path();
    record_all(root, ValidationReason::AuditValidation).unwrap();
    let first = read_strict(root).unwrap().docs["docs/repo-map.md"]
        .doc_sha256
        .clone();
    assert!(crate::core::audit_state::read_validated_original(root, &first).is_some());
    fs::write(root.join("docs/repo-map.md"), "# Map\nv2\n").unwrap();
    confirm(root, &[]).unwrap();
    assert!(crate::core::audit_state::read_validated_original(root, &first).is_none());
}

#[test]
fn a_path_outside_the_docs_is_never_recorded() {
    let dir = project();
    fs::write(dir.path().join("README.md"), "x").unwrap();
    record_validation(
        dir.path(),
        &["README.md".to_string(), "../escape.md".to_string()],
        ValidationReason::Acknowledged,
    )
    .unwrap();
    assert!(report(dir.path(), false).unwrap().docs.is_empty());
}

#[test]
fn human_text_added_after_validation_is_the_next_write_baseline() {
    for writer in [
        DocWriter::FullAudit,
        DocWriter::PartialAudit,
        DocWriter::Revalidation,
        DocWriter::TemplateReinstall,
        DocWriter::DocumentRepair,
        DocWriter::TdIndex,
    ] {
        let dir = project();
        let root = dir.path();
        record_all(root, ValidationReason::AuditValidation).unwrap();
        let human = "# Map\nline two\n\nDécision humaine: préserver les données.\n";
        fs::write(root.join("docs/repo-map.md"), human).unwrap();
        let snapshot = snapshot_tracked(root).unwrap();
        fs::write(root.join("docs/repo-map.md"), "# Map\nline two\n").unwrap();
        note_snapshot_writes(root, &snapshot, writer).unwrap();
        let report = report(root, true).unwrap();
        let doc = report
            .docs
            .iter()
            .find(|doc| doc.doc == "docs/repo-map.md")
            .unwrap();
        assert_eq!(doc.state, ProvenanceState::ToReconfirm, "{writer:?}");
        assert_eq!(doc.changes[0].before.as_deref(), Some(human), "{writer:?}");
        assert_eq!(doc.kronn_writes[0].writer, writer);
    }
}

#[test]
fn pending_losses_survive_human_edits_and_whole_audit_validation() {
    let dir = project();
    let root = dir.path();
    record_all(root, ValidationReason::AuditValidation).unwrap();
    kronn_writes(root, "docs/repo-map.md", "# Map\n", DocWriter::FullAudit);
    let human = "# Map\nNew human rule.\n";
    fs::write(root.join("docs/repo-map.md"), human).unwrap();
    let snapshot = snapshot_tracked(root).unwrap();
    fs::remove_file(root.join("docs/repo-map.md")).unwrap();
    note_snapshot_writes(root, &snapshot, DocWriter::PartialAudit).unwrap();
    record_all(root, ValidationReason::AuditValidation).unwrap();
    let status = report(root, true).unwrap();
    let doc = status
        .docs
        .iter()
        .find(|doc| doc.doc == "docs/repo-map.md")
        .unwrap();
    assert_eq!(doc.state, ProvenanceState::ToReconfirm);
    assert!(doc.changes.iter().all(|change| change.after.is_none()));
    assert!(doc
        .changes
        .iter()
        .any(|change| change.before.as_deref() == Some(human)));
    assert!(doc
        .changes
        .iter()
        .any(|change| change.before.as_deref() == Some("# Map\nline two\n")));
    confirm(root, &[]).unwrap();
    assert_eq!(report(root, true).unwrap().to_reconfirm, 0);
}

#[test]
fn successive_kronn_writes_do_not_turn_an_agent_output_into_a_human_baseline() {
    let dir = project();
    let root = dir.path();
    record_all(root, ValidationReason::AuditValidation).unwrap();
    for output in ["# First rewrite\n", "# Second rewrite\n"] {
        let snapshot = snapshot_tracked(root).unwrap();
        fs::write(root.join("docs/repo-map.md"), output).unwrap();
        note_snapshot_writes(root, &snapshot, DocWriter::FullAudit).unwrap();
    }
    record_all(root, ValidationReason::AuditValidation).unwrap();
    let status = report(root, true).unwrap();
    let doc = status
        .docs
        .iter()
        .find(|doc| doc.doc == "docs/repo-map.md")
        .unwrap();
    assert_eq!(doc.changes.len(), 1);
    assert_eq!(doc.changes[0].before.as_deref(), Some("# Map\nline two\n"));
    assert_eq!(doc.changes[0].after.as_deref(), Some("# Second rewrite\n"));
}

#[test]
fn snapshot_covers_unrecorded_nested_docs_and_reports_in_a_custom_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    crate::core::docs_location::set_project_override(&root.to_string_lossy(), Some("context"));
    fs::create_dir_all(root.join("context/reports/deep")).unwrap();
    fs::write(
        root.join("context/reports/deep/human.md"),
        "Do not remove this paragraph.\n",
    )
    .unwrap();
    for excluded in ["legacy", "var"] {
        fs::create_dir_all(root.join("context").join(excluded)).unwrap();
        fs::write(
            root.join("context").join(excluded).join("ignored.md"),
            [0xff],
        )
        .unwrap();
    }
    let snapshot = snapshot_tracked(root).unwrap();
    assert_eq!(snapshot.len(), 1);
    fs::remove_file(root.join("context/reports/deep/human.md")).unwrap();
    note_snapshot_writes(root, &snapshot, DocWriter::FullAudit).unwrap();
    let status = report(root, true).unwrap();
    assert_eq!(status.to_reconfirm, 1);
    assert_eq!(
        status.docs[0].changes[0].before.as_deref(),
        Some("Do not remove this paragraph.\n")
    );
    assert_eq!(status.docs[0].changes[0].after, None);
}

#[test]
fn generated_docs_only_need_reconfirmation_after_a_human_changes_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let snapshot = snapshot_tracked(root).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/AGENTS.md"), "# {{PROJECT_NAME}}\n").unwrap();
    note_snapshot_writes(root, &snapshot, DocWriter::TemplateReinstall).unwrap();
    let snapshot = snapshot_tracked(root).unwrap();
    fs::write(root.join("docs/AGENTS.md"), "# Generated project\n").unwrap();
    note_snapshot_writes(root, &snapshot, DocWriter::FullAudit).unwrap();
    // Retries and the final mechanical pass still use the pre-step snapshot.
    fs::write(
        root.join("docs/AGENTS.md"),
        "# Generated project\nStamped by Kronn.\n",
    )
    .unwrap();
    note_snapshot_writes(root, &snapshot, DocWriter::FullAudit).unwrap();
    assert_eq!(report(root, true).unwrap().to_reconfirm, 0);
    fs::write(
        root.join("docs/AGENTS.md"),
        "# Generated project\nHuman decision.\n",
    )
    .unwrap();
    let snapshot = snapshot_tracked(root).unwrap();
    fs::write(root.join("docs/AGENTS.md"), "# Generated project\n").unwrap();
    note_snapshot_writes(root, &snapshot, DocWriter::FullAudit).unwrap();
    let status = report(root, true).unwrap();
    assert_eq!(status.to_reconfirm, 1);
    assert!(status.docs[0].changes[0]
        .before
        .as_deref()
        .unwrap()
        .contains("Human decision."));
}

#[cfg(unix)]
#[test]
fn snapshot_refuses_symlinks_instead_of_following_unmanaged_documents() {
    let dir = project();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("unmanaged.md"),
        "Outside the audit surface.",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("unmanaged.md"),
        dir.path().join("docs/link.md"),
    )
    .unwrap();
    let error = snapshot_tracked(dir.path()).unwrap_err();
    assert!(error.contains("symbolic link"), "{error}");
}
