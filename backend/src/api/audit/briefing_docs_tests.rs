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

// ─── The audit prompt (KT-999) ──────────────────────────────────────────────

use super::super::briefing::{
    briefing_prompt_section, briefing_prompt_section_for, BRIEFING_PROMPT_MAX_CHARS,
};
use crate::core::docs_location::DocsRoot;
use crate::models::AuditKind;

/// A briefing with one location per treatment the prompt gives.
fn documented_briefing() -> String {
    let mut changelog = loc("CHANGELOG.md", BriefingDocState::Obsolete, false);
    changelog.kind = BriefingDocKind::Markdown;
    changelog.last_commit = Some("2021-04-09".into());
    let mut api = loc("apidoc", BriefingDocState::Reference, true);
    api.kind = BriefingDocKind::Api;
    api.last_commit = Some("2026-09-30".into());
    let mut wiki = loc("wiki", BriefingDocState::Old, false);
    wiki.kind = BriefingDocKind::Wiki;
    wiki.last_commit = Some("2023-01-05".into());
    let section = render_section(&answers(vec![changelog, api, wiki])).unwrap();
    format!("# Project Briefing\n\n## Purpose\nShip the API.\n\n{section}")
}

const ALL_KINDS: [AuditKind; 10] = [
    AuditKind::Full,
    AuditKind::Drift,
    AuditKind::Security,
    AuditKind::Docker,
    AuditKind::Performance,
    AuditKind::Accessibility,
    AuditKind::Rgaa,
    AuditKind::Database,
    AuditKind::ApiDesign,
    AuditKind::CodeQuality,
];

#[test]
fn the_documentation_reaches_only_the_steps_that_use_it_and_only_when_enabled() {
    let notes = documented_briefing();
    for rel in ["docs", "knowledge/ai"] {
        let docs_root = DocsRoot::from_rel(rel).unwrap();
        let mut reached: Vec<String> = Vec::new();
        for kind in ALL_KINDS {
            for step in super::super::steps_for(kind, &docs_root) {
                let on = briefing_prompt_section_for(&notes, &docs_root, step.target_file, true);
                let off = briefing_prompt_section_for(&notes, &docs_root, step.target_file, false);
                assert!(
                    !off.contains("Existing documentation"),
                    "{rel} {}",
                    step.target_file
                );
                assert!(!off.contains("CHANGELOG.md"), "{off}");
                assert_eq!(off, briefing_prompt_section(&notes, &docs_root));
                assert!(on.contains("## Project briefing (from the user)"));
                assert!(on.contains("Ship the API."));
                if on.contains("## Existing documentation (from the briefing)") {
                    reached.push(step.target_file.to_string());
                    assert!(
                        on.contains("`apidoc`") && on.contains("`CHANGELOG.md`"),
                        "{on}"
                    );
                } else {
                    assert_eq!(on, off, "{}", step.target_file);
                }
            }
        }
        reached.sort();
        reached.dedup();
        assert_eq!(
            reached,
            vec![
                format!("{rel}/AGENTS.md"),
                format!("{rel}/inconsistencies-api.md"),
                format!("{rel}/inconsistencies-tech-debt.md"),
            ],
            "{rel}"
        );
    }
}

#[test]
fn a_reference_is_cited_and_an_obsolete_one_is_tech_debt_never_out_of_scope() {
    let notes = documented_briefing();
    let root = DocsRoot::default_root();
    let debt =
        briefing_prompt_section_for(&notes, &root, "docs/inconsistencies-tech-debt.md", true);
    for wording in [
        "### Reference\nRead each one first, reuse its facts and cite it",
        "Each one is tech debt, whatever the scope or the dimension of this audit",
        "a row in the `Current list` table AND a detail file `docs/tech-debt/TD-<date>-<slug>.md`",
        "names the location and its last commit date",
        "Copy no fact from it. Never write that it is out of scope",
        "### Old, partial or not confirmed\nVerify each fact against the code before you reuse it.",
    ] {
        assert!(debt.contains(wording), "missing `{wording}` in:\n{debt}");
    }
    // Each location comes with its state and its date, under its own rule.
    let at = |needle: &str| {
        debt.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {debt}"))
    };
    assert!(
        at("### Reference") < at("`apidoc` — API reference; reference; last commit 2026-09-30")
    );
    assert!(
        at("### Obsolete") < at("`CHANGELOG.md` — Markdown page; obsolete; last commit 2021-04-09")
    );
    assert!(at("### Old, partial") < at("`wiki` — wiki; old; last commit 2023-01-05"));
    assert!(
        !debt.contains("kronn:existing-docs"),
        "the machine copy stays out"
    );

    for other in ["docs/AGENTS.md", "docs/inconsistencies-api.md"] {
        let block = briefing_prompt_section_for(&notes, &root, other, true);
        assert!(
            block.contains("Copy no fact from it and cite it for nothing."),
            "{block}"
        );
        assert!(
            block.contains("the tech-debt step records it. Never call it out of scope."),
            "{block}"
        );
        assert!(
            !block.contains("detail file"),
            "only step 8 writes the TD: {block}"
        );
        assert!(
            block.contains("Read each one first, reuse its facts and cite it"),
            "{block}"
        );
    }
}

#[test]
fn the_td_path_follows_the_docs_root_but_a_listed_path_is_never_rewritten() {
    let mut under_docs = loc("docs/api", BriefingDocState::Obsolete, false);
    under_docs.last_commit = Some("2020-02-02".into());
    let notes = format!(
        "# Briefing\n\n{}",
        render_section(&answers(vec![under_docs])).unwrap()
    );
    let root = DocsRoot::from_rel("knowledge/ai").unwrap();
    let block = briefing_prompt_section_for(
        &notes,
        &root,
        "knowledge/ai/inconsistencies-tech-debt.md",
        true,
    );
    assert!(
        block.contains("`knowledge/ai/tech-debt/TD-<date>-<slug>.md`"),
        "{block}"
    );
    assert!(block.contains("- `docs/api` — "), "{block}");
}

#[test]
fn a_briefing_without_documentation_adds_nothing_even_when_enabled() {
    let notes = "# Project Briefing\n\n## Purpose\nP\n";
    let root = DocsRoot::default_root();
    assert_eq!(
        briefing_prompt_section_for(notes, &root, "docs/inconsistencies-tech-debt.md", true),
        briefing_prompt_section(notes, &root)
    );
}

#[test]
fn the_block_stays_within_the_briefing_cap_and_drops_the_obsolete_ones_last() {
    let locations: Vec<BriefingDocLocation> = (0..detect::MAX_LOCATIONS)
        .map(|i| {
            let state = if i % 10 == 9 {
                BriefingDocState::Obsolete
            } else {
                BriefingDocState::Old
            };
            let mut l = loc(&format!("page-{i:02}.md"), state, false);
            l.note = "n".repeat(NOTE_MAX_CHARS);
            l.last_commit = Some("2020-01-01".into());
            l
        })
        .collect();
    let section = render_section(&answers(locations)).unwrap();
    let long_briefing = format!(
        "# Project Briefing\n\n## Purpose\n{}\n\n{section}",
        "word ".repeat(2_000)
    );
    let root = DocsRoot::default_root();
    let docs = prompt_block("docs/inconsistencies-tech-debt.md", &long_briefing, &root);
    assert!(
        docs.chars().count() <= PROMPT_MAX_CHARS,
        "{}",
        docs.chars().count()
    );
    for i in [9, 19, 29] {
        assert!(
            docs.contains(&format!("`page-{i:02}.md`")),
            "obsolete {i} dropped:\n{docs}"
        );
    }
    assert!(
        docs.contains("more location(s) not shown: read docs/briefing.md."),
        "{docs}"
    );

    let whole = briefing_prompt_section_for(
        &long_briefing,
        &root,
        "docs/inconsistencies-tech-debt.md",
        true,
    );
    let marker = whole.find("[Briefing truncated by Kronn:").unwrap();
    assert!(marker < whole.find("## Existing documentation").unwrap());
    // Only the truncation marker is over the cap.
    assert!(
        whole.chars().count() <= BRIEFING_PROMPT_MAX_CHARS + 300,
        "{}",
        whole.chars().count()
    );
}

#[test]
#[serial_test::serial]
fn the_setting_or_the_env_switch_turns_it_on_and_it_is_off_by_default() {
    let _restore = crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&[ENV_SWITCH]);
    crate::core::child_env::remove_var(ENV_SWITCH);
    assert!(!prompt_enabled(false));
    assert!(prompt_enabled(true));
    crate::core::child_env::set_var(ENV_SWITCH, "1");
    assert!(prompt_enabled(false));
    crate::core::child_env::set_var(ENV_SWITCH, "0");
    assert!(!prompt_enabled(false));
    assert!(!crate::core::config::default_config().audit_briefing_docs);
}
