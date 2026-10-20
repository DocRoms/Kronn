use super::*;

const MAKEFILE: &str = "\
.PHONY: test lint
NAME := app
test: ## run the suite
\tphpunit
lint check:
\tphpcs
build-%:
\tdocker build $*
aws:
\t./aws.sh
";

const PACKAGE: &str = r#"{
  "scripts": {"dev": "vite", "build": "vite build", "lint:fix": "eslint --fix"},
  "dependencies": {"vue": "3"},
  "devDependencies": {"eslint": "9", "vitest": "2"}
}"#;

const COMPOSER: &str = r#"{"scripts": {"cs": "phpcs", "test": ["phpunit"]}}"#;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let path = dir.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    dir
}

fn problems(files: &[(&str, &str)], doc: &str) -> Vec<String> {
    let dir = project(files);
    ProjectCommands::scan(dir.path()).problems(doc)
}

#[test]
fn a_make_target_the_makefile_does_not_define_is_a_problem() {
    // docroms: `make yarn-install` in a runbook, no such target.
    let found = problems(
        &[("Makefile", MAKEFILE)],
        "Setup: `make yarn-install` then `make test`.\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("`make yarn-install` is not a target"));
    assert!(
        found[0].contains("test"),
        "lists the real targets: {found:?}"
    );
}

#[test]
fn defined_pattern_and_multi_name_targets_pass() {
    let doc = "`make test`, `make lint`, `make check`, `make build-php`, `make aws`\n";
    assert!(problems(&[("Makefile", MAKEFILE)], doc).is_empty());
}

#[test]
fn a_variable_assignment_or_a_phony_declaration_defines_no_target() {
    let found = problems(&[("Makefile", MAKEFILE)], "`make NAME` and `make .PHONY`\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("`make NAME`"));
}

#[test]
fn a_target_of_any_makefile_in_the_tree_counts() {
    let files = [
        ("docker/Makefile", "up:\n\tdocker compose up\n"),
        ("api/rules.mk", "seed:\n\ttrue\n"),
    ];
    assert!(problems(&files, "`make up` and `make seed`\n").is_empty());
}

#[test]
fn no_makefile_at_all_is_said_so() {
    let found = problems(&[("README.md", "x")], "```bash\nmake install\n```\n");
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("has no Makefile"), "{found:?}");
}

#[test]
fn a_target_built_by_make_itself_is_not_judged() {
    let doc = "`make anything`\n";
    assert!(problems(&[("Makefile", "$(TARGETS):\n\ttrue\n")], doc).is_empty());
}

#[test]
fn make_with_another_directory_or_file_is_not_verifiable() {
    let doc = "`make -C docker up` and `make -f ci.mk deploy`\n";
    assert!(problems(&[("Makefile", MAKEFILE)], doc).is_empty());
}

#[test]
fn only_the_first_target_of_an_inline_span_is_judged_but_a_fence_line_judges_all() {
    let files = [("Makefile", MAKEFILE)];
    assert!(problems(&files, "`make test then read the log`\n").is_empty());
    let found = problems(&files, "```\nmake test nope\n```\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("`make nope`"));
}

#[test]
fn yarn_install_without_any_package_json_is_a_problem() {
    // docroms: Yarn lives in the dev image; the repository has no package.json.
    let found = problems(&[("Makefile", MAKEFILE)], "```bash\nyarn install\n```\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`yarn install`"));
    assert!(found[0].contains("no package.json"));
    assert!(problems(&[("package.json", PACKAGE)], "```bash\nyarn install\n```\n").is_empty());
}

#[test]
fn a_package_script_must_exist_in_some_package_json() {
    let files = [("front/package.json", PACKAGE)];
    let doc = "`yarn dev`, `npm run build`, `pnpm run lint:fix`, `yarn deploy`, `npm run ship`\n";
    let found = problems(&files, doc);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("`yarn deploy` is not a script"));
    assert!(found[1].starts_with("`npm run ship` is not a script"));
    assert!(found[1].contains("dev, lint:fix") || found[1].contains("build, dev"));
}

#[test]
fn a_declared_binary_runs_through_yarn_but_npm_and_pnpm_run_run_scripts_only() {
    let files = [("package.json", PACKAGE)];
    assert!(problems(&files, "`yarn eslint .` and `yarn tsc` and `yarn vitest`\n").is_empty());
    // `yarn run <bin>` runs an installed binary too.
    assert!(problems(&files, "`yarn run vitest` and `yarn run eslint . --fix`\n").is_empty());
    let found = problems(
        &files,
        "`npm vitest` and `npm run vitest` and `pnpm run vitest`\n",
    );
    assert_eq!(found.len(), 3, "{found:?}");
    // A binary no dependency brings is still unknown.
    let found = problems(&files, "`yarn run prettier .`\n");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn flags_and_their_values_do_not_hide_the_script() {
    let files = [("package.json", PACKAGE)];
    let found = problems(
        &files,
        "`pnpm --filter web run nope` and `npm --prefix web run dev`\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("`pnpm run nope`"), "{found:?}");
}

#[test]
fn global_or_neutral_node_commands_are_not_judged() {
    let doc = "`npm install -g yarn`, `yarn global add x`, `npx vitest`, `npm init`, `yarn dlx x`, `npm config get registry`\n";
    assert!(problems(&[("README.md", "x")], doc).is_empty());
}

#[test]
fn composer_scripts_and_project_commands_are_checked() {
    let files = [("composer.json", COMPOSER)];
    let doc = "`composer cs`, `composer run-script test`, `composer install`, `composer qa`, `composer run lint`\n";
    let found = problems(&files, doc);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("`composer qa`"));
    assert!(found[1].starts_with("`composer run lint`"));
    let found = problems(&[("README.md", "x")], "`composer install`\n");
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("no composer.json"));
    assert!(problems(&[("README.md", "x")], "`composer create-project a/b`\n").is_empty());
}

#[test]
fn commands_run_elsewhere_prose_placeholders_and_comments_are_not_judged() {
    let doc = "\
Run it inside the container: `docker compose exec php make nothing`.\n\
Use `make <target>` or `yarn $SCRIPT` or `make ${T}`.\n\
The word make and yarn test in prose are not commands.\n\
<!-- `make hidden-in-comment` -->\n\
```bash\n# make commented\nssh host make remote\n```\n";
    assert!(problems(&[("Makefile", MAKEFILE)], doc).is_empty());
}

#[test]
fn a_command_after_cd_or_an_environment_prefix_is_judged() {
    let found = problems(
        &[("Makefile", MAKEFILE)],
        "```bash\ncd app && CI=1 make ghost\n$ make test | tee out\n```\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("`make ghost`"));
}

#[test]
fn a_command_cited_twice_is_reported_once() {
    let found = problems(
        &[("Makefile", MAKEFILE)],
        "`make ghost` and again `make ghost`\n",
    );
    assert_eq!(found.len(), 1);
}

#[test]
fn dependency_and_build_directories_are_not_read() {
    let files = [
        ("Makefile", "test:\n\ttrue\n"),
        ("node_modules/x/package.json", r#"{"scripts":{"leak":"x"}}"#),
        ("vendor/y/Makefile", "leak:\n\ttrue\n"),
    ];
    let found = problems(&files, "`make leak` and `yarn leak`\n");
    assert_eq!(found.len(), 2, "{found:?}");
}

// ─── KT-1387 review: measured on 11 real audit repositories ────────────────

/// What the apollo repository declares: no `lint` script, `eslint` as a
/// dev dependency, a Makefile whose only `.PHONY` entry is `test`.
const APOLLO_PACKAGE: &str = r#"{
  "scripts": {"build": "x", "dev": "x", "doc": "x", "start": "x", "restart": "x", "test": "x"},
  "dependencies": {"@hapi/hapi": "21"},
  "devDependencies": {"@euronews/eslint-config-js": "3", "eslint": "9", "jest": "29"}
}"#;
const APOLLO_MAKEFILE: &str =
    ".PHONY: test\ntest:\n\tjest\nlint:\n\tyarn run eslint . --fix\naws:\n\t./aws.sh\n";

fn apollo() -> [(&'static str, &'static str); 2] {
    [
        ("package.json", APOLLO_PACKAGE),
        ("Makefile", APOLLO_MAKEFILE),
    ]
}

#[test]
fn yarn_run_of_a_declared_binary_is_not_a_problem() {
    // 7 of the 10 false positives: `yarn run eslint` with eslint a dependency.
    for doc in [
        "| Lint / format | `yarn run eslint . --fix` or `make lint` |\n",
        "| Linter (ESLint) | `.eslintrc.js` | `yarn run eslint . --fix` |\n",
        "| Linting (ESLint) | `make lint` or `yarn run eslint .` | `.eslintrc.js` | JavaScript |\n",
        "```bash\nyarn run eslint . --fix  # Auto-fix lint issues\n```\n",
        "Run: `yarn run eslint .` (check) or `yarn run eslint . --fix` (auto-fix)\n",
    ] {
        assert!(problems(&apollo(), doc).is_empty(), "{doc}");
    }
}

#[test]
fn npx_and_npm_exec_are_never_judged() {
    let doc = "`npx eslint .`, `npx create-thing`, `npm exec eslint`, `npm exec -- tsc`\n";
    assert!(problems(&apollo(), doc).is_empty());
    assert!(problems(&[("README.md", "x")], doc).is_empty());
}

#[test]
fn a_line_that_says_the_command_is_unavailable_claims_nothing() {
    // The remaining measured false positive of the docroms runbook.
    let doc = "| Install JS deps | `make yarn-install` → `yarn install` | not available; Node.js/Yarn installed in container |\n";
    assert!(problems(&[("Makefile", "up:\n\ttrue\n")], doc).is_empty());
    for line in [
        "`yarn install` is unavailable: this PHP project has no package.json.\n",
        "Do not run `make absent`: it was removed.\n",
        "`make restart` does not exist; use `docker compose restart`.\n",
        "`yarn install` n'est pas disponible dans ce dépôt.\n",
        "```bash\nyarn install  # no longer available here\n```\n",
    ] {
        assert!(problems(&[("README.md", "x")], line).is_empty(), "{line}");
    }
    // The same command with no such statement is still a claim.
    let found = problems(&[("README.md", "x")], "Run `yarn install` first.\n");
    assert_eq!(found.len(), 1, "{found:?}");
    // A word that only contains the phrase's letters does not exempt the line.
    let found = problems(&[("README.md", "x")], "plan/api: `yarn install`\n");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn yarn_add_remove_is_a_documentation_shorthand_for_two_built_ins() {
    let doc = "| `yarn.lock` | `yarn install` | Lock file | After any `yarn add/remove` |\n";
    assert!(problems(&apollo(), doc).is_empty());
    // Still judged like `yarn add` when there is nothing to add to.
    let found = problems(&[("README.md", "x")], "`yarn add/remove`\n");
    assert_eq!(found.len(), 1, "{found:?}");
    // A slash that is not made of built-ins stays a script name.
    let found = problems(&apollo(), "`yarn build/deploy`\n");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn the_invented_commands_of_the_corpus_are_still_found() {
    // The 8 true positives: an invented target, invented scripts, and Yarn
    // advice for a repository with no manifest.
    let found = problems(
        &apollo(),
        "`make restart`, `make doc`, `yarn lint`, `yarn lint:fix`\n",
    );
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(found[0].starts_with("`make restart` is not a target"));
    assert!(found[1].starts_with("`make doc` is not a target"));
    assert!(found[2].starts_with("`yarn lint` is not a script"));
    assert!(found[3].starts_with("`yarn lint:fix` is not a script"));
    let docroms = [("Makefile", "up:\n\ttrue\nrestart:\n\ttrue\n")];
    let found = problems(
        &docroms,
        "| Install JS deps | `make yarn-install` → `yarn install` |\n",
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[1].contains("no package.json"));
}

#[test]
fn the_exemptions_of_the_probes_stay() {
    let files = [
        (
            "package.json",
            r#"{"scripts":{"test":"x"},"devDependencies":{"eslint":"9","typescript":"5"}}"#,
        ),
        (
            "plugins/example/package.json",
            r#"{"name":"example","scripts":{"custom":"x"}}"#,
        ),
        ("composer.json", r#"{"scripts":{"check":"x"}}"#),
    ];
    for command in [
        "npm ci",
        "npm install",
        "yarn add example",
        "yarn workspace example run custom",
        "pnpm -r",
        "pnpm -r run test",
        "composer install",
        "composer require example/example",
        "npm i -g yarn",
        "yarn run eslint .",
        "yarn eslint .",
        "yarn run tsc",
        "yarn custom",
        "npm run custom",
    ] {
        let doc = format!("`{command}`\n");
        assert!(problems(&files, &doc).is_empty(), "{command}");
    }
    assert!(problems(
        &[("README.md", "x")],
        "Install Yarn with `npm i -g yarn`.\n"
    )
    .is_empty());
    assert!(problems(&[("README.md", "x")], "`pnpm -r`\n").is_empty());
    let found = problems(&[("README.md", "x")], "`composer install`\n");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_target_only_phony_declares_counts() {
    let files = [("Makefile", ".PHONY: ready\n")];
    assert!(problems(&files, "`make ready`\n").is_empty());
    let continued = [("Makefile", ".PHONY: a \\\n  b c # comment\n")];
    assert!(problems(&continued, "`make b` `make c` `make a`\n").is_empty());
    let found = problems(&continued, "`make d`\n");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_rule_indented_with_spaces_is_read() {
    let files = [(
        "Makefile",
        "  build:\n\t@echo build\n   serve dev: build\n\ttrue\n",
    )];
    assert!(problems(&files, "`make build`, `make serve` and `make dev`\n").is_empty());
}

#[test]
fn an_included_file_is_read_whatever_its_extension() {
    let included = |extra_name: &str, extra: &str, line: &str| {
        let files = [("Makefile", line), (extra_name, extra)];
        problems(&files, "`make build`\n")
    };
    let rule = "build:\n\t@echo build\n";
    assert!(included("tasks.mk", rule, "include tasks.mk\n").is_empty());
    assert!(included("tasks.rules", rule, "include tasks.rules\n").is_empty());
    assert!(included("tasks.rules", rule, "-include tasks.rules # tasks\n").is_empty());
    // Not included: its targets are not the Makefile's.
    let found = included("tasks.rules", rule, "all:\n\ttrue\n");
    assert_eq!(found.len(), 1, "{found:?}");
    // From the project root, from a subdirectory Makefile, and by pattern.
    let files = [
        ("Makefile", "include make/*.rules\n"),
        ("make/a.rules", "build:\n\ttrue\n"),
    ];
    assert!(problems(&files, "`make build`\n").is_empty());
    let files = [
        ("api/Makefile", "include ../shared/tasks.rules\n"),
        ("shared/tasks.rules", "build:\n\ttrue\n"),
    ];
    assert!(problems(&files, "`make build`\n").is_empty());
    // A path built from a variable is not followed, and a loop ends.
    let files = [(
        "Makefile",
        "include $(DIR)/x.rules\ninclude Makefile\nall:\n\ttrue\n",
    )];
    let found = problems(&files, "`make build`\n");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn recipes_conditionals_and_macros_do_not_declare_targets() {
    let makefile = "\
ifeq ($(MODE),a:b)
mode:\n\ttrue
endif
define help
usage: make nope
endef
build:
\tdocker run \\
\t  -v $(PWD):/app \\
\t  image:latest
\tdone: here
export PATH := /bin:$(PATH)
CFLAGS ::= -O2
";
    let files = [("Makefile", makefile)];
    assert!(problems(&files, "`make build` and `make mode`\n").is_empty());
    let found = problems(
        &files,
        "`make nope` `make usage` `make done` `make image`\n",
    );
    assert_eq!(found.len(), 4, "{found:?}");
}

/// A project at `outer/proj` next to files only the outside holds.
fn jailed(files: &[(&str, &str)], outside: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let outer = tempfile::tempdir().unwrap();
    let proj = outer.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    for (rel, body) in files {
        let path = proj.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    for (rel, body) in outside {
        let path = outer.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    (outer, proj)
}

const SENTINEL: &str = "OUTSIDE_REVIEW_SENTINEL";

fn outside_rules() -> String {
    format!("{SENTINEL}:\n\ttrue\nask:\n\ttrue\n")
}

/// The diagnostics for `make ask`, which no Makefile in the project defines.
fn asked_for(proj: &Path) -> Vec<String> {
    ProjectCommands::scan(proj).problems("`make ask`\n")
}

/// `make ask` is still unknown, and no outside target name reached the message.
fn assert_nothing_leaks(found: &[String]) {
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(!found[0].contains(SENTINEL), "{found:?}");
}

#[test]
fn an_include_cannot_leave_the_project_by_parent_directories() {
    let outside = outside_rules();
    let (_outer, proj) = jailed(
        &[("Makefile", "include ../outside.rules\nall:\n\ttrue\n")],
        &[("outside.rules", &outside)],
    );
    assert_nothing_leaks(&asked_for(&proj));
    // From a subdirectory Makefile, `../..` is outside as well.
    let (_outer, proj) = jailed(
        &[("api/Makefile", "include ../../outside.rules\n")],
        &[("outside.rules", &outside)],
    );
    assert_nothing_leaks(&asked_for(&proj));
    // And a path that goes down and back up past the root.
    let (_outer, proj) = jailed(
        &[("Makefile", "include a/../../outside.rules\n")],
        &[("outside.rules", &outside)],
    );
    assert_nothing_leaks(&asked_for(&proj));
}

#[test]
fn an_include_by_absolute_path_outside_the_project_is_refused() {
    let outside = outside_rules();
    let (outer, proj) = jailed(&[], &[("outside.rules", &outside)]);
    let absolute = outer.path().join("outside.rules");
    fs::write(
        proj.join("Makefile"),
        format!("include {}\nall:\n\ttrue\n", absolute.display()),
    )
    .unwrap();
    assert_nothing_leaks(&asked_for(&proj));
}

#[cfg(unix)]
#[test]
fn an_include_through_a_symlink_out_of_the_project_is_refused() {
    use std::os::unix::fs::symlink;
    let outside = outside_rules();
    // A link to an outside file, a link in the middle of the path, and a
    // pattern whose directory is a link.
    let (outer, proj) = jailed(
        &[(
            "Makefile",
            "include linked.rules\ninclude linkdir/inner.rules\ninclude glob/*.rules\n",
        )],
        &[
            ("outside.rules", &outside),
            ("outdir/inner.rules", &outside),
        ],
    );
    symlink(
        outer.path().join("outside.rules"),
        proj.join("linked.rules"),
    )
    .unwrap();
    symlink(outer.path().join("outdir"), proj.join("linkdir")).unwrap();
    symlink(outer.path().join("outdir"), proj.join("glob")).unwrap();
    assert_nothing_leaks(&asked_for(&proj));
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_project_is_not_followed_either() {
    use std::os::unix::fs::symlink;
    let (_outer, proj) = jailed(
        &[
            ("Makefile", "include link.rules\n"),
            ("real.rules", "other:\n\ttrue\n"),
        ],
        &[],
    );
    symlink(proj.join("real.rules"), proj.join("link.rules")).unwrap();
    // Refused like any link: the read never follows one below the root. The
    // real file is still read directly when something includes it by name.
    assert_eq!(asked_for(&proj).len(), 1);
}

#[test]
fn a_glob_whose_directory_is_outside_the_project_is_refused() {
    let outside = outside_rules();
    let (outer, proj) = jailed(
        &[("Makefile", "include ../outdir/*.rules\n")],
        &[("outdir/a.rules", &outside)],
    );
    assert_nothing_leaks(&asked_for(&proj));
    fs::write(
        proj.join("Makefile"),
        format!("include {}/outdir/*.rules\n", outer.path().display()),
    )
    .unwrap();
    assert_nothing_leaks(&asked_for(&proj));
}

#[test]
fn includes_inside_the_project_still_work() {
    let (_outer, proj) = jailed(
        &[
            (
                "Makefile",
                "include make/tasks.rules\ninclude make/*.inc\ninclude ./up/../make/more.rules\n",
            ),
            ("make/tasks.rules", "ask:\n\ttrue\n"),
            ("make/a.inc", "second:\n\ttrue\n"),
            ("make/more.rules", "third:\n\ttrue\n"),
            ("api/Makefile", "include ../make/tasks.rules\n"),
        ],
        &[],
    );
    let found = ProjectCommands::scan(&proj).problems("`make ask` `make second` `make third`\n");
    assert!(found.is_empty(), "{found:?}");
    // An absolute path that lands inside the project is inside the project.
    fs::write(
        proj.join("Makefile"),
        format!("include {}/make/tasks.rules\n", proj.display()),
    )
    .unwrap();
    assert!(asked_for(&proj).is_empty());
}

#[test]
fn what_includes_make_us_read_is_bounded() {
    let mut files: Vec<(String, String)> = (0..MAX_INCLUDE_FILES + 20)
        .map(|n| (format!("many/{n:04}.rules"), format!("t{n}:\n\ttrue\n")))
        .collect();
    files.push((
        "Makefile".into(),
        "include many/*.rules\ninclude big.rules\n".into(),
    ));
    // One oversized include is skipped whole.
    files.push((
        "big.rules".into(),
        format!(
            "huge:\n\ttrue\n#{}\n",
            "x".repeat(MAX_INCLUDE_FILE_BYTES as usize)
        ),
    ));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let dir = project(&refs);
    let found = ProjectCommands::scan(dir.path());
    assert!(found.included_files <= MAX_INCLUDE_FILES);
    assert!(!found.make_targets.contains("huge"));
}
