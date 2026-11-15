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
        for policy in [WorkerCommitHooks::Off, WorkerCommitHooks::Sandboxed] {
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

        {
            let policy = WorkerCommitHooks::Sandboxed;
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
            let refusal =
                crate::core::commit_hooks::run_gating_hook(&world.worktree, policy, "pre-commit")
                    .unwrap_err();
            assert!(is_tamper(&refusal.output), "{policy:?}: {refusal}");
        }
        world.assert_no_marker();
    }

    #[test]
    fn the_owners_own_hook_never_runs_as_the_host_user() {
        // The unconfined `main_checkout` mode is gone: a real hook executes
        // worktree content (`PATH=node_modules/.bin:$PATH`, lint-staged, make).
        // Under the only policy that runs a hook, it runs confined: it cannot
        // write outside the lane, and where there is no sandbox it is not run.
        let world = World::new();
        script(
            &world.main.path().join(".git/hooks/pre-commit"),
            &world.touch("OWNER"),
        );
        script(
            &world.worktree.join("node_modules/.bin/lint-staged"),
            &world.touch("WORKTREE_BINARY"),
        );
        fs::write(world.worktree.join("b.txt"), "b").unwrap();
        let _ = run_git_commit_for_worker(
            &world.worktree,
            &["b.txt".to_string()],
            "owner hook",
            false,
            false,
            None,
            WorkerCommitHooks::Sandboxed,
        );
        assert!(!world.marker("OWNER").exists());
        assert!(!world.marker("WORKTREE_BINARY").exists());
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

    #[test]
    fn a_nested_git_entry_is_refused_to_the_http_file_tools_whatever_its_case() {
        let world = World::new();
        let root = world.worktree.as_path();
        fs::create_dir_all(root.join("lib/.git")).unwrap();
        for requested in [
            "lib/.git/config",
            "lib/.GIT/config",
            "lib/.Git",
            "./lib/.git/hooks/x",
            ".GIT",
            ".GiT/config",
        ] {
            assert!(
                crate::api::agent_workspace_tools::resolve_writable_in_workspace(root, requested)
                    .is_err(),
                "{requested} must be refused"
            );
        }
        assert!(
            crate::api::agent_workspace_tools::resolve_writable_in_workspace(root, "lib/gitx.txt")
                .is_ok()
        );
    }

    /// A lane with a submodule: a gitlink in the index, the submodule's own
    /// repository (a full `.git` directory, as Kronn clones them) in the tree.
    fn world_with_submodule() -> World {
        let world = World::new();
        let sub = world.root().join("upstream-lib");
        git(
            world.root().as_path(),
            &["init", "-q", "-b", "main", sub.to_str().unwrap()],
        );
        git(&sub, &["config", "user.email", "t@t.test"]);
        git(&sub, &["config", "user.name", "T"]);
        fs::write(sub.join("code.txt"), "x").unwrap();
        git(&sub, &["add", "."]);
        git(&sub, &["commit", "-q", "-m", "lib"]);
        let sha = git(&sub, &["rev-parse", "HEAD"]);
        let target = world.worktree.join("lib");
        git(
            &world.worktree,
            &["clone", "-q", "--no-local", sub.to_str().unwrap(), "lib"],
        );
        assert!(target.join(".git").is_dir());
        git(
            &world.worktree,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},lib"),
            ],
        );
        world
    }

    #[test]
    fn a_submodule_config_the_worker_wrote_is_refused_and_no_filter_runs() {
        let world = world_with_submodule();
        // Control: a submodule as Kronn clones it is accepted.
        assert!(LaneGit::resolve(&world.worktree).is_ok());

        // The worker appends a clean filter to the submodule's own config and
        // names it in the submodule's attributes, then touches a file.
        let config = world.worktree.join("lib/.git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "[filter \"w\"]\n\tclean = {}; cat\n\trequired = false\n",
            world.touch("SUBMODULE_FILTER")
        ));
        fs::write(&config, text).unwrap();
        fs::write(world.worktree.join("lib/.gitattributes"), "* filter=w\n").unwrap();
        fs::write(world.worktree.join("lib/code.txt"), "changed").unwrap();

        let refused = |result: Result<std::process::Command, String>| {
            let error = result.expect_err("the submodule's configuration is not trusted");
            assert!(error.starts_with(NESTED_PREFIX), "{error}");
            assert!(error.contains("filter.w.clean"), "{error}");
        };
        refused(git_in_lane(&world.worktree));
        // Every command a Kronn path builds is refused before it starts.
        for args in [
            vec!["status", "--porcelain=v1"],
            vec!["diff", "HEAD"],
            vec!["add", "-A"],
            vec!["commit", "-a", "-m", "x"],
            vec!["diff-index", "HEAD"],
        ] {
            let result = crate::core::lane_git::git(&world.worktree).map(|mut command| {
                command.args(&args);
                command
            });
            refused(result);
        }
        world.assert_no_marker();
    }

    fn git_in_lane(work_dir: &Path) -> Result<std::process::Command, String> {
        crate::core::lane_git::git(work_dir)
    }

    #[test]
    fn a_submodule_gitfile_pointing_inside_the_worktree_is_refused() {
        let world = world_with_submodule();
        let evil = world.worktree.join("evil-modules");
        fs::create_dir_all(&evil).unwrap();
        fs::remove_dir_all(world.worktree.join("lib/.git")).unwrap();
        fs::write(
            world.worktree.join("lib/.git"),
            format!("gitdir: {}\n", evil.display()),
        )
        .unwrap();
        let error = crate::core::lane_git::git(&world.worktree).unwrap_err();
        assert!(error.starts_with(NESTED_PREFIX), "{error}");
    }

    #[test]
    fn a_new_embedded_repository_is_never_staged_for_the_worker() {
        let world = World::new();
        let nested = world.worktree.join("vendor/tool");
        fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        fs::write(nested.join("a.txt"), "a").unwrap();
        let error = run_git_commit_for_worker(
            &world.worktree,
            &["vendor".to_string()],
            "embed",
            false,
            false,
            None,
            WorkerCommitHooks::Off,
        )
        .unwrap_err();
        assert!(error.starts_with("EmbeddedRepository"), "{error}");
        assert_eq!(git(&world.worktree, &["ls-files", "vendor"]), "");
    }

    #[test]
    fn a_relative_program_in_the_registered_config_is_turned_off() {
        // `filter.rel.clean = sh tools/clean.sh` is the owner's configuration,
        // but the path resolves in the worktree: whatever the worker wrote there
        // would run as the host user.
        let world = World::new();
        git(
            &world.root(),
            &["config", "filter.rel.clean", "sh tools/clean.sh"],
        );
        git(
            &world.root(),
            &["config", "merge.rel.driver", "sh tools/merge.sh %A"],
        );
        git(
            &world.root(),
            &["config", "filter.abs.clean", "/usr/bin/true"],
        );
        script(
            &world.worktree.join("tools/clean.sh"),
            &format!("{}; cat", world.touch("RELFILTER")),
        );
        fs::write(world.worktree.join(".gitattributes"), "*.txt filter=rel\n").unwrap();
        fs::write(world.worktree.join("b.txt"), "b").unwrap();

        let command = crate::core::lane_git::git(&world.worktree).unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"filter.rel.clean=".to_string()), "{args:?}");
        assert!(
            args.contains(&"filter.rel.required=false".to_string()),
            "{args:?}"
        );
        assert!(
            args.contains(&"merge.rel.driver=false".to_string()),
            "{args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.starts_with("filter.abs.")),
            "an absolute program is the owner's own: {args:?}"
        );

        let mut add = crate::core::lane_git::git(&world.worktree).unwrap();
        let output = add.args(["add", "-A"]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        world.assert_no_marker();
    }

    #[test]
    fn a_program_that_names_no_worktree_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        for value in ["git-lfs clean -- %f", "/usr/bin/cat", "cat", "tr a-z A-Z"] {
            assert!(!names_worktree_content(value, dir.path()), "{value}");
        }
        fs::write(dir.path().join("clean.sh"), "").unwrap();
        for value in ["sh clean.sh", "./clean.sh", "sh tools/x.sh", "tools/clean"] {
            assert!(names_worktree_content(value, dir.path()), "{value}");
        }
    }

    #[test]
    fn a_directory_inside_the_worktree_is_pinned_like_the_worktree() {
        let world = World::new();
        let sub = world.worktree.join("pkg/deep");
        fs::create_dir_all(&sub).unwrap();
        let lane = LaneGit::resolve(&sub).unwrap();
        assert!(lane.is_pinned());
        assert_eq!(lane.work_dir(), world.worktree.canonicalize().unwrap());
        // Redirecting the pointer is caught from the subdirectory too.
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        assert!(is_tamper(&LaneGit::resolve(&sub).unwrap_err()));
        world.assert_no_marker();
    }

    #[test]
    fn the_read_helpers_run_nothing_in_a_redirected_worktree() {
        let world = World::new();
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        fs::write(world.worktree.join("b.txt"), "b").unwrap();
        // `git_or_refuse` is what the status / diff / blame / commit-patch helpers,
        // `disc_pr_template` and `disc_workspace::git_value` build on.
        let refused = git_or_refuse(&world.worktree)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        let _ = crate::api::git_ops::run_git_status(&world.worktree); // tolerant: may answer "nothing", but runs nothing
                                                                      // Some of these answer "nothing" instead of an error; what matters is that
                                                                      // no program the redirected repository names ran.
        let _ = crate::api::git_ops::run_git_diff(&world.worktree, "b.txt");
        let _ = crate::api::git_ops::run_git_blame(&world.worktree, "README.md");
        let _ = crate::api::git_ops::run_git_commit_patch(&world.worktree, "HEAD");
        assert!(git_keeping_owner_hooks(&world.worktree).is_err());
        world.assert_no_marker();
    }

    #[test]
    fn the_local_exclude_of_a_redirected_worktree_is_refused_and_a_symlinked_info_dir_too() {
        let world = World::new();
        // Healthy: written once, into the registered repository.
        crate::core::worktree::ensure_local_git_exclude(&world.worktree, "/.kronn-poc-*").unwrap();
        let exclude = fs::read_to_string(world.root().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("/.kronn-poc-*"), "{exclude}");
        // A symlinked `info` directory is not followed.
        let info = world.root().join(".git/info");
        let elsewhere = world.markers.path().join("elsewhere");
        fs::rename(&info, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &info).unwrap();
        let error =
            crate::core::worktree::ensure_local_git_exclude(&world.worktree, "/.kronn-other-*")
                .unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        assert!(!fs::read_to_string(elsewhere.join("exclude"))
            .unwrap()
            .contains("kronn-other"));
        fs::remove_file(&info).unwrap();
        fs::rename(&elsewhere, &info).unwrap();
        // A redirected pointer refuses the whole operation.
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        let error =
            crate::core::worktree::ensure_local_git_exclude(&world.worktree, "/.kronn-third-*")
                .unwrap_err();
        assert!(is_tamper(&error), "{error}");
        world.assert_no_marker();
    }

    #[test]
    fn a_fast_forward_into_a_redirected_target_worktree_runs_nothing() {
        let world = World::new();
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        let error = crate::core::worktree::fast_forward_to(&world.worktree, "HEAD").unwrap_err();
        assert!(is_tamper(&error), "{error}");
        world.assert_no_marker();
    }

    #[test]
    fn the_seatbelt_profile_denies_every_git_entry_of_the_worktree() {
        let world = World::new();
        let profile = crate::core::task_worker_sandbox::seatbelt_profile_read_only(
            String::new(),
            &[world.worktree.join(".git")],
            std::slice::from_ref(&world.worktree),
        );
        assert!(profile.contains("(literal "), "{profile}");
        assert!(profile.contains("[gG][iI][tT]"), "{profile}");
        assert!(profile.contains("deny file-write*"), "{profile}");
    }
    /// A `.git` pointer redirected to a partial clone whose HEAD names an object
    /// it does not have: any command that reads it makes git fetch it lazily,
    /// through the `core.sshCommand` the worker wrote.
    fn redirect_to_promisor_repo(world: &World) -> String {
        let evil = world.worktree.join("evil-repo");
        git(
            evil.parent().unwrap(),
            &["init", "-q", "-b", "main", evil.to_str().unwrap()],
        );
        let ssh = evil.join("ssh.sh");
        script(&ssh, &world.touch("LAZY_FETCH"));
        for (key, value) in [
            ("core.repositoryformatversion", "1"),
            ("extensions.partialclone", "origin"),
            // The transport is a program the worker names: `ext::` runs it.
            ("protocol.ext.allow", "always"),
            ("remote.origin.promisor", "true"),
            ("core.sshCommand", ssh.to_str().unwrap()),
        ] {
            git(&evil, &["config", key, value]);
        }
        git(
            &evil,
            &[
                "config",
                "remote.origin.url",
                &format!("ext::{}", ssh.display()),
            ],
        );
        let missing = "1234567890abcdef1234567890abcdef12345678";
        fs::write(evil.join(".git/refs/heads/main"), format!("{missing}\n")).unwrap();
        fs::write(
            world.worktree.join(".git"),
            format!("gitdir: {}/.git\n", evil.display()),
        )
        .unwrap();
        missing.to_string()
    }

    #[test]
    fn a_plain_rev_parse_in_a_redirected_worktree_lazy_fetches_through_the_workers_ssh_command() {
        // Control: the fixture is the attack, or the test below proves nothing.
        let world = World::new();
        redirect_to_promisor_repo(&world);
        let _ = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "HEAD^{commit}"])
            .current_dir(&world.worktree)
            .output()
            .unwrap();
        assert!(
            world.marker("LAZY_FETCH").exists(),
            "a plain `git rev-parse` fetches lazily with the redirected configuration"
        );
    }

    #[test]
    fn resolve_commit_and_is_ancestor_on_a_redirected_worktree_run_nothing() {
        let world = World::new();
        let missing = redirect_to_promisor_repo(&world);
        let error = crate::core::worktree::resolve_commit(&world.worktree, "HEAD").unwrap_err();
        assert!(is_tamper(&error), "{error}");
        let error =
            crate::core::worktree::is_ancestor(&world.worktree, &missing, "HEAD").unwrap_err();
        assert!(is_tamper(&error), "{error}");
        // From a subdirectory too.
        let sub = world.worktree.join("pkg");
        fs::create_dir_all(&sub).unwrap();
        assert!(crate::core::worktree::resolve_commit(&sub, "HEAD").is_err());
        world.assert_no_marker();
    }

    #[test]
    fn resolve_commit_on_a_healthy_worktree_and_on_the_checkout_still_works() {
        let world = World::new();
        let main_head = git(&world.root(), &["rev-parse", "main"]);
        assert_eq!(
            crate::core::worktree::resolve_commit(&world.worktree, "HEAD").unwrap(),
            main_head
        );
        assert_eq!(
            crate::core::worktree::resolve_commit(&world.root(), "main").unwrap(),
            main_head
        );
        assert!(crate::core::worktree::is_ancestor(&world.worktree, &main_head, "HEAD").unwrap());
    }

    #[test]
    fn the_lane_command_never_fetches_lazily() {
        let world = World::new();
        let command = crate::core::lane_git::git(&world.worktree).unwrap();
        let no_lazy = command
            .get_envs()
            .find(|(key, _)| *key == "GIT_NO_LAZY_FETCH")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(no_lazy.as_deref(), Some("1"));
    }

    #[test]
    fn a_materialised_submodule_is_its_own_repository_not_the_parent_worktree() {
        let world = world_with_submodule();
        let sub = world.worktree.join("lib");
        let sub_head = git(&sub, &["rev-parse", "HEAD"]);
        let parent_head = git(&world.worktree, &["rev-parse", "HEAD"]);
        assert_ne!(sub_head, parent_head);

        let lane = LaneGit::resolve(&sub).unwrap();
        assert_eq!(lane.work_dir(), sub.canonicalize().unwrap());
        let output = lane.command().args(["rev-parse", "HEAD"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            sub_head,
            "{output:?}"
        );
        // What re-provisioning asks, without removing the submodule first.
        crate::core::worktree::verify_worktree_head(&sub, &sub_head).unwrap();
        assert_eq!(
            crate::core::worktree::resolve_commit(&sub, "HEAD").unwrap(),
            sub_head
        );
        // And a directory inside it is the submodule too.
        let deeper = sub.join("pkg");
        fs::create_dir_all(&deeper).unwrap();
        assert_eq!(
            crate::core::worktree::resolve_commit(&deeper, "HEAD").unwrap(),
            sub_head
        );
        // The parent is still the parent.
        crate::core::worktree::verify_worktree_head(&world.worktree, &parent_head).unwrap();
    }

    #[test]
    fn a_submodule_the_worker_reconfigured_is_still_refused_when_asked_for_directly() {
        let world = world_with_submodule();
        let config = world.worktree.join("lib/.git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "[core]\n\tfsmonitor = {}\n",
            world.marker("SUB_FSMONITOR").display()
        ));
        fs::write(&config, text).unwrap();
        let error = LaneGit::resolve(&world.worktree.join("lib")).unwrap_err();
        assert!(error.starts_with(NESTED_PREFIX), "{error}");
        world.assert_no_marker();
    }

    #[test]
    fn a_repository_the_worker_made_inside_the_worktree_is_not_pinned_to_the_parent() {
        let world = World::new();
        let nested = world.worktree.join("vendor/pkg");
        fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        // Not a gitlink of the lane: Kronn runs nothing in it.
        let error = LaneGit::resolve(&nested).unwrap_err();
        assert!(error.starts_with(NESTED_PREFIX), "{error}");
        // The worktree itself is unaffected (the embedded repository is refused
        // at staging time, not here).
        assert!(LaneGit::resolve(&world.worktree).is_ok());
    }

    #[test]
    fn a_program_that_starts_git_itself_is_pinned_and_a_redirected_pointer_runs_nothing() {
        let world = World::new();
        // Healthy: the environment pins the registered repository and turns off
        // the file-system monitor and the hooks, as `gh`'s own `git status` sees.
        let mut command = std::process::Command::new("git");
        command
            .args(["config", "--get", "core.fsmonitor"])
            .current_dir(&world.worktree);
        pin_foreign_command(&mut command, &world.worktree);
        let output = command.output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "false");
        let mut command = std::process::Command::new("git");
        command
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(&world.worktree);
        pin_foreign_command(&mut command, &world.worktree);
        let output = command.output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "/dev/null");

        // Redirected: control, then the pinned command.
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        let _ = git_status(&world.worktree, &["status", "--porcelain"]);
        assert!(
            world.marker("FSMONITOR").exists(),
            "a plain `git status` runs the redirected fsmonitor"
        );
        fs::remove_file(world.marker("FSMONITOR")).unwrap();
        let mut command = std::process::Command::new("git");
        command
            .args(["status", "--porcelain"])
            .current_dir(&world.worktree);
        pin_foreign_command(&mut command, &world.worktree);
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{output:?}");
        world.assert_no_marker();
    }

    #[test]
    fn a_directory_under_worktrees_that_git_never_registered_gets_a_clear_message() {
        let world = World::new();
        let stray = world.root().join(".kronn/worktrees/not-a-worktree");
        fs::create_dir_all(&stray).unwrap();
        let error = crate::api::disc_git::git_allowed_in(&stray).unwrap_err();
        assert!(error.contains("does not run git"), "{error}");
        assert!(error.contains("not a worktree registered"), "{error}");
        // A registered worktree and the owner's checkout are fine.
        crate::api::disc_git::git_allowed_in(&world.worktree).unwrap();
        crate::api::disc_git::git_allowed_in(&world.root()).unwrap();
        // A redirected pointer says so too.
        let evil = world.worktree.join("evil-repo");
        world.redirect_pointer_to(&evil);
        let error = crate::api::disc_git::git_allowed_in(&world.worktree).unwrap_err();
        assert!(error.contains(TAMPER_PREFIX), "{error}");
        world.assert_no_marker();
    }

    #[test]
    fn a_commit_the_user_asks_for_in_their_own_checkout_still_runs_their_pre_commit_hook() {
        let world = World::new();
        // The owner's own hook, in the checkout they work in: it keeps running.
        script(
            &world.root().join(".git/hooks/pre-commit"),
            &world.touch("OWNER_PRE_COMMIT"),
        );
        fs::write(world.root().join("owner.txt"), "mine").unwrap();
        crate::api::git_ops::run_git_commit(
            &world.root(),
            &["owner.txt".to_string()],
            "owner commit",
            false,
            false,
        )
        .unwrap();
        assert!(
            world.marker("OWNER_PRE_COMMIT").exists(),
            "the owner's pre-commit hook must run for their own commit"
        );
    }

    #[test]
    fn a_submodule_check_also_runs_from_a_subdirectory_of_the_worktree() {
        let world = world_with_submodule();
        let config = world.worktree.join("lib/.git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str("[core]\n\tfsmonitor = /bin/true\n");
        fs::write(&config, text).unwrap();
        let sub = world.worktree.join("pkg");
        fs::create_dir_all(&sub).unwrap();
        let error = LaneGit::resolve(&sub).unwrap_err();
        assert!(error.starts_with(NESTED_PREFIX), "{error}");
    }
}
