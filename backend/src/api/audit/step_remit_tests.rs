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
    write(dir.path(), "docs/inconsistencies-performance.md", "old docs\n");
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one\n");
    write(dir.path(), ".ai/AGENTS.md", "new root\n");
    write(dir.path(), "CLAUDE.md", "pointer\n");
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "initial"]);
    dir
}

fn root(rel: &str) -> DocsRoot {
    DocsRoot::from_rel(rel).unwrap()
}

#[test]
fn a_commit_made_during_the_step_is_soft_reset_and_reported() {
    let dir = repo();
    let before = git(dir.path(), &["rev-parse", "HEAD"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    write(dir.path(), ".ai/inconsistencies-performance.md", "audit output\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "DOPS-0: step 12 quality review"]);
    write(dir.path(), ".ai/more.md", "second\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "DOPS-0: follow-up"]);

    let report = remit.settle_blocking();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), before, "HEAD is back");
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "main");
    assert_eq!(report.commits_undone.len(), 2, "{report:?}");
    assert!(report.commits_undone[0].contains("step 12 quality review"));
    assert!(report.commits_undone[1].contains("follow-up"), "oldest first");
    assert!(report.warnings.is_empty(), "{report:?}");
    // Nothing is lost: the work stays, uncommitted.
    assert_eq!(read(dir.path(), ".ai/inconsistencies-performance.md"), "audit output\n");
    assert_eq!(read(dir.path(), ".ai/more.md"), "second\n");
    let status = git(dir.path(), &["status", "--porcelain"]);
    assert!(status.contains(".ai/inconsistencies-performance.md"), "{status}");
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
    write(dir.path(), ".ai/inconsistencies-performance.md", "audit output\n");
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
    assert!(report.warnings.iter().any(|w| w.contains("not all the step's own")), "{report:?}");
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

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head, "mixed history is not rewritten");
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

    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "audit/step-12");
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
    assert!(report.warnings.iter().any(|w| w.contains("refs/tags/audit-done")), "{report:?}");
}

#[test]
fn a_history_rewrite_is_never_touched() {
    let dir = repo();
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    git(dir.path(), &["commit", "-q", "--amend", "-m", "initial, reworded"]);
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
    write(dir.path(), "docs/inconsistencies-performance.md", "old docs\naudit_history: x\n");
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one\naudit_history: x\n");
    write(dir.path(), ".ai/inconsistencies-performance.md", "the real output\n");
    write(dir.path(), "CLAUDE.md", "pointer\nKronn block\n");

    let report = remit.settle_blocking();

    assert_eq!(read(dir.path(), "docs/inconsistencies-performance.md"), "old docs\n");
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(report.reverted.len(), 2, "{report:?}");
    // Inside the root, and a Kronn-owned pointer: kept.
    assert_eq!(read(dir.path(), ".ai/inconsistencies-performance.md"), "the real output\n");
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
    assert_eq!(report.reverted, vec!["docs/tech-debt/TD-1.md".to_string()]);
}

#[test]
fn a_users_uncommitted_edit_outside_the_root_survives_the_step() {
    let dir = repo();
    write(dir.path(), "src/main.rs", "fn main() { /* user wip */ }\n");
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();

    // Untouched by the agent: left exactly as the user had it.
    let report = remit.settle_blocking();
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(read(dir.path(), "src/main.rs"), "fn main() { /* user wip */ }\n");

    // Changed again by the agent: back to the user's content, not to HEAD.
    write(dir.path(), "src/main.rs", "fn main() { agent() }\n");
    let report = remit.settle_blocking();
    assert_eq!(report.reverted, vec!["src/main.rs".to_string()]);
    assert_eq!(read(dir.path(), "src/main.rs"), "fn main() { /* user wip */ }\n");
}

#[test]
fn an_outside_edit_the_agent_committed_is_restored_after_the_soft_reset() {
    let dir = repo();
    let before = git(dir.path(), &["rev-parse", "HEAD"]);
    let remit = StepRemit::capture_blocking(dir.path(), &root(".ai")).unwrap();
    write(dir.path(), "docs/tech-debt/TD-1.md", "td one\naudit_history: x\n");
    write(dir.path(), ".ai/a.md", "a\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "everything"]);

    let report = remit.settle_blocking();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(report.commits_undone.len(), 1);
    assert_eq!(read(dir.path(), "docs/tech-debt/TD-1.md"), "td one\n");
    assert_eq!(read(dir.path(), ".ai/a.md"), "a\n");
    assert!(git(dir.path(), &["status", "--porcelain"]).lines().all(|l| l.contains(".ai/")));
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
    write(dir.path(), "app/.ai/b.md", "b\n");
    write(dir.path(), "other/c.md", "c\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "initial"]);
    let project = dir.path().join("app");
    let remit = StepRemit::capture_blocking(&project, &root(".ai")).unwrap();
    write(dir.path(), "app/docs/a.md", "a changed\n");
    write(dir.path(), "app/.ai/b.md", "b changed\n");
    let report = remit.settle_blocking();
    assert_eq!(report.reverted, vec!["docs/a.md".to_string()]);
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
    assert!(moved.contains("`git push`") && moved.contains("`git branch`") && moved.contains("`git tag`"));
    assert!(moved.contains("the repository's own commit conventions"));
    assert!(moved.contains("Write documentation ONLY under `.ai/`"));
    assert!(moved.contains("read-only history"));
    assert!(moved.contains("a leftover docs/, ai/, .ai/ or doc/ folder"), "the old folder is named: {moved}");
    assert!(!moved.contains("`docs/"), "a moved root's prompt never quotes `docs/`");
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
        reverted: vec!["docs/a.md".into(), "docs/b.md".into()],
        warnings: vec!["w".into()],
    };
    let lines = report.messages();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("soft reset") && lines[0].contains("abc123 DOPS-0: x"));
    assert!(lines[1].contains("docs/a.md, docs/b.md"));
    assert_eq!(lines[2], "w");
}
