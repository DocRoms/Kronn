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
    source_of(60, special)
}

fn source_of(count: usize, special: &[(usize, &str)]) -> Vec<String> {
    (1..=count)
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
    diverged_doc("docs/api.md", target_shift, child_shift)
}

/// [`diverged`] with the doc at `doc`.
fn diverged_doc(doc: &str, target_shift: usize, child_shift: usize) -> (tempfile::TempDir, String) {
    let repo = scratch_repo();
    let dir = repo.path();
    let base = source(&[(10, ALPHA), (30, BETA)]);
    write_lines(dir, "src/lib.rs", &base);
    fs::create_dir_all(dir.join(doc).parent().unwrap()).unwrap();
    fs::write(dir.join(doc), api_doc(10, 30)).unwrap();
    commit_all(dir, "base");
    assert_eq!(broken(dir), Vec::<String>::new());

    git(dir, &["checkout", "-q", "-b", "child"]);
    let mut lines = base.clone();
    insert_after(&mut lines, 5, child_shift, "child");
    write_lines(dir, "src/lib.rs", &lines);
    fs::write(dir.join(doc), api_doc(10 + child_shift, 30 + child_shift)).unwrap();
    commit_all(dir, "child shifts the file");
    assert_eq!(broken(dir), Vec::<String>::new());

    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    insert_after(&mut lines, 1, target_shift, "target");
    write_lines(dir, "src/lib.rs", &lines);
    fs::write(dir.join(doc), api_doc(10 + target_shift, 30 + target_shift)).unwrap();
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

/// The child documents a function against its own version of the file; the
/// target then shifts the file, so the merge breaks that citation without any
/// text conflict. `docs/notes.md` has no citation at all. Ends on `child`.
fn shifted_citation_repo() -> (tempfile::TempDir, String) {
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
    fs::write(dir.join("docs/notes.md"), "Notes\n").unwrap();
    commit_all(dir, "child documents the budget");
    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    insert_after(&mut lines, 1, 3, "target");
    write_lines(dir, "src/lib.rs", &lines);
    let target = commit_all(dir, "target shifts the file");
    git(dir, &["checkout", "-q", "child"]);
    (repo, target)
}

const SHIFTED_NEW: &str = "Retries: [src: file: src/lib.rs:43-44#retry_budget]\n";

/// The worker's merge, finished by Kronn with only `files` named.
fn start_worker_merge(dir: &Path, target: &str) {
    let merge = crate::core::cmd::git_cmd()
        .args(["merge", "--no-commit", "--no-ff", target])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(merge.status.success(), "the setup merges cleanly");
    assert!(merge::merge_in_progress(dir));
}

fn finish_worker_merge(dir: &Path, files: &[&str]) -> Result<String, String> {
    crate::api::git_ops::run_git_merge_commit_with_child_lock(
        dir,
        &files.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
        "merge the target",
        false,
        None,
    )
    .map(|committed| committed.hash)
}

fn staged(dir: &Path) -> Vec<String> {
    git(dir, &["diff", "--cached", "--name-only"])
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn unnamed_work_in_progress_in_another_doc_stays_out_of_the_merge_commit() {
    let (repo, target) = shifted_citation_repo();
    let dir = repo.path();
    start_worker_merge(dir, &target);
    fs::write(dir.join("docs/notes.md"), "Notes\nWIP-UNNAMED-DRAFT\n").unwrap();

    finish_worker_merge(dir, &["src/lib.rs"]).unwrap();

    assert_eq!(parents(dir), 2);
    assert_eq!(
        fs::read_to_string(dir.join("docs/new.md")).unwrap(),
        SHIFTED_NEW,
        "the citation the merge shifted is still re-anchored"
    );
    assert_eq!(git(dir, &["show", "HEAD:docs/notes.md"]), "Notes");
    assert_eq!(
        fs::read_to_string(dir.join("docs/notes.md")).unwrap(),
        "Notes\nWIP-UNNAMED-DRAFT\n",
        "the work in progress is still there, uncommitted"
    );
    assert_eq!(git(dir, &["status", "--porcelain"]), "M docs/notes.md");
}

#[test]
fn a_doc_to_re_anchor_with_unstaged_work_refuses_the_worker_merge() {
    let (repo, target) = shifted_citation_repo();
    let dir = repo.path();
    start_worker_merge(dir, &target);
    let draft = format!(
        "{}WIP-UNNAMED-DRAFT\n",
        "Retries: [src: file: src/lib.rs:40-41#retry_budget]\n"
    );
    fs::write(dir.join("docs/new.md"), &draft).unwrap();
    let head = git(dir, &["rev-parse", "HEAD"]);
    let index = git(dir, &["write-tree"]);

    let error = finish_worker_merge(dir, &["src/lib.rs"]).unwrap_err();

    assert!(error.contains("docs/new.md"), "{error}");
    assert!(error.contains("unstaged"), "{error}");
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert!(merge::merge_in_progress(dir), "the merge is left as it was");
    assert_eq!(git(dir, &["write-tree"]), index, "nothing was staged");
    assert_eq!(
        fs::read_to_string(dir.join("docs/new.md")).unwrap(),
        draft,
        "the work in progress is untouched"
    );
}

#[test]
fn a_partly_staged_doc_is_never_swept_whole_into_the_merge_commit() {
    let (repo, target) = shifted_citation_repo();
    let dir = repo.path();
    start_worker_merge(dir, &target);
    let doc = dir.join("docs/new.md");
    fs::write(
        &doc,
        "Intro\nRetries: [src: file: src/lib.rs:40-41#retry_budget]\n",
    )
    .unwrap();
    git(dir, &["add", "docs/new.md"]);
    fs::write(
        &doc,
        "Intro\nRetries: [src: file: src/lib.rs:40-41#retry_budget]\nWIP-UNNAMED-DRAFT\n",
    )
    .unwrap();
    let head = git(dir, &["rev-parse", "HEAD"]);

    let error = finish_worker_merge(dir, &["src/lib.rs"]).unwrap_err();

    assert!(error.contains("docs/new.md"), "{error}");
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert!(
        fs::read_to_string(&doc)
            .unwrap()
            .ends_with("WIP-UNNAMED-DRAFT\n"),
        "the draft is untouched"
    );
    assert_eq!(
        git(dir, &["show", ":docs/new.md"]),
        "Intro\nRetries: [src: file: src/lib.rs:40-41#retry_budget]",
        "the index still holds what was staged, no more"
    );
}

#[test]
fn a_doc_staged_by_hand_and_not_named_is_still_refused_not_re_anchored() {
    let (repo, target) = shifted_citation_repo();
    let dir = repo.path();
    start_worker_merge(dir, &target);
    fs::write(
        dir.join("docs/new.md"),
        "Retries: [src: file: src/lib.rs:40-41#retry_budget]\nstaged by hand\n",
    )
    .unwrap();
    git(dir, &["add", "docs/new.md"]);

    let error = finish_worker_merge(dir, &["src/lib.rs"]).unwrap_err();

    assert!(
        error.contains("belong neither to the merge nor to `files`"),
        "{error}"
    );
    assert!(error.contains("docs/new.md"), "{error}");
    assert!(merge::merge_in_progress(dir));
    assert!(staged(dir).contains(&"docs/new.md".to_string()));
}

#[test]
fn the_integration_bounces_instead_of_sweeping_a_dirty_doc_it_would_rewrite() {
    let (repo, target) = shifted_citation_repo();
    let dir = repo.path();
    let draft = "Retries: [src: file: src/lib.rs:40-41#retry_budget]\nWIP-UNNAMED-DRAFT\n";
    fs::write(dir.join("docs/new.md"), draft).unwrap();
    let head = git(dir, &["rev-parse", "HEAD"]);

    let CandidateOutcome::Conflict { files } = build_candidate(dir, &target).unwrap() else {
        panic!("a dirty doc to rewrite must bounce");
    };

    assert_eq!(files, vec!["docs/new.md".to_string()]);
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert!(!merge::merge_in_progress(dir));
    assert_eq!(fs::read_to_string(dir.join("docs/new.md")).unwrap(), draft);
}

#[test]
fn a_citation_into_a_cited_file_with_unstaged_edits_is_not_anchored_to_them() {
    let (repo, target) = shifted_citation_repo();
    let dir = repo.path();
    start_worker_merge(dir, &target);
    // The merge brought the shift; this edit on top of it is nobody's to commit.
    let lib = dir.join("src/lib.rs");
    let text = fs::read_to_string(&lib).unwrap();
    git(dir, &["add", "src/lib.rs"]);
    fs::write(&lib, format!("// WIP-UNNAMED-DRAFT\n{text}")).unwrap();

    // Naming `src/lib.rs` would stage the draft by the worker's own hand.
    let error = finish_worker_merge(dir, &["docs/notes.md"]).unwrap_err();

    assert!(error.contains("src/lib.rs"), "{error}");
    assert!(error.contains("unstaged"), "{error}");
    assert_eq!(
        fs::read_to_string(dir.join("docs/new.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:40-41#retry_budget]\n"
    );
}

/// 220 lines with the symbol at 10, 150 and 200; the child shifts the file by 3
/// and cites the first one; the target rewrites that first line.
fn far_occurrences_repo(symbols: &[usize]) -> (tempfile::TempDir, String) {
    let repo = scratch_repo();
    let dir = repo.path();
    let budget = "use_budget(retry_budget);";
    let special: Vec<(usize, &str)> = symbols.iter().map(|&at| (at, budget)).collect();
    let base = source_of(220, &special);
    write_lines(dir, "src/lib.rs", &base);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("docs/budget.md"),
        "Retries: [src: file: src/lib.rs:10-11#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "base");
    git(dir, &["checkout", "-q", "-b", "child"]);
    let mut lines = base.clone();
    insert_after(&mut lines, 1, 3, "child");
    write_lines(dir, "src/lib.rs", &lines);
    fs::write(
        dir.join("docs/budget.md"),
        "Retries: [src: file: src/lib.rs:13-14#retry_budget]\n",
    )
    .unwrap();
    commit_all(dir, "child shifts the file");
    assert_eq!(broken(dir), Vec::<String>::new());
    git(dir, &["checkout", "-q", "main"]);
    let mut lines = base;
    lines[9] = "let v10 = 0;".to_string();
    write_lines(dir, "src/lib.rs", &lines);
    let target = commit_all(dir, "target deletes the first reference");
    git(dir, &["checkout", "-q", "child"]);
    (repo, target)
}

#[test]
fn a_symbol_far_from_the_mapped_position_is_not_guessed_the_integration_bounces() {
    let (repo, target) = far_occurrences_repo(&[10, 150, 200]);
    let dir = repo.path();
    let head = git(dir, &["rev-parse", "HEAD"]);

    let CandidateOutcome::Conflict { files } = build_candidate(dir, &target).unwrap() else {
        panic!("the nearest occurrence is 140 lines away: a human picks");
    };

    assert_eq!(files, vec!["docs/budget.md".to_string()]);
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert!(!merge::merge_in_progress(dir));
    assert_eq!(
        fs::read_to_string(dir.join("docs/budget.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:13-14#retry_budget]\n"
    );
}

#[test]
fn the_worker_merge_is_refused_for_a_symbol_too_far_to_be_guessed() {
    let (repo, target) = far_occurrences_repo(&[10, 150, 200]);
    let dir = repo.path();
    start_worker_merge(dir, &target);

    let error = finish_worker_merge(dir, &["src/lib.rs"]).unwrap_err();

    assert!(error.contains("cannot be re-anchored safely"), "{error}");
    assert!(error.contains("docs/budget.md:1"), "{error}");
    assert!(merge::merge_in_progress(dir));
    assert_eq!(
        fs::read_to_string(dir.join("docs/budget.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:13-14#retry_budget]\n"
    );
}

#[test]
fn two_occurrences_equally_near_the_mapped_position_are_not_guessed() {
    // Merged, the references sit at 8 and 18, five lines either side of 13.
    let (repo, target) = far_occurrences_repo(&[5, 10, 15]);
    let dir = repo.path();

    let CandidateOutcome::Conflict { files } = build_candidate(dir, &target).unwrap() else {
        panic!("a tie is ambiguous");
    };
    assert_eq!(files, vec!["docs/budget.md".to_string()]);
}

#[test]
fn a_nearby_occurrence_within_the_limit_is_still_taken() {
    // Merged, the references sit at 8 and 90: only the first is within reach.
    let (repo, target) = far_occurrences_repo(&[5, 10, 87]);
    let dir = repo.path();

    let CandidateOutcome::Built { .. } = build_candidate(dir, &target).unwrap() else {
        panic!("the occurrence 5 lines away is unambiguous");
    };
    assert_eq!(
        fs::read_to_string(dir.join("docs/budget.md")).unwrap(),
        "Retries: [src: file: src/lib.rs:8-9#retry_budget]\n"
    );
    assert_eq!(broken(dir), Vec::<String>::new());
}

#[test]
fn only_the_citation_corpus_is_resolved_by_the_merge() {
    for doc in [
        "README.md",
        "CHANGELOG.md",
        "templates/docs/api.md",
        "backend/tests/fixtures/api.md",
        "prompts/api.md",
    ] {
        let (repo, target) = diverged_doc(doc, 3, 5);
        let dir = repo.path();
        let before = git(dir, &["rev-parse", "HEAD"]);

        let CandidateOutcome::Conflict { files } = build_candidate(dir, &target).unwrap() else {
            panic!("{doc} is not the corpus: an anchor-only conflict there still bounces");
        };
        assert!(files.iter().any(|f| f == doc), "{doc}: {files:?}");
        assert_eq!(git(dir, &["rev-parse", "HEAD"]), before, "{doc}");
    }
}

#[test]
fn the_second_doc_root_is_part_of_the_corpus() {
    let (repo, target) = diverged_doc("backend/docs/api.md", 3, 5);
    let dir = repo.path();

    let CandidateOutcome::Built { .. } = build_candidate(dir, &target).unwrap() else {
        panic!("backend/docs is a doc root");
    };
    assert_eq!(
        fs::read_to_string(dir.join("backend/docs/api.md")).unwrap(),
        api_doc(18, 38)
    );
}

#[test]
fn a_citation_to_a_file_the_merge_did_not_change_is_left_alone() {
    let repo = scratch_repo();
    let dir = repo.path();
    write_lines(dir, "src/lib.rs", &source(&[]));
    // `thing` moved long ago in a file nobody touches now: the unique symbol
    // would let a re-anchor fix it, but that is not this merge's business.
    write_lines(dir, "src/other.rs", &source(&[(5, "pub fn thing() {}")]));
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(
        dir.join("docs/old.md"),
        "Thing: [src: file: src/other.rs:2-3#thing]\n",
    )
    .unwrap();
    commit_all(dir, "base");
    git(dir, &["checkout", "-q", "-b", "child"]);
    let mut lines = source(&[]);
    insert_after(&mut lines, 1, 2, "child");
    write_lines(dir, "src/lib.rs", &lines);
    commit_all(dir, "child edits lib");
    git(dir, &["checkout", "-q", "main"]);
    let mut lines = source(&[]);
    insert_after(&mut lines, 30, 2, "target");
    write_lines(dir, "src/lib.rs", &lines);
    let target = commit_all(dir, "target edits lib");
    git(dir, &["checkout", "-q", "child"]);

    let CandidateOutcome::Built { .. } = build_candidate(dir, &target).unwrap() else {
        panic!("a clean merge must be built");
    };
    assert_eq!(
        fs::read_to_string(dir.join("docs/old.md")).unwrap(),
        "Thing: [src: file: src/other.rs:2-3#thing]\n"
    );
    let body = git(dir, &["log", "-1", "--format=%B"]);
    assert!(!body.contains("Doc citations"), "{body}");
}
