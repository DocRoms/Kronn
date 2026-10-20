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
fn a_bare_yarn_name_may_be_an_installed_binary_but_run_may_not() {
    let files = [("package.json", PACKAGE)];
    assert!(problems(&files, "`yarn eslint .` and `yarn tsc` and `yarn vitest`\n").is_empty());
    let found = problems(&files, "`yarn run vitest` and `npm vitest`\n");
    assert_eq!(found.len(), 2, "{found:?}");
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
