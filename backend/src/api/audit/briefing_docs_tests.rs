use super::*;
use crate::models::{BriefingDocLocation, BriefingProjectAnswers, BriefingTracker};

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("docs/adr")).unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("README.md"), "x").unwrap();
    std::fs::write(root.join("README.fr.md"), "x").unwrap();
    std::fs::write(root.join("openapi.yaml"), "x").unwrap();
    dir
}

fn loc(path: &str, state: BriefingDocState, sot: bool) -> BriefingDocLocation {
    BriefingDocLocation {
        path: path.into(),
        state,
        source_of_truth: sot,
        ..BriefingDocLocation::default()
    }
}

fn answers(locations: Vec<BriefingDocLocation>) -> BriefingDocsAnswers {
    BriefingDocsAnswers { locations }
}

#[test]
fn normalize_recomputes_kind_and_path_and_drops_duplicates() {
    let dir = repo();
    let mut first = loc("./docs/", BriefingDocState::Recent, true);
    first.kind = BriefingDocKind::Api; // a client claim, ignored
    first.last_commit = Some("1999-01-01".into()); // a client claim, ignored
    first.note = "  main guide  ".into();
    let out = normalize(
        dir.path(),
        &answers(vec![first, loc("docs", BriefingDocState::Old, false)]),
    )
    .unwrap();
    assert_eq!(out.locations.len(), 1);
    let l = &out.locations[0];
    assert_eq!(
        (
            l.path.as_str(),
            l.kind,
            l.note.as_str(),
            l.last_commit.clone()
        ),
        ("docs", BriefingDocKind::Docs, "main guide", None)
    );
}

#[test]
fn a_path_outside_the_repository_is_refused() {
    let dir = repo();
    let root = dir.path().join("..").join(dir.path().file_name().unwrap());
    for bad in ["../x", "/etc", "docs/../../x", "missing.md"] {
        assert!(
            normalize(
                &root,
                &answers(vec![loc(bad, BriefingDocState::Old, false)])
            )
            .is_err(),
            "{bad}"
        );
    }
}

#[test]
fn one_source_of_truth_per_kind_and_never_an_obsolete_one() {
    let dir = repo();
    // Two READMEs as sources of truth: refused.
    let err = normalize(
        dir.path(),
        &answers(vec![
            loc("README.md", BriefingDocState::Reference, true),
            loc("README.fr.md", BriefingDocState::Recent, true),
        ]),
    )
    .unwrap_err();
    assert!(err.contains("one source of truth per kind"), "{err}");
    // One per kind is fine: a README, the docs folder and the API spec.
    let ok = normalize(
        dir.path(),
        &answers(vec![
            loc("README.md", BriefingDocState::Reference, true),
            loc("README.fr.md", BriefingDocState::Old, false),
            loc("docs", BriefingDocState::Recent, true),
            loc("openapi.yaml", BriefingDocState::Reference, true),
        ]),
    )
    .unwrap();
    assert_eq!(ok.locations.iter().filter(|l| l.source_of_truth).count(), 3);
    let err = normalize(
        dir.path(),
        &answers(vec![loc("wiki", BriefingDocState::Obsolete, true)]),
    )
    .unwrap_err();
    assert!(err.contains("obsolete"), "{err}");
}

#[test]
fn a_note_is_bounded_and_single_line() {
    let dir = repo();
    let mut long = loc("wiki", BriefingDocState::Old, false);
    long.note = "x".repeat(NOTE_MAX_CHARS + 1);
    assert!(normalize(dir.path(), &answers(vec![long])).is_err());
    let mut comment = loc("wiki", BriefingDocState::Old, false);
    comment.note = "a <!-- b".into();
    assert!(normalize(dir.path(), &answers(vec![comment])).is_err());
    let mut many = Vec::new();
    for _ in 0..=detect::MAX_LOCATIONS {
        many.push(loc("wiki", BriefingDocState::Old, false));
    }
    assert!(normalize(dir.path(), &answers(many)).is_err());
}

#[test]
fn the_section_renders_readable_lines_and_round_trips() {
    let mut a = loc("docs", BriefingDocState::Partial, true);
    a.kind = BriefingDocKind::Docs;
    a.last_commit = Some("2024-03-02".into());
    a.note = "setup | deploy --> here".into();
    let b = loc("README.md", BriefingDocState::Unknown, false);
    let recorded = answers(vec![a, b]);
    let section = render_section(&recorded).unwrap();
    assert!(section.starts_with(DOCS_HEADING));
    assert!(section.contains(
        "- `docs` — documentation folder; partial; last commit 2024-03-02; **source of truth** for the documentation folder — note: setup \\| deploy --&gt; here"
    ), "{section}");
    assert!(section.contains(
        "- `README.md` — documentation folder; state not provided; last commit unknown\n"
    ));
    let marker = section.lines().last().unwrap();
    assert_eq!(marker.matches("-->").count(), 1, "{marker}");
    assert_eq!(parse(&section), Some(recorded));
    assert_eq!(render_section(&BriefingDocsAnswers::default()), None);
}

#[test]
fn the_section_stays_out_of_the_prompt_copy() {
    let section =
        render_section(&answers(vec![loc("docs", BriefingDocState::Old, false)])).unwrap();
    let notes = format!(
        "# Project Briefing\n\n## Purpose\nP\n\n{section}\n## Kronn options\n- Turn audit tech debt into Kronn tasks: yes\n"
    );
    let stripped = strip_section(&notes);
    assert!(!stripped.contains(DOCS_HEADING));
    assert!(!stripped.contains(DOCS_ANSWERS_PREFIX));
    assert!(stripped.contains("## Purpose\nP"));
    assert!(stripped.contains("## Kronn options\n- Turn audit"));
    let prompt = super::super::briefing::briefing_prompt_section(
        &notes,
        &crate::core::docs_location::DocsRoot::default_root(),
    );
    assert!(!prompt.contains("Existing documentation"), "{prompt}");
    assert!(!prompt.contains("`docs`"), "{prompt}");
}

#[test]
fn a_review_rewrite_gets_both_form_sections_back_before_the_kronn_options() {
    let project = BriefingProjectAnswers {
        tracker: BriefingTracker::Linear,
        ..BriefingProjectAnswers::default()
    };
    let facts = super::super::project_doc::render_facts_section(&project).unwrap();
    let mut doc = loc("docs", BriefingDocState::Reference, true);
    doc.last_commit = Some("2024-03-02".into());
    let recorded = answers(vec![doc]);
    let docs = render_section(&recorded).unwrap();
    let options = "## Kronn options\n- Turn audit tech debt into Kronn tasks: yes\n";
    let previous = format!("# Project Briefing\n\n## Purpose\nOld\n\n{facts}\n{docs}\n{options}");

    // The reviewer dropped the facts, garbled the docs and kept the options.
    let reviewed = format!(
        "# Project Briefing\n\n## Purpose\nRefined\n\n## Existing documentation\n- docs is obsolete\n\n{options}"
    );
    let merged = super::super::project_doc::reattach_facts(Some(&previous), &reviewed);
    assert!(merged.contains("Refined"));
    assert!(!merged.contains("docs is obsolete"));
    assert_eq!(parse(&merged), Some(recorded.clone()));
    assert_eq!(
        super::super::project_doc::parse_answers(&merged),
        Some(project)
    );
    assert_eq!(merged.matches(DOCS_HEADING).count(), 1);
    let facts_at = merged.find("## Project facts").unwrap();
    let docs_at = merged.find(DOCS_HEADING).unwrap();
    let options_at = merged.find("## Kronn options").unwrap();
    assert!(facts_at < docs_at && docs_at < options_at, "{merged}");
    assert!(merged.ends_with(options), "{merged}");
    assert!(crate::core::td_tasks::briefing_wants_td_tasks(&merged));

    // Documentation alone is restored too.
    let only_docs = format!("# Project Briefing\n\n{docs}");
    let merged =
        super::super::project_doc::reattach_facts(Some(&only_docs), "# Project Briefing\n");
    assert_eq!(parse(&merged), Some(recorded));
    assert!(!merged.contains("## Project facts"));
}
