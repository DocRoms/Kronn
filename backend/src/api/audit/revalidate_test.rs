use super::*;
use std::fs;
use std::process::Command;

const RULES: &str = "docs/coding-rules.md";

/// A documented repo whose fingerprints were recorded, then the code moved:
/// one line inserted on top (everything shifts) and the timeout changed.
fn drifted() -> tempfile::TempDir {
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
        "# Rules\n\
         Retries [src: file: src/lib.rs:2].\n\
         Timeout [src: file: src/lib.rs:6].\n\
         <!-- kronn:section name=\"team\" owner=\"human\" -->\n\
         ## Team\n\
         Owner note [src: file: src/lib.rs:1].\n\
         <!-- kronn:section:end -->\n",
    )
    .unwrap();
    fs::write(
        dir.join("docs/notes.md"),
        "Timeout again [src: file: src/lib.rs:6].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "// header\nfn alpha() {\n    let retries = 3;\n}\n\nfn beta() {\n    let timeout_ms = 9000;\n}\n",
    )
    .unwrap();
    tmp
}

fn git(dir: &FsPath, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(dir: &FsPath, message: &str, paths: &[&str]) {
    let mut add = vec!["add", "--"];
    add.extend_from_slice(paths);
    git(dir, &add);
    git(
        dir,
        &[
            "-c",
            "user.name=Kronn Test",
            "-c",
            "user.email=kronn@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
}

/// A repeated anchor whose second occurrence moves beyond the fingerprint
/// search window. The optional edit puts the old range's first line inside a
/// changed hunk, which must make diff mapping refuse it.
fn repeated_anchor_repo(change_endpoint: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn target() {}\n\nfn helper() {}\nfn target() {\n    let first = 1;\n    let second = 2;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join(RULES),
        "# Rules\nTarget [src: file: src/lib.rs:4-7#target].\n",
    )
    .unwrap();
    git(dir, &["init", "-b", "main"]);
    commit(dir, "baseline", &["src/lib.rs", RULES]);
    citation_drift::record_all(dir).unwrap();
    git(dir, &["switch", "-c", "feature"]);

    let mut moved = String::from("fn target() {}\n\nfn helper() {}\n");
    for index in 0..60 {
        moved.push_str(&format!("// inserted {index}\n"));
    }
    if change_endpoint {
        moved.push_str("pub fn target() {\n");
    } else {
        moved.push_str("fn target() {\n");
    }
    moved.push_str("    let first = 1;\n    let second = 2;\n}\n");
    fs::write(dir.join("src/lib.rs"), moved).unwrap();
    commit(dir, "move target", &["src/lib.rs"]);
    tmp
}

/// A repeated anchor sits on the old range's last line. The feature adds one
/// block before that range and another inside it, so the mapped span grows.
fn repeated_anchor_with_internal_insertion_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn target() {}\n\nfn helper() {}\nfn scope() {\n    let first = 1;\n    let second = 2;\n    let third = 3;\n    fn target() {}\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join(RULES),
        "# Rules\nTarget [src: file: src/lib.rs:4-8#target].\n",
    )
    .unwrap();
    git(dir, &["init", "-b", "main"]);
    commit(dir, "baseline", &["src/lib.rs", RULES]);
    citation_drift::record_all(dir).unwrap();
    git(dir, &["switch", "-c", "feature"]);

    let mut moved = String::from("fn target() {}\n\nfn helper() {}\n");
    for index in 0..60 {
        moved.push_str(&format!("// before {index}\n"));
    }
    moved.push_str(
        "fn scope() {\n    let first = 1;\n    // inside 0\n    // inside 1\n    let second = 2;\n    let third = 3;\n    fn target() {}\n}\n",
    );
    fs::write(dir.join("src/lib.rs"), moved).unwrap();
    commit(dir, "move and widen target range", &["src/lib.rs"]);
    tmp
}

fn rules_step() -> usize {
    owning_step(
        &super::super::assemble_chained_steps(AuditKind::Full),
        RULES,
        &crate::core::docs_location::DocsRoot::default(),
    )
    .unwrap()
}

fn cites(list: &[CitationStatus]) -> Vec<(&str, CitationState)> {
    list.iter().map(|c| (c.cite.as_str(), c.state)).collect()
}

#[test]
fn the_plan_sorts_each_citation_into_reanchor_agent_human_or_manual() {
    let tmp = drifted();
    let plan = plan(tmp.path());
    assert_eq!(plan.sources_state, SourcesState::NeedsReverification);
    assert_eq!(plan.agent_steps, vec![rules_step()]);
    assert_eq!(plan.reanchor_count, 2, "the human citation moves too");
    assert_eq!(plan.human_count, 1);

    let rules = plan.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert_eq!(rules.action, RevalidationAction::Agent);
    assert_eq!(rules.step, Some(rules_step()));
    let mut moved: Vec<&str> = rules.reanchor.iter().map(|c| c.cite.as_str()).collect();
    moved.sort_unstable();
    assert_eq!(moved, vec!["src/lib.rs:1", "src/lib.rs:2"]);
    assert!(rules
        .reanchor
        .iter()
        .all(|c| c.state == CitationState::Moved));
    assert_eq!(
        cites(&rules.broken),
        vec![("src/lib.rs:6", CitationState::Changed)]
    );
    assert_eq!(rules.human.len(), 1);
    assert_eq!(rules.human[0].section, "team");
    assert_eq!(rules.human[0].citation.cite, "src/lib.rs:1");

    // No audit step owns this doc: a person fixes or acknowledges it.
    let notes = plan
        .sections
        .iter()
        .find(|s| s.doc == "docs/notes.md")
        .unwrap();
    assert_eq!(notes.action, RevalidationAction::Manual);
    assert_eq!(notes.step, None);
}

#[test]
fn document_proof_plans_prose_edits_but_never_recreates_a_deleted_doc_silently() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("docs")).unwrap();
    fs::write(tmp.path().join(RULES), "# Reviewed policy\n").unwrap();
    let documents = crate::core::audit_documents::capture_json(tmp.path()).unwrap();
    fs::write(tmp.path().join(RULES), "# A changed policy\n").unwrap();
    let changed = plan_with_documents(tmp.path(), Some(&documents)).unwrap();
    assert_eq!(changed.agent_steps, vec![rules_step()]);
    assert!(prompt_blocks(&changed)[&rules_step()].contains("content changed since"));
    fs::remove_file(tmp.path().join(RULES)).unwrap();
    let deleted = plan_with_documents(tmp.path(), Some(&documents)).unwrap();
    assert!(deleted.agent_steps.is_empty());
    assert_eq!(deleted.sections[0].action, RevalidationAction::Manual);
    assert!(deleted.sections[0].doc_missing);
}

#[test]
fn re_anchoring_rewrites_a_human_section_too_and_marks_it_to_reconfirm() {
    let tmp = drifted();
    let dir = tmp.path();
    let notes_before = fs::read(dir.join("docs/notes.md")).unwrap();
    let done = apply_reanchors(dir).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].doc, RULES);
    assert_eq!(done[0].count, 2);
    let rules = fs::read_to_string(dir.join(RULES)).unwrap();
    assert!(
        rules.contains("Retries [src: file: src/lib.rs:3].\n"),
        "{rules}"
    );
    assert!(
        rules.contains("Owner note [src: file: src/lib.rs:2].\n"),
        "never refused: {rules}"
    );
    let provenance = crate::core::doc_provenance::report(dir, true).unwrap();
    assert_eq!(provenance.to_reconfirm, 1);
    assert_eq!(
        provenance.docs[0].kronn_writes[0].writer,
        crate::core::doc_provenance::DocWriter::Revalidation
    );
    assert!(
        rules.contains("Timeout [src: file: src/lib.rs:6].\n"),
        "{rules}"
    );
    assert_eq!(fs::read(dir.join("docs/notes.md")).unwrap(), notes_before);

    // Both moves are settled, the fingerprints follow; the broken one stays.
    let after = plan(dir);
    let section = after.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert!(section.reanchor.is_empty());
    assert_eq!(
        cites(&section.broken),
        vec![("src/lib.rs:6", CitationState::Changed)]
    );
    // Idempotent: a second pass has nothing left to move.
    assert!(apply_reanchors(dir).unwrap().is_empty());
}

#[test]
fn revalidation_uses_the_git_diff_to_disambiguate_a_repeated_anchor() {
    let tmp = repeated_anchor_repo(false);
    let dir = tmp.path();
    let before = plan(dir);
    let rules = before.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert_eq!(rules.action, RevalidationAction::Reanchor);
    assert_eq!(rules.reanchor.len(), 1);
    assert_eq!(rules.reanchor[0].current_line, Some(64));
    assert!(rules.broken.is_empty());

    let done = apply_reanchors(dir).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].count, 1);
    assert_eq!(
        fs::read_to_string(dir.join(RULES)).unwrap(),
        "# Rules\nTarget [src: file: src/lib.rs:64-67#target].\n"
    );
    assert!(plan(dir)
        .sections
        .iter()
        .all(|section| section.doc != RULES));
}

#[test]
fn revalidation_keeps_the_diff_mapped_end_after_an_internal_insertion() {
    let tmp = repeated_anchor_with_internal_insertion_repo();
    let dir = tmp.path();
    let before_doc = fs::read_to_string(dir.join(RULES)).unwrap();
    let base = crate::core::docs_citations::default_reanchor_base(dir)
        .unwrap()
        .unwrap();
    let mapped =
        crate::core::docs_citations::suggested_reanchors(&before_doc, dir, Some(&base)).unwrap();
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].line, 64);
    assert_eq!(mapped[0].end_line, 70);

    let before = plan(dir);
    let rules = before.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert_eq!(rules.action, RevalidationAction::Reanchor);
    assert_eq!(rules.reanchor.len(), 1);
    assert_eq!(rules.reanchor[0].current_line, Some(mapped[0].line));

    let done = apply_reanchors(dir).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].count, 1);
    let after_doc = fs::read_to_string(dir.join(RULES)).unwrap();
    assert_eq!(
        after_doc,
        "# Rules\nTarget [src: file: src/lib.rs:64-70#target].\n"
    );
    assert!(
        crate::core::docs_citations::check_doc(&after_doc, dir).is_empty(),
        "the rewritten range must still resolve its anchor"
    );
}

#[test]
fn revalidation_never_maps_an_endpoint_inside_a_changed_hunk() {
    let tmp = repeated_anchor_repo(true);
    let dir = tmp.path();
    let before_doc = fs::read_to_string(dir.join(RULES)).unwrap();
    let before = plan(dir);
    let rules = before.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert_eq!(rules.action, RevalidationAction::Agent);
    assert!(rules.reanchor.is_empty());
    assert_eq!(
        cites(&rules.broken),
        vec![("src/lib.rs:4-7", CitationState::Changed)]
    );

    assert!(apply_reanchors(dir).unwrap().is_empty());
    assert_eq!(fs::read_to_string(dir.join(RULES)).unwrap(), before_doc);
}

#[test]
fn a_section_with_only_moves_needs_no_agent_and_ends_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 3;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join(RULES),
        "# Rules\nRetries [src: file: src/lib.rs:2].\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    assert_eq!(plan(dir).sources_state, SourcesState::Clean);
    assert!(plan(dir).sections.is_empty());
    fs::write(
        dir.join("src/lib.rs"),
        "// a\n// b\nfn alpha() {\n    let retries = 3;\n}\n",
    )
    .unwrap();
    let before = plan(dir);
    assert_eq!(before.sections.len(), 1);
    assert_eq!(before.sections[0].action, RevalidationAction::Reanchor);
    assert!(before.agent_steps.is_empty());
    // A moved line alone does not make the sources stale.
    assert_eq!(before.sources_state, SourcesState::Clean);
    apply_reanchors(dir).unwrap();
    assert!(plan(dir).sections.is_empty());
    assert_eq!(
        fs::read_to_string(dir.join(RULES)).unwrap(),
        "# Rules\nRetries [src: file: src/lib.rs:4].\n"
    );
}

#[test]
fn the_agent_block_carries_the_skill_and_the_findings_of_its_doc_only() {
    let tmp = drifted();
    let blocks = prompt_blocks(&plan(tmp.path()));
    assert_eq!(blocks.len(), 1);
    let block = &blocks[&rules_step()];
    assert!(block.contains("# Audit revalidation"), "{block}");
    assert!(
        block.contains("## KRONN FINDINGS — `docs/coding-rules.md`"),
        "{block}"
    );
    assert!(
        block.contains(
            "- `[src: file: src/lib.rs:6]` — changed: the cited line now holds other content\n"
        ),
        "{block}"
    );
    assert!(
        block.contains("Kronn already re-anchored 2 moved citation(s)"),
        "{block}"
    );
    assert!(
        block.contains("- section `team`: `[src: file: src/lib.rs:1]` — moved to line 2"),
        "{block}"
    );
    assert!(!block.contains("notes.md"), "{block}");
}

#[test]
fn the_validation_comes_back_only_when_no_target_changed() {
    let tmp = drifted();
    let dir = tmp.path();
    crate::core::kronn_state::mark_validated(dir).unwrap();
    let date = crate::core::kronn_state::read(dir)
        .unwrap()
        .validated_at
        .unwrap();
    let snapshot = vec![(
        RULES.to_string(),
        super::super::validation::target_snapshot(dir, RULES).unwrap(),
    )];
    crate::core::kronn_state::revoke_validated(dir).unwrap();
    // Nothing validated before: nothing to give back.
    assert!(!restore_validation_if_untouched(dir, None, &snapshot, None));
    fs::write(dir.join(RULES), "# Rules\nRewritten.\n").unwrap();
    assert!(!restore_validation_if_untouched(
        dir,
        Some(&date),
        &snapshot,
        None
    ));
    assert!(crate::core::kronn_state::read(dir)
        .unwrap()
        .validated_at
        .is_none());
    let untouched = vec![(
        RULES.to_string(),
        super::super::validation::target_snapshot(dir, RULES).unwrap(),
    )];
    assert!(restore_validation_if_untouched(
        dir,
        Some(&date),
        &untouched,
        None
    ));
    assert_eq!(
        crate::core::kronn_state::read(dir).unwrap().validated_at,
        Some(date)
    );
}

#[test]
fn a_label_cited_in_both_parts_is_re_anchored_in_both() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 3;\n}\n",
    )
    .unwrap();
    let doc = "# Rules\n\
               Retries [src: file: src/lib.rs:2].\n\
               <!-- kronn:section name=\"team\" owner=\"human\" -->\n\
               Owner retry rule [src: file: src/lib.rs:2].\n\
               <!-- kronn:section:end -->\n";
    fs::write(dir.join(RULES), doc).unwrap();
    citation_drift::record_all(dir).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "// a\nfn alpha() {\n    let retries = 3;\n}\n",
    )
    .unwrap();

    let before = plan(dir);
    let section = before.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert_eq!(section.reanchor.len(), 1);
    assert_eq!(section.human.len(), 1);
    assert_eq!(section.action, RevalidationAction::Reanchor);

    apply_reanchors(dir).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join(RULES)).unwrap(),
        doc.replace("src/lib.rs:2", "src/lib.rs:3")
    );
    assert!(plan(dir).sections.iter().all(|s| s.doc != RULES));
    let provenance = crate::core::doc_provenance::report(dir, false).unwrap();
    assert_eq!(provenance.to_reconfirm, 1, "the human copy asks its owner");
}

// KT-1168 — relevance: a section the code outgrew is judged, proposed a
// correction, handed to its audit step and weighed by the badge, though no
// citation fingerprint broke.
#[test]
fn the_plan_judges_each_section_and_the_agent_block_carries_the_corrections() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "fn alpha() {\n    let retries = 3;\n}\n",
    )
    .unwrap();
    fs::write(dir.join("src/engine.rs"), "pub fn run() {}\n").unwrap();
    fs::write(
        dir.join(RULES),
        "# Rules\n## Retries\nRetries [src: file: src/lib.rs:2].\n## Engine\nHandled by `src/engine.rs`.\n",
    )
    .unwrap();
    citation_drift::record_all(dir).unwrap();
    assert_eq!(plan(dir).sources_state, SourcesState::Clean);
    assert!(plan(dir).sections.is_empty());

    fs::remove_file(dir.join("src/engine.rs")).unwrap();
    assert_eq!(
        citation_drift::check(dir).sources_state,
        SourcesState::Clean,
        "no cited line broke: only the relevance check sees it"
    );

    let planned = plan(dir);
    assert_eq!(planned.sources_state, SourcesState::NeedsReverification);
    assert_eq!(planned.relevance.no_longer_relevant, 1);
    assert_eq!(planned.relevance.still_true, 1);
    let rules = planned.sections.iter().find(|s| s.doc == RULES).unwrap();
    assert_eq!(rules.action, RevalidationAction::Agent);
    assert_eq!(rules.relevance.len(), 1);
    assert_eq!(rules.relevance[0].heading, "Engine");
    assert!(planned.agent_steps.contains(&rules_step()));

    let block = &prompt_blocks(&planned)[&rules_step()];
    assert!(block.contains("section `Engine` — no longer relevant to the code"));
    assert!(block.contains("proposed:"));
    assert!(block.contains("src/engine.rs"));
}
