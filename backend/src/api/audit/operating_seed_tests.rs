use super::*;
use crate::models::{
    BriefingCommitConvention, BriefingDefaultBranch, BriefingPerson, BriefingPersonRole,
    BriefingProjectAnswers, BriefingYesNo,
};

fn briefing(traps: &str, team: &str, project: Option<BriefingProjectAnswers>) -> String {
    let mut notes = super::super::super::briefing_load::render_questions([
        "A shop", team, "Mature", "Stripe", traps, "None.",
    ]);
    if let Some(section) = project
        .as_ref()
        .and_then(super::super::render_facts_section)
    {
        notes.push('\n');
        notes.push_str(&section);
    }
    notes
}

fn project() -> BriefingProjectAnswers {
    BriefingProjectAnswers {
        default_branch: BriefingDefaultBranch::Main,
        commit_convention: BriefingCommitConvention::Conventional,
        signoff_required: BriefingYesNo::Yes,
        people: vec![BriefingPerson {
            role: BriefingPersonRole::TechLead,
            name: "Alice".into(),
            contact: "alice@example.com".into(),
        }],
        ..BriefingProjectAnswers::default()
    }
}

/// A project with the folders the installer creates.
fn installed() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("briefing.md"), "# Project Briefing\n").unwrap();
    std::fs::write(docs.join("project.md"), "# Project\n").unwrap();
    crate::api::projects::ensure_agent_writable_subfolders(tmp.path(), &docs);
    (tmp, docs)
}

fn read(docs: &Path, rel: &str) -> String {
    std::fs::read_to_string(docs.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_briefing_seeds_gotchas_people_and_conventions_and_links_them() {
    // KT-819 DoD 11 — the three folders are not empty after an audit of a
    // briefed project, and each seeded file is reachable from its README.
    let (tmp, docs) = installed();
    let notes = briefing(
        "Never run the seed script twice.\n# not a heading\n<!-- nor a comment -->",
        "Two devs and a PO.",
        Some(project()),
    );
    let report = seed(tmp.path(), &docs, Some(&notes));
    assert!(report.errors.is_empty(), "{report:?}");
    for rel in [
        "gotchas/known-traps.md",
        "people/team.md",
        "conventions/git-workflow.md",
        "gotchas/README.md",
        "people/README.md",
        "conventions/README.md",
    ] {
        assert!(
            report.written.contains(&rel.to_string()),
            "{rel}: {report:?}"
        );
    }

    let traps = read(&docs, "gotchas/known-traps.md");
    assert!(
        traps.starts_with("<!-- kronn:generated=\"briefing\""),
        "{traps}"
    );
    assert!(traps.contains("> Never run the seed script twice."));
    // A line of the answer can never become structure of the doc.
    assert!(traps.contains("> \\# not a heading") || traps.contains("> # not a heading"));
    assert!(!traps.contains("\n<!-- nor a comment"), "{traps}");
    assert!(traps.contains("[Traps & Fragile Areas](../briefing.md)"));

    let team = read(&docs, "people/team.md");
    assert!(
        team.contains("| Tech lead | Alice | alice@example.com |"),
        "{team}"
    );
    assert!(team.contains("> Two devs and a PO."));

    let workflow = read(&docs, "conventions/git-workflow.md");
    assert!(
        workflow.contains("| Default branch | `main` |"),
        "{workflow}"
    );
    assert!(workflow.contains("| Commit convention | Conventional Commits |"));
    assert!(workflow.contains("| Sign-off (`Signed-off-by`) | Required |"));
    assert!(
        !workflow.contains("Merge strategy"),
        "unanswered is not listed"
    );
    assert!(workflow.contains("[project](../project.md)"));

    assert!(read(&docs, "gotchas/README.md").contains("- [known-traps](known-traps.md)"));
    assert!(read(&docs, "people/README.md").contains("- [team](team.md)"));
    assert!(read(&docs, "conventions/README.md").contains("- [git-workflow](git-workflow.md)"));
    // The scaffold text is kept under the marker.
    assert!(read(&docs, "gotchas/README.md").contains("## What goes here"));
}

#[test]
fn nothing_is_invented_and_nothing_written_when_the_briefing_says_nothing() {
    let (tmp, docs) = installed();
    let before = read(&docs, "gotchas/README.md");
    let notes = briefing("None.", "", None);
    let report = seed(tmp.path(), &docs, Some(&notes));
    assert_eq!(report, SeedReport::default(), "{report:?}");
    assert!(!docs.join("gotchas/known-traps.md").exists());
    assert!(!docs.join("people/team.md").exists());
    assert!(!docs.join("conventions/git-workflow.md").exists());
    assert_eq!(read(&docs, "gotchas/README.md"), before);
    // No briefing at all is the same.
    assert_eq!(seed(tmp.path(), &docs, None), SeedReport::default());
}

#[test]
fn a_reaudit_regenerates_but_a_file_a_person_took_over_is_left_alone() {
    let (tmp, docs) = installed();
    let notes = briefing("Cache is shared.", "", None);
    seed(tmp.path(), &docs, Some(&notes));
    // Unchanged briefing: byte-identical, nothing written.
    assert_eq!(seed(tmp.path(), &docs, Some(&notes)), SeedReport::default());

    // The briefing changed: the generated file follows it.
    let changed = briefing("Cache is shared. Redis flushes at 03:00.", "", None);
    let report = seed(tmp.path(), &docs, Some(&changed));
    assert_eq!(report.written, vec!["gotchas/known-traps.md".to_string()]);
    assert!(read(&docs, "gotchas/known-traps.md").contains("flushes at 03:00"));

    // A person removes the marker: the file is theirs from then on.
    std::fs::write(
        docs.join("gotchas/known-traps.md"),
        "# Our traps\n\nHand-written.\n",
    )
    .unwrap();
    let again = briefing("A completely different trap.", "", None);
    let report = seed(tmp.path(), &docs, Some(&again));
    assert_eq!(
        report.kept_human_owned,
        vec!["gotchas/known-traps.md".to_string()]
    );
    assert_eq!(
        read(&docs, "gotchas/known-traps.md"),
        "# Our traps\n\nHand-written.\n"
    );
    // ...and still the README lists it.
    assert!(read(&docs, "gotchas/README.md").contains("(known-traps.md)"));
}

#[test]
fn a_file_the_briefing_no_longer_feeds_goes_away_with_its_readme_link() {
    let (tmp, docs) = installed();
    seed(tmp.path(), &docs, Some(&briefing("A trap.", "", None)));
    assert!(docs.join("gotchas/known-traps.md").is_file());

    let report = seed(tmp.path(), &docs, Some(&briefing("None", "", None)));
    assert_eq!(report.removed, vec!["gotchas/known-traps.md".to_string()]);
    assert!(!docs.join("gotchas/known-traps.md").exists());
    assert!(!read(&docs, "gotchas/README.md").contains("- [known-traps]"));
    assert!(read(&docs, "gotchas/README.md").starts_with("# Gotchas"));
}

#[test]
fn a_hand_edited_readme_is_never_rewritten() {
    let (tmp, docs) = installed();
    std::fs::write(
        docs.join("gotchas/README.md"),
        "# Our gotchas\n\nCurated.\n",
    )
    .unwrap();
    let report = seed(tmp.path(), &docs, Some(&briefing("A trap.", "", None)));
    assert!(report.errors.is_empty());
    assert_eq!(
        read(&docs, "gotchas/README.md"),
        "# Our gotchas\n\nCurated.\n"
    );
    // The seeded file still exists; only the link could not be added.
    assert!(docs.join("gotchas/known-traps.md").is_file());
}

#[test]
fn an_old_scaffold_readme_is_upgraded() {
    let (tmp, docs) = installed();
    std::fs::write(
        docs.join("people/README.md"),
        "# People\n\n> **Empty by design after the initial audit.** Optional folder.\n",
    )
    .unwrap();
    seed(
        tmp.path(),
        &docs,
        Some(&briefing("None.", "Two devs.", None)),
    );
    let readme = read(&docs, "people/README.md");
    assert!(readme.contains("- [team](team.md)"), "{readme}");
    assert!(!readme.contains("Empty by design"), "{readme}");
}

#[test]
fn existing_documents_are_pointed_to_never_copied() {
    // « à partir des docs existantes »: what the team already wrote and Kronn
    // moved to legacy/ is listed from the folder it belongs to.
    let (tmp, docs) = installed();
    for (name, body) in [
        ("legacy/CONTRIBUTING.md", "# Contributing\n"),
        ("legacy/guide/troubleshooting.md", "# Troubleshooting\n"),
        ("legacy/OWNERS.md", "# Owners\n"),
        ("legacy/api-reference.md", "# API\n"),
        ("legacy/my file.md", "# Spaces are not linked\n"),
    ] {
        let path = docs.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let report = seed(tmp.path(), &docs, None);
    assert!(report.errors.is_empty(), "{report:?}");
    assert!(read(&docs, "conventions/README.md")
        .contains("- [legacy/CONTRIBUTING.md](../legacy/CONTRIBUTING.md)"));
    assert!(read(&docs, "gotchas/README.md")
        .contains("- [legacy/guide/troubleshooting.md](../legacy/guide/troubleshooting.md)"));
    assert!(read(&docs, "people/README.md").contains("- [legacy/OWNERS.md](../legacy/OWNERS.md)"));
    for folder in ["conventions", "gotchas", "people"] {
        let readme = read(&docs, &format!("{folder}/README.md"));
        assert!(!readme.contains("api-reference"), "{readme}");
        assert!(!readme.contains("my file"), "{readme}");
    }
    // The documents themselves are untouched.
    assert_eq!(read(&docs, "legacy/CONTRIBUTING.md"), "# Contributing\n");
}

#[test]
fn the_seeded_docs_raise_no_blocking_documentary_diagnostic() {
    // Every link the seed writes resolves, and no placeholder survives, so a
    // briefed audit still reaches Completed.
    let (tmp, docs) = installed();
    std::fs::write(
        docs.join("AGENTS.md"),
        "# Project\n\n| Known traps | [gotchas/](gotchas/README.md), [conventions/](conventions/README.md), [people/](people/README.md) |\n",
    )
    .unwrap();
    let notes = briefing(
        "Needs {{ENV}} set.\nSee {{X}}",
        "Team {{A}}",
        Some(project()),
    );
    seed(tmp.path(), &docs, Some(&notes));
    let report = crate::core::document_optimization::analyze(tmp.path()).unwrap();
    let blocking: Vec<_> = report.blocking_diagnostics().collect();
    assert!(blocking.is_empty(), "{blocking:#?}");
    for rel in ["gotchas/known-traps.md", "people/team.md"] {
        assert!(!read(&docs, rel).contains("{{"), "{rel}");
    }
    assert!(report
        .diagnostics
        .iter()
        .all(|d| d.code != "orphan_document" || !d.path.contains("known-traps")));
}

#[test]
fn the_template_ships_and_routes_every_box_of_the_operating_docs() {
    // KT-819 DoD 11 — process, templates (gabarits), environments, skills,
    // examples, and the three agent-writable folders all exist in the shipped
    // template and are reachable from `docs/AGENTS.md`.
    let templates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    let read = |rel: &str| {
        std::fs::read_to_string(templates.join(rel))
            .unwrap_or_else(|e| panic!("templates/docs/{rel}: {e}"))
    };
    let agents = read("AGENTS.md");
    for (link, file) in [
        ("workflow/README.md", "workflow/README.md"),
        ("environments.md", "environments.md"),
        ("examples/README.md", "examples/README.md"),
        ("project.md", "project.md"),
        ("gotchas/README.md", "gotchas/README.md"),
        ("conventions/README.md", "conventions/README.md"),
        ("people/README.md", "people/README.md"),
    ] {
        assert!(
            agents.contains(&format!("({link})")),
            "AGENTS.md must route to {link}"
        );
        assert!(!read(file).trim().is_empty(), "{file} is empty");
    }
    // The ticket and PR templates live in `templates/`, reached from the
    // workflow index; skills are indexed by the Kronn resources pointer.
    assert!(read("workflow/README.md").contains("(../templates/README.md)"));
    assert!(read("templates/README.md").contains("pull-request.md"));
    assert!(agents.contains("`kronn/INDEX.md`"));
    // No folder README claims to be empty by design any more.
    for folder in ["conventions", "gotchas", "people"] {
        assert!(!read(&format!("{folder}/README.md")).contains("Empty by design"));
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_folder_is_never_written_through() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let docs = root.join("docs");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, docs.join("gotchas")).unwrap();
    let report = seed(&root, &docs, Some(&briefing("A trap.", "", None)));
    assert!(!report.errors.is_empty(), "{report:?}");
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
}
