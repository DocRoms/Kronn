// KT-1360 — an audit step stays inside its remit: no commit it made survives,
// and with a moved docs root nothing outside that root is left edited.

use super::*;

fn git_in(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> String {
    let mut cmd = crate::core::cmd::git_cmd();
    cmd.arg("-c")
        .arg("user.name=Fixture")
        .arg("-c")
        .arg("user.email=fixture@example.invalid")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .arg("-C")
        .arg(dir)
        .args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_in(dir, args, &[])
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

/// A repository holding the old `docs/` folder and a `.ai/` root, committed.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    write(
        dir.path(),
        "docs/inconsistencies-performance.md",
        "old docs\n",
    );
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one\n");
    write(dir.path(), ".ai/AGENTS.md", "new root\n");
    write(dir.path(), "CLAUDE.md", "pointer\n");
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "initial"]);
    dir
}

fn paths(report: &RemitReport) -> Vec<&str> {
    report.reverted.iter().map(|r| r.path.as_str()).collect()
}

fn root(rel: &str) -> DocsRoot {
    DocsRoot::from_rel(rel).unwrap()
}

#[test]
fn a_commit_made_during_the_step_is_soft_reset_and_reported() {
    let dir = repo();
    let before = git(dir.path(), &["rev-parse", "HEAD"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    write(
        dir.path(),
        ".ai/inconsistencies-performance.md",
        "audit output\n",
    );
    git(dir.path(), &["add", "-A"]);
    git(
        dir.path(),
        &["commit", "-q", "-m", "DOPS-0: step 12 quality review"],
    );
    write(dir.path(), ".ai/more.md", "second\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "DOPS-0: follow-up"]);

    let report = remit.settle_blocking();

    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD"]),
        before,
        "HEAD is back"
    );
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "main");
    assert_eq!(report.commits_undone.len(), 2, "{report:?}");
    assert!(report.commits_undone[0].contains("step 12 quality review"));
    assert!(
        report.commits_undone[1].contains("follow-up"),
        "oldest first"
    );
    assert!(report.warnings.is_empty(), "{report:?}");
    // Nothing is lost: the work stays, uncommitted.
    assert_eq!(
        read(dir.path(), ".ai/inconsistencies-performance.md"),
        "audit output\n"
    );
    assert_eq!(read(dir.path(), ".ai/more.md"), "second\n");
    let status = git(dir.path(), &["status", "--porcelain"]);
    assert!(
        status.contains(".ai/inconsistencies-performance.md"),
        "{status}"
    );
    assert!(report.messages()[0].contains("uncommitted"));
}

#[test]
fn settling_twice_finds_nothing_left_to_do() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "x"]);
    assert!(!remit.settle_blocking().is_empty());
    assert!(remit.settle_blocking().is_empty());
}

#[test]
fn a_step_without_commit_or_stray_edit_reports_nothing() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(
        dir.path(),
        ".ai/inconsistencies-performance.md",
        "audit output\n",
    );
    write(dir.path(), "untracked.txt", "left alone\n");
    assert!(remit.settle_blocking().is_empty());
    assert_eq!(read(dir.path(), "untracked.txt"), "left alone\n");
}

#[test]
fn a_commit_authored_before_the_step_is_never_touched() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    // The user's own commit lands while the step runs, dated before it began.
    write(dir.path(), "src/main.rs", "fn main() { println!(); }\n");
    git(dir.path(), &["add", "-A"]);
    git_in(
        dir.path(),
        &["commit", "-q", "-m", "user work"],
        &[("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")],
    );
    let user_head = git(dir.path(), &["rev-parse", "HEAD"]);

    let report = remit.settle_blocking();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), user_head);
    assert!(report.commits_undone.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("not all the step's own")),
        "{report:?}"
    );
}

#[test]
fn a_step_commit_stacked_on_a_user_commit_is_left_with_a_warning() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "src/main.rs", "// user\n");
    git(dir.path(), &["add", "-A"]);
    git_in(
        dir.path(),
        &["commit", "-q", "-m", "user work"],
        &[("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")],
    );
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "agent work"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let report = remit.settle_blocking();

    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD"]),
        head,
        "mixed history is not rewritten"
    );
    assert!(report.commits_undone.is_empty());
    assert!(!report.warnings.is_empty());
}

#[test]
fn a_branch_switch_is_reported_and_not_undone() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["checkout", "-q", "-b", "audit/step-12"]);
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "on a new branch"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let report = remit.settle_blocking();

    assert_eq!(
        git(dir.path(), &["branch", "--show-current"]),
        "audit/step-12"
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(report.commits_undone.is_empty());
    let text = report.messages().join("\n");
    assert!(text.contains("branch changed"), "{text}");
    assert!(text.contains("refs/heads/audit/step-12"), "{text}");
}

#[test]
fn a_new_tag_is_reported() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["tag", "audit-done"]);
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("refs/tags/audit-done")),
        "{report:?}"
    );
}

#[test]
fn a_history_rewrite_is_never_touched() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(
        dir.path(),
        &["commit", "-q", "--amend", "-m", "initial, reworded"],
    );
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let report = remit.settle_blocking();
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(report.commits_undone.is_empty());
    assert!(!report.warnings.is_empty());
}

#[test]
fn edits_to_the_old_docs_folder_are_reverted_when_the_root_moved() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    // Step 12 followed the existing TD sheets of docs/ instead of .ai/.
    write(
        dir.path(),
        "docs/inconsistencies-performance.md",
        "old docs\naudit_history: x\n",
    );
    write(
        dir.path(),
        "docs/tech-debt/TD-1.md",
        "td one\naudit_history: x\n",
    );
    write(
        dir.path(),
        ".ai/inconsistencies-performance.md",
        "the real output\n",
    );
    write(dir.path(), "CLAUDE.md", "pointer\nKronn block\n");

    let report = remit.settle_blocking();

    assert_eq!(
        read(dir.path(), "docs/inconsistencies-performance.md"),
        "old docs\n"
    );
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(report.reverted.len(), 2, "{report:?}");
    // Inside the root, and a Kronn-owned pointer: kept.
    assert_eq!(
        read(dir.path(), ".ai/inconsistencies-performance.md"),
        "the real output\n"
    );
    assert_eq!(read(dir.path(), "CLAUDE.md"), "pointer\nKronn block\n");
    let text = report.messages().join("\n");
    assert!(text.contains("docs/tech-debt/TD-1.md"), "{text}");
    assert!(text.contains("outside the docs root"), "{text}");
}

#[test]
fn a_deleted_tracked_file_outside_the_root_comes_back() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    std::fs::remove_file(dir.path().join("docs/tech-debt/TD-1.md")).unwrap();
    let report = remit.settle_blocking();
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(paths(&report), vec!["docs/tech-debt/TD-1.md"]);
}

/// The agent swaps an AI-doc folder for a link to a folder of another project
/// and leaves the real one aside. Returns the victim folder.
#[cfg(unix)]
fn swap_docs_for_a_link(dir: &Path) -> tempfile::TempDir {
    let victim = tempfile::tempdir().unwrap();
    write(victim.path(), "tech-debt/TD-1.md", "victim content\n");
    std::fs::rename(dir.join("docs"), dir.join("docs.aside")).unwrap();
    std::os::unix::fs::symlink(victim.path(), dir.join("docs")).unwrap();
    victim
}

/// KT-1595 — the restore of a file the user had dirty never WRITES through an
/// AI-doc folder the agent swapped for a link.
#[cfg(unix)] // swaps a folder for a symlink through std::os::unix
#[test]
fn a_restore_never_writes_through_an_ai_folder_swapped_for_a_link() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, user wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let victim = swap_docs_for_a_link(dir.path());

    let report = remit.settle_blocking();

    assert_eq!(
        read(victim.path(), "tech-debt/TD-1.md"),
        "victim content\n",
        "the folder behind the link is untouched"
    );
    assert!(report.reverted.is_empty(), "{report:?}");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/TD-1.md") && w.contains("could not be restored")),
        "the refusal is reported: {report:?}"
    );
}

/// KT-1595 (Argon-2) — a file that was CLEAN before the step goes back from
/// HEAD without git writing its path: an AI-doc folder swapped for a link never
/// receives the HEAD content.
#[cfg(unix)] // swaps a folder for a symlink through std::os::unix
#[test]
fn a_put_back_from_head_never_writes_through_an_ai_folder_swapped_for_a_link() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    let victim = swap_docs_for_a_link(dir.path());

    let report = remit.settle_blocking();

    assert_eq!(
        read(victim.path(), "tech-debt/TD-1.md"),
        "victim content\n",
        "the folder behind the link is untouched"
    );
    assert!(report.reverted.is_empty(), "{report:?}");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/TD-1.md") && w.contains("could not be restored")),
        "the refusal is reported: {report:?}"
    );
}

/// KT-1595 (Argon-2) — the swap lands between the read of the current bytes and
/// the write of the HEAD blob: the content goes to the folder the call holds.
#[cfg(unix)] // the swap probe plants a symlink through std::os::unix
#[test]
fn a_folder_swapped_right_before_the_put_back_from_head_never_receives_it() {
    use crate::core::fs_guard::swap_probe;
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    let victim = tempfile::tempdir().unwrap();
    // The keep of the current bytes comes first; the swap fires on the write.
    swap_probe::arm_after(&dir.path().join("docs/tech-debt"), victim.path(), 1);

    let _ = remit.settle_blocking();

    assert!(swap_probe::fired(), "the swap really happened");
    assert_eq!(std::fs::read_dir(victim.path()).unwrap().count(), 0);
    assert_eq!(
        read(dir.path(), "docs/tech-debt.moved/TD-1.md"),
        "td one\n",
        "the HEAD content went to the folder that was held"
    );
}

/// KT-1595 (Argon-2) — the executable bit git records comes back with the
/// content.
#[cfg(unix)] // checks chmod mode bits
#[test]
fn a_put_back_from_head_restores_the_recorded_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo();
    let path = dir.path().join("docs/tech-debt/TD-1.md");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "exec"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let report = remit.settle_blocking();

    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(report.reverted.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

/// KT-1595 — a pre-step "deleted" state must not delete the file of the folder
/// behind the swapped link.
#[cfg(unix)] // swaps a folder for a symlink through std::os::unix
#[test]
fn a_restore_never_deletes_through_an_ai_folder_swapped_for_a_link() {
    let dir = repo();
    std::fs::remove_file(dir.path().join("docs/tech-debt/TD-1.md")).unwrap();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let victim = swap_docs_for_a_link(dir.path());

    let report = remit.settle_blocking();

    assert_eq!(
        read(victim.path(), "tech-debt/TD-1.md"),
        "victim content\n",
        "the file behind the link is still there"
    );
    assert!(report.reverted.is_empty(), "{report:?}");
}

/// KT-1595 — the swap lands between the checks and the write itself: the
/// restore goes to the folder it holds, never to the link's target.
#[cfg(unix)] // the swap probe plants a symlink through std::os::unix
#[test]
fn a_folder_swapped_right_before_the_restore_write_never_receives_it() {
    use crate::core::fs_guard::swap_probe;
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, user wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    let victim = tempfile::tempdir().unwrap();
    // The read and the keep come first; the swap fires on the write.
    swap_probe::arm_after(&dir.path().join("docs/tech-debt"), victim.path(), 2);

    let _ = remit.settle_blocking();

    assert!(swap_probe::fired(), "the swap really happened");
    assert_eq!(std::fs::read_dir(victim.path()).unwrap().count(), 0);
}

/// KT-1595 — a link at the tracked name itself is not read, kept nor
/// overwritten either.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_link_planted_at_the_tracked_name_is_left_alone() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, user wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let victim = tempfile::tempdir().unwrap();
    write(victim.path(), "secret.md", "victim content\n");
    std::fs::remove_file(dir.path().join("docs/tech-debt/TD-1.md")).unwrap();
    std::os::unix::fs::symlink(
        victim.path().join("secret.md"),
        dir.path().join("docs/tech-debt/TD-1.md"),
    )
    .unwrap();

    let report = remit.settle_blocking();

    assert_eq!(read(victim.path(), "secret.md"), "victim content\n");
    assert!(report.reverted.is_empty(), "{report:?}");
}

#[test]
fn a_users_uncommitted_edit_in_another_ai_folder_is_restored_to_what_it_was() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, user wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    // Untouched by the agent: left exactly as the user had it.
    let report = remit.settle_blocking();
    assert!(report.is_empty(), "{report:?}");

    // Changed again by the agent: back to the user's content, not to HEAD.
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    let report = remit.settle_blocking();
    assert_eq!(paths(&report), vec!["docs/tech-debt/TD-1.md"]);
    assert_eq!(
        read(dir.path(), "docs/tech-debt/TD-1.md"),
        "td one, user wip\n"
    );
}

/// KT-1595 round 7: a file the user had dirty keeps the user's INDEX entry too.
/// The agent's staged version never survives the restore, whether the agent
/// also left its bytes in the working tree or wrote the user's bytes back.
#[test]
fn a_users_dirty_ai_file_gets_its_index_entry_back_too() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, user wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let staged_before = git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]);

    // A: the agent writes and stages its version.
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    let report = remit.settle_blocking();
    assert_eq!(paths(&report), vec!["docs/tech-debt/TD-1.md"]);
    assert_eq!(
        read(dir.path(), "docs/tech-debt/TD-1.md"),
        "td one, user wip\n"
    );
    assert_eq!(
        git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]),
        staged_before
    );

    // B: the agent stages its version, then writes the user's bytes back.
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, user wip\n");
    let report = remit.settle_blocking();
    assert_eq!(
        git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]),
        staged_before
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/TD-1.md") && w.contains("index entry")),
        "{report:?}"
    );
    // Settled: nothing left to do.
    assert!(remit.settle_blocking().is_empty());
}

/// KT-1595 round 8: a file the user had dirty that the agent brought back to
/// HEAD (`git restore`, `git checkout HEAD --`) is no longer listed by `git
/// status`, and is restored all the same: bytes and staged version.
#[test]
fn a_users_dirty_file_the_agent_restored_to_head_comes_back() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, staged wip\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, wip\n");
    write(dir.path(), "src/main.rs", "fn main() { wip() }\n");
    let staged_before = git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    git(
        dir.path(),
        &[
            "restore",
            "--staged",
            "--worktree",
            "--",
            "docs/tech-debt/TD-1.md",
            "src/main.rs",
        ],
    );
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    let report = remit.settle_blocking();

    assert_eq!(paths(&report), vec!["docs/tech-debt/TD-1.md"]);
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one, wip\n");
    assert_eq!(
        git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]),
        staged_before
    );
    // A source file is never touched, only reported.
    assert_eq!(read(dir.path(), "src/main.rs"), "fn main() {}\n");
    assert!(
        report.warnings.iter().any(|w| w.contains("src/main.rs")),
        "{report:?}"
    );
}

/// KT-1595 round 8: a source file the agent hid with `--assume-unchanged` is
/// still a change.
#[test]
fn a_source_file_hidden_with_assume_unchanged_is_reported() {
    let dir = repo();
    write(dir.path(), "src/main.rs", "wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(
        dir.path(),
        &["update-index", "--assume-unchanged", "src/main.rs"],
    );
    let report = remit.settle_blocking();
    assert!(
        report.warnings.iter().any(|w| w.contains("src/main.rs")),
        "{report:?}"
    );
}

/// KT-1595 round 8: the whole index state comes back: an intent-to-add entry
/// stays one (no empty file staged), and a flag the user set is set again.
#[test]
fn an_intent_to_add_entry_and_a_flag_come_back_as_they_were() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, wip\n");
    git(
        dir.path(),
        &["update-index", "--skip-worktree", "docs/tech-debt/TD-1.md"],
    );
    write(dir.path(), "docs/tech-debt/TD-9.md", "new sheet\n");
    git(dir.path(), &["add", "-N", "docs/tech-debt/TD-9.md"]);
    let before = git(dir.path(), &["ls-files", "-v", "-s", "docs/tech-debt"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    assert!(remit.index_before.contains_key("docs/tech-debt/TD-9.md"));

    git(dir.path(), &["add", "docs/tech-debt/TD-9.md"]);
    git(
        dir.path(),
        &[
            "update-index",
            "--no-skip-worktree",
            "docs/tech-debt/TD-1.md",
        ],
    );
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    remit.settle_blocking();

    assert_eq!(
        git(dir.path(), &["ls-files", "-v", "-s", "docs/tech-debt"]),
        before
    );
    assert_eq!(
        git(
            dir.path(),
            &[
                "diff",
                "--cached",
                "--name-only",
                "--",
                "docs/tech-debt/TD-9.md"
            ]
        ),
        "",
        "an intent-to-add entry stages nothing"
    );
}

/// KT-1595 round 8: the stages of a conflict are part of what the user had: an
/// agent's `git add` of the file does not settle the user's merge.
#[test]
fn a_users_conflict_keeps_its_stages() {
    let dir = repo();
    git(dir.path(), &["switch", "-q", "-c", "side"]);
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, side\n");
    git(dir.path(), &["commit", "-q", "-am", "side"]);
    git(dir.path(), &["switch", "-q", "main"]);
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, main\n");
    git(dir.path(), &["commit", "-q", "-am", "main"]);
    let merge = crate::core::cmd::git_cmd()
        .args([
            "-c",
            "user.name=F",
            "-c",
            "user.email=f@e.invalid",
            "merge",
            "-q",
            "side",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the fixture needs a conflict");
    let stages_before = git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]);
    assert_eq!(stages_before.lines().count(), 3, "{stages_before}");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    remit.settle_blocking();

    assert_eq!(
        git(dir.path(), &["ls-files", "-s", "docs/tech-debt/TD-1.md"]),
        stages_before
    );
}

/// KT-1595 round 8: an index that cannot be written (a lock) does not hide the
/// content restore: the overwritten bytes are still reported as a blob.
#[test]
fn a_locked_index_keeps_the_restored_blob_reported() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    std::fs::write(dir.path().join(".git/index.lock"), "").unwrap();
    let report = remit.settle_blocking();
    std::fs::remove_file(dir.path().join(".git/index.lock")).unwrap();
    assert_eq!(paths(&report), vec!["docs/tech-debt/TD-1.md"]);
    assert!(report.reverted[0].blob.is_some());
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one, wip\n");
    assert!(
        report.warnings.iter().any(|w| w.contains("index entry")),
        "{report:?}"
    );
}

/// KT-1595 round 9: a flag set during the step hides the agent's edit from
/// `git status`; settle compares the flags too.
#[test]
fn an_edit_hidden_by_a_flag_set_during_the_step_is_restored_or_reported() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    git(
        dir.path(),
        &[
            "update-index",
            "--assume-unchanged",
            "docs/tech-debt/TD-1.md",
        ],
    );
    write(dir.path(), "src/main.rs", "fn main() { agent() }\n");
    git(
        dir.path(),
        &["update-index", "--skip-worktree", "src/main.rs"],
    );
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    let report = remit.settle_blocking();
    assert_eq!(paths(&report), vec!["docs/tech-debt/TD-1.md"]);
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert!(
        git(dir.path(), &["ls-files", "-v", "docs/tech-debt/TD-1.md"]).starts_with('H'),
        "the flag the agent set is cleared"
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("src/main.rs")),
        "{report:?}"
    );
}

/// KT-1595 round 9: the recorded entries go back NUL-separated, so a path with a
/// newline cannot smuggle a second entry (another file's index) in.
#[cfg(unix)] // a newline in a file name cannot exist on Windows
#[test]
fn a_newline_in_a_path_never_rewrites_another_index_entry() {
    let dir = repo();
    write(dir.path(), "Makefile", "all:\n");
    git(dir.path(), &["add", "Makefile"]);
    git(dir.path(), &["commit", "-q", "-m", "make"]);
    let makefile_before = git(dir.path(), &["ls-files", "-s", "Makefile"]);
    let evil = git(dir.path(), &["hash-object", "-w", "--stdin"]); // empty blob
    let name = format!("docs/tech-debt/n\n100644 {evil} 0\tMakefile");
    write(dir.path(), &name, "odd\n");
    git(dir.path(), &["add", "--", &name]);
    git(dir.path(), &["commit", "-q", "-m", "odd"]);
    write(dir.path(), &name, "odd, user wip\n");
    git(dir.path(), &["add", "--", &name]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), &name, "odd, agent\n");
    git(dir.path(), &["add", "--", &name]);
    remit.settle_blocking();
    assert_eq!(
        git(dir.path(), &["ls-files", "-s", "Makefile"]),
        makefile_before
    );
    assert_eq!(read(dir.path(), &name), "odd, user wip\n");
}

/// KT-1595 round 9: an intent-to-add over a path HEAD has (`rm --cached` then
/// `add -N`) stays one.
#[test]
fn an_intent_to_add_over_a_head_path_stays_one() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, rewritten\n");
    git(
        dir.path(),
        &["rm", "-q", "--cached", "docs/tech-debt/TD-1.md"],
    );
    git(dir.path(), &["add", "-N", "docs/tech-debt/TD-1.md"]);
    let status_before = git(
        dir.path(),
        &["status", "--porcelain", "docs/tech-debt/TD-1.md"],
    );
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    assert!(remit.index_before["docs/tech-debt/TD-1.md"].intent_to_add);
    git(dir.path(), &["add", "docs/tech-debt/TD-1.md"]);
    remit.settle_blocking();
    assert_eq!(
        git(
            dir.path(),
            &["status", "--porcelain", "docs/tech-debt/TD-1.md"]
        ),
        status_before
    );
}

/// KT-1595 round 9: a file the agent adds in another AI-doc folder is reported.
#[test]
fn a_file_the_agent_adds_in_another_ai_folder_is_reported() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-3.md", "agent sheet\n");
    git(dir.path(), &["add", "docs/tech-debt/TD-3.md"]);
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/TD-3.md") && w.contains("added")),
        "{report:?}"
    );
}

/// KT-1595 round 9: a tracked name the user had made a link, which the agent
/// replaced, is reported (it was never read, and is still never written).
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_refused_link_the_agent_replaced_is_reported() {
    let dir = repo();
    std::fs::remove_file(dir.path().join("docs/tech-debt/TD-1.md")).unwrap();
    std::os::unix::fs::symlink("../../CLAUDE.md", dir.path().join("docs/tech-debt/TD-1.md"))
        .unwrap();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(
        dir.path(),
        &["checkout", "HEAD", "--", "docs/tech-debt/TD-1.md"],
    );
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/TD-1.md")),
        "{report:?}"
    );
}

/// KT-1595 round 9: both flags (`s`) come back.
#[test]
fn both_flags_come_back() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, wip\n");
    git(
        dir.path(),
        &["update-index", "--skip-worktree", "docs/tech-debt/TD-1.md"],
    );
    git(
        dir.path(),
        &[
            "update-index",
            "--assume-unchanged",
            "docs/tech-debt/TD-1.md",
        ],
    );
    let tag_before = git(dir.path(), &["ls-files", "-v", "docs/tech-debt/TD-1.md"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(
        dir.path(),
        &[
            "update-index",
            "--no-skip-worktree",
            "--no-assume-unchanged",
            "docs/tech-debt/TD-1.md",
        ],
    );
    remit.settle_blocking();
    assert_eq!(
        git(dir.path(), &["ls-files", "-v", "docs/tech-debt/TD-1.md"]),
        tag_before
    );
}

/// KT-1595 round 10: a refused path under a folder that is a link is
/// fingerprinted without stat'ing through that link, and replacing the link
/// with a real folder is reported.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_link_folder_replaced_during_the_step_is_reported() {
    let dir = repo();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("TD-1.md"), "outside\n").unwrap();
    std::fs::remove_dir_all(dir.path().join("docs/tech-debt")).unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("docs/tech-debt")).unwrap();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    assert!(
        remit.refused_before["docs/tech-debt/TD-1.md"].starts_with("link at"),
        "{:?}",
        remit.refused_before
    );
    std::fs::remove_file(dir.path().join("docs/tech-debt")).unwrap();
    git(
        dir.path(),
        &["checkout", "HEAD", "--", "docs/tech-debt/TD-1.md"],
    );
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/TD-1.md")),
        "{report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("TD-1.md")).unwrap(),
        "outside\n"
    );
}

/// KT-1595 round 10: past FLAGGED_CONTENT_MAX flagged files their bytes are not
/// kept; a flag the agent cleared is still seen, and such a file is never
/// rewritten from HEAD.
#[test]
fn many_flagged_files_keep_their_index_and_are_never_rewritten_from_head() {
    let dir = repo();
    let many: Vec<String> = (0..=FLAGGED_CONTENT_MAX)
        .map(|n| format!("docs/tech-debt/f{n}.md"))
        .collect();
    for rel in &many {
        write(dir.path(), rel, "x\n");
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "many"]);
    let mut args = vec!["update-index", "--assume-unchanged"];
    args.extend(many.iter().map(String::as_str));
    git(dir.path(), &args);
    write(dir.path(), "docs/tech-debt/f0.md", "user hidden wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    assert!(!remit.dirty_outside.contains_key("docs/tech-debt/f0.md"));
    git(
        dir.path(),
        &[
            "update-index",
            "--no-assume-unchanged",
            "docs/tech-debt/f0.md",
            "docs/tech-debt/f1.md",
        ],
    );
    let report = remit.settle_blocking();
    assert_eq!(
        read(dir.path(), "docs/tech-debt/f0.md"),
        "user hidden wip\n"
    );
    assert!(git(dir.path(), &["ls-files", "-v", "docs/tech-debt/f1.md"]).starts_with('h'));
    assert!(git(dir.path(), &["ls-files", "-v", "docs/tech-debt/f0.md"]).starts_with('h'));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("docs/tech-debt/f1.md")),
        "{report:?}"
    );
}

/// KT-1595 round 10: a clean filter, a replace ref or `core.bare` the agent
/// writes into the repository never runs a program nor hides its edit from the
/// settle, and the configuration change is reported.
#[cfg(unix)] // runs a `#!/bin/sh` fixture as a program
#[test]
fn a_repository_reconfigured_by_the_agent_runs_nothing_and_hides_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let marker = dir.path().join("filter-ran");
    let script = dir.path().join(".git/clean.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        dir.path(),
        &["config", "filter.k.clean", script.to_str().unwrap()],
    );
    std::fs::create_dir_all(dir.path().join(".git/info")).unwrap();
    std::fs::write(
        dir.path().join(".git/info/attributes"),
        "docs/tech-debt/TD-1.md filter=k\n",
    )
    .unwrap();
    git(dir.path(), &["config", "log.showSignature", "true"]);
    write(dir.path(), "docs/tech-debt/TD-1.md", "td ONE\n");
    let report = remit.settle_blocking();
    assert!(!marker.exists(), "the clean filter must not run");
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("git configuration")),
        "{report:?}"
    );
}

#[test]
fn a_replace_ref_never_hides_an_edit() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one, agent\n");
    let agent_blob = git(dir.path(), &["hash-object", "-w", "docs/tech-debt/TD-1.md"]);
    let head_blob = git(dir.path(), &["rev-parse", "HEAD:docs/tech-debt/TD-1.md"]);
    git(dir.path(), &["replace", &head_blob, &agent_blob]);
    let report = remit.settle_blocking();
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert!(
        report.warnings.iter().any(|w| w.contains("replace")),
        "{report:?}"
    );
}

/// KT-1595 round 11: programs the agent declares through config hooks, an
/// included file, a filter name holding `=` or the worktree config never run.
#[cfg(unix)] // runs `#!/bin/sh` fixtures as programs
#[test]
fn hooks_and_hidden_filters_the_agent_declares_never_run() {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let marker = dir.path().join("program-ran");
    let script = dir.path().join(".git/prog.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let prog = script.to_str().unwrap();
    git(dir.path(), &["config", "hook.k.command", prog]);
    git(
        dir.path(),
        &["config", "--add", "hook.k.event", "post-index-change"],
    );
    git(
        dir.path(),
        &["config", "--add", "hook.k.event", "reference-transaction"],
    );
    let include = dir.path().join(".git/extra.cfg");
    std::fs::write(&include, format!("[filter \"inc\"]\n\tclean = {prog}\n")).unwrap();
    git(
        dir.path(),
        &["config", "include.path", include.to_str().unwrap()],
    );
    git(dir.path(), &["config", "filter.a=b.clean", prog]);
    std::fs::create_dir_all(dir.path().join(".git/info")).unwrap();
    std::fs::write(
        dir.path().join(".git/info/attributes"),
        "docs/tech-debt/TD-1.md filter=inc\ndocs/inconsistencies-performance.md filter=a=b\n",
    )
    .unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td ONE\n");
    write(
        dir.path(),
        "docs/inconsistencies-performance.md",
        "OLD docs\n",
    );
    let report = remit.settle_blocking();
    assert!(!marker.exists(), "no configured program may run");
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(
        read(dir.path(), "docs/inconsistencies-performance.md"),
        "old docs\n"
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("git configuration")),
        "{report:?}"
    );
}

/// KT-1595 round 12: a file stored through a filter (Git LFS) is never
/// rewritten from its raw blob: with filters off a refreshed file looks edited,
/// and the blob is not its content.
#[test]
fn a_filtered_file_is_never_rewritten_from_its_raw_blob() {
    let dir = repo();
    write(
        dir.path(),
        ".gitattributes",
        "docs/tech-debt/TD-1.md filter=up\n",
    );
    git(dir.path(), &["config", "filter.up.clean", "tr a-z A-Z"]);
    git(dir.path(), &["config", "filter.up.smudge", "cat"]);
    git(dir.path(), &["add", ".gitattributes"]);
    git(dir.path(), &["add", "--renormalize", "."]);
    git(dir.path(), &["commit", "-q", "-m", "filtered"]);
    assert_eq!(
        git(
            dir.path(),
            &["cat-file", "blob", "HEAD:docs/tech-debt/TD-1.md"]
        ),
        "TD ONE"
    );
    // An older time, recorded by the user's git (which runs the filter): the
    // file is clean before the step, not racily clean.
    let now = std::time::SystemTime::now();
    let rel = "docs/tech-debt/TD-1.md";
    set_mtime(dir.path(), rel, now - std::time::Duration::from_secs(10));
    git(dir.path(), &["update-index", "--refresh"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    // The same bytes, a later time: git must read the file again.
    set_mtime(dir.path(), rel, now + std::time::Duration::from_secs(5));
    let report = remit.settle_blocking();
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert!(
        report.warnings.iter().any(|w| w.contains("`up` filter")),
        "{report:?}"
    );
}

fn set_mtime(dir: &Path, rel: &str, at: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(dir.join(rel))
        .unwrap()
        .set_modified(at)
        .unwrap();
}

fn set_len(dir: &Path, rel: &str, len: u64) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join(rel))
        .unwrap()
        .set_len(len)
        .unwrap();
}

/// KT-1595 round 13: when the agent makes a user's dirty file unreadable (grown
/// past REMIT_CONTENT_MAX), the user's version is kept as a blob and named, and
/// the user's staged state still comes back.
#[test]
fn a_users_version_is_kept_when_the_agent_grows_the_file_past_the_limit() {
    let dir = repo();
    let rel = "docs/tech-debt/TD-1.md";
    write(dir.path(), rel, "td one, user\n");
    git(dir.path(), &["add", rel]);
    let staged = git(dir.path(), &["rev-parse", &format!(":{rel}")]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["reset", "-q", "--", rel]);
    set_len(dir.path(), rel, REMIT_CONTENT_MAX + 1);
    let report = remit.settle_blocking();
    let warning = report
        .warnings
        .iter()
        .find(|w| w.contains(rel) && w.contains("blob"))
        .unwrap_or_else(|| panic!("{report:?}"));
    let blob = warning
        .split(|c: char| !c.is_ascii_hexdigit())
        .find(|token| token.len() == 40 || token.len() == 64)
        .unwrap_or_else(|| panic!("no blob named: {warning}"));
    assert_eq!(git(dir.path(), &["cat-file", "-p", blob]), "td one, user");
    assert_eq!(git(dir.path(), &["rev-parse", &format!(":{rel}")]), staged);
}

/// KT-1595 round 14: putting back a user's intent-to-add never stages what the
/// agent left at that path (a folder of files, a link).
#[test]
fn an_intent_to_add_never_stages_what_the_agent_put_at_its_path() {
    let dir = repo();
    let rel = "docs/tech-debt/TD-9.md";
    write(dir.path(), rel, "td nine, user\n");
    git(dir.path(), &["add", "-N", "--", rel]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["update-index", "--force-remove", "--", rel]);
    std::fs::remove_file(dir.path().join(rel)).unwrap();
    write(dir.path(), &format!("{rel}/payload.sh"), "echo agent\n");
    let report = remit.settle_blocking();
    assert_eq!(
        git(dir.path(), &["ls-files", "--", &format!("{rel}/")]),
        "",
        "the agent's files were staged: {report:?}"
    );
}

/// KT-1595 round 15: a user's intent-to-add on a SYMLINK comes back as it was
/// when the agent staged it during the step.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_users_intent_to_add_symlink_comes_back() {
    let dir = repo();
    let rel = "docs/tech-debt/link.md";
    std::os::unix::fs::symlink("TD-1.md", dir.path().join(rel)).unwrap();
    git(dir.path(), &["add", "-N", "--", rel]);
    let before = git(dir.path(), &["ls-files", "-s", "--", rel]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["add", "--", rel]);
    let report = remit.settle_blocking();
    assert_eq!(
        git(dir.path(), &["ls-files", "-s", "--", rel]),
        before,
        "{report:?}"
    );
    assert!(
        !report.warnings.iter().any(|w| w.contains("not a file")),
        "{report:?}"
    );
}

/// KT-1595 round 13: a file too large to keep still has its index compared: a
/// flag the agent sets on it is reported and put back.
#[test]
fn an_index_change_on_a_file_too_large_to_keep_is_put_back() {
    let dir = repo();
    let rel = "docs/tech-debt/TD-1.md";
    set_len(dir.path(), rel, REMIT_CONTENT_MAX + 1);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(
        dir.path(),
        &["update-index", "--assume-unchanged", "--", rel],
    );
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains(rel) && w.contains("index")),
        "{report:?}"
    );
    assert!(git(dir.path(), &["ls-files", "-v", "--", rel]).starts_with("H "));
}

/// KT-1595 round 13: a same-size edit of a file too large to keep, its time
/// put back, is still seen (the fingerprint holds the change time and inode).
#[cfg(unix)] // writes in place through std::os::unix FileExt
#[test]
fn a_same_size_edit_with_its_time_put_back_is_still_seen() {
    use std::os::unix::fs::FileExt;
    let dir = repo();
    let rel = "docs/tech-debt/TD-1.md";
    set_len(dir.path(), rel, REMIT_CONTENT_MAX + 1);
    let mtime = std::fs::metadata(dir.path().join(rel))
        .unwrap()
        .modified()
        .unwrap();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::File::options()
        .write(true)
        .open(dir.path().join(rel))
        .unwrap()
        .write_all_at(b"agent", 0)
        .unwrap();
    set_mtime(dir.path(), rel, mtime);
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains(rel) && w.contains("changed during it")),
        "{report:?}"
    );
}

/// KT-1595 round 12: a dirty file past REMIT_CONTENT_MAX is never read whole:
/// it is left as it is, and reported only once it changed.
#[test]
fn a_dirty_file_too_large_to_keep_is_left_as_it_is() {
    let dir = repo();
    let rel = "docs/tech-debt/TD-1.md";
    set_len(dir.path(), rel, REMIT_CONTENT_MAX + 1);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let report = remit.settle_blocking();
    assert!(report.is_empty(), "untouched, nothing to say: {report:?}");
    set_len(dir.path(), rel, REMIT_CONTENT_MAX + 2);
    let report = remit.settle_blocking();
    assert_eq!(
        std::fs::metadata(dir.path().join(rel)).unwrap().len(),
        REMIT_CONTENT_MAX + 2
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("larger than")),
        "{report:?}"
    );
}

/// KT-1595 round 12: a tracked file the agent grew past REMIT_CONTENT_MAX is
/// left as it is (its bytes cannot be kept as a blob first), and reported.
#[test]
fn a_file_the_agent_grew_past_the_limit_is_left_as_it_is() {
    let dir = repo();
    let rel = "docs/tech-debt/TD-1.md";
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    set_len(dir.path(), rel, REMIT_CONTENT_MAX + 1);
    let report = remit.settle_blocking();
    assert_eq!(
        std::fs::metadata(dir.path().join(rel)).unwrap().len(),
        REMIT_CONTENT_MAX + 1
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("larger than")),
        "{report:?}"
    );
}

/// KT-1595 round 11: a staged rename whose new name is not UTF-8 keeps the
/// status fields paired: the origin is restored, nothing bogus is reported and
/// no settle panics.
#[cfg(unix)] // names a file with non-UTF-8 bytes
#[test]
fn a_rename_to_a_non_utf8_name_keeps_the_status_paired() {
    use std::io::Write;
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    let blob = git(dir.path(), &["rev-parse", "HEAD:docs/tech-debt/TD-1.md"]);
    git(
        dir.path(),
        &["rm", "-q", "--cached", "docs/tech-debt/TD-1.md"],
    );
    std::fs::remove_file(dir.path().join("docs/tech-debt/TD-1.md")).unwrap();
    // The new name lives in the index only (APFS refuses non-UTF-8 names).
    let mut entry = format!("100644 {blob} 0\t").into_bytes();
    entry.extend_from_slice(b"docs/tech-debt/\xff.md\0");
    let mut child = crate::core::cmd::git_cmd()
        .args(["update-index", "-z", "--index-info"])
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&entry).unwrap();
    assert!(child.wait().unwrap().success());
    let report = remit.settle_blocking();
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert!(
        report.warnings.iter().all(|w| !w.contains("s/tech-debt")),
        "{report:?}"
    );
}

/// KT-1595 round 11: an orphan branch (HEAD unreadable) before a step is said,
/// never a silent unguarded step.
#[test]
fn a_step_on_an_unreadable_head_is_reported() {
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "--orphan", "wip"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai"))
        .expect("a repository with commits is guarded or reported");
    let report = remit.settle_blocking();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("could not be read before the step")),
        "{report:?}"
    );
}

/// KT-1595 round 7: a source file the user had dirty, whose bytes the agent
/// left alone but whose staged version it changed, is listed (never touched).
#[test]
fn a_source_file_whose_index_the_agent_changed_is_reported() {
    let dir = repo();
    write(dir.path(), "src/main.rs", "wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "src/main.rs", "agent\n");
    git(dir.path(), &["add", "src/main.rs"]);
    write(dir.path(), "src/main.rs", "wip\n");
    let report = remit.settle_blocking();
    assert_eq!(read(dir.path(), "src/main.rs"), "wip\n");
    assert!(
        report.warnings.iter().any(|w| w.contains("src/main.rs")),
        "{report:?}"
    );
}

#[test]
fn a_source_file_changed_during_the_step_is_never_touched_only_reported() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    // The user keeps coding while the audit runs.
    write(
        dir.path(),
        "src/main.rs",
        "fn main() { /* edited during the audit */ }\n",
    );
    git(dir.path(), &["add", "src/main.rs"]);

    let report = remit.settle_blocking();

    assert_eq!(
        read(dir.path(), "src/main.rs"),
        "fn main() { /* edited during the audit */ }\n"
    );
    assert!(
        git(dir.path(), &["status", "--porcelain"]).contains("M  src/main.rs"),
        "index untouched"
    );
    assert!(report.reverted.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("src/main.rs") && w.contains("left untouched")),
        "{report:?}"
    );
}

#[test]
fn a_source_file_the_user_already_had_dirty_is_reported_only_when_it_changes_again() {
    let dir = repo();
    write(dir.path(), "src/main.rs", "wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    assert!(remit.settle_blocking().is_empty());
    write(dir.path(), "src/main.rs", "wip, more\n");
    let report = remit.settle_blocking();
    assert_eq!(read(dir.path(), "src/main.rs"), "wip, more\n");
    assert!(
        report.warnings.iter().any(|w| w.contains("src/main.rs")),
        "{report:?}"
    );
}

#[test]
fn a_nested_agents_md_does_not_make_a_source_folder_an_ai_folder() {
    let dir = repo();
    write(
        dir.path(),
        "src/AGENTS.md",
        "# my own notes for the agent\n",
    );
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "notes"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "src/main.rs", "changed\n");
    let report = remit.settle_blocking();
    assert!(report.reverted.is_empty(), "{report:?}");
    assert_eq!(read(dir.path(), "src/main.rs"), "changed\n");
}

#[test]
fn what_a_restore_overwrites_stays_readable_as_a_blob() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(
        dir.path(),
        "docs/tech-debt/TD-1.md",
        "td one\nedited during the step\n",
    );

    let report = remit.settle_blocking();

    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    let blob = report.reverted[0].blob.clone().expect("a blob is reported");
    assert_eq!(
        git(dir.path(), &["cat-file", "-p", &blob]),
        "td one\nedited during the step"
    );
    let text = report.messages().join("\n");
    assert!(
        text.contains(&blob) && text.contains("git cat-file -p"),
        "{text}"
    );
}

#[test]
fn a_restore_of_a_user_dirty_ai_file_keeps_the_newer_bytes_as_a_blob() {
    let dir = repo();
    write(dir.path(), "docs/tech-debt/TD-1.md", "user wip\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(
        dir.path(),
        "docs/tech-debt/TD-1.md",
        "user wip\nlater edit\n",
    );

    let report = remit.settle_blocking();

    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "user wip\n");
    let blob = report.reverted[0].blob.clone().unwrap();
    assert_eq!(
        git(dir.path(), &["cat-file", "-p", &blob]),
        "user wip\nlater edit"
    );
}

#[test]
fn a_commit_touching_source_is_kept_with_a_warning() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    // Authored during the step, plain, linear — but it changes src/.
    write(dir.path(), "src/main.rs", "fn main() { fix_typo() }\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "fix: typo in src/"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let report = remit.settle_blocking();

    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD"]),
        head,
        "history untouched"
    );
    assert!(report.commits_undone.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("src/main.rs") && w.contains("may be yours")),
        "{report:?}"
    );
}

#[test]
fn a_range_with_one_source_commit_is_left_whole() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "audit"]);
    write(dir.path(), "src/main.rs", "// user\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "user"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let report = remit.settle_blocking();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(report.commits_undone.is_empty());
}

#[test]
fn a_commit_touching_only_the_root_and_a_pointer_is_still_undone() {
    let dir = repo();
    let before = git(dir.path(), &["rev-parse", "HEAD"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), ".ai/a.md", "a\n");
    write(dir.path(), "CLAUDE.md", "pointer\nKronn block\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "DOPS-0: audit"]);

    let report = remit.settle_blocking();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(report.commits_undone.len(), 1, "{report:?}");
    assert!(report.warnings.is_empty(), "{report:?}");
}

#[test]
fn an_outside_edit_the_agent_committed_is_restored_after_the_soft_reset() {
    let dir = repo();
    let before = git(dir.path(), &["rev-parse", "HEAD"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(
        dir.path(),
        "docs/tech-debt/TD-1.md",
        "td one\naudit_history: x\n",
    );
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "everything"]);

    let report = remit.settle_blocking();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(report.commits_undone.len(), 1);
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(read(dir.path(), ".ai/a.md"), "a\n");
    assert!(git(dir.path(), &["status", "--porcelain"])
        .lines()
        .all(|l| l.contains(".ai/")));
}

#[test]
fn a_restored_outside_edit_leaves_no_staged_version_behind() {
    // KT-1595 round 5 (NEW-1): after `reset --soft` the index still holds the
    // agent's committed version; the put-back must restore the index entry too,
    // or the user's next commit re-commits the reverted edit.
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one\nagent edit\n");
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "everything"]);

    let report = remit.settle_blocking();

    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert!(report.warnings.is_empty(), "{report:?}");
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        !staged.contains("docs/tech-debt/TD-1.md"),
        "the agent's version is still staged: {staged}"
    );
    // (`git()` trims its output)
    assert_eq!(
        git(dir.path(), &["show", ":docs/tech-debt/TD-1.md"]),
        "td one"
    );
}

#[test]
fn a_staged_outside_edit_below_the_repository_top_is_put_back_in_the_index() {
    // The index path is top-relative: a project below the top needs its prefix.
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    write(dir.path(), "app/docs/a.md", "a\n");
    write(dir.path(), "app/docs/.kronn.json", "{}\n");
    write(dir.path(), "app/.ai/b.md", "b\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "initial"]);
    let project = dir.path().join("app");
    let remit = StepRemit::capture_blocking(&project, &root(".ai")).unwrap();
    write(dir.path(), "app/docs/a.md", "a changed\n");
    git(dir.path(), &["add", "app/docs/a.md"]);

    let report = remit.settle_blocking();

    assert_eq!(paths(&report), vec!["docs/a.md"]);
    assert_eq!(read(&project, "docs/a.md"), "a\n");
    assert_eq!(git(dir.path(), &["show", ":app/docs/a.md"]), "a");
    assert!(
        git(dir.path(), &["diff", "--cached", "--name-only"])
            .trim()
            .is_empty(),
        "nothing may stay staged"
    );
}

#[test]
fn with_a_moved_head_the_files_are_not_restored_from_a_stale_commit() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["checkout", "-q", "-b", "other"]);
    write(dir.path(), "docs/tech-debt/TD-1.md", "changed\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "elsewhere"]);
    let report = remit.settle_blocking();
    assert!(report.reverted.is_empty());
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "changed\n");
}

#[test]
fn a_project_below_the_repository_top_is_guarded_in_its_own_paths() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    write(dir.path(), "app/docs/a.md", "a\n");
    write(dir.path(), "app/docs/.kronn.json", "{}\n");
    write(dir.path(), "app/.ai/b.md", "b\n");
    write(dir.path(), "other/c.md", "c\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "initial"]);
    let project = dir.path().join("app");
    let remit = StepRemit::capture_blocking(&project, &root(".ai")).unwrap();
    write(dir.path(), "app/docs/a.md", "a changed\n");
    write(dir.path(), "app/.ai/b.md", "b changed\n");
    let report = remit.settle_blocking();
    assert_eq!(paths(&report), vec!["docs/a.md"]);
    assert_eq!(read(&project, "docs/a.md"), "a\n");
    assert_eq!(read(&project, ".ai/b.md"), "b changed\n");
}

#[test]
fn outside_a_git_repository_there_is_nothing_to_guard() {
    let dir = tempfile::tempdir().unwrap();
    assert!(StepRemit::capture_blocking(dir.path(), &root(".ai")).is_none());
}

#[test]
fn the_step_prompt_forbids_commits_and_marks_the_old_folder_read_only() {
    let now = chrono::Utc::now();
    let moved = super::super::dated_step_prompt("", "STEP BODY", now, &root(".ai"));
    assert!(moved.contains("NEVER run `git commit`"), "{moved}");
    assert!(
        moved.contains("`git push`")
            && moved.contains("`git branch`")
            && moved.contains("`git tag`")
    );
    assert!(moved.contains("the repository's own commit conventions"));
    assert!(moved.contains("Write documentation ONLY under `.ai/`"));
    assert!(moved.contains("read-only history"));
    assert!(
        moved.contains("a leftover docs/, ai/, .ai/ or doc/ folder"),
        "the old folder is named: {moved}"
    );
    assert!(
        !moved.contains("`docs/"),
        "a moved root's prompt never quotes `docs/`"
    );
    assert!(moved.contains("`.ai/tech-debt/`"));

    let default = super::super::dated_step_prompt("", "STEP BODY", now, &DocsRoot::default());
    assert!(default.contains("NEVER run `git commit`"));
    assert!(default.contains("Write documentation ONLY under `docs/`"));
    assert!(default.contains("read-only history"));
}

#[test]
fn the_report_reads_as_one_line_per_finding() {
    let report = RemitReport {
        commits_undone: vec!["abc123 DOPS-0: x".into()],
        reverted: vec![
            Reverted {
                path: "docs/a.md".into(),
                blob: Some("deadbeef".into()),
            },
            Reverted {
                path: "docs/b.md".into(),
                blob: None,
            },
        ],
        warnings: vec!["w".into()],
    };
    let lines = report.messages();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("soft reset") && lines[0].contains("abc123 DOPS-0: x"));
    assert!(lines[1].contains(
        "docs/a.md (the version before the restore is blob deadbeef: `git cat-file -p deadbeef`)"
    ));
    assert!(lines[1].contains("docs/b.md (deleted by the step)"));
    assert_eq!(lines[2], "w");
}
