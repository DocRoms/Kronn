//! KT-1745 — Kronn's own commits and merges for a worker never run a hook that
//! lives in the worker-writable worktree.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::api::git_ops::{run_git_commit_for_worker, run_git_commit_with_child_lock};
use crate::core::worktree::{build_candidate_with_hooks, CandidateOutcome};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[cfg(unix)]
fn write_hook(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn write_hook(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
}

/// A main checkout, a linked worktree of it (the worker's), and a directory
/// outside both where a hook would leave a marker.
struct Lane {
    main: tempfile::TempDir,
    _parent: tempfile::TempDir,
    worktree: PathBuf,
    marker_dir: tempfile::TempDir,
}

impl Lane {
    fn new() -> Self {
        let main = tempfile::Builder::new()
            .prefix("kronn-hooks-main")
            .tempdir()
            .unwrap();
        git(main.path(), &["init", "-b", "main"]);
        git(main.path(), &["config", "user.email", "test@test.com"]);
        git(main.path(), &["config", "user.name", "Test"]);
        git(main.path(), &["config", "core.autocrlf", "false"]);
        fs::write(main.path().join("README.md"), "# test").unwrap();
        git(main.path(), &["add", "."]);
        git(main.path(), &["commit", "-m", "init"]);
        // Where Kronn puts a worker's worktree: `<repo>/.kronn/worktrees/<lane>`.
        let main_root = main.path().canonicalize().unwrap();
        let worktree = main_root.join(".kronn/worktrees/task-kt-1745-hooks");
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            main.path(),
            &["worktree", "add", "-b", "work", worktree.to_str().unwrap()],
        );
        Self {
            main,
            _parent: tempfile::Builder::new()
                .prefix("kronn-hooks-unused")
                .tempdir()
                .unwrap(),
            worktree,
            marker_dir: tempfile::Builder::new()
                .prefix("kronn-hooks-marker")
                .tempdir()
                .unwrap(),
        }
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.marker_dir.path().join(name)
    }

    /// husky's layout: a relative `core.hooksPath` whose hooks are tracked in the
    /// worktree, which the worker writes.
    fn husky(&self, hook: &str, body: &str) {
        git(&self.worktree, &["config", "core.hooksPath", ".husky"]);
        write_hook(&self.worktree.join(".husky").join(hook), body);
    }

    fn touch_marker(&self, name: &str) -> String {
        format!("touch '{}'", self.marker(name).display())
    }

    fn add_file(&self, name: &str) -> Vec<String> {
        fs::write(self.worktree.join(name), name).unwrap();
        vec![name.to_string()]
    }
}

#[cfg(unix)]
mod unix {
    use super::*;

    #[test]
    fn a_plain_git_commit_would_run_the_worktree_hook() {
        // Control: the fixture is the attack, or the tests below prove nothing.
        let lane = Lane::new();
        lane.husky("pre-commit", &lane.touch_marker("control"));
        lane.add_file("a.txt");
        git(&lane.worktree, &["add", "a.txt"]);
        git(&lane.worktree, &["commit", "-m", "plain"]);
        assert!(
            lane.marker("control").exists(),
            "the relative core.hooksPath hook runs under a plain git commit"
        );
    }

    #[test]
    fn a_worker_commit_never_runs_a_relative_hooks_path_hook() {
        let lane = Lane::new();
        for hook in [
            "pre-commit",
            "commit-msg",
            "prepare-commit-msg",
            "post-commit",
        ] {
            lane.husky(hook, &lane.touch_marker(hook));
        }
        let files = lane.add_file("a.txt");

        let committed = run_git_commit_with_child_lock(
            &lane.worktree,
            &files,
            "worker work",
            false,
            false,
            None,
        )
        .unwrap();

        assert!(!committed.hash.is_empty());
        for hook in [
            "pre-commit",
            "commit-msg",
            "prepare-commit-msg",
            "post-commit",
        ] {
            assert!(
                !lane.marker(hook).exists(),
                "{hook} of the worktree ran as the host user"
            );
        }
    }

    #[test]
    fn the_dco_sign_off_is_still_enforced_without_the_commit_msg_hook() {
        let lane = Lane::new();
        // A DCO `commit-msg` in the worktree: it would run if hooks did.
        lane.husky("commit-msg", "grep -q '^Signed-off-by: ' \"$1\" || exit 1");
        let files = lane.add_file("a.txt");
        run_git_commit_with_child_lock(&lane.worktree, &files, "dco", false, false, None).unwrap();
        let message = git(&lane.worktree, &["log", "-1", "--format=%B"]);
        assert!(
            message.contains("Signed-off-by: Test <test@test.com>"),
            "Kronn signs the commit off itself: {message}"
        );
    }

    #[test]
    fn a_failing_hook_is_hook_refused_with_its_output_and_commits_nothing() {
        let lane = Lane::new();
        write_hook(
            &lane.main.path().join(".git/hooks/pre-commit"),
            "echo 'lint: 3 errors' >&2; echo 'second line'; exit 1",
        );
        let files = lane.add_file("a.txt");
        let before = git(&lane.worktree, &["rev-parse", "HEAD"]);

        let error = run_git_commit_for_worker(
            &lane.worktree,
            &files,
            "refused",
            false,
            false,
            None,
            WorkerCommitHooks::Sandboxed,
        )
        .unwrap_err();

        assert!(error.starts_with(HOOK_REFUSED_PREFIX), "{error}");
        assert!(error.contains("pre-commit"), "{error}");
        // Confined, the hook's own output travels; on a host with no usable
        // sandbox (or inside another one) the refusal says it was not run.
        assert!(
            (error.contains("lint: 3 errors") && error.contains("second line"))
                || error.contains("not run:"),
            "{error}"
        );
        assert_eq!(git(&lane.worktree, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn the_default_policy_is_off_and_runs_no_hook_at_all() {
        let lane = Lane::new();
        write_hook(
            &lane.main.path().join(".git/hooks/pre-commit"),
            "echo refuse >&2; exit 1",
        );
        let files = lane.add_file("a.txt");
        run_git_commit_for_worker(
            &lane.worktree,
            &files,
            "off",
            false,
            false,
            None,
            WorkerCommitHooks::default(),
        )
        .unwrap();
        assert_eq!(WorkerCommitHooks::default(), WorkerCommitHooks::Off);
    }

    #[test]
    fn a_sandboxed_hook_never_writes_outside_the_worktree() {
        let lane = Lane::new();
        lane.husky("pre-commit", &lane.touch_marker("escape"));
        let files = lane.add_file("a.txt");

        let result = run_git_commit_for_worker(
            &lane.worktree,
            &files,
            "sandboxed",
            false,
            false,
            None,
            WorkerCommitHooks::Sandboxed,
        );

        // Under a usable sandbox the write is denied and the hook fails; without
        // one (or without the lane) the hook is not run at all. Either way the
        // worker's hook never wrote outside its worktree as the host user.
        assert!(
            !lane.marker("escape").exists(),
            "the hook wrote outside the worktree: {result:?}"
        );
        if let Err(error) = result {
            assert!(error.starts_with(HOOK_REFUSED_PREFIX), "{error}");
        }
    }

    #[test]
    fn a_candidate_merge_never_runs_a_relative_hooks_path_hook() {
        let lane = Lane::new();
        for hook in ["pre-merge-commit", "post-merge", "pre-commit", "commit-msg"] {
            lane.husky(hook, &lane.touch_marker(hook));
        }
        let files = lane.add_file("child.txt");
        run_git_commit_with_child_lock(&lane.worktree, &files, "child", false, false, None)
            .unwrap();
        fs::write(lane.main.path().join("parent.txt"), "moved").unwrap();
        git(lane.main.path(), &["add", "parent.txt"]);
        git(lane.main.path(), &["commit", "-m", "parent moves"]);
        let parent_tip = git(lane.main.path(), &["rev-parse", "HEAD"]);

        let build = build_candidate_with_hooks(&lane.worktree, &parent_tip, WorkerCommitHooks::Off)
            .unwrap();

        let CandidateOutcome::Built { sha } = build.outcome else {
            panic!("expected a built candidate, got {:?}", build.outcome);
        };
        assert!(!sha.is_empty());
        for hook in ["pre-merge-commit", "post-merge", "pre-commit", "commit-msg"] {
            assert!(!lane.marker(hook).exists(), "{hook} ran as the host user");
        }
        // The merge commit is a real two-parent merge, signed off by Kronn, not
        // by a `commit-msg` hook.
        assert_eq!(
            git(&lane.worktree, &["rev-parse", &format!("{sha}^2")]),
            parent_tip
        );
        let message = git(&lane.worktree, &["log", "-1", "--format=%B", &sha]);
        assert!(
            message.contains("Signed-off-by: Test <test@test.com>"),
            "{message}"
        );
    }

    #[test]
    fn a_fast_forward_candidate_does_not_run_the_worktree_post_merge_hook() {
        let lane = Lane::new();
        lane.husky("post-merge", &lane.touch_marker("post-merge"));
        fs::write(lane.main.path().join("parent.txt"), "moved").unwrap();
        git(lane.main.path(), &["add", "parent.txt"]);
        git(lane.main.path(), &["commit", "-m", "parent moves"]);
        let parent_tip = git(lane.main.path(), &["rev-parse", "HEAD"]);

        let build = build_candidate_with_hooks(&lane.worktree, &parent_tip, WorkerCommitHooks::Off)
            .unwrap();

        assert!(matches!(build.outcome, CandidateOutcome::Built { .. }));
        assert!(!lane.marker("post-merge").exists());
    }

    #[test]
    fn a_refusing_merge_hook_is_hook_refused_not_an_empty_conflict() {
        let lane = Lane::new();
        write_hook(
            &lane.main.path().join(".git/hooks/pre-merge-commit"),
            "echo 'merge gate: tests missing' >&2; exit 1",
        );
        let files = lane.add_file("child.txt");
        run_git_commit_with_child_lock(&lane.worktree, &files, "child", false, false, None)
            .unwrap();
        fs::write(lane.main.path().join("parent.txt"), "moved").unwrap();
        git(lane.main.path(), &["add", "parent.txt"]);
        git(lane.main.path(), &["commit", "-m", "parent moves"]);
        let parent_tip = git(lane.main.path(), &["rev-parse", "HEAD"]);
        let before = git(&lane.worktree, &["rev-parse", "HEAD"]);

        let build =
            build_candidate_with_hooks(&lane.worktree, &parent_tip, WorkerCommitHooks::Sandboxed)
                .unwrap();

        let CandidateOutcome::HookRefused { hook, output } = build.outcome else {
            panic!("expected HookRefused, got {:?}", build.outcome);
        };
        assert_eq!(hook, "pre-merge-commit");
        assert!(
            output.contains("merge gate: tests missing") || output.contains("not run:"),
            "{output}"
        );
        assert_eq!(git(&lane.worktree, &["rev-parse", "HEAD"]), before);
        assert!(
            !crate::core::docs_citations::merge::merge_in_progress(&lane.worktree),
            "the refused merge was undone"
        );
    }
}

#[test]
fn the_policy_defaults_to_off_and_round_trips_as_snake_case() {
    assert_eq!(WorkerCommitHooks::default(), WorkerCommitHooks::Off);
    for (policy, text) in [
        (WorkerCommitHooks::Off, "\"off\""),
        (WorkerCommitHooks::Sandboxed, "\"sandboxed\""),
    ] {
        assert_eq!(serde_json::to_string(&policy).unwrap(), text);
        assert_eq!(
            serde_json::from_str::<WorkerCommitHooks>(text).unwrap(),
            policy
        );
    }
    // The unconfined `main_checkout` mode is gone: a stored one reads as the
    // confined policy, never as a hook run as the host user.
    assert_eq!(
        serde_json::from_str::<WorkerCommitHooks>("\"main_checkout\"").unwrap(),
        WorkerCommitHooks::Sandboxed
    );
    let workspace: crate::models::ProjectWorkspace = serde_json::from_str("{}").unwrap();
    assert_eq!(workspace.worker_commit_hooks, WorkerCommitHooks::Off);
}

#[test]
fn first_lines_bounds_what_travels_with_a_refusal() {
    let long: String = (0..100).map(|n| format!("line {n}\n")).collect();
    assert_eq!(first_lines(&long).lines().count(), REFUSAL_LINES);
    assert!(first_lines(&"x".repeat(10_000)).chars().count() <= REFUSAL_CHARS + 1);
    assert_eq!(first_lines("\n\n"), "");
}

#[cfg(unix)]
#[test]
fn the_hook_wrapper_keeps_both_streams_with_their_line_breaks_and_the_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("pre-commit");
    write_hook(
        &hook,
        "echo 'lint: 3 errors' >&2; echo 'second line'; echo 'third' >&2; exit 3",
    );
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", HOOK_WRAPPER])
        .arg(&hook)
        .env("TMPDIR", dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    // What the probe runner does to a failure's stream: collapse whitespace.
    let collapsed = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        decode_hook_output(&collapsed),
        "lint: 3 errors\nsecond line\nthird"
    );
    // Nothing is left behind in the temp directory but the hook itself.
    let left: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
    assert_eq!(left.len(), 1, "{left:?}");
}

#[cfg(unix)]
#[test]
fn the_hook_wrapper_bounds_what_a_chatty_hook_prints() {
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("pre-commit");
    write_hook(
        &hook,
        "yes 'a long chatty line of lint output' | head -n 5000; exit 1",
    );
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", HOOK_WRAPPER])
        .arg(&hook)
        .env("TMPDIR", dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.len() <= 8192, "{}", output.stdout.len());
}

#[cfg(unix)]
#[test]
fn the_hook_wrapper_says_so_when_it_cannot_capture_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("pre-commit");
    write_hook(&hook, "echo 'lint: 3 errors'; exit 2");
    // A TMPDIR that does not exist: `mktemp` fails.
    let missing = dir.path().join("no-such-dir");
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", HOOK_WRAPPER])
        .arg(&hook)
        .env("TMPDIR", &missing)
        .output()
        .unwrap();
    // The hook's exit status is still the wrapper's, and the reason is named
    // instead of an empty answer ("printed nothing").
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("could not capture the hook output"), "{text}");
    assert!(text.contains("mktemp failed"), "{text}");
}
