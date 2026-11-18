/// `gh` and `glab` start git in the repository: they get the git-host
/// environment and the project's GitHub token, never the backend's.
#[cfg(unix)]
// dumps the child environment with probe::env_dumping_program, a #!/bin/sh script made executable through unix mode bits
#[test]
fn gh_and_glab_run_without_the_backend_environment() {
    use crate::core::child_env::probe;
    probe::plant_real_sentinel();
    let bin = tempfile::tempdir().unwrap();
    let gh_out = probe::env_dumping_program(bin.path(), "gh");
    let glab_out = probe::env_dumping_program(bin.path(), "glab");
    let path = format!(
        "{}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
        bin.path().display()
    );
    let repo = |remote: &str| {
        let dir = tempfile::tempdir().unwrap();
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", remote]] {
            assert!(crate::core::cmd::git_cmd()
                .args(&args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success());
        }
        dir
    };
    let github = repo("https://github.com/acme/app.git");
    let gitlab = repo("https://gitlab.com/acme/app.git");
    let project_env = vec![("GH_TOKEN".to_string(), "project-token".to_string())];
    let home = bin.path().to_str().unwrap();
    let create = probe::with_secret_parent(&path, home, || {
        check_pr_url(github.path(), "feature", &project_env);
        check_pr_url(gitlab.path(), "feature", &project_env);
        host_cli_command("gh", github.path(), &project_env)
    });

    let gh = probe::read_dump(&gh_out);
    probe::assert_dump_without_secrets(&gh, &["GH_TOKEN"]);
    assert_eq!(
        gh.get("GH_TOKEN").map(String::as_str),
        Some("project-token"),
        "gh gets the connected project's token, not the backend's"
    );
    let glab = probe::read_dump(&glab_out);
    probe::assert_dump_without_secrets(&glab, &[]);

    probe::assert_built_without_secrets(&create, &path, &["GH_TOKEN"]);
    assert_eq!(
        probe::env_of(&create).get("GH_TOKEN").map(String::as_str),
        Some("project-token")
    );
}

/// The push token reaches no repository hook: neither in an argument, nor
/// in the environment, and the authenticated push runs no hook (B3-10).
#[cfg(unix)] // asserts POSIX mode bits, which Windows does not have; runs a POSIX shell script fixture
#[test]
fn a_push_never_shows_its_token_to_a_pre_push_hook() {
    use std::os::unix::fs::PermissionsExt;
    let token = "ghp_sentinel_push_token";
    let base = tempfile::tempdir().unwrap();
    let remote = base.path().join("remote.git");
    let repo = base.path().join("repo");
    let git = |dir: &Path, args: &[&str]| {
        let output = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    git(
        base.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(base.path(), &["init", "-q", repo.to_str().unwrap()]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(repo.join("f.txt"), "x").unwrap();
    git(&repo, &["add", "f.txt"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    let dump = base.path().join("hook.dump");
    let hook = repo.join(".git/hooks/pre-push");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n{{ echo \"$@\"; /usr/bin/env; git config --list; }} >> '{}'\n",
            dump.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let encoded = {
        use base64::{engine::general_purpose::STANDARD, Engine};
        STANDARD.encode(format!("x-access-token:{token}"))
    };

    // A non-GitHub remote: the user's own push, hook included, no token.
    let env = vec![("GH_TOKEN".to_string(), token.to_string())];
    let pushed = run_git_push(&repo, &env).unwrap();
    assert!(pushed.success, "{}", pushed.message);
    let seen = std::fs::read_to_string(&dump).unwrap();
    assert!(!seen.contains(token) && !seen.contains(&encoded), "{seen}");

    // The authenticated form: token only in a scoped header, hooks off.
    std::fs::remove_file(&dump).unwrap();
    let branch = String::from_utf8(
        crate::core::cmd::git_cmd()
            .args(["branch", "--show-current"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let mut command =
        authenticated_push_command(&repo, remote.to_str().unwrap(), branch.trim(), token).unwrap();
    assert!(command
        .get_args()
        .all(|arg| !arg.to_string_lossy().contains(token)));
    assert!(!crate::core::child_env::probe::env_of(&command)
        .values()
        .any(|value| value.contains(token)));
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!dump.exists(), "the authenticated push ran a hook");
}
#[test]
fn status_without_pr_lookup_reports_only_a_link_already_known() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    git(&["checkout", "-q", "-b", "feature"]);

    let unknown = run_git_status_page_without_pr_lookup(dir.path(), 0, 10).unwrap();
    assert_eq!(unknown.branch, "feature");
    assert_eq!(unknown.pr_url, None, "no lookup, nothing known");

    PR_URLS.lock().unwrap().insert(
        (dir.path().to_path_buf(), "feature".to_string()),
        (
            Some("https://example.test/pr/7".into()),
            std::time::Instant::now(),
        ),
    );
    let known = run_git_status_page_without_pr_lookup(dir.path(), 0, 10).unwrap();
    assert_eq!(known.pr_url.as_deref(), Some("https://example.test/pr/7"));
    forget_pr_urls(dir.path());
}

#[cfg(unix)] // stands in `sleep` and `echo` for the gh CLI; Windows has neither as a program
#[test]
fn a_pr_lookup_that_hangs_is_given_up_instead_of_blocking_the_discussion() {
    // KT-983 — `gh pr view` runs on every git-status; a slow network must
    // cost at most the limit, never the whole page.
    let mut command = std::process::Command::new("sleep");
    command.arg("5");
    let started = std::time::Instant::now();
    let output = super::output_within(command, std::time::Duration::from_millis(200));
    assert!(output.is_none());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    let mut quick = std::process::Command::new("echo");
    quick.arg("https://example.test/pr/1");
    let output = super::output_within(quick, std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "https://example.test/pr/1"
    );
}

use super::*;

#[test]
fn exec_refuses_env() {
    assert!(validate_exec_command("env").is_err());
    assert!(validate_exec_command("env FOO=1").is_err());
}

#[test]
fn exec_refuses_a_read_outside_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn x() {}").unwrap();
    for cmd in [
        "cat /etc/passwd",
        "cat ../../x",
        "head -n 3 ../secret",
        "tail src/../../secret",
        "find / -name passwd",
        "stat /etc",
        "grep -r root /etc",
        "rg --file=/etc/passwd src",
        "wc -l /etc/hosts",
        "du -sh ..",
        "file /bin/sh",
        "tree /",
        "ls /",
    ] {
        assert!(
            validate_exec_command(cmd).is_ok(),
            "{cmd} passes the allow-list"
        );
        assert!(exec_argv(&root, cmd).is_err(), "{cmd} must be refused");
    }
    for cmd in [
        "cat src/lib.rs",
        "head -n 1 src/lib.rs",
        "grep -rn \"/api/\" src",
        "find . -name \"*.rs\"",
        "ls -la",
        "wc -l src/lib.rs",
        "git status",
    ] {
        assert!(exec_argv(&root, cmd).is_ok(), "{cmd} must be accepted");
    }
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn exec_refuses_a_symlink_leaving_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(dir.path().join("secret"), "s").unwrap();
    std::os::unix::fs::symlink(dir.path().join("secret"), root.join("link")).unwrap();
    assert!(exec_argv(&root, "cat link").is_err());
}

#[test]
fn exec_refuses_find_actions_and_git_no_index() {
    let root = tempfile::tempdir().unwrap();
    assert!(exec_argv(root.path(), "find . -exec cat {} +").is_err());
    assert!(exec_argv(root.path(), "find . -delete").is_err());
    assert!(validate_exec_command("git diff --no-index /etc/passwd README.md").is_err());
    assert!(validate_exec_command("git log --output=x").is_err());
}

#[cfg(unix)] // runs `cat` and `echo` as programs and reads /etc/passwd, which Windows lacks
#[test]
fn exec_runs_an_in_project_read_without_a_shell_or_the_backend_env() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note é.txt"), "bonjour").unwrap();
    let read = run_exec(dir.path(), "cat 'note é.txt'", &[]).unwrap();
    assert_eq!(read.stdout, "bonjour");
    assert_eq!(read.exit_code, 0);
    // No shell: a variable stays literal instead of expanding.
    let echoed = run_exec(dir.path(), "echo $HOME", &[]).unwrap();
    assert_eq!(echoed.stdout.trim(), "$HOME");
    assert!(run_exec(dir.path(), "cat /etc/passwd", &[]).is_err());
}

#[test]
fn the_exec_command_carries_a_built_environment() {
    let dir = tempfile::tempdir().unwrap();
    let command = crate::core::child_env::with_parent_env(
        &[
            ("PATH", "/usr/bin:/bin"),
            ("KRONN_AUTH_TOKEN", "admin"),
            ("KRONN_ENCRYPTION_KEK", "raw"),
            ("ANTHROPIC_API_KEY", "sk"),
        ],
        || exec_command(dir.path(), "git status", &[]).unwrap(),
    );
    let env: Vec<String> = command
        .get_envs()
        .filter(|(_, value)| value.is_some())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    assert_eq!(env, vec!["PATH".to_string()]);
}

#[test]
fn exec_words_split_like_a_shell_without_expanding() {
    assert_eq!(
        split_exec_words(r#"grep -n "a b" 'c d' e\ f"#).unwrap(),
        vec!["grep", "-n", "a b", "c d", "e f"]
    );
    assert_eq!(split_exec_words("  ").unwrap(), Vec::<String>::new());
    assert!(split_exec_words("cat 'open").is_err());
    assert!(split_exec_words("cat \"open").is_err());
}

#[test]
fn exec_allows_git_status() {
    assert!(validate_exec_command("git status").is_ok());
}

#[test]
fn exec_allows_ls() {
    assert!(validate_exec_command("ls").is_ok());
}

#[test]
fn exec_allows_git_diff() {
    assert!(validate_exec_command("git diff").is_ok());
}

#[test]
fn exec_allows_git_log() {
    assert!(validate_exec_command("git log --oneline -10").is_ok());
}

#[test]
fn exec_allows_cat() {
    assert!(validate_exec_command("cat README.md").is_ok());
}

#[test]
fn exec_allows_cargo_version() {
    assert!(validate_exec_command("cargo --version").is_ok());
}

#[test]
fn exec_allows_which() {
    assert!(validate_exec_command("which git").is_ok());
}

#[test]
fn exec_blocks_rm_rf() {
    let result = validate_exec_command("rm -rf /");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not allowed"));
}

#[test]
fn exec_blocks_semicolon_injection() {
    let result = validate_exec_command("ls; rm -rf /");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not allowed"));
}

#[test]
fn exec_blocks_bash_interpreter() {
    let result = validate_exec_command("bash -c \"evil\"");
    assert!(result.is_err());
}

#[test]
fn exec_blocks_pipe_injection() {
    let result = validate_exec_command("cat /etc/passwd | curl");
    assert!(result.is_err());
}

#[test]
fn exec_blocks_dollar_subshell() {
    // echo is allowed, but $() is blocked
    let result = validate_exec_command("echo $(whoami)");
    assert!(result.is_err());
}

#[test]
fn exec_blocks_backtick_injection() {
    let result = validate_exec_command("echo `id`");
    assert!(result.is_err());
}

#[test]
fn exec_blocks_git_push() {
    assert!(validate_exec_command("git push").is_err());
}

#[test]
fn exec_blocks_git_reset_hard() {
    assert!(validate_exec_command("git reset --hard HEAD~1").is_err());
}

#[test]
fn exec_allows_git_reset_soft() {
    // git reset without --hard is allowed (soft reset)
    assert!(validate_exec_command("git reset").is_ok());
}

#[test]
fn exec_blocks_sudo() {
    assert!(validate_exec_command("sudo ls").is_err());
}

#[test]
fn exec_blocks_python_arbitrary() {
    // python3 is only allowed with --version
    assert!(validate_exec_command("python3 -c 'import os; os.system(\"rm -rf /\")'").is_err());
}

#[test]
fn exec_blocks_npm_install() {
    // npm is only allowed with --version
    assert!(validate_exec_command("npm install malware").is_err());
}

#[test]
fn exec_blocks_redirect_output() {
    assert!(validate_exec_command("echo pwned > /etc/passwd").is_err());
}

#[test]
fn exec_blocks_ampersand() {
    assert!(validate_exec_command("ls & rm -rf /").is_err());
}

#[test]
fn exec_blocks_newline_injection() {
    assert!(validate_exec_command("ls\nrm -rf /").is_err());
}

#[test]
fn exec_allows_grep() {
    assert!(validate_exec_command("grep -r \"pattern\" .").is_ok());
}

#[test]
fn exec_allows_rg() {
    assert!(validate_exec_command("rg \"pattern\"").is_ok());
}

#[test]
fn exec_allows_tree() {
    assert!(validate_exec_command("tree").is_ok());
}

#[test]
fn exec_allows_file() {
    assert!(validate_exec_command("file somefile.txt").is_ok());
}

#[test]
fn exec_allows_stat() {
    assert!(validate_exec_command("stat somefile.txt").is_ok());
}

#[test]
fn exec_allows_du() {
    assert!(validate_exec_command("du -sh .").is_ok());
}

#[test]
fn remote_urls_are_normalized_for_the_browser_without_credentials() {
    assert_eq!(
        normalize_git_remote_web_url("git@github.com:DocRoms/Kronn.git"),
        Some("https://github.com/DocRoms/Kronn".into())
    );
    assert_eq!(
        normalize_git_remote_web_url("ssh://git@gitlab.example.com/team/app.git"),
        Some("https://gitlab.example.com/team/app".into())
    );
    assert_eq!(
        normalize_git_remote_web_url("https://oauth:secret@gitlab.com/team/app.git"),
        Some("https://gitlab.com/team/app".into())
    );
    assert_eq!(normalize_git_remote_web_url("../local-repo"), None);
}

// ── Commit args tests ────────────────────────────────────────────────────

fn make_test_repo(name: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix(&format!("kronn-git-{}", name))
        .tempdir()
        .unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("init.txt"), "init").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn git_status_exposes_repository_overview_metadata() {
    let repo = make_test_repo("overview-metadata");
    std::process::Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:team/demo.git"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["tag", "v1.2.3"])
        .current_dir(repo.path())
        .output()
        .unwrap();

    let status = run_git_status(repo.path()).unwrap();

    assert_eq!(
        status.remote_url.as_deref(),
        Some("https://github.com/team/demo")
    );
    assert_eq!(
        status.pull_requests_url.as_deref(),
        Some("https://github.com/team/demo/pulls")
    );
    assert_eq!(status.last_tag.as_deref(), Some("v1.2.3"));
}

#[test]
fn git_branches_returns_bounded_local_branch_graph() {
    let repo = make_test_repo("branch-graph");
    std::process::Command::new("git")
        .args(["switch", "-c", "feature/graph"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::fs::write(repo.path().join("feature.txt"), "feature").unwrap();
    std::process::Command::new("git")
        .args(["add", "feature.txt"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "feature commit"])
        .current_dir(repo.path())
        .output()
        .unwrap();

    let graph = run_git_branches(repo.path()).unwrap();

    assert_eq!(graph.current_branch, "feature/graph");
    assert_eq!(graph.default_branch, "main");
    assert!(graph
        .branches
        .iter()
        .any(|branch| branch.name == "feature/graph" && branch.is_current));
    assert!(graph
        .branches
        .iter()
        .any(|branch| branch.name == "main" && !branch.is_remote));
    assert_eq!(graph.commits[0].subject, "feature commit");
    assert!(!graph.truncated);
}

#[test]
fn git_switch_changes_clean_local_branch() {
    let repo = make_test_repo("switch-clean");
    std::process::Command::new("git")
        .args(["branch", "feature/safe"])
        .current_dir(repo.path())
        .output()
        .unwrap();

    let switched = run_git_switch_branch(repo.path(), "feature/safe").unwrap();

    assert_eq!(switched.branch, "feature/safe");
    assert_eq!(
        git_output(repo.path(), &["branch", "--show-current"]).unwrap(),
        "feature/safe"
    );
}

#[test]
fn git_switch_refuses_dirty_worktree_without_changing_branch() {
    let repo = make_test_repo("switch-dirty");
    std::process::Command::new("git")
        .args(["branch", "feature/blocked"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::fs::write(repo.path().join("init.txt"), "local edit").unwrap();

    let error = run_git_switch_branch(repo.path(), "feature/blocked").unwrap_err();

    assert!(error.contains("modifications locales"));
    assert_eq!(
        git_output(repo.path(), &["branch", "--show-current"]).unwrap(),
        "main"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("init.txt")).unwrap(),
        "local edit"
    );
}

#[test]
fn git_switch_rejects_unknown_or_malformed_branch() {
    let repo = make_test_repo("switch-invalid");

    assert!(run_git_switch_branch(repo.path(), "--upload-pack=evil").is_err());
    assert!(run_git_switch_branch(repo.path(), "missing").is_err());
    assert_eq!(
        git_output(repo.path(), &["branch", "--show-current"]).unwrap(),
        "main"
    );
}

fn repo_git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// `(full message, "author|committer")` of HEAD.
fn head_commit(repo: &Path) -> (String, String) {
    let show = |format: &str| {
        let out = std::process::Command::new("git")
            .args(["log", "-1", format])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    (show("--format=%B"), show("--format=%an <%ae>|%cn <%ce>"))
}

fn commit_file(repo: &Path, name: &str) {
    std::fs::write(repo.join(name), "content").unwrap();
    run_git_commit(repo, &[name.into()], "test commit", false, false).unwrap();
}

#[test]
fn commit_has_no_signoff_when_the_project_does_not_sign() {
    let repo = make_test_repo("no-signoff");
    commit_file(repo.path(), "file.txt");
    let (message, identity) = head_commit(repo.path());
    assert!(!message.contains("Signed-off-by:"), "{message}");
    assert_eq!(
        identity,
        "Test User <test@test.com>|Test User <test@test.com>"
    );
}

#[test]
fn commit_signs_off_when_the_project_requires_it() {
    let repo = make_test_repo("signoff");
    repo_git(repo.path(), &["config", "kronn.signoff", "true"]);
    commit_file(repo.path(), "file.txt");
    let (message, _) = head_commit(repo.path());
    assert!(
        message.contains("Signed-off-by: Test User <test@test.com>"),
        "{message}"
    );
}

#[test]
fn commit_signs_off_in_a_dco_repository_without_any_setting() {
    let repo = make_test_repo("dco");
    std::fs::write(repo.path().join("DCO"), "Developer Certificate of Origin").unwrap();
    commit_file(repo.path(), "file.txt");
    let (message, _) = head_commit(repo.path());
    assert!(message.contains("Signed-off-by:"), "{message}");
}

#[test]
fn commit_uses_the_project_identity_for_author_committer_and_signoff() {
    let repo = make_test_repo("identity");
    repo_git(repo.path(), &["config", "kronn.signoff", "true"]);
    repo_git(repo.path(), &["config", "kronn.commitName", "Corp Dev"]);
    repo_git(
        repo.path(),
        &["config", "kronn.commitEmail", "dev@corp.example"],
    );
    commit_file(repo.path(), "file.txt");
    let (message, identity) = head_commit(repo.path());
    assert_eq!(
        identity,
        "Corp Dev <dev@corp.example>|Corp Dev <dev@corp.example>"
    );
    assert!(
        message.contains("Signed-off-by: Corp Dev <dev@corp.example>"),
        "{message}"
    );
    assert!(!message.contains("test@test.com"), "{message}");
}

#[test]
fn commit_with_only_a_global_identity_uses_it_and_writes_no_repository_config() {
    // The harness's global config is the only identity there is.
    crate::core::git_global_config::isolate_global_git_config_for_tests();
    let dir = tempfile::tempdir().unwrap();
    repo_git(dir.path(), &["init", "-q", "-b", "main"]);
    commit_file(dir.path(), "file.txt");
    let (_, identity) = head_commit(dir.path());
    assert_eq!(
        identity,
        "Kronn Test <kronn-test@example.invalid>|Kronn Test <kronn-test@example.invalid>"
    );
    let config = std::fs::read_to_string(dir.path().join(".git/config")).unwrap();
    assert!(!config.contains("[user]"), "{config}");
}

#[test]
fn a_merge_commit_follows_the_projects_sign_off_and_identity() {
    for signoff in [false, true] {
        let repo = make_test_repo("merge-policy");
        let dir = repo.path();
        repo_git(dir, &["config", "kronn.commitName", "Corp Dev"]);
        repo_git(dir, &["config", "kronn.commitEmail", "dev@corp.example"]);
        repo_git(
            dir,
            &[
                "config",
                "kronn.signoff",
                if signoff { "true" } else { "false" },
            ],
        );
        // Two linked worktrees: the work happens in checkouts Kronn owns,
        // never by switching the main one.
        std::fs::write(dir.join("main.txt"), "main").unwrap();
        repo_git(dir, &["add", "main.txt"]);
        repo_git(dir, &["commit", "-q", "-m", "main work"]);
        let side = dir.join(".kronn/worktrees/side");
        repo_git(
            dir,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                side.to_str().unwrap(),
                "HEAD~1",
            ],
        );
        std::fs::write(side.join("side.txt"), "side").unwrap();
        repo_git(&side, &["add", "side.txt"]);
        repo_git(&side, &["commit", "-q", "-m", "side work"]);
        repo_git(&side, &["merge", "-q", "--no-commit", "--no-ff", "main"]);

        run_git_merge_commit_with_child_lock(
            &side,
            &["side.txt".to_string()],
            "merge main",
            false,
            None,
        )
        .unwrap();
        let (message, identity) = head_commit(&side);
        assert_eq!(
            message.contains("Signed-off-by: Corp Dev <dev@corp.example>"),
            signoff,
            "{message}"
        );
        assert_eq!(
            identity,
            "Corp Dev <dev@corp.example>|Corp Dev <dev@corp.example>"
        );
    }
}

// ── parse_committed_diff tests ───────────────────────────────────────────

#[test]
fn parse_committed_diff_handles_modified_added_deleted() {
    let out = "M\tsrc/lib.rs\nA\tdocs/new.md\nD\told.txt";
    let parsed = parse_committed_diff(out);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].path, "src/lib.rs");
    assert_eq!(parsed[0].status, "modified");
    assert!(parsed[0].staged);
    assert_eq!(parsed[1].path, "docs/new.md");
    assert_eq!(parsed[1].status, "added");
    assert_eq!(parsed[2].path, "old.txt");
    assert_eq!(parsed[2].status, "deleted");
}

#[test]
fn parse_committed_diff_renames_use_destination_path() {
    let out = "R100\told/path.rs\tnew/path.rs";
    let parsed = parse_committed_diff(out);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].path, "new/path.rs");
    assert_eq!(parsed[0].status, "renamed");
}

#[test]
fn parse_committed_diff_ignores_empty_and_garbage() {
    let out = "\n\nZ\tweird\nM\tok.rs\n";
    let parsed = parse_committed_diff(out);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].path, "ok.rs");
}

#[test]
fn parse_committed_diff_type_change_treated_as_modified() {
    let out = "T\tsymlink.txt";
    let parsed = parse_committed_diff(out);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].status, "modified");
}

#[test]
fn parse_git_blame_porcelain_returns_author_and_time_per_line() {
    let output = "\
0123456789abcdef0123456789abcdef01234567 1 1 1
author Ada Lovelace
author-mail <ada@example.test>
author-time 1710000000
author-tz +0100
filename src/main.rs
\tfirst
fedcba9876543210fedcba9876543210fedcba98 2 2 1
author Grace Hopper
author-mail <grace@example.test>
author-time 1720000000
author-tz +0200
filename src/main.rs
\tsecond";
    let lines = parse_git_blame_porcelain(output);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].line_number, 1);
    assert_eq!(lines[0].author, "Ada Lovelace");
    assert_eq!(lines[0].author_time, 1_710_000_000);
    assert_eq!(lines[1].line_number, 2);
    assert_eq!(lines[1].author, "Grace Hopper");
    assert_eq!(lines[1].commit, "fedcba9876543210fedcba9876543210fedcba98");
}

// ── run_git_status committed_files integration tests ─────────────────────

fn make_branch_repo(name: &str) -> tempfile::TempDir {
    let repo = make_test_repo(name);
    // Create a feature branch with two commits worth of changes.
    std::process::Command::new("git")
        .args(["checkout", "-b", "feature/x"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::fs::write(repo.path().join("added.txt"), "added").unwrap();
    std::fs::write(repo.path().join("init.txt"), "modified").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "feature changes"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    repo
}

#[test]
fn run_git_status_exposes_committed_files_on_feature_branch() {
    let repo = make_branch_repo("committed-feature");
    let status = run_git_status(repo.path()).unwrap();
    assert_eq!(status.branch, "feature/x");
    assert_eq!(status.default_branch, "main");
    assert!(!status.is_default_branch);
    let paths: Vec<&str> = status
        .committed_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert!(
        paths.contains(&"added.txt"),
        "expected added.txt in {:?}",
        paths
    );
    assert_eq!(status.commits.len(), 1);
    assert_eq!(status.commits_total, 1);
    assert_eq!(status.commits_offset, 0);
    assert!(!status.commits_truncated);
    assert_eq!(status.commits[0].subject, "feature changes");
    assert!(
        paths.contains(&"init.txt"),
        "expected init.txt in {:?}",
        paths
    );
    for f in &status.committed_files {
        assert!(f.staged, "committed files should be marked staged: {:?}", f);
    }
}

#[test]
fn run_git_status_pages_a_300_plus_commit_branch_without_materializing_it() {
    let repo = make_test_repo("commit-pages");
    std::process::Command::new("git")
        .args(["checkout", "-b", "feature/history"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    // One fast-import stream instead of 305 `git commit` processes.
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let head = String::from_utf8(head.stdout).unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut stream = String::new();
    for index in 0..305 {
        let message = format!("history {index}");
        stream.push_str(&format!(
                "commit refs/heads/feature/history\ncommitter Kronn Test <test@kronn.invalid> {} +0000\ndata {}\n{}\n",
                now + index,
                message.len(),
                message
            ));
        if index == 0 {
            stream.push_str(&format!("from {}\n", head.trim()));
        }
    }
    let mut import = std::process::Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(repo.path())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    import
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let output = import.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "fast-import failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let first = run_git_status_page(repo.path(), 0, 40, &[]).unwrap();
    assert_eq!(first.commits_total, 305);
    assert_eq!(first.commits_offset, 0);
    assert_eq!(first.commits.len(), 40);
    assert!(first.commits_truncated);
    assert_eq!(first.commits[0].subject, "history 304");
    assert_eq!(first.commits[39].subject, "history 265");

    let second = run_git_status_page(repo.path(), 40, 40, &[]).unwrap();
    assert_eq!(second.commits_total, 305);
    assert_eq!(second.commits_offset, 40);
    assert_eq!(second.commits.len(), 40);
    assert!(second.commits_truncated);
    assert_eq!(second.commits[0].subject, "history 264");

    let last = run_git_status_page(repo.path(), 300, 1_000, &[]).unwrap();
    assert_eq!(last.commits_total, 305);
    assert_eq!(last.commits_offset, 300);
    assert_eq!(last.commits.len(), 5);
    assert!(!last.commits_truncated);
    assert_eq!(last.commits[0].subject, "history 4");
    assert_eq!(last.commits[4].subject, "history 0");
}

#[test]
fn resolve_default_branch_resolves_main_on_a_feature_branch() {
    let repo = make_branch_repo("default-branch");
    assert_eq!(resolve_default_branch(repo.path()), "main");
}

#[test]
fn run_git_diff_committed_shows_the_branch_diff_for_a_committed_file() {
    // Regression for the GitPanel "committed on branch" bug: the file is
    // committed (clean working tree), so a plain `git diff` is useless —
    // the committed diff (`main...HEAD`) must surface the change.
    let repo = make_branch_repo("committed-diff");
    let res = run_git_diff_committed(repo.path(), "added.txt").unwrap();
    assert!(
        res.diff.contains("added.txt"),
        "committed diff must reference the file, got: {:?}",
        res.diff
    );
    assert!(
        res.diff.contains("@@"),
        "committed diff must contain a hunk header, got: {:?}",
        res.diff
    );
}

#[test]
fn run_git_status_committed_files_empty_on_default_branch() {
    let repo = make_test_repo("on-main");
    let status = run_git_status(repo.path()).unwrap();
    assert!(status.is_default_branch);
    assert!(
        status.committed_files.is_empty(),
        "expected no committed_files on default branch, got {:?}",
        status.committed_files
    );
}

#[test]
fn run_git_status_committed_and_uncommitted_are_disjoint_sections() {
    let repo = make_branch_repo("disjoint");
    // Add an uncommitted change on top of the committed work.
    std::fs::write(repo.path().join("untracked.txt"), "wip").unwrap();
    let status = run_git_status(repo.path()).unwrap();
    let committed_paths: Vec<&str> = status
        .committed_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    let uncommitted_paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
    assert!(committed_paths.contains(&"added.txt"));
    assert!(uncommitted_paths.contains(&"untracked.txt"));
    // The committed section must NOT leak the uncommitted file (and vice versa for committed-only paths).
    assert!(!committed_paths.contains(&"untracked.txt"));
    assert!(!uncommitted_paths.contains(&"added.txt"));
}

#[test]
fn commit_without_sign_uses_no_gpg_sign() {
    let repo = make_test_repo("nogpg");
    // Set commit.gpgsign=true to simulate a user config that would fail without --no-gpg-sign
    std::process::Command::new("git")
        .args(["config", "commit.gpgsign", "true"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    // Set a nonexistent signing key to guarantee failure if --no-gpg-sign doesn't work
    std::process::Command::new("git")
        .args(["config", "user.signingkey", "/nonexistent/key"])
        .current_dir(repo.path())
        .output()
        .unwrap();

    std::fs::write(repo.path().join("file.txt"), "content").unwrap();
    let result = run_git_commit(repo.path(), &["file.txt".into()], "no gpg", false, false);
    assert!(
        result.is_ok(),
        "commit should succeed with --no-gpg-sign even when gpgsign=true: {:?}",
        result.err()
    );
}
// ─── KT-67 — commit detail behind an annotated line ─────────────────────

#[test]
fn commit_detail_refuses_anything_that_is_not_a_hash() {
    // The value is interpolated into a git invocation and blame only ever
    // hands us hashes, so a refusal is the right answer — not a best effort.
    for bad in [
        "HEAD",
        "main",
        "../../etc/passwd",
        "abc123; rm -rf /",
        "abc",                                       // too short
        "0123456789012345678901234567890123456789a", // too long
        "zzzzzzz",                                   // not hex
        "",
    ] {
        assert!(!valid_commit_ish(bad), "{bad:?} must be refused");
    }
    for good in ["abc1234", "0123456789abcdef0123456789abcdef01234567"] {
        assert!(valid_commit_ish(good), "{good:?} must be accepted");
    }

    let repo = make_test_repo("commit-detail-refuse");
    let err = run_git_commit_detail(repo.path(), "HEAD").unwrap_err();
    assert!(err.contains("invalid commit hash"), "{err}");
}

#[test]
fn commit_detail_reports_message_author_and_branches() {
    let repo = make_test_repo("commit-detail-ok");
    std::fs::write(repo.path().join("a.txt"), "a").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "commit",
            "-m",
            "sujet du commit",
            "-m",
            "corps sur\nplusieurs lignes",
        ])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let detail = run_git_commit_detail(repo.path(), &sha).unwrap();
    assert_eq!(detail.sha, sha);
    assert_eq!(detail.short_sha, sha[..detail.short_sha.len()]);
    assert_eq!(detail.subject, "sujet du commit");
    assert!(
        detail.body.contains("corps sur"),
        "body was {:?}",
        detail.body
    );
    assert_eq!(detail.author_name, "Test User");
    assert_eq!(detail.author_email, "test@test.com");
    assert!(detail.author_time > 0, "author_time must be a real epoch");
    assert_eq!(detail.files_changed, 1);
    assert!(
        detail.branches.contains(&"main".to_string()),
        "{:?}",
        detail.branches
    );
    assert!(!detail.branches_truncated);
}

#[test]
fn commit_detail_truncates_a_long_branch_list_honestly() {
    let repo = make_test_repo("commit-detail-branches");
    let sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    // Every branch here contains the root commit.
    for i in 0..COMMIT_BRANCHES_CAP + 3 {
        std::process::Command::new("git")
            .args(["branch", &format!("topic-{i}")])
            .current_dir(repo.path())
            .output()
            .unwrap();
    }

    let detail = run_git_commit_detail(repo.path(), &sha).unwrap();
    assert_eq!(
        detail.files_changed, 1,
        "the initial commit must count its root-tree file"
    );
    assert_eq!(
        detail.branches.len(),
        COMMIT_BRANCHES_CAP,
        "list must be capped"
    );
    assert!(
        detail.branches_truncated,
        "a capped list must SAY it was capped — otherwise the UI implies it is complete",
    );
}

#[test]
fn commit_detail_on_an_unknown_hash_fails_instead_of_inventing() {
    let repo = make_test_repo("commit-detail-unknown");
    let err =
        run_git_commit_detail(repo.path(), "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap_err();
    assert!(err.contains("git show failed"), "{err}");
}

fn commit_all(repo: &Path, message: &str) -> String {
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

/// KT-75 — the patch must be the commit against its PARENT, not against
/// whatever the file looks like today.
#[test]
fn commit_patch_shows_the_change_as_it_was_made() {
    let repo = make_test_repo("commit-patch-parent");
    std::fs::write(repo.path().join("a.txt"), "premiere ligne\n").unwrap();
    commit_all(repo.path(), "initial");
    std::fs::write(
        repo.path().join("a.txt"),
        "premiere ligne\ndeuxieme ligne\n",
    )
    .unwrap();
    let second = commit_all(repo.path(), "ajoute une ligne");
    // The file moves on afterwards: the patch of `second` must not change.
    std::fs::write(repo.path().join("a.txt"), "tout autre chose\n").unwrap();
    commit_all(repo.path(), "reecrit tout");

    let patch = run_git_commit_patch(repo.path(), &second).unwrap();
    assert_eq!(patch.sha, second);
    assert_eq!(patch.subject, "ajoute une ligne");
    assert!(!patch.is_root);
    assert_eq!(patch.files_changed, 1);
    assert!(patch.patch.contains("+deuxieme ligne"), "{}", patch.patch);
    assert!(
        !patch.patch.contains("tout autre chose"),
        "the later rewrite leaked into an older commit's patch: {}",
        patch.patch
    );
    assert!(!patch.truncated);
}

/// The first commit has no parent. Without `--root`, `git show` prints the
/// message and no diff at all — the tab would open empty.
#[test]
fn commit_patch_covers_the_root_commit() {
    // `make_test_repo` already lands one commit, so the root is ITS commit —
    // asking git for it beats assuming the one we just made is first.
    let repo = make_test_repo("commit-patch-root");
    std::fs::write(repo.path().join("second.txt"), "suite\n").unwrap();
    let child = commit_all(repo.path(), "deuxieme commit");
    let root = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    assert_ne!(root, child);

    let patch = run_git_commit_patch(repo.path(), &root).unwrap();
    assert!(patch.is_root, "the first commit must be reported as root");
    assert!(
        patch.patch.contains("+init"),
        "root patch was {:?}",
        patch.patch
    );
    assert_eq!(patch.files_changed, 1);
    assert!(!run_git_commit_patch(repo.path(), &child).unwrap().is_root);
}

#[test]
fn commit_patch_truncates_on_a_line_boundary_and_says_so() {
    let repo = make_test_repo("commit-patch-truncate");
    // Comfortably past the cap, so the branch is actually exercised.
    let big: String = (0..40_000).map(|i| format!("ligne numero {i}\n")).collect();
    std::fs::write(repo.path().join("big.txt"), big).unwrap();
    let sha = commit_all(repo.path(), "gros fichier");

    let patch = run_git_commit_patch(repo.path(), &sha).unwrap();
    assert!(patch.truncated, "a 600 KB patch must be reported as cut");
    assert!(patch.patch.len() <= COMMIT_PATCH_MAX_BYTES);
    assert!(
        patch.patch.ends_with('\n'),
        "the cut must land on a line boundary, not mid-hunk"
    );
}

#[test]
fn commit_patch_refuses_a_non_hash_and_an_unknown_hash() {
    let repo = make_test_repo("commit-patch-guards");
    assert!(run_git_commit_patch(repo.path(), "HEAD")
        .unwrap_err()
        .contains("invalid commit hash"));
    assert!(run_git_commit_patch(repo.path(), "../../etc/passwd")
        .unwrap_err()
        .contains("invalid commit hash"));
    assert!(
        run_git_commit_patch(repo.path(), "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .unwrap_err()
            .contains("git show failed")
    );
}

#[test]
fn github_remotes_map_to_a_credential_free_https_url() {
    for remote in [
        "git@github.com:acme/app.git",
        "ssh://git@github.com/acme/app.git",
        "https://github.com/acme/app.git",
        "https://x-access-token:old@github.com/acme/app.git",
    ] {
        assert_eq!(
            github_https_url(remote).as_deref(),
            Some("https://github.com/acme/app.git"),
            "{remote}"
        );
    }
    assert_eq!(github_https_url("git@gitlab.com:acme/app.git"), None);
    assert_eq!(github_https_url("/srv/git/app.git"), None);
}
