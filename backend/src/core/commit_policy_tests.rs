use super::*;

use crate::core::cmd::git_cmd;
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
fn the_kronn_git_config_keys_are_a_detection_source_over_the_repository_identity() {
    let repo = repo();
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    git(repo.path(), &["config", "kronn.commitName", "Bot"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "bot@corp.example"],
    );
    let policy = CommitPolicy::for_repo(repo.path());
    assert_eq!(policy.identity_source, IdentitySource::KronnConfig);
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
    let (identity, source) = choose_identity(None, None, None, None, None);
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

#[cfg(unix)] // writes an executable hook script
#[test]
fn a_commit_msg_hook_checking_the_sign_off_requires_it() {
    use std::os::unix::fs::PermissionsExt;

    let repo = repo();
    let hook = repo.path().join(".git/hooks/commit-msg");
    fs::write(
        &hook,
        "#!/bin/sh\ngrep -q '^Signed-off-by: ' \"$1\" || exit 1\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let policy = CommitPolicy::for_repo(repo.path());
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::CommitMsgHook);
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
fn the_repository_git_config_beats_the_briefing_and_the_detection() {
    let repo = repo();
    fs::write(repo.path().join("DCO"), "dco").unwrap();
    git(repo.path(), &["config", "kronn.signoff", "false"]);
    let policy = CommitPolicy::for_repo(repo.path());
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::KronnConfig);
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

fn card(name: &str, email: &str, signoff: SignoffSetting) -> ProjectCommitSettings {
    ProjectCommitSettings {
        name: (!name.is_empty()).then(|| name.to_string()),
        email: (!email.is_empty()).then(|| email.to_string()),
        signoff,
    }
}

#[test]
fn the_project_card_identity_beats_every_detected_one() {
    let repo = repo();
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    git(repo.path(), &["config", "kronn.commitName", "Detected"]);
    git(
        repo.path(),
        &["config", "kronn.commitEmail", "detected@corp.example"],
    );
    let detected = CommitPolicy::detect(repo.path());
    assert_eq!(detected.identity_source, IdentitySource::KronnConfig);
    let policy = detected.with_settings(&card("Bot", "bot@corp.example", SignoffSetting::Auto));
    assert_eq!(policy.identity_source, IdentitySource::Setting);
    assert_eq!(policy.identity.display(), "Bot <bot@corp.example>");
}

#[test]
fn an_unset_card_changes_nothing_and_a_half_identity_is_ignored() {
    let repo = repo();
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    let detected = CommitPolicy::detect(repo.path());
    assert_eq!(
        detected
            .clone()
            .with_settings(&ProjectCommitSettings::default()),
        detected
    );
    let half = detected
        .clone()
        .with_settings(&card("Bot", "", SignoffSetting::Auto));
    assert_eq!(half.identity_source, IdentitySource::Repository);
}

#[test]
fn the_card_sign_off_is_always_never_or_the_detection() {
    let repo = repo();
    fs::write(repo.path().join("DCO"), "dco").unwrap();
    let detected = CommitPolicy::detect(repo.path());
    assert!(detected.signoff);
    assert_eq!(detected.signoff_source, SignoffSource::DcoFile);

    let auto = detected
        .clone()
        .with_settings(&card("", "", SignoffSetting::Auto));
    assert!(auto.signoff);
    assert_eq!(auto.signoff_source, SignoffSource::DcoFile);
    let never = detected
        .clone()
        .with_settings(&card("", "", SignoffSetting::Never));
    assert!(!never.signoff);
    assert_eq!(never.signoff_source, SignoffSource::Setting);

    let unsigned = self::repo();
    let plain = CommitPolicy::detect(unsigned.path());
    assert!(!plain.signoff);
    let always = plain.with_settings(&card("", "", SignoffSetting::Always));
    assert!(always.signoff);
    assert_eq!(always.signoff_source, SignoffSource::Setting);
}

#[test]
fn the_published_card_settings_reach_the_repository_and_its_worktrees() {
    let repo = repo();
    commit(repo.path(), "base");
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    assert_eq!(
        CommitPolicy::for_repo(repo.path()).identity.email,
        "work@corp.example"
    );
    register_project(
        repo.path(),
        &card("Bot", "bot@corp.example", SignoffSetting::Always),
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
    for path in [repo.path(), linked.as_path()] {
        let policy = CommitPolicy::for_repo(path);
        assert_eq!(policy.identity.display(), "Bot <bot@corp.example>");
        assert_eq!(policy.identity_source, IdentitySource::Setting);
        assert!(policy.signoff);
    }
    // Clearing the card puts the detection back.
    register_project(repo.path(), &ProjectCommitSettings::default());
    let policy = CommitPolicy::for_repo(&linked);
    assert_eq!(policy.identity.email, "work@corp.example");
    assert!(!policy.signoff);
}

#[test]
fn only_a_global_identity_is_flagged_as_one_no_project_decided() {
    let bare = repo();
    let global = CommitPolicy::for_repo(bare.path());
    assert!(global.uses_global_identity());
    // The card settles it, and so does the repository.
    assert!(!global
        .clone()
        .with_settings(&card("Bot", "bot@corp.example", SignoffSetting::Auto))
        .uses_global_identity());
    git(bare.path(), &["config", "user.name", "Work Person"]);
    git(bare.path(), &["config", "user.email", "work@corp.example"]);
    assert!(!CommitPolicy::for_repo(bare.path()).uses_global_identity());
}

fn who(name: &str, email: &str) -> Option<CommitIdentity> {
    Some(CommitIdentity {
        name: name.into(),
        email: email.into(),
    })
}

#[test]
fn the_identity_precedence_runs_card_repository_kronn_global_machine_placeholder() {
    let source = |a, b, c, d| choose_identity(a, b, None, c, d).1;
    let (kc, repo, kg, machine) = (
        who("KC", "kc@corp.example"),
        who("Repo", "repo@corp.example"),
        who("Global", "global@kronn.example"),
        who("Machine", "me@personal.example"),
    );
    // 2. The repository's own, before the Kronn global one.
    assert_eq!(
        source(kc.clone(), repo.clone(), kg.clone(), machine.clone()),
        IdentitySource::KronnConfig
    );
    let (identity, repository) =
        choose_identity(None, repo.clone(), None, kg.clone(), machine.clone());
    assert_eq!(repository, IdentitySource::Repository);
    assert_eq!(identity.email, "repo@corp.example");
    // 2b. What git gives this directory only (conditional include), also
    // before the Kronn global one.
    let (identity, conditional) = choose_identity(
        None,
        None,
        who("Dir", "dir@corp.example"),
        kg.clone(),
        machine.clone(),
    );
    assert_eq!(conditional, IdentitySource::Conditional);
    assert_eq!(identity.email, "dir@corp.example");
    // 3. The Kronn global setting, before the machine's.
    let (identity, kronn_global) = choose_identity(None, None, None, kg.clone(), machine.clone());
    assert_eq!(kronn_global, IdentitySource::KronnGlobal);
    assert_eq!(identity.email, "global@kronn.example");
    // 4. The machine's git config.
    let (identity, machine_source) = choose_identity(None, None, None, None, machine);
    assert_eq!(machine_source, IdentitySource::Global);
    assert_eq!(identity.email, "me@personal.example");
    // 5. The placeholder.
    assert_eq!(source(None, None, None, None), IdentitySource::Fallback);
}

#[test]
fn a_company_repository_identity_is_never_overwritten_by_the_kronn_global_one() {
    let repo = repo();
    let global = card("Global", "global@kronn.example", SignoffSetting::Auto);
    // Nothing in the repository: the Kronn global identity is used, and it is no
    // reason for the machine-identity warning.
    let inherited = CommitPolicy::detect_with(repo.path(), &global);
    assert_eq!(inherited.identity_source, IdentitySource::KronnGlobal);
    assert_eq!(
        inherited.identity.display(),
        "Global <global@kronn.example>"
    );
    assert!(!inherited.uses_global_identity());
    // The repository says who commits for it: that wins.
    git(repo.path(), &["config", "user.name", "Work Person"]);
    git(repo.path(), &["config", "user.email", "work@corp.example"]);
    let own = CommitPolicy::detect_with(repo.path(), &global);
    assert_eq!(own.identity_source, IdentitySource::Repository);
    assert_eq!(own.identity.email, "work@corp.example");
    // The project's own setting wins over both.
    let project = own.with_settings(&card("Bot", "bot@corp.example", SignoffSetting::Auto));
    assert_eq!(project.identity_source, IdentitySource::Setting);
    assert_eq!(project.identity.email, "bot@corp.example");
}

#[test]
fn without_a_kronn_global_identity_the_machine_one_is_used_and_flagged() {
    let repo = repo();
    let policy = CommitPolicy::detect_with(repo.path(), &ProjectCommitSettings::default());
    assert_eq!(policy.identity_source, IdentitySource::Global);
    assert!(policy.uses_global_identity());
    // A half Kronn global identity is no identity.
    let half = CommitPolicy::detect_with(repo.path(), &card("Global", "", SignoffSetting::Auto));
    assert_eq!(half.identity_source, IdentitySource::Global);
}

#[test]
fn the_kronn_global_sign_off_decides_only_what_the_repository_leaves_open() {
    let always = card("", "", SignoffSetting::Always);
    let never = card("", "", SignoffSetting::Never);
    let auto = ProjectCommitSettings::default();

    // Nothing detected: the Kronn global setting, else off.
    let bare = repo();
    let policy = CommitPolicy::detect_with(bare.path(), &always);
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::KronnGlobal);
    let policy = CommitPolicy::detect_with(bare.path(), &never);
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::KronnGlobal);
    let policy = CommitPolicy::detect_with(bare.path(), &auto);
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::Default);

    // The repository's own answers (DCO file, git config, signed history) win.
    let dco = repo();
    fs::write(dco.path().join("DCO"), "dco").unwrap();
    let policy = CommitPolicy::detect_with(dco.path(), &never);
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::DcoFile);
    let configured = repo();
    git(configured.path(), &["config", "kronn.signoff", "false"]);
    let policy = CommitPolicy::detect_with(configured.path(), &always);
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::KronnConfig);
    let signed = repo();
    for n in 0..4 {
        commit(
            signed.path(),
            &format!("change {n}\n\nSigned-off-by: A <a@x.example>"),
        );
    }
    let policy = CommitPolicy::detect_with(signed.path(), &never);
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::History);

    // The project's own setting wins over all of them.
    let policy = CommitPolicy::detect_with(dco.path(), &always).with_settings(&card(
        "",
        "",
        SignoffSetting::Never,
    ));
    assert!(!policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::Setting);
    let policy = CommitPolicy::detect_with(bare.path(), &never).with_settings(&card(
        "",
        "",
        SignoffSetting::Always,
    ));
    assert!(policy.signoff);
    assert_eq!(policy.signoff_source, SignoffSource::Setting);
}

#[test]
fn the_card_identity_is_both_fields_and_a_plausible_address() {
    let ok = card(" Bot ", " bot@corp.example ", SignoffSetting::Auto)
        .normalized()
        .unwrap();
    assert_eq!(ok.identity(), Some(("Bot", "bot@corp.example")));
    assert!(card("", "", SignoffSetting::Always).normalized().is_ok());
    for (name, email) in [
        ("Bot", ""),
        ("", "bot@corp.example"),
        ("Bot", "not-an-address"),
        ("Bot", "a b@corp.example"),
        ("Bot", "@corp.example"),
        ("Bot", "bot@localhost"),
        ("Bo<t", "bot@corp.example"),
        ("Bot", "bot@corp.example>"),
        ("Bo\nt", "bot@corp.example"),
    ] {
        assert!(
            card(name, email, SignoffSetting::Auto)
                .normalized()
                .is_err(),
            "{name:?} {email:?}"
        );
    }
}

// ── Identity from a conditional include (includeIf gitdir/hasconfig/onbranch) ──

/// A directory under which the isolated global git config (the harness's own
/// scratch file, see `git_global_config::test_work_dir`) `includeIf`s a work
/// identity. The condition names this process' own directory, so no other test
/// sees it; the include file it points at is written here, once.
fn work_dir() -> &'static Path {
    static WORK: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    WORK.get_or_init(|| {
        crate::core::git_global_config::isolate_global_git_config_for_tests();
        let (work, include) = crate::core::git_global_config::test_work_dir();
        fs::write(
            include,
            "[user]\n\tname = Work Person\n\temail = work@company.example\n",
        )
        .unwrap();
        work
    })
}

fn repo_in(parent: &Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir_in(parent).unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

#[test]
fn an_identity_the_global_config_gives_only_this_directory_is_the_repositorys_own() {
    let work = repo_in(work_dir());
    let global = card("Global", "global@kronn.example", SignoffSetting::Auto);
    let policy = CommitPolicy::detect_with(work.path(), &global);
    // Git's effective identity here is the conditional include's, not the Kronn
    // global one: a personal setting never signs a company repository.
    assert_eq!(policy.identity.email, "work@company.example");
    assert_eq!(policy.identity_source.as_str(), "conditional");
    assert!(!policy.uses_global_identity());
    // The Kronn global setting is out of the way: nothing to warn about either.
    let bare = CommitPolicy::detect_with(work.path(), &ProjectCommitSettings::default());
    assert_eq!(bare.identity.email, "work@company.example");
    assert!(!bare.uses_global_identity());
}

#[test]
fn a_repository_outside_the_conditional_include_takes_the_kronn_global_identity() {
    let outside = repo();
    let global = card("Global", "global@kronn.example", SignoffSetting::Auto);
    let policy = CommitPolicy::detect_with(outside.path(), &global);
    assert_eq!(policy.identity_source, IdentitySource::KronnGlobal);
    assert_eq!(policy.identity.email, "global@kronn.example");
}

#[test]
fn the_plain_machine_identity_is_still_flagged_without_an_include_or_a_kronn_global() {
    let outside = repo();
    work_dir();
    let policy = CommitPolicy::detect_with(outside.path(), &ProjectCommitSettings::default());
    assert_eq!(policy.identity_source, IdentitySource::Global);
    assert!(policy.uses_global_identity());
    assert_eq!(policy.identity.email, "kronn-test@example.invalid");
}

#[test]
fn the_repository_own_identity_and_the_project_card_still_beat_a_conditional_one() {
    let work = repo_in(work_dir());
    git(work.path(), &["config", "user.name", "Local"]);
    git(
        work.path(),
        &["config", "user.email", "local@company.example"],
    );
    let detected = CommitPolicy::detect_with(work.path(), &ProjectCommitSettings::default());
    assert_eq!(detected.identity_source, IdentitySource::Repository);
    assert_eq!(detected.identity.email, "local@company.example");
    let carded = detected.with_settings(&card("Bot", "bot@company.example", SignoffSetting::Auto));
    assert_eq!(carded.identity_source, IdentitySource::Setting);
}
