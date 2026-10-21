//! KT-1404 — the repository facts several audit steps state, computed once.
//!
//! Two documents of one audit disagreed on whether `tests/` was empty (43 PHP
//! test files in the other), two on the Node version CI runs, and a security
//! pass wrote "no hardcoded secrets" over a committed token. Each step read
//! the repository on its own, so each could be wrong on its own. These facts
//! are computed by Kronn from `git ls-files` and the CI files, and handed to
//! the steps that state them: the project index, the testing document, the
//! operations document, the security pass, the debt index and the final review
//! (which makes every document agree with them).

use std::path::Path;

use crate::core::{ci_facts, secret_facts::SecretFacts};

/// The tracked files, `None` when `project` is not a git repository.
pub fn tracked_paths(project: &Path) -> Option<Vec<String>> {
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(project)
        .args(["ls-files", "-z", "--cached"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|b| *b == 0)
            .filter_map(|raw| std::str::from_utf8(raw).ok())
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// The CI block of a repository whose CI files Kronn did not find: a statement
/// about what it read, never a claim that the repository has no CI.
const NO_CI: &str = "## CI facts — computed by Kronn from the workflow files\n\nKronn found no GitHub Actions workflow, `.gitlab-ci.yml` or Jenkins file among the tracked files. It reads no other CI system (CircleCI, Azure Pipelines, Bitbucket Pipelines, Drone): if the repository has one, read it and describe it only as it states; otherwise write that no CI configuration is tracked. Never describe a CI pipeline, a trigger or a CI runtime version you did not read in a file.\n";

/// The facts of one run, each rendered as the prompt block of the steps that state it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFacts {
    pub tests: Option<String>,
    pub ci: String,
    pub secrets: String,
}

impl RepoFacts {
    pub fn collect(project: &Path) -> Self {
        let tracked = tracked_paths(project);
        Self::from_listing(project, tracked.as_deref())
    }

    pub fn from_listing(project: &Path, tracked: Option<&[String]>) -> Self {
        // The secret scan first: its values scrub the CI facts.
        let secrets = SecretFacts::detect(project, tracked);
        Self {
            tests: tracked.map(|tracked| test_block(project, tracked)),
            ci: ci_facts::render_block(&ci_facts::detect_scrubbed(
                project,
                tracked,
                &secrets.literals,
            ))
            .unwrap_or_else(|| NO_CI.to_string()),
            secrets: secrets.render_block(),
        }
    }
}

// ─── Tracked tests ───────────────────────────────────────────────────────

const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "target",
    ".git",
    "dist",
    "build",
    ".yarn",
    ".pnpm-store",
    "docs",
];

/// The most `.rs` files opened for inline tests, and the most bytes of each.
const MAX_INLINE_SCAN: usize = 4_000;
const INLINE_FILE_CAP: u64 = 512 * 1024;

/// The directories named like a test directory.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec", "specs"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Php,
    Js,
    Python,
    Go,
    Rust,
    Jvm,
    Ruby,
    DotNet,
    Bats,
}

impl Group {
    /// What the files of this group are, for the line.
    fn pattern(self) -> &'static str {
        match self {
            Group::Php => "`*Test.php`",
            Group::Js => "`*.test.*`, `*.spec.*`, `*.cy.*`, files under `__tests__/`",
            Group::Python => "`test_*.py`, `*_test.py`",
            Group::Go => "`*_test.go`",
            Group::Rust => "`*_test.rs`, `*_tests.rs`, `.rs` files under `tests/`",
            Group::Jvm => "`*Test.java|kt`, `*Tests.java|kt`, `*IT.java`",
            Group::Ruby => "`*_spec.rb`, `*_test.rb`",
            Group::DotNet => "`*Test.cs`, `*Tests.cs`",
            Group::Bats => "`*.bats`",
        }
    }
}

fn group_of(path: &str) -> Option<Group> {
    let lower = path.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('/').collect();
    if parts.iter().any(|part| SKIPPED_DIRS.contains(part)) {
        return None;
    }
    let name = *parts.last()?;
    let original = path.rsplit('/').next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    let (original_stem, _) = original.rsplit_once('.')?;
    let in_test_dir = parts[..parts.len() - 1]
        .iter()
        .any(|part| TEST_DIRS.contains(part));
    let in_dunder_tests = parts[..parts.len() - 1].contains(&"__tests__");
    match ext {
        "php" if original_stem.ends_with("Test") => Some(Group::Php),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "vue"
            if stem.ends_with(".test")
                || stem.ends_with(".spec")
                || stem.ends_with(".cy")
                || in_dunder_tests =>
        {
            Some(Group::Js)
        }
        "py" if stem.starts_with("test_") || stem.ends_with("_test") => Some(Group::Python),
        "go" if stem.ends_with("_test") => Some(Group::Go),
        "rs" if stem.ends_with("_test")
            || stem.ends_with("_tests")
            || (in_test_dir && parts[..parts.len() - 1].contains(&"tests")) =>
        {
            Some(Group::Rust)
        }
        "java" | "kt"
            if original_stem.ends_with("Test")
                || original_stem.ends_with("Tests")
                || original_stem.ends_with("IT") =>
        {
            Some(Group::Jvm)
        }
        "rb" if stem.ends_with("_spec") || stem.ends_with("_test") => Some(Group::Ruby),
        "cs" if original_stem.ends_with("Test") || original_stem.ends_with("Tests") => {
            Some(Group::DotNet)
        }
        "bats" => Some(Group::Bats),
        _ => None,
    }
}

/// The runner a group is written for, from the tracked configs and the manifests.
fn runner_of(group: Group, project: &Path, tracked: &[String]) -> String {
    let has = |pred: &dyn Fn(&str) -> bool| tracked.iter().any(|p| pred(p));
    let root_file = |name: &str| tracked.iter().any(|p| p == name);
    let manifest_mentions = |manifest: &str, needles: &[&str]| -> Vec<String> {
        let Ok(bytes) = crate::core::rooted_io::read_capped(project, Path::new(manifest), 1 << 20)
        else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&bytes).to_string();
        needles
            .iter()
            .filter(|needle| text.contains(&format!("\"{needle}\"")))
            .map(|needle| needle.to_string())
            .collect()
    };
    match group {
        Group::Php => {
            if has(&|p| p.ends_with("phpunit.xml") || p.ends_with("phpunit.xml.dist"))
                || !manifest_mentions("composer.json", &["phpunit/phpunit"]).is_empty()
            {
                "PHPUnit".into()
            } else if !manifest_mentions("composer.json", &["pestphp/pest"]).is_empty() {
                "Pest".into()
            } else {
                "PHP tests (no runner config tracked)".into()
            }
        }
        Group::Js => {
            let mut runners: Vec<String> = Vec::new();
            for (marker, name) in [
                ("jest.config", "Jest"),
                ("vitest.config", "Vitest"),
                ("playwright.config", "Playwright"),
                ("cypress.config", "Cypress"),
                (".mocharc", "Mocha"),
                ("karma.conf", "Karma"),
            ] {
                let present = has(&|p: &str| {
                    p.rsplit('/')
                        .next()
                        .is_some_and(|file| file.starts_with(marker))
                });
                if present && !runners.iter().any(|r| r == name) {
                    runners.push(name.to_string());
                }
            }
            for (needle, name) in [
                ("jest", "Jest"),
                ("vitest", "Vitest"),
                ("@playwright/test", "Playwright"),
                ("cypress", "Cypress"),
                ("mocha", "Mocha"),
                ("jasmine", "Jasmine"),
            ] {
                if !manifest_mentions("package.json", &[needle]).is_empty()
                    && !runners.iter().any(|r| r == name)
                {
                    runners.push(name.to_string());
                }
            }
            if runners.is_empty() {
                "JS/TS tests (no runner found in the config files or package.json)".into()
            } else {
                runners.join(" + ")
            }
        }
        Group::Python => {
            if root_file("pytest.ini")
                || has(&|p: &str| p.ends_with("conftest.py"))
                || root_file("tox.ini")
            {
                "pytest".into()
            } else {
                "Python tests (pytest or unittest)".into()
            }
        }
        Group::Go => "go test".into(),
        Group::Rust => "cargo test".into(),
        Group::Jvm => "JUnit-style tests".into(),
        Group::Ruby => "RSpec / Minitest".into(),
        Group::DotNet => ".NET tests".into(),
        Group::Bats => "bats".into(),
    }
}

/// The tracked test files per runner and the test directories, from `git ls-files`.
pub fn test_block(project: &Path, tracked: &[String]) -> String {
    let mut counts: std::collections::BTreeMap<Group, usize> = std::collections::BTreeMap::new();
    let mut inline_rust = 0usize;
    let mut opened = 0usize;
    for path in tracked {
        if let Some(group) = group_of(path) {
            *counts.entry(group).or_default() += 1;
        } else if path.ends_with(".rs")
            && !path
                .split('/')
                .any(|part| SKIPPED_DIRS.contains(&part.to_ascii_lowercase().as_str()))
            && opened < MAX_INLINE_SCAN
        {
            // Rust tests live beside the code: `#[cfg(test)] mod tests` in `src/`.
            opened += 1;
            if let Ok(bytes) =
                crate::core::rooted_io::read_capped(project, Path::new(path), INLINE_FILE_CAP)
            {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("#[cfg(test)]")
                    || text.contains("#[cfg(all(test")
                    || text.contains("#[test]")
                {
                    inline_rust += 1;
                }
            }
        }
    }
    // Directories named like a test directory, with every tracked file below.
    let mut dirs: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for path in tracked {
        let parts: Vec<&str> = path.split('/').collect();
        if parts
            .iter()
            .any(|part| SKIPPED_DIRS.contains(&part.to_ascii_lowercase().as_str()))
        {
            continue;
        }
        if let Some(at) = parts[..parts.len().saturating_sub(1)]
            .iter()
            .position(|part| TEST_DIRS.contains(&part.to_ascii_lowercase().as_str()))
        {
            if at <= 2 {
                *dirs.entry(parts[..=at].join("/")).or_default() += 1;
            }
        }
    }
    let mut out = String::from(
        "## Test facts — computed by Kronn from `git ls-files` (ground truth: the tracked test files, counted by Kronn)\n\n",
    );
    if counts.is_empty() && dirs.is_empty() && inline_rust == 0 {
        out.push_str(
            "- Tracked test files found by Kronn: none (no test file by name, no `.rs` file with an inline test module, no `tests/`-style directory).\n",
        );
    }
    for (group, count) in &counts {
        let inline = if *group == Group::Rust && inline_rust > 0 {
            format!(", plus at least {inline_rust} `.rs` file(s) with an inline `#[cfg(test)]` / `#[test]`")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "- {}: at least {count} tracked test file(s) ({}){inline}\n",
            runner_of(*group, project, tracked),
            group.pattern()
        ));
    }
    if inline_rust > 0 && !counts.contains_key(&Group::Rust) {
        out.push_str(&format!(
            "- cargo test: no test file by name, but at least {inline_rust} tracked `.rs` file(s) with an inline `#[cfg(test)]` / `#[test]`\n"
        ));
    }
    let mut dirs: Vec<(String, usize)> = dirs.into_iter().collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (dir, count) in dirs.iter().take(5) {
        out.push_str(&format!(
            "- `{dir}/` holds {count} tracked file(s) in total\n"
        ));
    }
    out.push_str(
        "\nRules for every statement about tests:\n\
         1. These are FLOORS, counted by Kronn from `git ls-files` and, for Rust, the files that carry an inline test attribute: a project can test in ways Kronn does not see (other naming, tests inside other source files, a separate repository). State a test-file count as `at least N` as given here, and give the same one in every document (the project index, the repo map, the testing document, the debt sheets). Never write that `tests/` is empty, or that the project has no tests, when a count above is not zero.\n\
         2. When Kronn found none, write `no test file found by Kronn's name-based count`, then read the repository before saying more; this is not a statement that no test exists.\n\
         3. A count of test methods or cases is not known to Kronn: count it in the files you read, name what you counted, or write `not counted`.\n\
         4. Where the tests run (a CI job, a label, a schedule) comes from the CI facts, not from this block.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn listing(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn the_php_tests_are_counted_per_runner_and_the_directory_is_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "phpunit.xml.dist", "<phpunit/>");
        let mut paths = vec!["phpunit.xml.dist".to_string()];
        for n in 0..43 {
            paths.push(format!("tests/Unit/Case{n}Test.php"));
        }
        paths.push("tests/bootstrap.php".to_string());
        paths.push("src/Kernel.php".to_string());
        let block = test_block(tmp.path(), &paths);
        assert!(
            block.contains("- PHPUnit: at least 43 tracked test file(s) (`*Test.php`)"),
            "{block}"
        );
        assert!(
            block.contains("- `tests/` holds 44 tracked file(s) in total"),
            "{block}"
        );
        assert!(
            block.contains("Never write that `tests/` is empty"),
            "{block}"
        );
    }

    #[test]
    fn js_tests_name_the_runner_found_in_the_manifest_and_the_configs() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "package.json",
            "{\"devDependencies\": {\"jest\": \"^29\"}}",
        );
        let paths = listing(&[
            "package.json",
            "jest.config.js",
            "src/a.test.js",
            "src/b.spec.ts",
            "src/__tests__/c.js",
            "src/plain.js",
        ]);
        let block = test_block(tmp.path(), &paths);
        assert!(
            block.contains("- Jest: at least 3 tracked test file(s)"),
            "{block}"
        );
    }

    #[test]
    fn a_project_with_no_test_says_none() {
        let tmp = tempfile::tempdir().unwrap();
        let block = test_block(tmp.path(), &listing(&["src/main.rs", "README.md"]));
        assert!(
            block.contains("Tracked test files found by Kronn: none"),
            "{block}"
        );
        // The absence is Kronn's count, never a prohibition to mention tests.
        assert!(
            block.contains("no test file found by Kronn's name-based count"),
            "{block}"
        );
        assert!(
            block.contains("not a statement that no test exists"),
            "{block}"
        );
        assert!(!block.contains("never claim tests exist"), "{block}");
    }

    #[test]
    fn vendored_and_documented_files_are_not_tests() {
        let tmp = tempfile::tempdir().unwrap();
        let block = test_block(
            tmp.path(),
            &listing(&[
                "vendor/x/FooTest.php",
                "node_modules/y/a.test.js",
                "docs/tests/a.md",
                "docs/FooTest.php",
            ]),
        );
        assert!(
            block.contains("Tracked test files found by Kronn: none"),
            "{block}"
        );
    }

    #[test]
    fn inline_rust_tests_are_counted_and_worded_as_a_floor() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
        );
        write(tmp.path(), "src/main.rs", "fn main() {}\n");
        let block = test_block(tmp.path(), &listing(&["src/lib.rs", "src/main.rs"]));
        assert!(
            block.contains("at least 1 tracked `.rs` file(s) with an inline `#[cfg(test)]`"),
            "{block}"
        );
        assert!(!block.contains("found by Kronn: none"), "{block}");
        assert!(block.contains("These are FLOORS"), "{block}");
        // Beside a test file by name, the inline files are added to its line.
        let block = test_block(
            tmp.path(),
            &listing(&["src/lib.rs", "src/main.rs", "crate/tests/it.rs"]),
        );
        assert!(
            block.contains("cargo test: at least 1 tracked test file(s)")
                && block.contains("plus at least 1 `.rs` file(s) with an inline"),
            "{block}"
        );
    }

    #[test]
    fn go_python_and_rust_are_each_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let block = test_block(
            tmp.path(),
            &listing(&[
                "a/x_test.go",
                "py/test_a.py",
                "py/b_test.py",
                "py/conftest.py",
                "crate/tests/it.rs",
                "crate/src/lib_test.rs",
            ]),
        );
        assert!(
            block.contains("- go test: at least 1 tracked test file(s)"),
            "{block}"
        );
        assert!(
            block.contains("- pytest: at least 2 tracked test file(s)"),
            "{block}"
        );
        assert!(
            block.contains("- cargo test: at least 2 tracked test file(s)"),
            "{block}"
        );
    }

    #[test]
    fn the_collected_facts_carry_each_block() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".github/workflows/ci.yml",
            "on: push\njobs:\n  t:\n    runs-on: x\n    steps: []\n",
        );
        let paths = listing(&[".github/workflows/ci.yml", "tests/AThingTest.php"]);
        let facts = RepoFacts::from_listing(tmp.path(), Some(&paths));
        assert!(facts.tests.unwrap().contains("tracked test file(s)"));
        assert!(facts.ci.contains("runs on: push"));
        assert!(facts.secrets.contains("Secret facts"));
        // Without a git listing the tests are unknown, the secrets "not evaluated".
        let facts = RepoFacts::from_listing(tmp.path(), None);
        assert_eq!(facts.tests, None);
        assert!(facts.secrets.contains("Not evaluated safely"));
    }

    #[test]
    fn a_repository_without_a_readable_ci_file_is_told_so_and_never_to_invent_one() {
        let tmp = tempfile::tempdir().unwrap();
        let facts = RepoFacts::from_listing(tmp.path(), Some(&listing(&["src/main.rs"])));
        assert!(
            facts.ci.contains("found no GitHub Actions workflow"),
            "{}",
            facts.ci
        );
        assert!(
            facts.ci.contains("Never describe a CI pipeline"),
            "{}",
            facts.ci
        );
    }

    #[test]
    fn a_detected_literal_stays_out_of_the_ci_facts_in_every_condition_form() {
        // Short and built apart: only the repository-wide scrub and the
        // operand-form redaction can catch it, and no secret sits in this file.
        let marker = ["q7Zk", "2mX9"].concat();
        let forms = [
            format!("env.DEPLOY_TOKEN == '{marker}'"),
            format!("contains(env.DEPLOY_TOKEN, '{marker}')"),
            format!("'{marker}' == env.DEPLOY_TOKEN"),
            // Not an operand of a context: only the detected value gives it away.
            format!("contains(github.ref_name, '{marker}')"),
        ];
        for condition in forms {
            let tmp = tempfile::tempdir().unwrap();
            write(
                tmp.path(),
                ".github/workflows/ci.yml",
                &format!(
                    "on: push\nenv:\n  DEPLOY_TOKEN: {marker}\njobs:\n  deploy:\n    if: {condition}\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ready\n"
                ),
            );
            let facts =
                RepoFacts::from_listing(tmp.path(), Some(&listing(&[".github/workflows/ci.yml"])));
            assert!(
                facts.secrets.contains("literal credential"),
                "the fixture must be detected: {}",
                facts.secrets
            );
            assert!(!facts.ci.contains(&marker), "{condition}: {}", facts.ci);
            assert!(!facts.secrets.contains(&marker));
            assert!(facts.ci.contains("if: "), "{}", facts.ci);
        }
    }

    #[test]
    fn a_git_repository_is_listed_and_a_plain_folder_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(tracked_paths(tmp.path()), None);
        let git = |args: &[&str]| {
            let status = crate::core::cmd::git_cmd()
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        write(tmp.path(), "tests/AThingTest.php", "<?php");
        write(tmp.path(), "untracked.txt", "x");
        git(&["add", "tests/AThingTest.php"]);
        assert_eq!(
            tracked_paths(tmp.path()),
            Some(vec!["tests/AThingTest.php".to_string()])
        );
    }
}
