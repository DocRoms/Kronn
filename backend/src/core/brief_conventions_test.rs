//! KT-1747 — a brief describes the project it is for.

use super::*;
use crate::core::project_toolchains::detect_toolchains;

fn put(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// The shape of a front-end repository: a root Node app with a Makefile and a
/// PHP application in a sub-directory.
fn front_euronews_like(root: &Path) {
    put(
        root,
        "package.json",
        r#"{"dependencies":{"react":"18.0.0"},"devDependencies":{"vite":"5"}}"#,
    );
    put(root, "yarn.lock", "");
    put(root, "Makefile", "test:\n\ttrue\n");
    put(root, "application/composer.json", "{}");
}

#[test]
fn the_agent_instruction_files_of_the_project_are_pointed_at_with_an_excerpt() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "AGENTS.md",
        "# Rules\n\nRun `make test` before you commit.\nUse yarn, never npm.\n",
    );
    put(dir.path(), "CLAUDE.md", "See AGENTS.md\n");

    let docs = ai_docs(dir.path());

    assert_eq!(docs[0].path, "AGENTS.md");
    assert!(docs[0]
        .excerpt
        .contains("Run `make test` before you commit."));
    assert!(docs[0].excerpt.contains("Use yarn, never npm."));
    assert_eq!(docs[1].path, "CLAUDE.md");
    assert!(ai_docs(&dir.path().join("nothing-here")).is_empty());
}

#[test]
fn a_line_that_could_carry_a_credential_is_never_quoted() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "AGENTS.md",
        "Deploy with `make deploy`\nexport API_TOKEN=abc123\nthe password is hunter2\nghp_0123456789abcdef\n",
    );

    let docs = ai_docs(dir.path());

    assert_eq!(docs[0].excerpt, "Deploy with `make deploy`");
}

#[test]
fn the_conventions_section_of_a_front_end_repository_names_its_tools_and_rules() {
    let dir = tempfile::tempdir().unwrap();
    front_euronews_like(dir.path());
    put(
        dir.path(),
        "AGENTS.md",
        "Run `make test` before you commit.\n",
    );
    let profile = detect_toolchains(dir.path());

    let section = section(dir.path(), &profile, false);

    assert!(section.starts_with("## Project conventions\n"), "{section}");
    assert!(section.contains("- yarn at `.`"), "{section}");
    assert!(section.contains("- composer at `application`"), "{section}");
    assert!(section.contains("- make at `.`"), "{section}");
    assert!(section.contains("read `AGENTS.md` first"), "{section}");
    assert!(
        section.contains("Run `make test` before you commit."),
        "{section}"
    );
    let french = super::section(dir.path(), &profile, true);
    assert!(
        french.contains("règles du projet pour un agent : lis `AGENTS.md`"),
        "{french}"
    );
}

#[test]
fn doc_citations_are_for_a_project_whose_docs_carry_them() {
    let dir = tempfile::tempdir().unwrap();
    front_euronews_like(dir.path());
    assert!(!has_doc_citations(dir.path()));

    put(dir.path(), "docs/guide.md", "Plain prose, no source.\n");
    assert!(!has_doc_citations(dir.path()));

    put(
        dir.path(),
        "docs/architecture/api.md",
        "The router [src: file: src/app.ts:10].\n",
    );
    assert!(has_doc_citations(dir.path()));
}

#[test]
fn a_web_ui_is_a_node_root_with_a_ui_framework() {
    let ui = tempfile::tempdir().unwrap();
    front_euronews_like(ui.path());
    assert!(has_web_ui(ui.path(), &detect_toolchains(ui.path())));

    let nested = tempfile::tempdir().unwrap();
    put(nested.path(), "composer.json", "{}");
    put(
        nested.path(),
        "assets/front/package.json",
        r#"{"devDependencies":{"vite":"5"}}"#,
    );
    assert!(has_web_ui(nested.path(), &detect_toolchains(nested.path())));

    let api = tempfile::tempdir().unwrap();
    put(
        api.path(),
        "package.json",
        r#"{"dependencies":{"express":"4"}}"#,
    );
    put(api.path(), "Makefile", "all:\n");
    assert!(!has_web_ui(api.path(), &detect_toolchains(api.path())));

    let php = tempfile::tempdir().unwrap();
    put(php.path(), "composer.json", "{}");
    assert!(!has_web_ui(php.path(), &detect_toolchains(php.path())));
}

#[test]
fn the_commit_format_of_the_history_is_in_the_conventions_section() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = crate::core::cmd::git_cmd()
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    for n in 0..12 {
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            &format!("PRJ-{n} fix(api): thing {n}"),
        ]);
    }
    put(dir.path(), "composer.json", "{}");
    let profile = detect_toolchains(dir.path());

    let en = section(dir.path(), &profile, false);
    let fr = section(dir.path(), &profile, true);

    assert!(
        en.contains("- commit format: the history writes `type(scope): subject`"),
        "{en}"
    );
    assert!(en.contains("ticket key `PRJ`"), "{en}");
    assert!(fr.contains("- format des commits:"), "{fr}");
}
