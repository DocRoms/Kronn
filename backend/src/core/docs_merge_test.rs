use super::*;
use std::path::PathBuf;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn template_docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs")
}

/// A documented repo: hand-written root AGENTS.md, a non-standard
/// `documentation/` tree, a hand-written v2 file, a binary and a Kronn block.
fn documented_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(
        root,
        "AGENTS.md",
        b"# Team agent guide\n\nRead this before touching the code.\n\n\
## Critical rules\n\n- Never push to main.\n- Do not edit generated files.\n\n\
## Running tests\n\nRun `make iso-test` inside Docker.\n\n\
## Deployment\n\nMerging to main deploys to staging via GitHub Actions.\n",
    );
    write(
        root,
        "CLAUDE.md",
        b"<!-- KRONN-MANAGED-BLOCK:START -->\n> Kronn project context\n<!-- KRONN-MANAGED-BLOCK:END -->\n\n\
# Claude notes\n\nPrefer small diffs.\n",
    );
    write(
        root,
        "README.md",
        b"# Front Apollo\n\nHapi API over OpenSearch.\n\n## License\n\nProprietary.\n",
    );
    write(
        root,
        "DEVELOPMENT.md",
        b"## Pull requests\n\nOne approval from CODEOWNERS.\n\n\
## Commit format\n\ntype(Scope):EW-1234 subject\n",
    );
    write(root, "CHANGELOG.md", b"## 1.0\n\n- first release\n");
    write(
        root,
        "documentation/architecture.md",
        b"# Architecture\n\nRequests flow through Hapi routes.\n\n\
```sh\n# not a heading\nmake up\n```\n\n## Random thoughts\n\nWe might move to Fastify one day.\n",
    );
    write(
        root,
        "documentation/onboarding/jira.md",
        b"Tickets start in Proposed.\nComponent = repository.\n",
    );
    write(
        root,
        "documentation/misc/notes.md",
        b"## Coffee machine\n\nSecond floor.\n",
    );
    write(
        root,
        "docs/coding-rules.md",
        b"# Our coding rules\n\nNever a ticket number in code.\n",
    );
    write(
        root,
        "docs/ops/branch-envs.md",
        b"# Branch environments\n\nShut down at 20:30 UTC.\n",
    );
    write(
        root,
        "docs/schema.png",
        &[0x89, b'P', b'N', b'G', 0, 1, 2, 3],
    );
    write(root, "src/main.ts", b"// # not a doc\n");
    tmp
}

fn plan_for(root: &Path) -> (BTreeSet<String>, MergePlan) {
    let existing = snapshot_tree(root).unwrap();
    let template = load_template(&template_docs()).unwrap();
    let docs_dir = choose_docs_dir(&existing, None);
    let plan = plan_merge(root, &existing, &template, &docs_dir).unwrap();
    (existing, plan)
}

fn all_hashes(root: &Path) -> BTreeMap<String, String> {
    let files: Vec<String> = snapshot_tree(root).unwrap().into_iter().collect();
    fingerprint(root, &files)
}

#[test]
fn no_existing_line_is_lost_every_one_is_extracted_or_listed_in_place() {
    let tmp = documented_repo();
    let root = tmp.path();
    let (existing, plan) = plan_for(root);
    let originals: BTreeMap<String, Vec<u8>> = source_paths(&existing)
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(root.join(&p)).unwrap();
            (p, bytes)
        })
        .collect();
    apply_plan(root, &plan).unwrap();

    let mut checked = 0;
    for (source, bytes) in &originals {
        let rows: Vec<&Correspondence> = plan
            .correspondence
            .iter()
            .filter(|r| &r.source == source)
            .collect();
        assert!(!rows.is_empty(), "{source} is missing from the table");
        // Whatever the action, the source itself is untouched.
        assert_eq!(
            &std::fs::read(root.join(source)).unwrap(),
            bytes,
            "{source} changed"
        );
        let Ok(text) = std::str::from_utf8(bytes) else {
            assert_eq!(rows[0].action, MappingAction::NotProse);
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let number = idx + 1;
            let row = rows
                .iter()
                .find(|r| r.start_line <= number && number <= r.end_line)
                .unwrap_or_else(|| panic!("{source}:{number} `{line}` has no row"));
            match row.action {
                MappingAction::Extracted => {
                    let target = row.target.as_ref().unwrap();
                    let written = std::fs::read_to_string(root.join(target)).unwrap();
                    let imported = written.split(IMPORT_START).nth(1).expect("import block");
                    assert!(
                        imported.lines().any(|l| l == line),
                        "{source}:{number} `{line}` not found in {target}"
                    );
                }
                MappingAction::TargetExists
                | MappingAction::Unmapped
                | MappingAction::KronnManaged => {}
                MappingAction::NotProse => panic!("prose file {source} marked not prose"),
            }
            checked += 1;
        }
    }
    assert!(checked >= 25, "fixture lines checked: {checked}");
}

#[test]
fn the_fixture_maps_sections_to_the_expected_v2_files() {
    let tmp = documented_repo();
    let (_, plan) = plan_for(tmp.path());
    let find = |source: &str, section: Option<&str>| {
        plan.correspondence
            .iter()
            .find(|r| r.source == source && r.section.as_deref() == section)
            .unwrap_or_else(|| panic!("no row for {source} {section:?}"))
            .clone()
    };
    let expect = |source: &str, section: Option<&str>, target: Option<&str>, action| {
        let row = find(source, section);
        assert_eq!(row.target.as_deref(), target, "{source} {section:?}");
        assert_eq!(row.action, action, "{source} {section:?}");
    };
    use MappingAction::*;
    expect(
        "AGENTS.md",
        Some("Running tests"),
        Some("docs/testing-quality.md"),
        Extracted,
    );
    expect(
        "AGENTS.md",
        Some("Deployment"),
        Some("docs/workflow/ci-cd.md"),
        Extracted,
    );
    expect(
        "AGENTS.md",
        Some("Critical rules"),
        Some("docs/AGENTS.md"),
        Extracted,
    );
    expect(
        "DEVELOPMENT.md",
        Some("Pull requests"),
        Some("docs/workflow/pull-requests.md"),
        Extracted,
    );
    expect(
        "DEVELOPMENT.md",
        Some("Commit format"),
        Some("docs/workflow/commits.md"),
        Extracted,
    );
    expect(
        "documentation/architecture.md",
        Some("Architecture"),
        Some("docs/architecture/overview.md"),
        Extracted,
    );
    expect(
        "documentation/onboarding/jira.md",
        None,
        Some("docs/workflow/tickets.md"),
        Extracted,
    );
    expect(
        "docs/ops/branch-envs.md",
        Some("Branch environments"),
        Some("docs/environments.md"),
        Extracted,
    );
    // A hand-written file at a v2 path is the target itself: never rewritten.
    expect(
        "docs/coding-rules.md",
        Some("Our coding rules"),
        Some("docs/coding-rules.md"),
        TargetExists,
    );
    expect(
        "documentation/misc/notes.md",
        Some("Coffee machine"),
        None,
        Unmapped,
    );
    expect("README.md", Some("License"), None, Unmapped);
    expect("docs/schema.png", None, None, NotProse);
    assert_eq!(find("CLAUDE.md", None).action, KronnManaged);
    // A `#` inside a code fence is not a heading.
    assert!(!plan
        .correspondence
        .iter()
        .any(|r| r.section.as_deref() == Some("not a heading")));
    assert!(!plan
        .correspondence
        .iter()
        .any(|r| r.source == "CHANGELOG.md" || r.source.starts_with("src/")));
    assert!(plan
        .existing_targets
        .contains(&"docs/coding-rules.md".to_string()));
    assert!(plan.unmapped().count() >= 2);
}

#[test]
fn no_existing_file_is_overwritten_moved_or_removed() {
    let tmp = documented_repo();
    let root = tmp.path();
    // A hand-written v2 entry file must survive too.
    write(
        root,
        "docs/AGENTS.md",
        b"# Hand-written entry\n\nOur own rules.\n",
    );
    let before = all_hashes(root);
    let (_, plan) = plan_for(root);
    assert!(plan
        .existing_targets
        .contains(&"docs/AGENTS.md".to_string()));
    assert!(!plan.writes.iter().any(|w| before.contains_key(&w.path)));

    let outcomes = apply_plan(root, &plan).unwrap();
    assert!(outcomes.iter().all(|(_, o)| *o == WriteOutcome::Created));
    let after = all_hashes(root);
    for (path, digest) in &before {
        assert_eq!(
            after.get(path),
            Some(digest),
            "{path} was overwritten or removed"
        );
    }
    let added: BTreeSet<&String> = after.keys().filter(|p| !before.contains_key(*p)).collect();
    let planned: BTreeSet<&String> = plan.writes.iter().map(|w| &w.path).collect();
    assert_eq!(added, planned);
    // Nothing went to a quarantine.
    assert!(!root.join("docs/legacy").exists());
}

#[test]
fn every_planned_file_has_its_own_diff_and_the_table_ships_in_it() {
    let tmp = documented_repo();
    let (_, plan) = plan_for(tmp.path());
    let report = plan
        .writes
        .iter()
        .find(|w| w.path == format!("docs/{REPORT_REL}"))
        .expect("correspondence report is part of the writes");
    assert!(report
        .content
        .contains("| `DEVELOPMENT.md` | Pull requests |"));
    assert!(report.content.contains("unmapped, kept in place"));
    for file in &plan.writes {
        let diff = addition_diff(&file.path, &file.content);
        assert!(diff.starts_with(&format!("--- /dev/null\n+++ b/{}\n", file.path)));
        let added = diff
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count();
        assert_eq!(added, file.content.lines().count(), "{}", file.path);
    }
    let tests = plan
        .writes
        .iter()
        .find(|w| w.path == "docs/testing-quality.md")
        .unwrap();
    assert_eq!(tests.imported_sections, 1);
    assert!(tests.content.contains("### From `AGENTS.md` (lines"));
}

#[test]
fn a_resumed_merge_plans_the_same_and_touches_nothing() {
    let tmp = documented_repo();
    let root = tmp.path();
    let (base, plan) = plan_for(root);
    apply_plan(root, &plan).unwrap();
    // A reviewer edits a proposed file before resuming.
    std::fs::write(root.join("docs/glossary.md"), "edited by a human\n").unwrap();
    let before = all_hashes(root);
    let template = load_template(&template_docs()).unwrap();
    let again = plan_merge(root, &base, &template, "docs").unwrap();
    assert_eq!(again, plan);
    let outcomes = apply_plan(root, &again).unwrap();
    assert!(outcomes
        .iter()
        .all(|(_, o)| *o == WriteOutcome::AlreadyPresent));
    assert_eq!(all_hashes(root), before);
}

#[test]
fn a_changed_source_stops_the_merge() {
    let mut before = BTreeMap::new();
    before.insert("AGENTS.md".to_string(), "aa".to_string());
    let mut after = before.clone();
    assert!(verify_unchanged(&before, &after).is_ok());
    after.insert("AGENTS.md".to_string(), "bb".to_string());
    let err = verify_unchanged(&before, &after).unwrap_err();
    assert!(err.contains("AGENTS.md"));
    after.remove("AGENTS.md");
    assert!(verify_unchanged(&before, &after).is_err());
}

#[test]
fn a_symlinked_doc_is_left_in_place_and_never_followed() {
    let tmp = documented_repo();
    let root = tmp.path();
    let outside = TempDir::new().unwrap();
    write(
        outside.path(),
        "secret.md",
        b"## Testing\n\nOUTSIDE-MARKER-7f3a\n",
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path().join("secret.md"), root.join("docs/link.md"))
        .unwrap();
    #[cfg(unix)]
    {
        let (_, plan) = plan_for(root);
        let row = plan
            .correspondence
            .iter()
            .find(|r| r.source == "docs/link.md")
            .unwrap();
        assert_eq!(row.action, MappingAction::NotProse);
        assert!(!plan
            .writes
            .iter()
            .any(|w| w.content.contains("OUTSIDE-MARKER-7f3a")));
    }
}

#[test]
fn a_doc_only_layout_keeps_its_directory_and_an_explicit_location_wins() {
    let mut existing = BTreeSet::new();
    existing.insert("doc/setup.md".to_string());
    assert_eq!(choose_docs_dir(&existing, None), "doc");
    existing.insert("docs/schema.png".to_string());
    assert_eq!(choose_docs_dir(&existing, None), "docs");
    assert_eq!(choose_docs_dir(&existing, Some("/handbook/")), "handbook");
    assert_eq!(choose_docs_dir(&BTreeSet::new(), None), "docs");
}

#[test]
fn every_rule_target_exists_in_the_shipped_template() {
    let template = load_template(&template_docs()).unwrap();
    for target in rule_targets() {
        assert!(
            template.iter().any(|t| t.rel == target),
            "{target} missing from templates/docs"
        );
    }
}

#[test]
fn classify_uses_word_boundaries() {
    assert_eq!(classify("Overview"), Some("architecture/overview.md"));
    assert_eq!(classify("Latest news"), None);
    assert_eq!(classify("PR checklist"), Some("workflow/pull-requests.md"));
    assert_eq!(classify("CI/CD"), Some("workflow/ci-cd.md"));
    assert_eq!(
        classify("Known issues"),
        Some("inconsistencies-tech-debt.md")
    );
    assert_eq!(classify("License"), None);
}
