//! KT-1747 / KT-1751 — the brief of a project that is not Kronn names that
//! project's own tools and carries none of Kronn's own rules (citations, UI
//! captures, the cargo gate), and never tells a worker to skip tests the
//! project runs itself.

use super::*;
use crate::core::project_toolchains::detect_toolchains;

fn put(root: &std::path::Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn brief_of(lang: RoomLang, worktree: &std::path::Path) -> String {
    worker_brief_markdown_in(
        lang,
        "KT-9",
        "Task",
        "Objective",
        &[],
        &worktree.to_string_lossy(),
        "kronn/task/kt-9",
        "abc1234",
        true,
        true,
        None,
        true,
    )
}

/// A root Node API (no UI framework), a PHP application below it, a Makefile.
fn api_with_php_application(root: &std::path::Path) {
    put(root, "package.json", r#"{"dependencies":{"express":"4"}}"#);
    put(root, "yarn.lock", "");
    put(root, "Makefile", "test:\n\ttrue\n");
    put(root, "application/composer.json", "{}");
    put(root, "AGENTS.md", "Run `make test` before you commit.\n");
}

fn capabilities(rust: Capability, frontend: Capability) -> ToolchainCapabilities {
    ToolchainCapabilities {
        rust_tests: rust,
        frontend_tests: frontend,
        port_binding: Capability {
            available: true,
            reason: "ok".into(),
        },
    }
}

fn not_applicable(reason: &str) -> Capability {
    Capability {
        available: false,
        reason: format!("not applicable: {reason}"),
    }
}

#[test]
fn another_projects_brief_has_no_citations_no_ui_captures_and_its_own_conventions() {
    let dir = tempfile::tempdir().unwrap();
    api_with_php_application(dir.path());

    for lang in [RoomLang::En, RoomLang::Fr] {
        let brief = brief_of(lang, dir.path());

        for kronn_only in [
            "## Doc citations",
            "## Citations de documentation",
            "## UI captures",
            "## Captures d'UI",
            "cargo clippy",
        ] {
            assert!(!brief.contains(kronn_only), "{kronn_only}: {brief}");
        }
        assert!(brief.contains("yarn at `.`") || brief.contains("yarn à `.`"));
        assert!(brief.contains("composer"), "{brief}");
        // The project's own agent instructions are pointed at, with an excerpt.
        assert!(brief.contains("`AGENTS.md`"), "{brief}");
        assert!(
            brief.contains("Run `make test` before you commit."),
            "{brief}"
        );
    }
}

#[test]
fn a_project_with_a_web_ui_and_cited_docs_keeps_both_sections() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"react":"18"}}"#,
    );
    put(
        dir.path(),
        "docs/guide.md",
        "See [src: file: src/app.ts:1].\n",
    );

    let en = brief_of(RoomLang::En, dir.path());
    let fr = brief_of(RoomLang::Fr, dir.path());

    assert!(en.contains("## Doc citations") && en.contains("## UI captures"));
    assert!(fr.contains("## Citations de documentation") && fr.contains("## Captures d'UI"));
}

#[test]
fn a_project_with_an_unknown_toolchain_gets_the_generic_gate_not_the_cargo_one() {
    // A Python project: no manifest Kronn detects.
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "pyproject.toml", "[project]\n");
    assert!(detect_toolchains(dir.path()).is_empty());

    for lang in [RoomLang::En, RoomLang::Fr] {
        let brief = brief_of(lang, dir.path());
        assert!(!brief.contains("cargo clippy"), "{brief}");
        assert!(!brief.contains("cargo fmt"), "{brief}");
    }
}

#[test]
fn a_root_node_app_that_tests_with_its_own_runner_is_never_told_its_tests_are_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    api_with_php_application(dir.path());
    let profile = detect_toolchains(dir.path());
    let caps = capabilities(
        not_applicable("no Cargo.toml in the worktree"),
        not_applicable("no Node root of the worktree uses vitest"),
    );

    for lang in [RoomLang::En, RoomLang::Fr] {
        let section = toolchain_brief_section_in(&caps, Some(&profile), lang);

        assert!(!section.contains("vitest`)"), "{section}");
        assert!(!section.contains("❌ unavailable"), "{section}");
        assert!(!section.contains("❌ indisponible"), "{section}");
        assert!(!section.contains("Rust tests"), "{section}");
        assert!(
            section.contains("project's own test commands")
                || section.contains("propres au projet"),
            "{section}"
        );
    }
}

#[test]
fn a_nested_crate_gets_its_rust_line_and_a_vitest_root_its_frontend_line() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "package.json",
        r#"{"devDependencies":{"vitest":"1"}}"#,
    );
    put(dir.path(), "redis-flush/Cargo.toml", "");
    let profile = detect_toolchains(dir.path());
    let ok = |reason: &str| Capability {
        available: true,
        reason: reason.into(),
    };
    let caps = capabilities(ok("nextest works"), ok("vitest 1 runs"));

    let section = toolchain_brief_section_in(&caps, Some(&profile), RoomLang::En);

    assert!(
        section.contains("Rust tests (`cargo nextest`): ✅"),
        "{section}"
    );
    assert!(
        section.contains("Frontend tests (`vitest`): ✅"),
        "{section}"
    );
}
