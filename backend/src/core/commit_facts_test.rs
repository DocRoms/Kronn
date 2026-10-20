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
fn a_ci_step_and_an_unnamed_hook_are_enforcement() {
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
    let hook = tools.iter().find(|t| t.name == "commit-msg hook").unwrap();
    assert_eq!(
        hook.enforcement,
        Enforcement::Enforced(".githooks/commit-msg:2".into())
    );
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

#[test]
fn a_live_line_that_runs_the_checker_is_enforcement_in_every_form() {
    for (command, tool) in [
        ("npx commitlint --edit $1", "commitlint"),
        ("npx --no -- commitlint --edit \"$1\"", "commitlint"),
        ("yarn commitlint --from origin/main", "commitlint"),
        ("pnpm exec commitlint --edit", "commitlint"),
        ("npm exec -- commitlint --edit", "commitlint"),
        ("npm run commitlint", "commitlint"),
        ("node_modules/.bin/commitlint --edit", "commitlint"),
        ("npm ci && npx commitlint --from main", "commitlint"),
        ("CI=1 commitlint --last", "commitlint"),
        ("cz check --rev-range origin/main..HEAD", "commitizen"),
        (
            "python -m commitizen check --commit-msg-file $1",
            "commitizen",
        ),
        ("gitlint --commits origin/main..HEAD", "gitlint"),
    ] {
        let ran = runs_checker(command);
        assert_eq!(ran, Some(Some(tool)), "{command}");
    }
    for command in [
        "npm install --save-dev @commitlint/cli",
        "npm run build",
        "echo commitlint",
        "COMMITLINT_DISABLED: true",
        "cz commit",
        "pip install gitlint",
        "make lint",
    ] {
        assert_eq!(runs_checker(command), None, "{command}");
    }
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
