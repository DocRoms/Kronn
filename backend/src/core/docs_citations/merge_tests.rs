//! End to end on a scratch repository (KT-1321): two branches shift the same
//! cited file and the integration merge settles the doc citations itself.

use super::*;
use crate::core::worktree::{build_candidate, CandidateOutcome};
use std::fs;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::core::cmd::git_cmd()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn scratch_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &["config", "user.email", "dev@example.com"],
        &["config", "user.name", "Dev"],
        &["config", "commit.gpgsign", "false"],
    ] {
        git(dir.path(), args);
    }
    dir
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

/// `lines` numbered 1..=60 as `let vN = N;`, with `special` lines replaced.
fn source(special: &[(usize, &str)]) -> Vec<String> {
    (1..=60)
        .map(|n| match special.iter().find(|(at, _)| *at == n) {
            Some((_, text)) => text.to_string(),
            None => format!("let v{n} = {n};"),
        })
        .collect()
}

fn write_lines(dir: &Path, rel: &str, lines: &[String]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn insert_after(lines: &mut Vec<String>, after: usize, count: usize, tag: &str) {
    for k in 0..count {
        lines.insert(after + k, format!("let {tag}{k} = 0;"));
    }
}

/// The citations of every doc of the repository that do not resolve.
fn broken(dir: &Path) -> Vec<String> {
    check_corpus(dir)
        .into_iter()
        .map(|(doc, f)| {
            format!(
                "{}:{} [src: {}] {}",
                doc.display(),
                f.line,
                f.citation,
                f.reason
            )
        })
        .collect()
}

const API_DOC: &str = "# API\n\nAlpha: [src: file: src/lib.rs:{A}-{A2}#alpha]\nBeta: [src: file: src/lib.rs:{B}-{B2}#beta]\n";

fn api_doc(alpha: usize, beta: usize) -> String {
    API_DOC
        .replace("{A2}", &(alpha + 1).to_string())
        .replace("{A}", &alpha.to_string())
        .replace("{B2}", &(beta + 1).to_string())
        .replace("{B}", &beta.to_string())
}

const ALPHA: &str = "pub fn alpha() {}";
const BETA: &str = "pub fn beta() {}";

/// `main` is the target, `child` the task branch, both shifting `src/lib.rs`
/// above the cited functions by a different amount and re-anchoring the doc.
fn diverged(target_shift: usize, child_shift: usize) -> (tempfile::TempDir, String) {
    let repo = scratch_repo();
    let dir = repo.path();
    let base = source(&[(10, ALPHA), (30, BETA)]);
    write_lines(dir, "src/lib.rs", &base);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(dir.join("docs/api.md"), api_doc(10, 30)).unwrap();
    commit_all(dir, "base");
    assert_eq!(broken(dir), Vec::<String>::new());

    git(dir, &["checkout", "-q", "-b", "child"]);
    let mut lines = base.clone();
    insert_after(&mut lines, 5, child_shift, "child");
    write_lines(dir, "src/lib.rs", &lines);
    fs::write(
        dir.join("docs/api.md"),
        api_doc(10 + child_shift, 30 + child_shift),
    )
    .unwrap();
    commit_all(dir, "child shifts the file");
    assert_eq!(broken(dir), Vec::<String>::new());

    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    insert_after(&mut lines, 1, target_shift, "target");
    write_lines(dir, "src/lib.rs", &lines);
    fs::write(
        dir.join("docs/api.md"),
        api_doc(10 + target_shift, 30 + target_shift),
    )
    .unwrap();
    let target = commit_all(dir, "target shifts the file");
    assert_eq!(broken(dir), Vec::<String>::new());

    git(dir, &["checkout", "-q", "child"]);
    (repo, target)
}

fn parents(dir: &Path) -> usize {
    git(dir, &["log", "-1", "--format=%P"])
        .split_whitespace()
        .count()
}

#[test]
fn two_branches_shifting_the_same_file_integrate_without_a_bounce() {
    let (repo, target) = diverged(3, 5);
    let dir = repo.path();

    // Without KT-1321 this is a conflict on the two doc lines.
    let probe = crate::core::cmd::git_cmd()
        .args(["merge", "--no-commit", &target])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        !probe.status.success(),
        "the setup must conflict on the doc"
    );
    git(dir, &["merge", "--abort"]);

    let outcome = build_candidate(dir, &target).unwrap();
    let CandidateOutcome::Built { sha } = outcome else {
        panic!("anchor-only conflicts must not bounce: {outcome:?}");
    };

    assert_eq!(git(dir, &["rev-parse", "HEAD"]), sha);
    assert_eq!(parents(dir), 2, "one real merge commit");
    assert!(!dir.join(".git/MERGE_HEAD").exists());
    let body = git(dir, &["log", "-1", "--format=%B"]);
    assert!(
        body.contains("Signed-off-by: Dev <dev@example.com>"),
        "{body}"
    );
    assert!(
        body.contains("Doc citations: 1 anchor-only conflict(s)"),
        "{body}"
    );
    assert!(!body.contains("# Conflicts"), "{body}");

    // 3 + 5 lines were added above both functions.
    let doc = fs::read_to_string(dir.join("docs/api.md")).unwrap();
    assert_eq!(doc, api_doc(18, 38));
    assert_eq!(broken(dir), Vec::<String>::new(), "every citation resolves");
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[test]
fn a_real_text_conflict_in_the_doc_still_bounces() {
    let (repo, _) = diverged(3, 5);
    let dir = repo.path();
    // The child also rewrote the sentence the target rewrote.
    let child = fs::read_to_string(dir.join("docs/api.md"))
        .unwrap()
        .replace("Alpha:", "Alpha (the first one):");
    fs::write(dir.join("docs/api.md"), child).unwrap();
    commit_all(dir, "child rewords");
    git(dir, &["checkout", "-q", "main"]);
    let target_text = fs::read_to_string(dir.join("docs/api.md"))
        .unwrap()
        .replace("Alpha:", "Alpha, entry point:");
    fs::write(dir.join("docs/api.md"), target_text).unwrap();
    let target = commit_all(dir, "target rewords");
    git(dir, &["checkout", "-q", "child"]);
    let before = git(dir, &["rev-parse", "HEAD"]);

    let outcome = build_candidate(dir, &target).unwrap();
    let CandidateOutcome::Conflict { files } = outcome else {
        panic!("a conflict on the prose must bounce: {outcome:?}");
    };
    assert!(files.iter().any(|f| f == "docs/api.md"), "{files:?}");
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), before);
    assert!(!dir.join(".git/MERGE_HEAD").exists());
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[test]
fn anchor_only_doc_conflicts_do_not_hide_a_real_conflict_elsewhere() {
    let (repo, _) = diverged(3, 5);
    let dir = repo.path();
    fs::write(dir.join("src/notes.rs"), "child\n").unwrap();
    commit_all(dir, "child notes");
    git(dir, &["checkout", "-q", "main"]);
    fs::write(dir.join("src/notes.rs"), "target\n").unwrap();
    let target = commit_all(dir, "target notes");
    git(dir, &["checkout", "-q", "child"]);
    let before = git(dir, &["rev-parse", "HEAD"]);

    let CandidateOutcome::Conflict { files } = build_candidate(dir, &target).unwrap() else {
        panic!("a source conflict must bounce");
    };
    assert_eq!(files, vec!["src/notes.rs".to_string()]);
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), before);
    assert!(!dir.join(".git/MERGE_HEAD").exists());
}

/// A merge with no text conflict at all that still breaks a citation: the child
/// adds a doc against its own version of the file, the target moves the file.
#[test]
fn a_clean_merge_reanchors_the_citations_it_shifted_before_the_check() {
    let repo = scratch_repo();
    let dir = repo.path();
    let budget = "use_budget(retry_budget);";
    let base = source(&[(10, budget), (25, budget), (40, budget)]);
    write_lines(dir, "src/lib.rs", &base);
    commit_all(dir, "base");

    git(dir, &["checkout", "-q", "-b", "child"]);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("docs/new.md"),
        "Retries: [src: file: src/lib.rs:40-41#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "child documents the budget");
    assert_eq!(broken(dir), Vec::<String>::new());

    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    insert_after(&mut lines, 1, 3, "target");
    write_lines(dir, "src/lib.rs", &lines);
    let target = commit_all(dir, "target shifts the file");
    git(dir, &["checkout", "-q", "child"]);

    // The marker is no longer valid once merged: three references, no unique one.
    let CandidateOutcome::Built { .. } = build_candidate(dir, &target).unwrap() else {
        panic!("a clean merge must be built");
    };
    assert_eq!(parents(dir), 2);
    assert_eq!(
        fs::read_to_string(dir.join("docs/new.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:43-44#retry_budget]\n"
    );
    assert_eq!(broken(dir), Vec::<String>::new());
    let body = git(dir, &["log", "-1", "--format=%B"]);
    assert!(body.contains("1 citation(s) re-anchored"), "{body}");
    assert!(body.contains("Signed-off-by: "), "{body}");
}

#[test]
fn an_ambiguous_symbol_picks_the_occurrence_nearest_the_mapped_line() {
    let repo = scratch_repo();
    let dir = repo.path();
    let budget = "use_budget(retry_budget);";
    let base = source(&[(10, budget), (25, budget), (40, budget)]);
    write_lines(dir, "src/lib.rs", &base);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("docs/budget.md"),
        "Retries: [src: file: src/lib.rs:40-42#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "base");

    // The child moves the file by 3 and re-anchors.
    git(dir, &["checkout", "-q", "-b", "child"]);
    let mut lines = base.clone();
    insert_after(&mut lines, 1, 3, "child");
    write_lines(dir, "src/lib.rs", &lines);
    fs::write(
        dir.join("docs/budget.md"),
        "Retries: [src: file: src/lib.rs:43-45#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "child shifts the file");
    assert_eq!(broken(dir), Vec::<String>::new());

    // The target adds 12 lines mid-file and edits the last cited line: no line
    // map covers the range, three references remain, and the nearest to the OLD
    // position (line 40) is the wrong one.
    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    lines[41] = "let v42 = 4242;".to_string();
    insert_after(&mut lines, 20, 12, "target");
    write_lines(dir, "src/lib.rs", &lines);
    let target = commit_all(dir, "target grows the file");
    git(dir, &["checkout", "-q", "child"]);

    let CandidateOutcome::Built { .. } = build_candidate(dir, &target).unwrap() else {
        panic!("a clean merge must be built");
    };
    // References are now at 13, 40 and 55; the cited start 43 maps to 55.
    assert_eq!(
        fs::read_to_string(dir.join("docs/budget.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:55-57#retry_budget]\n"
    );
    assert_eq!(broken(dir), Vec::<String>::new());
}

#[test]
fn a_citation_that_no_side_wrote_is_left_alone() {
    let repo = scratch_repo();
    let dir = repo.path();
    let budget = "use_budget(retry_budget);";
    write_lines(
        dir,
        "src/lib.rs",
        &source(&[(10, budget), (25, budget), (40, budget)]),
    );
    fs::create_dir_all(dir.join("docs")).unwrap();
    // Already broken before any merge: it fits no version of the file.
    fs::write(
        dir.join("docs/stale.md"),
        "Retries: [src: file: src/lib.rs:50-51#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "base");
    git(dir, &["checkout", "-q", "-b", "child"]);
    fs::write(dir.join("child.txt"), "x\n").unwrap();
    commit_all(dir, "child");
    git(dir, &["checkout", "-q", "main"]);
    fs::write(dir.join("target.txt"), "y\n").unwrap();
    let target = commit_all(dir, "target");
    git(dir, &["checkout", "-q", "child"]);

    let CandidateOutcome::Built { .. } = build_candidate(dir, &target).unwrap() else {
        panic!("a clean merge must be built");
    };
    assert_eq!(
        fs::read_to_string(dir.join("docs/stale.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:50-51#retry_budget]\n",
        "a citation the merge did not shift is not the merge's to rewrite"
    );
    let body = git(dir, &["log", "-1", "--format=%B"]);
    assert!(!body.contains("Doc citations"), "{body}");
}

/// The worker's route: it merges the target by hand and Kronn finishes it.
#[test]
fn a_workers_merge_commit_settles_the_doc_without_make_docs_reanchor() {
    let (repo, target) = diverged(3, 5);
    let dir = repo.path();
    let merge = crate::core::cmd::git_cmd()
        .args(["merge", "--no-commit", "--no-ff", &target])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the doc conflicts");
    assert!(merge::merge_in_progress(dir));

    // The worker names only a file it touched; it resolved nothing by hand.
    let committed = crate::api::git_ops::run_git_merge_commit_with_child_lock(
        dir,
        &["docs/api.md".to_string()],
        "merge the target",
        false,
        None,
    )
    .unwrap();
    assert!(committed.message.contains("Doc citations: 1 anchor-only"));
    assert_eq!(parents(dir), 2);
    assert_eq!(
        fs::read_to_string(dir.join("docs/api.md")).unwrap(),
        api_doc(18, 38)
    );
    assert_eq!(broken(dir), Vec::<String>::new());
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

/// `make docs-reanchor` during the worker's unfinished merge needs no base.
#[test]
fn the_gate_reanchors_an_unfinished_merge_from_both_of_its_sides() {
    let repo = scratch_repo();
    let dir = repo.path();
    let budget = "use_budget(retry_budget);";
    let base = source(&[(10, budget), (25, budget), (40, budget)]);
    write_lines(dir, "src/lib.rs", &base);
    commit_all(dir, "base");
    git(dir, &["checkout", "-q", "-b", "child"]);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("docs/new.md"),
        "Retries: [src: file: src/lib.rs:40-41#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "child documents the budget");
    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    insert_after(&mut lines, 1, 3, "target");
    write_lines(dir, "src/lib.rs", &lines);
    let target = commit_all(dir, "target shifts the file");
    git(dir, &["checkout", "-q", "child"]);
    git(dir, &["merge", "-q", "--no-commit", "--no-ff", &target]);

    let env = |name: &str| match name {
        REANCHOR_ENV => Some("all".to_string()),
        REANCHOR_BASE_ENV => Some(REANCHOR_BASE_MERGE.to_string()),
        _ => None,
    };
    let run = gate(dir, &env).unwrap();
    assert!(
        run.verdict.blocking.is_empty(),
        "{}",
        run.verdict.failure_message()
    );
    assert!(run.notes.contains("1 rewritten"), "{}", run.notes);
}
