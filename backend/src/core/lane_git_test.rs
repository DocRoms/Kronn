//! KT-1745 — Kronn's git commands on a worker's worktree are pinned to the
//! registered repository and never run what the worktree names: no hook, no
//! filter, no file-system monitor, no signing program, no merge driver, and a
//! redirected `.git` pointer is refused (and reported) instead of followed.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::api::git_ops::{run_git_commit_for_worker, run_git_commit_with_child_lock};
use crate::core::worktree::{build_candidate_with_hooks, CandidateOutcome};
use crate::models::WorkerCommitHooks;

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

/// A git whose output is not asserted (a command expected to fail).
fn git_status(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success()
}

#[cfg(unix)]
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
}

/// A registered project checkout, the worker's worktree where Kronn puts it
/// (`<repo>/.kronn/worktrees/<lane>`), and a directory outside both where
/// anything the worker makes Kronn run leaves a marker.
struct World {
    main: tempfile::TempDir,
    worktree: PathBuf,
    markers: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let main = tempfile::Builder::new()
            .prefix("kronn-lane-git")
            .tempdir()
            .unwrap();
        git(main.path(), &["init", "-b", "main"]);
        git(main.path(), &["config", "user.email", "test@test.com"]);
        git(main.path(), &["config", "user.name", "Test"]);
        git(main.path(), &["config", "core.autocrlf", "false"]);
        fs::write(main.path().join("README.md"), "one\ntwo\nthree\n").unwrap();
        git(main.path(), &["add", "."]);
        git(main.path(), &["commit", "-m", "init"]);
        let root = main.path().canonicalize().unwrap();
        let worktree = root.join(".kronn/worktrees/task-kt-1745-poc");
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            main.path(),
            &["worktree", "add", "-b", "work", worktree.to_str().unwrap()],
        );
        Self {
            main,
            worktree,
            markers: tempfile::Builder::new()
                .prefix("kronn-lane-markers")
                .tempdir()
                .unwrap(),
        }
    }

    fn root(&self) -> PathBuf {
        self.main.path().canonicalize().unwrap()
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.markers.path().join(name)
    }

    fn touch(&self, name: &str) -> String {
        format!("touch '{}'", self.marker(name).display())
    }

    /// husky: a relative `core.hooksPath` whose hooks the worker writes.
    fn husky(&self, hook: &str, body: &str) {
        git(&self.worktree, &["config", "core.hooksPath", ".husky"]);
        script(&self.worktree.join(".husky").join(hook), body);
    }

    /// The child commits a change to `README.md` line one; the parent commits
    /// another: merging the parent tip into the child conflicts.
    fn conflicting_histories(&self) -> String {
        fs::write(self.worktree.join("README.md"), "child\ntwo\nthree\n").unwrap();
        run_git_commit_with_child_lock(
            &self.worktree,
            &["README.md".to_string()],
            "child edits",
            false,
            false,
            None,
        )
        .unwrap();
        fs::write(self.main.path().join("README.md"), "parent\ntwo\nthree\n").unwrap();
        git(self.main.path(), &["add", "README.md"]);
        git(self.main.path(), &["commit", "-m", "parent edits"]);
        git(self.main.path(), &["rev-parse", "HEAD"])
    }

    /// A `.git` pointer the worker redirected to a repository of its own,
    /// whose configuration names a program for every hook of git it can reach.
    fn redirect_pointer_to(&self, evil: &Path) {
        git(
            evil.parent().unwrap(),
            &["init", "-q", evil.to_str().unwrap()],
        );
        let config = |key: &str, value: &str| {
            git(evil, &["config", key, value]);
        };
        config("user.email", "evil@evil.test");
        config("user.name", "Evil");
        config("filter.x.clean", &format!("{}; cat", self.touch("FILTER")));
        config("filter.x.smudge", &format!("{}; cat", self.touch("SMUDGE")));
        let fsmonitor = evil.join("fsmonitor.sh");
        script(&fsmonitor, &self.touch("FSMONITOR"));
        config("core.fsmonitor", fsmonitor.to_str().unwrap());
        let gpg = evil.join("gpg.sh");
        script(&gpg, &self.touch("GPG"));
        config("gpg.program", gpg.to_str().unwrap());
        config("commit.gpgsign", "true");
        config(
            "merge.d.driver",
            &format!("{}; false", self.touch("DRIVER")),
        );
        config("core.hooksPath", evil.join("hooks").to_str().unwrap());
        script(&evil.join("hooks/pre-commit"), &self.touch("EVIL_HOOK"));
        config("core.worktree", self.worktree.to_str().unwrap());
        fs::write(
            self.worktree.join(".gitattributes"),
            "*.txt filter=x\n*.md merge=d\n",
        )
        .unwrap();
        fs::write(
            self.worktree.join(".git"),
            format!("gitdir: {}/.git\n", evil.display()),
        )
        .unwrap();
    }

    fn assert_no_marker(&self) {
        let found: Vec<String> = fs::read_dir(self.markers.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(found.is_empty(), "the worker made Kronn run: {found:?}");
    }
}

fn is_tamper(error: &str) -> bool {
    error.starts_with(TAMPER_PREFIX)
}

#[test]
fn a_managed_worktree_is_pinned_to_its_registration_not_to_its_pointer() {
    let world = World::new();
    let lane = LaneGit::resolve(&world.worktree).unwrap();
    let pinned = lane.pinned().expect("a managed worktree is pinned");
    assert_eq!(pinned.repo, world.root());
    assert_eq!(pinned.common_dir, world.root().join(".git"));
    assert_eq!(
        pinned.git_dir,
        world.root().join(".git/worktrees/task-kt-1745-poc")
    );
    let command = lane.command();
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    for expected in [
        format!("--git-dir={}", pinned.git_dir.display()),
        format!("--work-tree={}", world.worktree.display()),
        "core.hooksPath=/dev/null".to_string(),
        "core.fsmonitor=false".to_string(),
        "commit.gpgSign=false".to_string(),
        "tag.gpgSign=false".to_string(),
    ] {
        assert!(args.contains(&expected), "{expected} not in {args:?}");
    }
    assert_eq!(command.get_current_dir(), Some(world.worktree.as_path()));
}

#[test]
fn a_checkout_that_is_not_a_managed_worktree_is_hardened_but_not_pinned() {
    let world = World::new();
    let lane = LaneGit::resolve(&world.root()).unwrap();
    assert!(!lane.is_pinned());
    let args: Vec<String> = lane
        .command()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"core.hooksPath=/dev/null".to_string()));
    assert!(!args.iter().any(|arg| arg.starts_with("--git-dir")));
    // The user's own push keeps the owner's hooks there, and not in a worktree.
    let owner = git_keeping_owner_hooks(&world.root()).unwrap();
    assert!(!owner
        .get_args()
        .any(|arg| arg.to_string_lossy().contains("hooksPath")));
    let worker = git_keeping_owner_hooks(&world.worktree).unwrap();
    assert!(worker
        .get_args()
        .any(|arg| arg.to_string_lossy() == "core.hooksPath=/dev/null"));
}

#[test]
fn a_worktree_kronn_created_with_portable_relative_paths_is_pinned_too() {
    // KT-331: `.git` and `<admin>/gitdir` are both written relative.
    let world = World::new();
    let base = git(world.main.path(), &["rev-parse", "HEAD"]);
    let info = crate::core::worktree::create_task_worktree(&world.root(), "KT-1745", "rel1", &base)
        .unwrap();
    let worktree = Path::new(&info.path);
    let pointer = fs::read_to_string(worktree.join(".git")).unwrap();
    assert!(pointer.starts_with("gitdir: ../"), "{pointer}");

    let lane = LaneGit::resolve(worktree).unwrap();
    let pinned = lane.pinned().unwrap();
    assert!(pinned
        .git_dir
        .starts_with(world.root().join(".git/worktrees")));
    lane.verify().unwrap();
    fs::write(
        worktree.join(".git"),
        "gitdir: ../../../.git/worktrees/other\n",
    )
    .unwrap();
    assert!(is_tamper(&lane.verify().unwrap_err()));
}

#[test]
fn a_worktree_git_registers_nothing_for_is_refused() {
    let world = World::new();
    let stranger = world.root().join(".kronn/worktrees/task-never-registered");
    fs::create_dir_all(&stranger).unwrap();
    fs::write(stranger.join(".git"), "gitdir: /anywhere\n").unwrap();
    let error = LaneGit::resolve(&stranger).unwrap_err();
    assert!(error.contains("not a worktree registered"), "{error}");
}

#[test]
fn the_pointer_is_checked_every_time_and_a_restored_pointer_is_trusted_again() {
    let world = World::new();
    let lane = LaneGit::resolve(&world.worktree).unwrap();
    let pointer = world.worktree.join(".git");
    let original = fs::read_to_string(&pointer).unwrap();
    lane.verify().unwrap();

    fs::write(&pointer, "gitdir: /somewhere/else\n").unwrap();
    let error = lane.verify().unwrap_err();
    assert!(is_tamper(&error), "{error}");
    assert!(error.contains("/somewhere/else"), "{error}");
    assert!(is_tamper(&LaneGit::resolve(&world.worktree).unwrap_err()));

    fs::write(&pointer, format!("{original}gitdir: /second\n")).unwrap();
    assert!(is_tamper(&lane.verify().unwrap_err()), "two lines");

    fs::remove_file(&pointer).unwrap();
    assert!(is_tamper(&lane.verify().unwrap_err()), "a missing pointer");
    fs::create_dir(&pointer).unwrap();
    assert!(is_tamper(&lane.verify().unwrap_err()), "a directory");
    fs::remove_dir(&pointer).unwrap();

    fs::write(&pointer, original).unwrap();
    lane.verify().unwrap();
}

#[cfg(unix)]
mod unix {
    use super::*;

    #[test]
    fn a_plain_git_follows_a_redirected_pointer_and_runs_its_programs() {
        // Control: the fixture is the attack, or the tests below prove nothing.
        let world = World::new();
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        fs::write(world.worktree.join("b.txt"), "b").unwrap();
        git(&world.worktree, &["add", "--", "b.txt"]);
        assert!(
            world.marker("FILTER").exists(),
            "a plain `git add` runs the filter the redirected configuration names"
        );
    }

    #[test]
    fn a_redirected_pointer_makes_every_kronn_git_command_refuse_and_nothing_runs() {
        let world = World::new();
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        fs::write(world.worktree.join("b.txt"), "b").unwrap();
        fs::write(world.worktree.join("d.md"), "d").unwrap();

        // The commit and the staging of `task_exec_commit` / `git_commit`.
        for policy in [
            WorkerCommitHooks::Off,
            WorkerCommitHooks::MainCheckout,
            WorkerCommitHooks::Sandboxed,
        ] {
            let error = run_git_commit_for_worker(
                &world.worktree,
                &["b.txt".to_string(), "d.md".to_string()],
                "evil",
                false,
                false,
                None,
                policy,
            )
            .unwrap_err();
            assert!(is_tamper(&error), "{policy:?}: {error}");
        }
        // The inspections Kronn makes of a worker's tree.
        assert!(is_tamper(
            &crate::core::worktree::worktree_dirty_files(&world.worktree).unwrap_err()
        ));
        assert!(is_tamper(
            &crate::core::worktree::committed_file_changes(&world.worktree, "HEAD", "HEAD")
                .unwrap_err()
        ));
        assert!(is_tamper(
            &crate::core::worktree::committer_identity(&world.worktree).unwrap_err()
        ));
        assert!(is_tamper(
            &crate::core::worktree::verify_worktree_head(&world.worktree, "x").unwrap_err()
        ));
        assert!(is_tamper(
            &crate::api::agent_workspace_tools::git_status_payload(&world.worktree).unwrap_err()
        ));
        assert!(is_tamper(
            &crate::api::agent_workspace_tools::git_diff_payload(&world.worktree, None, None)
                .unwrap_err()
        ));
        // The candidate merge, which commits as the host user.
        let error = build_candidate_with_hooks(&world.worktree, "HEAD", WorkerCommitHooks::Off)
            .unwrap_err();
        assert!(is_tamper(&error), "{error}");
        // A doc re-anchoring and a push go through the same builder.
        assert!(!crate::api::git_ops::merge_in_progress(&world.worktree));
        assert!(
            crate::core::docs_citations::merge::reanchor_branch_leaving_unplaced(
                &world.worktree,
                "HEAD"
            )
            .is_err()
        );
        assert!(is_tamper(
            &crate::api::git_ops::run_git_push(&world.worktree, &[]).unwrap_err()
        ));

        world.assert_no_marker();
    }

    #[test]
    fn a_pointer_into_the_lane_target_cannot_pass_for_the_main_checkout() {
        // `<main>/target/wt-<lane>` is a write root of the lane, outside the
        // worktree: a git directory the worker built there is not the main
        // checkout's, whatever `--git-common-dir` says through the pointer.
        let world = World::new();
        let lane_target = world.root().join("target/wt-kt-1745-poc");
        fs::create_dir_all(&lane_target).unwrap();
        let evil = lane_target.join("evil");
        world.redirect_pointer_to(&evil);
        script(
            &evil.join(".git/hooks/pre-commit"),
            &world.touch("LANE_TARGET_HOOK"),
        );
        fs::write(world.worktree.join("b.txt"), "b").unwrap();

        for policy in [
            WorkerCommitHooks::MainCheckout,
            WorkerCommitHooks::Sandboxed,
        ] {
            let error = run_git_commit_for_worker(
                &world.worktree,
                &["b.txt".to_string()],
                "evil",
                false,
                false,
                None,
                policy,
            )
            .unwrap_err();
            assert!(is_tamper(&error), "{policy:?}: {error}");
            let refusal = crate::core::commit_hooks::run_gating_hook(
                &world.worktree,
                policy,
                "pre-commit",
                None,
            )
            .unwrap_err();
            assert!(is_tamper(&refusal.output), "{policy:?}: {refusal}");
        }
        world.assert_no_marker();
    }

    #[test]
    fn the_main_checkout_hook_comes_from_the_registered_repo_even_when_the_pointer_lies() {
        // Not tampered: the registered checkout's own hook runs, the worktree's
        // and the lane target's never do.
        let world = World::new();
        script(
            &world.main.path().join(".git/hooks/pre-commit"),
            &world.touch("OWNER"),
        );
        let lane_target = world.root().join("target/wt-kt-1745-poc/hooks");
        script(&lane_target.join("pre-commit"), &world.touch("LANE_TARGET"));
        fs::write(world.worktree.join("b.txt"), "b").unwrap();
        run_git_commit_for_worker(
            &world.worktree,
            &["b.txt".to_string()],
            "owner hook",
            false,
            false,
            None,
            WorkerCommitHooks::MainCheckout,
        )
        .unwrap();
        assert!(world.marker("OWNER").exists());
        assert!(!world.marker("LANE_TARGET").exists());
    }

    #[test]
    fn a_candidate_merge_conflict_aborts_without_firing_a_worktree_hook() {
        let world = World::new();
        let parent_tip = world.conflicting_histories();
        for hook in [
            "reference-transaction",
            "post-checkout",
            "post-merge",
            "post-rewrite",
        ] {
            world.husky(hook, &world.touch(hook));
        }

        let build =
            build_candidate_with_hooks(&world.worktree, &parent_tip, WorkerCommitHooks::Off)
                .unwrap();

        assert_eq!(
            build.outcome,
            CandidateOutcome::Conflict {
                files: vec!["README.md".to_string()]
            }
        );
        assert!(
            !crate::core::docs_citations::merge::merge_in_progress(&world.worktree),
            "the merge was aborted"
        );
        world.assert_no_marker();
    }

    #[test]
    fn a_plain_merge_abort_would_fire_the_worktree_hook() {
        // Control for the test above.
        let world = World::new();
        let parent_tip = world.conflicting_histories();
        world.husky(
            "reference-transaction",
            &world.touch("reference-transaction"),
        );
        assert!(!git_status(
            &world.worktree,
            &["merge", "--no-edit", &parent_tip]
        ));
        git(&world.worktree, &["merge", "--abort"]);
        assert!(
            world.marker("reference-transaction").exists(),
            "a plain `git merge --abort` fires the hook of the worktree"
        );
    }

    #[test]
    fn a_merge_refused_before_it_starts_is_an_error_and_not_an_empty_conflict() {
        let world = World::new();
        let parent_tip = world.conflicting_histories();
        // Uncommitted work on a file the merge changes: git refuses to start.
        fs::write(world.worktree.join("README.md"), "dirty\ntwo\nthree\n").unwrap();

        let result =
            build_candidate_with_hooks(&world.worktree, &parent_tip, WorkerCommitHooks::Off);

        let error = result.expect_err("a refused merge is not a conflict");
        assert!(error.contains("refused before it started"), "{error}");
        assert!(
            error.contains("README.md"),
            "git's own words travel: {error}"
        );
    }

    #[test]
    fn a_worker_cannot_write_its_pointer_through_the_http_agent_file_tools() {
        let world = World::new();
        let root = &world.worktree;
        for requested in [".git", "./.git"] {
            let refusal =
                crate::api::agent_workspace_tools::resolve_writable_in_workspace(root, requested)
                    .unwrap_err();
            assert!(
                refusal.message().contains("repository pointer"),
                "{requested}"
            );
        }
        std::os::unix::fs::symlink(root.join(".git"), root.join("innocent")).unwrap();
        assert!(
            crate::api::agent_workspace_tools::resolve_writable_in_workspace(root, "innocent")
                .is_err(),
            "a link to the pointer is the pointer"
        );
        let pointer = fs::read_to_string(root.join(".git")).unwrap();
        for result in [
            crate::api::agent_workspace_tools::write_file_payload(root, ".git", "gitdir: /evil"),
            crate::api::agent_workspace_tools::write_file_payload(
                root,
                "innocent",
                "gitdir: /evil",
            ),
        ] {
            assert!(result.is_err(), "{result:?}");
        }
        assert_eq!(fs::read_to_string(root.join(".git")).unwrap(), pointer);
        // Anything else is still writable, and `.git` is still readable.
        assert!(
            crate::api::agent_workspace_tools::resolve_writable_in_workspace(root, "README.md")
                .is_ok()
        );
        assert!(crate::api::agent_workspace_tools::resolve_in_workspace(root, ".git").is_ok());
    }
}
