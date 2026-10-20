use super::*;

fn put(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let status = crate::core::cmd::git_cmd()
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A repository whose history is `subjects`, oldest first.
fn repo(subjects: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    for subject in subjects {
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", subject],
        );
    }
    dir
}

fn repeated(subject: &'static str, times: usize) -> Vec<&'static str> {
    std::iter::repeat_n(subject, times).collect()
}

fn log(subjects: &[&str]) -> String {
    subjects
        .iter()
        .enumerate()
        .map(|(i, s)| format!("\u{1e}sha{i:04}\u{1f}{s}\u{1f}\u{1f}\n"))
        .collect()
}

#[test]
fn subjects_are_read_by_shape() {
    let cases = [
        (
            "feat(seo) ajoute le sitemap",
            Form::ScopedPlain,
            Some("feat"),
            Some("seo"),
        ),
        (
            "fix(a11y): corrige le focus",
            Form::ScopedColon,
            Some("fix"),
            Some("a11y"),
        ),
        ("docs: update readme", Form::BareColon, Some("docs"), None),
        ("update readme", Form::Free, None, None),
        ("Fix the thing", Form::Free, None, None),
        (
            "feat(x)!: drop v1",
            Form::ScopedColon,
            Some("feat"),
            Some("x"),
        ),
        ("wip", Form::Free, None, None),
    ];
    for (subject, form, kind, scope) in cases {
        let parsed = parse_subject(subject);
        assert_eq!(parsed.form, form, "{subject}");
        assert_eq!(parsed.kind.as_deref(), kind, "{subject}");
        assert_eq!(parsed.scope.as_deref(), scope, "{subject}");
    }
}

#[test]
fn ticket_keys_are_found_leading_or_after_the_type() {
    let leading = parse_subject("EW-1234 feat(cms) add a block");
    assert_eq!(leading.ticket, Some(("EW".into(), true)));
    assert_eq!(leading.form, Form::ScopedPlain);
    let bracketed = parse_subject("[DD-77] fix: crash");
    assert_eq!(bracketed.ticket, Some(("DD".into(), true)));
    let after = parse_subject("feat(Cms):EW-1234 add a block");
    assert_eq!(after.ticket, Some(("EW".into(), false)));
    assert_eq!(after.form, Form::ScopedColon);
    assert_eq!(parse_subject("fix: no ticket here").ticket, None);
    assert_eq!(parse_subject("UTF-8 handling").ticket, None);
}

#[test]
fn a_french_history_without_colons_is_reported_as_such() {
    // docroms: the history uses `type(scope) subject`, French, no colon; the
    // agent wrote Conventional Commits with the colon.
    let mut subjects = repeated("feat(seo) ajoute le sitemap de la page", 8);
    subjects.extend(repeated("fix(a11y) corrige le focus du menu", 6));
    subjects.push("fix(a11y): corrige un lien");
    subjects.push("Merge branch 'main' into feature");
    let facts = CommitFacts::from_log(&log(&subjects));
    assert_eq!(facts.scoped_plain, 14);
    assert_eq!(facts.scoped_colon, 1);
    assert_eq!(facts.merges, 1);
    assert_eq!(facts.analysed(), 15);
    assert_eq!(facts.dominant_form(), Some(Form::ScopedPlain));
    assert_eq!(facts.types[0], ("feat".to_string(), 8));
    assert_eq!(facts.scopes[0], ("seo".to_string(), 8));
    let block = facts.render_block("docs/workflow/commits.md");
    assert!(
        block.contains("`type(scope) subject` — NO colon: 14 of 15 (93%)"),
        "{block}"
    );
    assert!(
        block.contains("`type(scope): subject` — with colon: 1 of 15 (6%)"),
        "{block}"
    );
    assert!(
        block.contains("written in 1 subject(s), absent in 14"),
        "{block}"
    );
    assert!(block.contains("mostly French"), "{block}");
    assert!(block.contains("[src: commit: sha0000]"), "{block}");
    assert!(block.contains("Ticket keys: none seen"), "{block}");
    assert!(
        block.contains("Rules for `docs/workflow/commits.md`"),
        "{block}"
    );
}

#[test]
fn ticket_prefixes_and_their_place_are_counted() {
    let mut subjects = repeated("feat(Cms):EW-1234 add a block", 7);
    subjects.extend(repeated("fix(Api):DD-9 handle the timeout", 4));
    let facts = CommitFacts::from_log(&log(&subjects));
    assert_eq!(facts.tickets.len(), 2);
    assert_eq!(facts.tickets[0].prefix, "EW");
    assert_eq!(facts.tickets[0].after_type, 7);
    let block = facts.render_block("docs/workflow/commits.md");
    assert!(
        block.contains("`EW-<number>` on 7 (after the type)"),
        "{block}"
    );
    assert!(
        block.contains("`DD-<number>` on 4 (after the type)"),
        "{block}"
    );
    assert!(block.contains("mostly English"), "{block}");
}

#[test]
fn a_short_history_establishes_no_pattern() {
    let facts = CommitFacts::from_log(&log(&["feat: a", "fix: b"]));
    let block = facts.render_block("docs/workflow/commits.md");
    assert!(block.contains("no pattern can be observed"), "{block}");
    assert!(
        block.contains("Not formalized — ask before assuming."),
        "{block}"
    );
    assert!(!block.contains("Subject shape observed"), "{block}");
}

#[test]
fn an_opt_in_commit_target_is_not_enforcement() {
    // apollo: `make commit` -> nicommitizen.sh, with Commitizen in package.json;
    // nothing forces it, yet the doc said it enforces every commit.
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "Makefile",
        "test:\n\tphpunit\n\ncommit:\n\t@./nicommitizen.sh\n",
    );
    put(dir.path(), "nicommitizen.sh", "#!/bin/sh\necho types\n");
    put(
        dir.path(),
        "package.json",
        "{\n  \"devDependencies\": {\n    \"commitizen\": \"4\"\n  }\n}\n",
    );
    let tools = detect_tooling(dir.path());
    let cz = tools.iter().find(|t| t.name == "commitizen").unwrap();
    assert_eq!(cz.enforcement, Enforcement::OptIn);
    assert_eq!(cz.evidence, vec!["package.json:3".to_string()]);
    let make = tools
        .iter()
        .find(|t| t.name.starts_with("Makefile target"))
        .unwrap();
    assert_eq!(make.enforcement, Enforcement::OptIn);
    assert!(make.name.contains("make commit"));
    assert!(make.evidence.contains(&"Makefile:4".to_string()));
    assert!(make.evidence.contains(&"nicommitizen.sh:1".to_string()));
    assert!(!tools
        .iter()
        .any(|t| matches!(t.enforcement, Enforcement::Enforced(_))));

    let mut facts = CommitFacts::from_log(&log(&["feat(a) x"; 12]));
    facts.tooling = tools;
    let block = facts.render_block("docs/workflow/commits.md");
    assert!(
        block.contains("opt-in, run by hand — nothing enforces it"),
        "{block}"
    );
    assert!(!block.contains("ENFORCED by"), "{block}");
    assert!(block.contains("[src: file: nicommitizen.sh:1]"), "{block}");
    assert!(
        block.contains("Read every tool file listed above"),
        "{block}"
    );
}

#[test]
fn a_commit_msg_hook_running_commitlint_is_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "commitlint.config.js", "module.exports = {};\n");
    put(
        dir.path(),
        ".husky/commit-msg",
        "#!/bin/sh\nnpx --no -- commitlint --edit \"$1\"\n",
    );
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].name, "commitlint");
    assert_eq!(
        tools[0].enforcement,
        Enforcement::Enforced(".husky/commit-msg:2".into())
    );
}

#[test]
fn a_config_nothing_runs_is_not_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), ".commitlintrc.json", "{}\n");
    // An empty hook enforces nothing.
    put(dir.path(), ".husky/commit-msg", "#!/bin/sh\n# todo\n");
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].enforcement, Enforcement::ConfiguredOnly);
    let facts = CommitFacts {
        tooling: tools,
        ..CommitFacts::default()
    };
    assert!(facts.render_block("c.md").contains("not enforced"));
}

#[test]
fn a_ci_action_is_enforcement_and_an_unrecognised_hook_is_only_configured() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        ".github/workflows/pr.yml",
        "name: pr\njobs:\n  title:\n    steps:\n      - uses: amannn/action-semantic-pull-request@v5\n",
    );
    put(
        dir.path(),
        ".githooks/commit-msg",
        "#!/bin/sh\ngrep -qE '^(feat|fix)' \"$1\"\n",
    );
    let tools = detect_tooling(dir.path());
    let ci = tools
        .iter()
        .find(|t| t.name == "CI commit-message check")
        .unwrap();
    assert_eq!(
        ci.enforcement,
        Enforcement::Enforced(".github/workflows/pr.yml:5".into())
    );
    // A hook that runs a command Kronn cannot tell is a message checker is
    // listed, but never called enforcement.
    let hook = tools.iter().find(|t| t.name == "commit-msg hook").unwrap();
    assert_eq!(hook.enforcement, Enforcement::ConfiguredOnly);
    assert_eq!(hook.evidence, vec![".githooks/commit-msg:2".to_string()]);
}

#[test]
fn a_tool_named_only_in_a_ci_comment_is_not_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        ".github/workflows/ci.yml",
        "name: CI\n# TODO: add commitlint in a future iteration\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo tests\n",
    );
    assert!(detect_tooling(dir.path()).is_empty());

    // Neither a trailing comment, a label nor an echo runs it.
    put(
        dir.path(),
        ".github/workflows/lint.yml",
        "name: commitlint reminder\njobs:\n  x:\n    steps:\n      - name: Run commitlint\n        run: make lint # commitlint comes later\n      - run: echo \"commitlint is not wired yet\"\n",
    );
    assert!(detect_tooling(dir.path()).is_empty());

    // With the same config file present, the tool is configured, not enforced.
    put(dir.path(), "commitlint.config.js", "module.exports = {};\n");
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].enforcement, Enforcement::ConfiguredOnly);
}

#[test]
fn an_install_step_or_an_env_key_does_not_run_the_checker() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "commitlint.config.js", "module.exports = {};\n");
    for (body, why) in [
        (
            "jobs:\n  x:\n    steps:\n      - run: npm install --save-dev @commitlint/cli\n",
            "install",
        ),
        ("steps:\n  - run: yarn add -D @commitlint/cli\n", "yarn add"),
        ("steps:\n  - run: pip install commitizen gitlint\n", "pip"),
        (
            "jobs:\n  x:\n    steps:\n      - run: make lint\n        env:\n          COMMITLINT_DISABLED: true\n",
            "env key",
        ),
        ("steps:\n  - run: npm ci && echo commitlint\n", "echo"),
        (
            "steps:\n  - uses: actions/checkout@v4\n    with:\n      commitlint-config: commitlint.config.js\n",
            "action input",
        ),
        (
            "steps:\n  - run: gitlint install-hook\n",
            "gitlint install-hook",
        ),
        ("steps:\n  - run: cz commit\n", "cz commit"),
    ] {
        put(dir.path(), ".github/workflows/ci.yml", body);
        let tools = detect_tooling(dir.path());
        assert_eq!(tools.len(), 1, "{why}: {tools:?}");
        assert_eq!(tools[0].enforcement, Enforcement::ConfiguredOnly, "{why}");
    }
    // A hook that only installs the tool checks nothing either.
    let hooked = tempfile::tempdir().unwrap();
    put(
        hooked.path(),
        ".husky/commit-msg",
        "#!/bin/sh\nnpm install --save-dev @commitlint/cli\n",
    );
    assert!(detect_tooling(hooked.path()).is_empty());
}

/// Whether a one-line shell text runs a checker, with the given scripts.
fn runs(command: &str, scripts: &[(&str, &str)]) -> Option<Option<&'static str>> {
    let scripts: Scripts = scripts
        .iter()
        .map(|(name, body)| (name.to_string(), body.to_string()))
        .collect();
    first_checker(&shell_commands(command, 1), &scripts)
}

#[test]
fn a_live_line_that_runs_the_checker_is_enforcement_in_every_form() {
    for (command, tool) in [
        ("npx commitlint --edit $1", "commitlint"),
        ("npx --no -- commitlint --edit \"$1\"", "commitlint"),
        ("yarn commitlint --from origin/main", "commitlint"),
        ("pnpm exec commitlint --edit", "commitlint"),
        ("npm exec -- commitlint --edit", "commitlint"),
        ("node_modules/.bin/commitlint --edit", "commitlint"),
        ("npm ci && npx commitlint --from main", "commitlint"),
        ("CI=1 commitlint --last", "commitlint"),
        ("cz check --rev-range origin/main..HEAD", "commitizen"),
        (
            "python -m commitizen check --commit-msg-file $1",
            "commitizen",
        ),
        ("gitlint --commits origin/main..HEAD", "gitlint"),
        ("sh -c 'npx commitlint --edit'", "commitlint"),
        (
            "if ! npx commitlint --edit \"$1\"; then exit 1; fi",
            "commitlint",
        ),
        ("echo checking; npx commitlint --from HEAD~1", "commitlint"),
    ] {
        assert_eq!(runs(command, &[]), Some(Some(tool)), "{command}");
    }
    for command in [
        "npm install --save-dev @commitlint/cli",
        "npm run build",
        "echo commitlint",
        "COMMITLINT_DISABLED: true",
        "cz commit",
        "pip install gitlint",
        "make lint",
        // KT-1387 round 3: only the executed program counts.
        "echo 'example; commitlint --edit message'",
        "echo \"npx commitlint\" && true",
        "printf 'commitlint --edit' > note",
        "npx commitlint --help",
        "commitlint --version",
        "npx commitlint --print-config",
        "command -v commitlint",
        "which commitlint",
        "cat commitlint.config.js",
        "rm node_modules/.bin/commitlint",
        "x=$(npx commitlint --edit)",
        "echo `npx commitlint`",
        "npm run commitlint",
    ] {
        assert_eq!(runs(command, &[]), None, "{command}");
    }
}

#[test]
fn a_package_script_counts_only_when_its_own_command_runs_the_checker() {
    let real = [("commitlint", "commitlint --from HEAD~1")];
    let echoes = [("commitlint", "echo checker is not configured")];
    for command in [
        "npm run commitlint",
        "npm run-script commitlint",
        "yarn commitlint",
        "yarn run commitlint",
        "pnpm run commitlint",
    ] {
        assert_eq!(runs(command, &real), Some(Some("commitlint")), "{command}");
        assert_eq!(runs(command, &echoes), None, "{command}");
    }
    // A script under another name, calling the tool, counts as well.
    assert_eq!(
        runs(
            "npm run lint:commit",
            &[("lint:commit", "npx commitlint --edit")]
        ),
        Some(Some("commitlint"))
    );
    // One level only: a script that calls another script is not followed.
    assert_eq!(
        runs(
            "npm run a",
            &[("a", "npm run commitlint"), ("commitlint", "commitlint")]
        ),
        None
    );
    // A script the package does not define cannot be resolved.
    assert_eq!(runs("npm run commitlint", &[("test", "jest")]), None);
    assert_eq!(runs("npm run commitlint", &[]), None);
    // A script that only echoes a command in a string.
    assert_eq!(
        runs("npm run c", &[("c", "echo 'commitlint --edit'")]),
        None
    );
}

fn ci_tool(body: &str, package: Option<&str>) -> Vec<Tool> {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), ".github/workflows/ci.yml", body);
    if let Some(package) = package {
        put(dir.path(), "package.json", package);
    }
    detect_tooling(dir.path())
}

#[test]
fn the_reviewers_adversarial_ci_and_hook_cases_are_never_enforced() {
    for (why, body) in [
        (
            "quoted semicolon echo",
            "jobs:\n  t:\n    steps:\n      - run: echo 'example; commitlint --edit message'\n",
        ),
        (
            "--help",
            "jobs:\n  t:\n    steps:\n      - run: npx commitlint --help\n",
        ),
        (
            "env block example",
            "env:\n  DOCUMENTATION_EXAMPLE: |\n    npx commitlint --edit message\njobs:\n  t:\n    steps:\n      - run: echo tests\n",
        ),
        (
            "env entry",
            "jobs:\n  t:\n    env:\n      HOOK: npx commitlint\n    steps:\n      - run: make lint\n",
        ),
        (
            "run example inside an env block scalar",
            "env:\n  EXAMPLE: |\n    run: npx commitlint --edit\njobs:\n  t:\n    steps:\n      - run: make lint\n",
        ),
        (
            "name label",
            "jobs:\n  t:\n    steps:\n      - name: npx commitlint --edit\n        run: make lint\n",
        ),
        (
            "heredoc body",
            "jobs:\n  t:\n    steps:\n      - run: |\n          cat <<EOF\n          npx commitlint --edit\n          EOF\n",
        ),
    ] {
        let tools = ci_tool(body, None);
        assert!(tools.is_empty(), "{why}: {tools:?}");
    }

    // The same words with the checker configured: configured, never enforced.
    let configured = tempfile::tempdir().unwrap();
    put(
        configured.path(),
        "commitlint.config.js",
        "module.exports = {};\n",
    );
    put(
        configured.path(),
        ".github/workflows/ci.yml",
        "jobs:\n  t:\n    steps:\n      - run: echo 'example; commitlint --edit message'\n      - run: npx commitlint --help\n",
    );
    put(
        configured.path(),
        ".husky/commit-msg",
        "#!/bin/sh\necho 'example; commitlint --edit message'\n",
    );
    let tools = detect_tooling(configured.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].enforcement, Enforcement::ConfiguredOnly);
}

#[test]
fn an_npm_script_that_only_echoes_is_configured_and_one_that_runs_is_enforced() {
    let ci = "jobs:\n  t:\n    steps:\n      - run: npm run commitlint\n";
    let echoes = ci_tool(
        ci,
        Some(r#"{"scripts":{"commitlint":"echo checker is not configured"}}"#),
    );
    // Named in package.json, run by nothing real: configured, not enforced.
    assert_eq!(echoes.len(), 1, "{echoes:?}");
    assert_eq!(echoes[0].enforcement, Enforcement::ConfiguredOnly);
    let runs = ci_tool(
        ci,
        Some(
            r#"{"scripts":{"commitlint":"commitlint --from HEAD~1"},"devDependencies":{"@commitlint/cli":"^19"}}"#,
        ),
    );
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(
        runs[0].enforcement,
        Enforcement::Enforced(".github/workflows/ci.yml:4".into())
    );
}

#[test]
fn real_multi_line_and_list_ci_commands_are_enforcement_at_their_line() {
    // GitHub block scalar: the line of the command itself.
    let block = ci_tool(
        "jobs:\n  t:\n    steps:\n      - run: |\n          echo Checking\n          npx commitlint --from HEAD~1\n",
        None,
    );
    assert_eq!(
        block[0].enforcement,
        Enforcement::Enforced(".github/workflows/ci.yml:6".into())
    );
    // GitLab list under `script:`, indented or at the key's own column.
    for list in [
        "lint:\n  script:\n    - echo go\n    - npx commitlint --from HEAD~1\n",
        "lint:\n  script:\n  - echo go\n  - npx commitlint --from HEAD~1\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), ".gitlab-ci.yml", list);
        let tools = detect_tooling(dir.path());
        assert_eq!(tools.len(), 1, "{list}: {tools:?}");
        assert_eq!(
            tools[0].enforcement,
            Enforcement::Enforced(".gitlab-ci.yml:4".into())
        );
    }
    // Jenkins `sh` steps, single and triple quoted.
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "Jenkinsfile",
        "pipeline {\n  stages {\n    stage('x') {\n      steps {\n        // sh 'npx commitlint'\n        sh 'echo hi'\n        sh \"\"\"\n          npx commitlint --from HEAD~1\n        \"\"\"\n      }\n    }\n  }\n}\n",
    );
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(
        tools[0].enforcement,
        Enforcement::Enforced("Jenkinsfile:8".into())
    );
}

#[test]
fn a_shell_text_is_split_like_a_shell_would() {
    let cmds = shell_commands(
        "#!/bin/sh\ncat <<'EOF'\nnpx commitlint\nEOF\necho \"a\\\nb\" ; ls \\\n -l\nx=$(date; npx commitlint)\nnpx commitlint\n",
        1,
    );
    let words: Vec<(usize, Vec<&str>)> = cmds
        .iter()
        .map(|c| (c.line, c.words.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(words[0], (2, vec!["cat"]));
    assert_eq!(words[1], (5, vec!["echo", "ab"]));
    assert_eq!(words[2], (6, vec!["ls", "-l"]));
    assert!(words.iter().any(|(_, w)| w[0] == "ls"), "{words:?}");
    // The substitution is one doubtful command; the last line is its own.
    assert!(cmds.iter().any(|c| c.doubtful));
    let last = cmds.last().unwrap();
    assert_eq!(last.words, ["npx", "commitlint"]);
    assert_eq!(last.line, 9, "{words:?}");
}

#[test]
fn a_ci_action_for_commit_messages_is_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        ".github/workflows/ci.yml",
        "steps:\n  - uses: wagoid/commitlint-github-action@v6\n",
    );
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].name, "commitlint");
    assert_eq!(
        tools[0].enforcement,
        Enforcement::Enforced(".github/workflows/ci.yml:2".into())
    );
}

#[test]
fn a_ci_step_that_runs_commitlint_is_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        ".github/workflows/ci.yml",
        "name: CI\njobs:\n  lint:\n    steps:\n      # commit messages\n      - run: npx commitlint --from origin/main\n",
    );
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].name, "commitlint");
    assert_eq!(
        tools[0].enforcement,
        Enforcement::Enforced(".github/workflows/ci.yml:6".into())
    );
}

#[test]
fn a_hook_that_only_mentions_the_tool_is_not_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        ".husky/commit-msg",
        "#!/bin/sh\n# commitlint is not installed; this hook only reminds us\necho Please review your commit message\n",
    );
    assert!(detect_tooling(dir.path()).is_empty());

    // The husky v8 loader line alone checks nothing either.
    put(
        dir.path(),
        ".husky/commit-msg",
        "#!/bin/sh\n. \"$(dirname \"$0\")/_/husky.sh\"\n",
    );
    assert!(detect_tooling(dir.path()).is_empty());

    // Behind the loader, a real run is enforcement and points at its line.
    put(
        dir.path(),
        ".husky/commit-msg",
        "#!/bin/sh\n. \"$(dirname \"$0\")/_/husky.sh\"\n\nnpx --no-install commitlint --edit \"$1\"\n",
    );
    let tools = detect_tooling(dir.path());
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].name, "commitlint");
    assert_eq!(
        tools[0].enforcement,
        Enforcement::Enforced(".husky/commit-msg:4".into())
    );
}

#[test]
fn a_lefthook_or_pre_commit_comment_is_not_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "lefthook.yml",
        "commit-msg:\n  # commands:\n  #   lint: commitlint --edit\n",
    );
    put(
        dir.path(),
        ".pre-commit-config.yaml",
        "# add gitlint at the commit-msg stage someday\nrepos: []\n",
    );
    assert!(detect_tooling(dir.path()).is_empty());
}

#[test]
fn lefthook_and_pre_commit_commit_msg_stages_are_hooks() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "lefthook.yml",
        "pre-commit:\n  commands:\n    lint:\n      run: make lint\ncommit-msg:\n  commands:\n    lint:\n      run: npx commitlint --edit {1}\n",
    );
    let tools = detect_tooling(dir.path());
    let commitlint = tools.iter().find(|t| t.name == "commitlint").unwrap();
    assert_eq!(
        commitlint.enforcement,
        Enforcement::Enforced("lefthook.yml:5".into())
    );
    // A lefthook with no commit-msg block enforces nothing.
    let none = tempfile::tempdir().unwrap();
    put(
        none.path(),
        "lefthook.yml",
        "pre-commit:\n  commands:\n    x:\n      run: commitlint\n",
    );
    assert!(detect_tooling(none.path()).is_empty());
}

#[test]
fn no_tooling_is_said_so() {
    let dir = tempfile::tempdir().unwrap();
    assert!(detect_tooling(dir.path()).is_empty());
    let block = CommitFacts::default().render_block("docs/workflow/commits.md");
    assert!(block.contains("- none (no commitizen"), "{block}");
}

#[test]
fn the_facts_come_from_the_real_git_history() {
    let mut subjects: Vec<String> = (0..11)
        .map(|i| format!("feat(web) ajoute la page {i}"))
        .collect();
    subjects.push("fix(web): corrige la page".into());
    let refs: Vec<&str> = subjects.iter().map(String::as_str).collect();
    let dir = repo(&refs);
    git(
        dir.path(),
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-s",
            "-m",
            "feat(web) ajoute la signature",
        ],
    );
    put(dir.path(), "Makefile", "commit:\n\t./cz.sh\n");
    let facts = CommitFacts::detect(dir.path());
    assert_eq!(facts.scoped_plain, 12);
    assert_eq!(facts.scoped_colon, 1);
    assert_eq!(facts.signed_off, 1);
    assert_eq!(facts.co_authored, 0);
    assert_eq!(facts.examples.len(), 3);
    assert!(facts.examples.iter().all(|(sha, _)| !sha.is_empty()));
    assert_eq!(facts.tooling.len(), 1);
    let block = facts.render_block("docs/workflow/commits.md");
    assert!(block.contains("`Signed-off-by` on 1 of 13"), "{block}");
    assert!(block.contains("`Co-authored-by` on 0 of 13"), "{block}");
}

#[test]
fn a_directory_that_is_not_a_repository_has_no_history() {
    let dir = tempfile::tempdir().unwrap();
    let facts = CommitFacts::detect(dir.path());
    assert_eq!(facts.analysed(), 0);
    assert!(facts
        .render_block("docs/workflow/commits.md")
        .contains("no pattern can be observed"));
}

/// The tools reported as ENFORCED, by name.
fn enforced(dir: &Path) -> Vec<String> {
    detect_tooling(dir)
        .into_iter()
        .filter(|t| matches!(t.enforcement, Enforcement::Enforced(_)))
        .map(|t| t.name)
        .collect()
}

#[test]
fn a_hook_that_runs_no_checker_is_configured_never_enforced() {
    // The reviewer's context fixtures: `--help`, a wrapped echo, a `cat`.
    for body in [
        "#!/bin/sh\nnpx commitlint --help\n",
        "#!/bin/sh\nsh -c 'echo Reminder only'\n",
        "#!/bin/sh\ncat README.md\n",
        "#!/bin/sh\n./scripts/notify.sh \"$1\"\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "README.md", "Reminder only.\n");
        put(dir.path(), ".husky/commit-msg", body);
        assert!(enforced(dir.path()).is_empty(), "{body}");
        let facts = CommitFacts {
            tooling: detect_tooling(dir.path()),
            ..CommitFacts::default()
        };
        let block = facts.render_block("c.md");
        assert!(!block.contains("ENFORCED by"), "{body}\n{block}");
    }
    // A lefthook `commit-msg` command that runs no checker.
    let lefthook = tempfile::tempdir().unwrap();
    put(
        lefthook.path(),
        "lefthook.yml",
        "commit-msg:\n  commands:\n    notify:\n      run: ./scripts/notify.sh {1}\n    help:\n      run: npx commitlint --help\n",
    );
    assert!(enforced(lefthook.path()).is_empty());
    // Positive controls: a hook that really runs the checker.
    for body in [
        "#!/bin/sh\nnpx commitlint --edit message\n",
        "#!/bin/sh\n. \"$(dirname \"$0\")/_/husky.sh\"\nnpx --no -- commitlint --edit \"$1\"\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), ".husky/commit-msg", body);
        assert_eq!(
            enforced(dir.path()),
            vec!["commitlint".to_string()],
            "{body}"
        );
    }
}

#[test]
fn a_command_key_under_env_with_or_variables_is_not_a_command() {
    let cases = [
        // The reviewer's fixture: `run` is an environment variable.
        "name: CI\non: push\nenv:\n  run: npx commitlint --edit message\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo tests\n",
        "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@v4\n        env:\n          run: npx commitlint --edit message\n        run: echo tests\n",
        "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@v4\n        with:\n          script: npx commitlint --edit message\n",
        "variables:\n  script: npx commitlint --edit message\nbuild:\n  script:\n    - make build\n",
        "jobs:\n  test:\n    env:\n      script:\n        - npx commitlint --edit message\n    steps:\n      - run: make test\n",
        "jobs:\n  test:\n    env:\n      run: |\n        npx commitlint --edit message\n    steps:\n      - run: make test\n",
    ];
    for (index, body) in cases.iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), ".github/workflows/ci.yml", body);
        assert!(enforced(dir.path()).is_empty(), "case {index}:\n{body}");
        assert!(detect_tooling(dir.path()).is_empty(), "case {index}");
    }
    // The same command under a real step is enforcement.
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        ".github/workflows/ci.yml",
        "env:\n  CI: true\njobs:\n  test:\n    steps:\n      - name: lint\n        env:\n          FORCE_COLOR: 1\n        run: npx commitlint --from HEAD~1\n",
    );
    assert_eq!(
        detect_tooling(dir.path())[0].enforcement,
        Enforcement::Enforced(".github/workflows/ci.yml:9".into())
    );
}

#[test]
fn a_step_switched_off_is_not_enforcement() {
    for body in [
        "jobs:\n  t:\n    steps:\n      - if: false\n        run: npx commitlint --from HEAD~1\n",
        "jobs:\n  t:\n    steps:\n      - if: ${{ false }}\n        run: npx commitlint --from HEAD~1\n",
        "jobs:\n  t:\n    steps:\n      - run: npx commitlint --from HEAD~1\n        if: \"false\"\n",
        "jobs:\n  t:\n    steps:\n      - name: x\n        if: false # temporarily\n        run: |\n          npx commitlint --from HEAD~1\n",
        "jobs:\n  t:\n    if: false\n    steps:\n      - run: npx commitlint --from HEAD~1\n",
        "jobs:\n  t:\n    steps:\n      - if: false\n        uses: wagoid/commitlint-github-action@v6\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), ".github/workflows/ci.yml", body);
        assert!(enforced(dir.path()).is_empty(), "{body}");
    }
    // A condition that can be true, or a neighbour step switched off, changes
    // nothing for a live step.
    for body in [
        "jobs:\n  t:\n    steps:\n      - if: github.event_name == 'pull_request'\n        run: npx commitlint --from HEAD~1\n",
        "jobs:\n  t:\n    steps:\n      - if: false\n        run: echo skipped\n      - run: npx commitlint --from HEAD~1\n",
        "jobs:\n  t:\n    steps:\n      - run: npx commitlint --from HEAD~1\n      - if: false\n        run: echo skipped\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), ".github/workflows/ci.yml", body);
        assert_eq!(enforced(dir.path()), vec!["commitlint".to_string()], "{body}");
    }
    // A lefthook command that is skipped.
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "lefthook.yml",
        "commit-msg:\n  commands:\n    lint:\n      skip: true\n      run: npx commitlint --edit {1}\n",
    );
    assert!(enforced(dir.path()).is_empty());
}

/// A project directory next to an `outside` one holding what must never be read.
#[cfg(unix)]
fn jailed_project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let outer = tempfile::tempdir().unwrap();
    let project = outer.path().join("project");
    let outside = outer.path().join("outside");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&outside).unwrap();
    (outer, project, outside)
}

#[cfg(unix)]
#[test]
fn a_makefile_that_links_out_of_the_project_puts_nothing_in_the_facts() {
    use std::os::unix::fs::symlink;
    let (_outer, project, outside) = jailed_project();
    let body = "commit:\n\techo OUTSIDE_RECIPE_REVIEW_SENTINEL\n";
    fs::write(outside.join("outside.mk"), body).unwrap();
    symlink(outside.join("outside.mk"), project.join("Makefile")).unwrap();
    let facts = CommitFacts::detect(&project);
    assert!(facts.tooling.is_empty(), "{:?}", facts.tooling);
    assert!(!facts
        .render_block("c.md")
        .contains("OUTSIDE_RECIPE_REVIEW_SENTINEL"));

    // The same file as a hard link, in the root and one level down.
    let (_outer2, project2, outside2) = jailed_project();
    fs::write(outside2.join("outside.mk"), body).unwrap();
    fs::create_dir_all(project2.join("sub")).unwrap();
    for name in ["Makefile", "sub/Makefile"] {
        if fs::hard_link(outside2.join("outside.mk"), project2.join(name)).is_err() {
            return;
        }
    }
    let facts = CommitFacts::detect(&project2);
    assert!(facts.tooling.is_empty(), "{:?}", facts.tooling);
    assert!(!facts
        .render_block("c.md")
        .contains("OUTSIDE_RECIPE_REVIEW_SENTINEL"));
}

#[cfg(unix)]
#[test]
fn no_file_read_by_the_facts_follows_a_link_out_of_the_project() {
    use std::os::unix::fs::symlink;
    let (_outer, project, outside) = jailed_project();
    let hook = "#!/bin/sh\nnpx commitlint --edit \"$1\"\n";
    let ci = "jobs:\n  t:\n    steps:\n      - run: npx commitlint --from HEAD~1\n";
    let lefthook = "commit-msg:\n  commands:\n    l:\n      run: npx commitlint --edit {1}\n";
    for (name, body) in [
        ("hook", hook),
        ("ci.yml", ci),
        ("lefthook.yml", lefthook),
        (
            "package.json",
            "{\"devDependencies\": {\"commitizen\": \"1\"}}",
        ),
        (
            "pre-commit.yaml",
            "repos:\n  - hooks:\n      - id: gitlint\n        stages: [commit-msg]\n",
        ),
        ("pyproject.toml", "[tool.commitizen]\n"),
        ("Jenkinsfile", "sh 'npx commitlint --edit'\n"),
        ("commitlint.config.js", "module.exports = {};\n"),
    ] {
        fs::write(outside.join(name), body).unwrap();
    }
    fs::create_dir_all(project.join(".husky")).unwrap();
    fs::create_dir_all(project.join(".github")).unwrap();
    symlink(outside.join("hook"), project.join(".husky/commit-msg")).unwrap();
    symlink(outside.join("lefthook.yml"), project.join("lefthook.yml")).unwrap();
    symlink(outside.join("package.json"), project.join("package.json")).unwrap();
    symlink(
        outside.join("pre-commit.yaml"),
        project.join(".pre-commit-config.yaml"),
    )
    .unwrap();
    symlink(
        outside.join("pyproject.toml"),
        project.join("pyproject.toml"),
    )
    .unwrap();
    symlink(outside.join("Jenkinsfile"), project.join("Jenkinsfile")).unwrap();
    symlink(
        outside.join("commitlint.config.js"),
        project.join("commitlint.config.js"),
    )
    .unwrap();
    // A workflows directory that is itself a link out.
    fs::create_dir_all(outside.join("workflows")).unwrap();
    fs::write(outside.join("workflows/ci.yml"), ci).unwrap();
    symlink(outside.join("workflows"), project.join(".github/workflows")).unwrap();
    assert!(
        detect_tooling(&project).is_empty(),
        "{:?}",
        detect_tooling(&project)
    );

    // Hard links to the same outside files.
    let (_outer2, project2, outside2) = jailed_project();
    fs::create_dir_all(project2.join(".husky")).unwrap();
    fs::create_dir_all(project2.join(".github/workflows")).unwrap();
    fs::write(outside2.join("hook"), hook).unwrap();
    fs::write(outside2.join("ci.yml"), ci).unwrap();
    fs::write(outside2.join("lefthook.yml"), lefthook).unwrap();
    for (from, to) in [
        ("hook", ".husky/commit-msg"),
        ("ci.yml", ".github/workflows/ci.yml"),
        ("lefthook.yml", "lefthook.yml"),
    ] {
        if fs::hard_link(outside2.join(from), project2.join(to)).is_err() {
            return;
        }
    }
    assert!(
        detect_tooling(&project2).is_empty(),
        "{:?}",
        detect_tooling(&project2)
    );
}

#[test]
fn a_makefile_commit_target_names_the_tool_and_the_line_never_the_recipe() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "Makefile",
        "commit:\n\t@echo RECIPE_TEXT_SENTINEL && ./nicommitizen.sh\n",
    );
    put(dir.path(), "nicommitizen.sh", "#!/bin/sh\n");
    let facts = CommitFacts::detect(dir.path());
    assert_eq!(facts.tooling.len(), 1, "{:?}", facts.tooling);
    let tool = &facts.tooling[0];
    assert_eq!(tool.name, "Makefile target `make commit`");
    assert_eq!(
        tool.evidence,
        vec!["Makefile:1".to_string(), "nicommitizen.sh:1".to_string()]
    );
    assert_eq!(tool.enforcement, Enforcement::OptIn);
    let block = facts.render_block("c.md");
    assert!(!block.contains("RECIPE_TEXT_SENTINEL"), "{block}");
    // A target whose name is not a plain identifier is not reported.
    put(
        dir.path(),
        "Makefile",
        "git-commit-$(shell echo X):\n\techo hi\n",
    );
    assert!(CommitFacts::detect(dir.path()).tooling.is_empty());
}

#[cfg(unix)]
#[test]
fn a_script_evidence_outside_the_project_is_never_cited() {
    let (_outer, project, outside) = jailed_project();
    fs::write(outside.join("evil.sh"), "#!/bin/sh\n").unwrap();
    put(&project, "Makefile", "commit:\n\t../outside/evil.sh\n");
    let tools = detect_tooling(&project);
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0].evidence, vec!["Makefile:1".to_string()]);
}
