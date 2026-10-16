use super::*;
use std::collections::BTreeMap;
use std::process::Command;
use tempfile::TempDir;

fn sh_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn template_docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs")
}

/// A committed, documented repo with an identity for the accept commit.
fn documented_git_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    sh_git(root, &["init", "-q", "-b", "main"]);
    sh_git(root, &["config", "user.name", "Test"]);
    sh_git(root, &["config", "user.email", "test@example.com"]);
    write(
        root,
        "AGENTS.md",
        "# Agent guide\n\n## Running tests\n\nmake iso-test\n",
    );
    write(root, "doc/deploy.md", "# Deployment\n\nTag then push.\n");
    write(
        root,
        "docs/coding-rules.md",
        "# Rules\n\nNo ticket ids in code.\n",
    );
    sh_git(root, &["add", "-A"]);
    sh_git(root, &["commit", "-q", "-m", "init"]);
    tmp
}

fn hashes(root: &Path) -> BTreeMap<String, String> {
    let files: Vec<String> = tracked_files(root).unwrap().into_iter().collect();
    docs_merge::fingerprint(root, &files)
}

#[test]
fn the_read_only_plan_writes_nothing_and_lists_untracked_docs() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    write(root, "docs/local-notes.md", "## Debugging\n\nuntracked\n");
    let status_before = sh_git(root, &["status", "--porcelain"]);
    let view = preview_plan(root, &template_docs()).unwrap();
    assert_eq!(sh_git(root, &["status", "--porcelain"]), status_before);
    assert!(!root.join("docs/AGENTS.md").exists());
    assert_eq!(
        view.untracked_in_checkout,
        vec!["docs/local-notes.md".to_string()]
    );
    assert!(view
        .files
        .iter()
        .all(|f| f.outcome.is_none() && !f.diff.is_empty()));
    assert!(view.table_markdown.contains("`AGENTS.md`"));
}

#[test]
fn a_local_merge_runs_in_a_worktree_with_one_diff_per_file_and_resumes() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let before = hashes(root);
    let view = run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();

    // The checkout is untouched: same bytes, nothing new.
    assert_eq!(hashes(root), before);
    assert!(!root.join("docs/AGENTS.md").exists());
    let wt = PathBuf::from(view.worktree_path.clone().unwrap());
    assert!(
        wt.starts_with(root.join(".kronn/worktrees")) || wt.ends_with("fixture--template-v2-merge")
    );
    assert_eq!(view.branch.as_deref(), Some(docs_merge::MERGE_BRANCH));
    assert!(!view.resumed);

    // Every new file has its diff; no existing file appears as modified.
    assert!(view.files.iter().any(|f| f.path == "docs/AGENTS.md"));
    assert!(view
        .files
        .iter()
        .any(|f| f.path == "docs/conventions/README.md"));
    for file in &view.files {
        assert!(file
            .diff
            .starts_with(&format!("--- /dev/null\n+++ b/{}\n", file.path)));
        assert_eq!(file.outcome, Some(WriteOutcome::Created));
    }
    assert!(!view.files.iter().any(|f| f.path == "docs/coding-rules.md"));
    assert!(modified_existing_docs(&wt).unwrap().is_empty());
    let testing = std::fs::read_to_string(wt.join("docs/testing-quality.md")).unwrap();
    assert!(testing.contains("make iso-test"));
    let ci = std::fs::read_to_string(wt.join("docs/workflow/ci-cd.md")).unwrap();
    assert!(ci.contains("Tag then push."));

    // Resume: same worktree, same table, nothing rewritten.
    let again = run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    assert!(again.resumed);
    assert_eq!(again.worktree_path, view.worktree_path);
    assert_eq!(again.correspondence, view.correspondence);
    assert!(again
        .files
        .iter()
        .filter(|f| view
            .files
            .iter()
            .any(|v| v.path == f.path && v.imported_sections > 0))
        .all(|f| f.outcome == Some(WriteOutcome::AlreadyPresent)));

    // Accept two files: only they are committed, on the merge branch.
    let accepted = accept_merge(
        root,
        &[
            "docs/AGENTS.md".to_string(),
            "docs/testing-quality.md".to_string(),
        ],
    )
    .unwrap();
    let committed = sh_git(&wt, &["show", "--name-only", "--format=", &accepted.commit]);
    let committed: BTreeSet<&str> = committed.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        committed,
        // The folder's state file travels with the files it records as Kronn's.
        [
            "docs/.kronn.json",
            "docs/AGENTS.md",
            "docs/testing-quality.md"
        ]
        .into_iter()
        .collect()
    );
    assert!(
        wt.join("docs/glossary.md").exists(),
        "unaccepted files are kept"
    );
    assert_eq!(
        sh_git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );
    assert_eq!(hashes(root), before);

    // An existing doc cannot be "accepted" into the commit.
    assert!(accept_merge(root, &["docs/coding-rules.md".to_string()]).is_err());

    discard_merge(root).unwrap();
    assert!(!wt.exists());
    assert!(existing_merge_worktree(root).is_none());
}

#[test]
fn an_edited_existing_doc_in_the_worktree_stops_the_resume() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let view = run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    let wt = PathBuf::from(view.worktree_path.unwrap());
    write(&wt, "docs/coding-rules.md", "rewritten\n");
    let err =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap_err();
    assert!(err.contains("docs/coding-rules.md"), "{err}");
    assert!(err.contains("resume"), "{err}");
}

#[test]
fn a_remote_merge_starts_from_the_fetched_default_branch() {
    let origin = TempDir::new().unwrap();
    sh_git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
    let tmp = documented_git_repo();
    let root = tmp.path();
    let origin_url = origin.path().to_string_lossy().to_string();
    sh_git(root, &["remote", "add", "origin", &origin_url]);
    sh_git(root, &["push", "-q", "origin", "main"]);

    // A teammate documents the glossary on the remote only.
    let other = TempDir::new().unwrap();
    sh_git(other.path(), &["clone", "-q", &origin_url, "."]);
    write(
        other.path(),
        "documentation/terms.md",
        "## Glossary\n\nEW = Euronews\n",
    );
    sh_git(other.path(), &["add", "-A"]);
    sh_git(other.path(), &["commit", "-q", "-m", "terms"]);
    sh_git(other.path(), &["push", "-q", "origin", "main"]);

    let view = run_merge_in_worktree(root, "Fixture", MergeBase::Remote, &template_docs()).unwrap();
    assert_eq!(view.base, MergeBase::Remote);
    assert!(view.base_ref.starts_with("origin/"), "{}", view.base_ref);
    let row = view
        .correspondence
        .iter()
        .find(|r| r.source == "documentation/terms.md")
        .expect("remote-only doc is mapped");
    assert_eq!(row.target.as_deref(), Some("docs/glossary.md"));
    let wt = PathBuf::from(view.worktree_path.unwrap());
    assert!(std::fs::read_to_string(wt.join("docs/glossary.md"))
        .unwrap()
        .contains("EW = Euronews"));
    // The local checkout never received the remote commit.
    assert!(!root.join("documentation/terms.md").exists());
}

#[test]
fn a_non_git_project_gets_a_plan_but_no_worktree() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "docs/setup.md", "## Installation\n\nnpm i\n");
    let view = preview_plan(tmp.path(), &template_docs()).unwrap();
    assert!(view.files.iter().any(|f| f.path == "docs/AGENTS.md"));
    let err =
        run_merge_in_worktree(tmp.path(), "x", MergeBase::Local, &template_docs()).unwrap_err();
    assert!(err.contains("not a git repository"));
}

/// Everything a read must leave alone: refs, worktrees, status and the index bytes.
fn git_snapshot(root: &Path) -> (String, String, String, Vec<u8>) {
    (
        sh_git(root, &["for-each-ref"]),
        sh_git(root, &["worktree", "list", "--porcelain"]),
        sh_git(root, &["status", "--porcelain"]),
        std::fs::read(root.join(".git/index")).unwrap(),
    )
}

#[test]
fn merge_state_without_a_merge_reports_nothing_in_progress() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let state = merge_state(root);
    assert_eq!(
        state,
        TemplateMergeState {
            branch: docs_merge::MERGE_BRANCH.to_string(),
            checkout_branch: Some("main".to_string()),
            ..TemplateMergeState::default()
        }
    );
    let view = preview_plan(root, &template_docs()).unwrap();
    assert_eq!(view.merge, state);

    let plain = TempDir::new().unwrap();
    write(plain.path(), "docs/setup.md", "## Installation\n\nnpm i\n");
    let state = merge_state(plain.path());
    assert!(!state.worktree_exists && !state.branch_exists);
    assert_eq!(state.checkout_branch, None);
}

#[test]
fn merge_state_sees_a_worktree_in_progress_without_writing() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    assert!(started.merge.worktree_exists);

    let before = git_snapshot(root);
    let view = preview_plan(root, &template_docs()).unwrap();
    assert_eq!(git_snapshot(root), before);

    let state = view.merge;
    assert!(state.worktree_exists);
    assert!(state.branch_exists);
    assert_eq!(state.worktree_path, started.worktree_path);
    assert_eq!(state.branch_ahead_commits, 0);
    assert_eq!(state.pending_files as usize, started.files.len());
    assert!(state.pending_files > 0);
}

#[test]
fn merge_state_counts_accepted_commits_not_in_the_checkout_branch() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    accept_merge(root, &["docs/AGENTS.md".to_string()]).unwrap();
    accept_merge(root, &["docs/glossary.md".to_string()]).unwrap();

    let state = merge_state(root);
    assert!(state.worktree_exists);
    assert_eq!(state.branch_ahead_commits, 2);
    assert_eq!(state.pending_files as usize, started.files.len() - 2);

    // The worktree gone, the accepted branch still waits to be integrated.
    let wt = started.worktree_path.unwrap();
    sh_git(root, &["worktree", "remove", "--force", &wt]);
    let before = git_snapshot(root);
    let state = merge_state(root);
    assert_eq!(git_snapshot(root), before);
    assert!(!state.worktree_exists);
    assert_eq!(state.worktree_path, None);
    assert_eq!(state.pending_files, 0);
    assert!(state.branch_exists);
    assert_eq!(state.branch_ahead_commits, 2);
}

#[test]
fn merge_state_reports_zero_ahead_once_the_branch_is_merged() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    accept_merge(root, &["docs/AGENTS.md".to_string()]).unwrap();
    sh_git(
        root,
        &[
            "worktree",
            "remove",
            "--force",
            &started.worktree_path.unwrap(),
        ],
    );
    sh_git(
        root,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge v2",
            docs_merge::MERGE_BRANCH,
        ],
    );

    let state = merge_state(root);
    assert!(state.branch_exists);
    assert!(!state.worktree_exists);
    assert_eq!(state.branch_ahead_commits, 0);
}

#[test]
fn merge_state_does_not_count_the_remote_commits_a_remote_merge_started_from() {
    let origin = TempDir::new().unwrap();
    sh_git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
    let tmp = documented_git_repo();
    let root = tmp.path();
    let origin_url = origin.path().to_string_lossy().to_string();
    sh_git(root, &["remote", "add", "origin", &origin_url]);
    sh_git(root, &["push", "-q", "origin", "main"]);
    let other = TempDir::new().unwrap();
    sh_git(other.path(), &["clone", "-q", &origin_url, "."]);
    for n in 0..3 {
        write(other.path(), &format!("src/f{n}.txt"), "x\n");
        sh_git(other.path(), &["add", "-A"]);
        sh_git(other.path(), &["commit", "-q", "-m", "upstream"]);
    }
    sh_git(other.path(), &["push", "-q", "origin", "main"]);

    run_merge_in_worktree(root, "Fixture", MergeBase::Remote, &template_docs()).unwrap();
    accept_merge(root, &["docs/AGENTS.md".to_string()]).unwrap();
    // The local main is three upstream commits behind; only the accepted one counts.
    assert_eq!(merge_state(root).branch_ahead_commits, 1);
}

#[test]
fn kronn_state_files_are_not_untracked_docs() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    write(root, "docs/briefing.md", "# Project Briefing\n");
    write(root, "docs/.kronn.json", "{}");
    write(root, "docs/.kronn-audit-run.json", "{}");
    assert!(untracked_docs(root).is_empty());
    let view = preview_plan(root, &template_docs()).unwrap();
    assert!(view.untracked_in_checkout.is_empty());
    // A doc of the team's, even next to them, is still reported.
    write(root, "docs/local-notes.md", "## Debugging\n");
    assert_eq!(
        untracked_docs(root),
        vec!["docs/local-notes.md".to_string()]
    );
}

#[test]
fn an_accept_records_the_files_as_kronns_and_the_folder_as_installed() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    let wt = PathBuf::from(started.worktree_path.unwrap());
    accept_merge(
        root,
        &["docs/AGENTS.md".to_string(), "docs/glossary.md".to_string()],
    )
    .unwrap();
    let state = crate::core::kronn_state::read(&wt).expect("a state file in the commit");
    assert!(state.template_installed_at.is_some());
    assert_eq!(
        state
            .kronn_written
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["AGENTS.md", "glossary.md"]
    );
    assert!(sh_git(&wt, &["status", "--porcelain", "docs/.kronn.json"]).is_empty());

    // A second accept adds to the record; the first entries stay.
    accept_merge(root, &["docs/decisions.md".to_string()]).unwrap();
    let state = crate::core::kronn_state::read(&wt).unwrap();
    assert_eq!(state.kronn_written.len(), 3);
    assert!(state.kronn_written.contains_key("AGENTS.md"));
}

#[test]
fn a_corrupt_state_file_stops_the_accept_without_committing() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    let wt = PathBuf::from(started.worktree_path.unwrap());
    write(&wt, "docs/.kronn.json", "{ not json");
    let err = accept_merge(root, &["docs/AGENTS.md".to_string()]).unwrap_err();
    assert!(err.contains("not valid JSON"), "{err}");
    assert_eq!(merge_state(root).branch_ahead_commits, 0);
}

#[test]
fn the_pending_integration_refusal_carries_an_error_code() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    assert!(audit_launch_refusal(root).is_none());
    run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    accept_merge(root, &["docs/AGENTS.md".to_string()]).unwrap();
    let refusal = audit_launch_refusal(root).expect("pending integration");
    assert_eq!(refusal["error_code"], PENDING_INTEGRATION_CODE);
    assert!(refusal["error"]
        .as_str()
        .unwrap()
        .contains("pending integration"));
}

#[test]
fn an_integrated_merge_leaves_neither_worktree_nor_branch() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    let wt = PathBuf::from(started.worktree_path.unwrap());

    // Files still to review: nothing is cleaned.
    clean_integrated_merge(root);
    assert!(wt.exists() && branch_exists(root));

    let all: Vec<String> = started.files.iter().map(|f| f.path.clone()).collect();
    accept_merge(root, &all).unwrap();
    // Accepted, not integrated: nothing is cleaned.
    clean_integrated_merge(root);
    assert!(wt.exists() && branch_exists(root));

    sh_git(
        root,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge v2",
            docs_merge::MERGE_BRANCH,
        ],
    );
    let state = merge_state(root);
    assert!(state.worktree_exists);
    assert_eq!((state.pending_files, state.branch_ahead_commits), (0, 0));
    clean_integrated_merge(root);
    assert!(!wt.exists());
    assert!(!branch_exists(root));
    let state = merge_state(root);
    assert!(!state.worktree_exists && !state.branch_exists);
}

#[test]
fn a_resumed_plan_never_reads_the_merges_own_files_as_sources() {
    let tmp = documented_git_repo();
    let root = tmp.path();
    let started =
        run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    let human: BTreeSet<String> = started
        .correspondence
        .iter()
        .map(|r| r.source.clone())
        .collect();
    let all: Vec<String> = started.files.iter().map(|f| f.path.clone()).collect();
    accept_merge(root, &all).unwrap();
    let again = run_merge_in_worktree(root, "Fixture", MergeBase::Local, &template_docs()).unwrap();
    let sources: BTreeSet<String> = again
        .correspondence
        .iter()
        .map(|r| r.source.clone())
        .collect();
    assert_eq!(sources, human);
}

#[test]
fn a_remote_plan_warns_about_local_docs_the_remote_base_lacks() {
    let origin = TempDir::new().unwrap();
    sh_git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
    let tmp = documented_git_repo();
    let root = tmp.path();
    let origin_url = origin.path().to_string_lossy().to_string();
    sh_git(root, &["remote", "add", "origin", &origin_url]);
    sh_git(root, &["push", "-q", "origin", "main"]);

    // Nothing local-only yet: no warning.
    let clean =
        run_merge_in_worktree(root, "Fixture", MergeBase::Remote, &template_docs()).unwrap();
    assert!(clean.warnings.is_empty(), "{:?}", clean.warnings);
    discard_merge(root).unwrap();

    // A doc committed but not pushed, and one never committed.
    write(root, "docs/unpushed.md", "## Debugging\n\nlogs\n");
    sh_git(root, &["add", "-A"]);
    sh_git(root, &["commit", "-q", "-m", "unpushed"]);
    write(root, "docs/untracked.md", "## Glossary\n\nx\n");
    let view = run_merge_in_worktree(root, "Fixture", MergeBase::Remote, &template_docs()).unwrap();
    assert_eq!(view.warnings.len(), 1, "{:?}", view.warnings);
    let warning = &view.warnings[0];
    assert!(warning.contains("docs/unpushed.md"), "{warning}");
    assert!(warning.contains("docs/untracked.md"), "{warning}");
    assert!(warning.contains("add/add"), "{warning}");
    // The local base has everything: nothing to warn about.
    assert!(preview_plan(root, &template_docs())
        .unwrap()
        .warnings
        .is_empty());
}
