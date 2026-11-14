use super::*;

use std::fs;

fn git(root: &Path, args: &[&str]) {
    let status = git_cmd()
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

fn repo() -> tempfile::TempDir {
    crate::core::git_global_config::isolate_global_git_config_for_tests();
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

fn commit(root: &Path, message: &str) {
    git(
        root,
        &[
            "-c",
            "user.name=Someone",
            "-c",
            "user.email=someone@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ],
    );
}

#[test]
fn the_repository_identity_is_the_default() {
    let repo = repo();
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    let policy = CommitPolicy::for_repo(repo.path());
    assert_eq!(policy.identity_source, IdentitySource::Repository);
    assert_eq!(policy.identity.display(), "Work Person <work@corp.example>");
}

#[test]
fn a_project_identity_overrides_the_repository_one() {
    let repo = repo();
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    git(repo.path(), &["config", "kronn.commitName", "Bot"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "bot@corp.example"],
    );
    let policy = CommitPolicy::for_repo(repo.path());
    assert_eq!(policy.identity_source, IdentitySource::Project);
    assert_eq!(policy.identity.display(), "Bot <bot@corp.example>");
}

#[test]
fn half_a_project_identity_is_not_used() {
    let repo = repo();
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "bot@corp.example"],
    );
    let policy = CommitPolicy::for_repo(repo.path());
    assert_eq!(policy.identity_source, IdentitySource::Repository);
}

#[test]
fn a_global_only_identity_is_used_but_reported_as_global() {
    // The test harness's global config carries an identity; the repository has none.
    let repo = repo();
    let policy = CommitPolicy::for_repo(repo.path());
    assert_eq!(policy.identity_source, IdentitySource::Global);
    assert_ne!(policy.identity.email, FALLBACK_EMAIL);
}

#[test]
fn no_identity_at_all_is_the_placeholder_and_writes_no_config() {
    let (identity, source) = choose_identity(None, None, None);
    assert_eq!(source, IdentitySource::Fallback);
    assert_eq!(identity.display(), "Kronn <kronn@localhost>");
    // Nothing here writes a git config: a repository stays as it was.
    let repo = repo();
    let _ = CommitPolicy::for_repo(repo.path());
    let config = fs::read_to_string(repo.path().join(".git/config")).unwrap();
    assert!(!config.contains("kronn@localhost"), "{config}");
}

#[test]
fn the_identity_reaches_git_as_author_and_committer() {
    let repo = repo();
    git(repo.path(), &["config", "kronn.commitName", "Bot"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "bot@corp.example"],
    );
    let policy = CommitPolicy::for_repo(repo.path());
    let mut command = git_cmd();
    command
        .args(["commit", "-q", "--allow-empty", "--no-gpg-sign", "-m", "x"])
        .current_dir(repo.path());
    policy.apply_identity(&mut command, false);
    assert!(command.status().unwrap().success());
    let shown = git_cmd()
        .args(["log", "-1", "--format=%an <%ae>|%cn <%ce>"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout).trim(),
        "Bot <bot@corp.example>|Bot <bot@corp.example>"
    );
}

#[test]
fn an_amend_keeps_its_author() {
    let repo = repo();
    commit(repo.path(), "first");
    git(repo.path(), &["config", "kronn.commitName", "Bot"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "bot@corp.example"],
    );
    let policy = CommitPolicy::for_repo(repo.path());
    let mut command = git_cmd();
    command
        .args([
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "again",
        ])
        .current_dir(repo.path());
    policy.apply_identity(&mut command, true);
    assert!(command.status().unwrap().success());
    let shown = git_cmd()
        .args(["log", "-1", "--format=%an|%cn"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&shown.stdout).trim(), "Someone|Bot");
}

#[test]
fn no_sign_off_by_default() {
    let repo = repo();
    let policy = CommitPolicy::for_repo(repo.path());
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::Default);
}

#[test]
fn a_dco_file_requires_the_sign_off() {
    let repo = repo();
    fs::write(repo.path().join("DCO"), "Developer Certificate of Origin").unwrap();
    let policy = CommitPolicy::for_repo(repo.path());
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::DcoFile);
}

#[test]
fn a_signed_history_requires_the_sign_off_and_an_unsigned_one_does_not() {
    let signed = repo();
    for n in 0..4 {
        commit(
            signed.path(),
            &format!("change {n}\n\nSigned-off-by: Someone <someone@example.com>"),
        );
    }
    let policy = CommitPolicy::for_repo(signed.path());
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::History);

    let unsigned = repo();
    for n in 0..4 {
        commit(unsigned.path(), &format!("change {n}"));
    }
    let policy = CommitPolicy::for_repo(unsigned.path());
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::History);
}

#[test]
fn one_signed_commit_in_an_unsigned_history_is_not_a_habit() {
    let repo = repo();
    for n in 0..5 {
        commit(repo.path(), &format!("change {n}"));
    }
    commit(
        repo.path(),
        "kronn\n\nSigned-off-by: Someone <someone@example.com>",
    );
    assert!(!CommitPolicy::for_repo(repo.path()).signoff);
}

#[test]
fn the_setting_beats_the_briefing_and_the_detection() {
    let repo = repo();
    fs::write(repo.path().join("DCO"), "dco").unwrap();
    git(repo.path(), &["config", "kronn.signoff", "false"]);
    let policy = CommitPolicy::for_repo(repo.path());
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::Setting);
    git(repo.path(), &["config", "kronn.signoff", "true"]);
    assert!(CommitPolicy::for_repo(repo.path()).signoff);
}

fn briefing(signoff: &str) -> String {
    format!(
        "# Project Briefing\n\n<!-- kronn:project-answers v1 {{\"signoff_required\":\"{signoff}\"}} -->\n"
    )
}

#[test]
fn the_project_briefing_decides_before_detection() {
    let repo = repo();
    fs::write(repo.path().join("DCO"), "dco").unwrap();
    fs::create_dir_all(repo.path().join("docs")).unwrap();
    fs::write(repo.path().join("docs/AGENTS.md"), "# x").unwrap();
    fs::write(repo.path().join("docs/briefing.md"), briefing("no")).unwrap();
    let policy = CommitPolicy::for_repo(repo.path());
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::Briefing);

    fs::write(repo.path().join("docs/briefing.md"), briefing("yes")).unwrap();
    assert!(CommitPolicy::for_repo(repo.path()).signoff);
}

#[test]
fn a_linked_worktree_answers_for_its_repository() {
    let repo = repo();
    commit(repo.path(), "base");
    git(repo.path(), &["config", "kronn.signoff", "true"]);
    git(repo.path(), &["config", "kronn.commitName", "Bot"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "bot@corp.example"],
    );
    let linked = repo.path().join(".kronn/worktrees/linked");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    let policy = CommitPolicy::for_repo(&linked);
    assert!(policy.signoff);
    assert_eq!(policy.identity.display(), "Bot <bot@corp.example>");
}
