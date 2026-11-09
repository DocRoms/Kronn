//! KT-1645 — an agent never moves the owner's main checkout.
//!
//! A reviewer's subagent ran `git checkout --detach` in the owner's main
//! checkout, then restored HEAD by hand. Nothing was lost that time, but any
//! agent whose working directory is (or can reach) a project's main checkout can
//! move its HEAD or stash its uncommitted files.
//!
//! Every CLI agent launch (`agents::runner::try_spawn`) gets a `git` shim first
//! on its `PATH` ([`apply`]). The shim (`git_guard.sh`, one implementation, run
//! by the tests below) refuses `git checkout`, `git switch`, `git reset` and
//! `git stash` (except `stash list` / `stash show`) when the repository it runs
//! in is a main checkout, with a message that tells the agent what to do
//! instead. A linked worktree, which is the agent's own checkout, and every
//! other git command go straight to the real git.
//!
//! The human's confirmation is [`CONFIRM_ENV`] in the launch environment: Kronn
//! sets it for a launch the human confirmed, and nothing sets it today.
//!
//! This is a safety net against accidents, not a sandbox: an absolute path to
//! git, or a git alias, is not seen. Windows agents get no shim.

use std::path::{Path, PathBuf};

/// The git guard script, written as `git` into the launch's shim directory.
pub const SHIM: &str = include_str!("git_guard.sh");

/// Set to `1` in an agent's environment when the human confirmed that this
/// launch may move the main checkout: the shim then lets the command through.
pub const CONFIRM_ENV: &str = "KRONN_MAIN_CHECKOUT_GIT_CONFIRMED";

/// Where the shim lives for a launch in `work_dir`. Inside the working
/// directory (git-ignored like `.kronn/tmp`): owned by the user, readable by
/// the agent's sandbox, which the Kronn data directory is not.
pub fn shim_dir(work_dir: &Path) -> PathBuf {
    work_dir.join(".kronn").join("bin")
}

/// Write the shim (executable) into [`shim_dir`] and return that directory.
/// Idempotent, and safe against concurrent launches: the script is written to a
/// private name, then renamed over `git`.
#[cfg(unix)]
pub fn install(work_dir: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let dir = shim_dir(work_dir);
    let target = dir.join("git");
    if std::fs::read_to_string(&target).is_ok_and(|current| current == SHIM)
        && std::fs::metadata(&target).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    let staging = dir.join(format!(".git.{}.tmp", std::process::id()));
    std::fs::write(&staging, SHIM)?;
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&staging, &target)?;
    Ok(dir)
}

/// `PATH` with `dir` in front.
#[cfg(unix)]
fn path_with_front(dir: &Path, path: Option<&std::ffi::OsStr>) -> std::ffi::OsString {
    let mut entries = vec![dir.to_path_buf()];
    if let Some(path) = path {
        entries.extend(std::env::split_paths(path));
    }
    std::env::join_paths(entries).unwrap_or_else(|_| dir.as_os_str().to_os_string())
}

/// Put the guard in front of `git` for an agent about to start in `work_dir`.
/// Best effort: a directory Kronn cannot write leaves the launch as it was.
///
/// The shim is a safety net against accidents, not a boundary: the agent runs
/// as the same user, so it can call git by absolute path, use an alias, or set
/// [`CONFIRM_ENV`] itself — the variable is read from the agent's own
/// environment, and nothing here can tell a human's confirmation from the
/// agent's. What it stops is the careless `git checkout --detach`.
///
/// The shim lives in the checkout (an agent's sandbox can read it there, not in
/// Kronn's data directory), so `.kronn/bin/` is added to the repository's
/// `.git/info/exclude`: the user's `git status` stays clean without a line in
/// their tracked `.gitignore`.
#[cfg(unix)]
pub fn apply(command: &mut std::process::Command, work_dir: &Path) {
    let dir = match install(work_dir) {
        Ok(dir) => dir,
        Err(error) => {
            tracing::warn!("git guard not installed in {}: {error}", work_dir.display());
            return;
        }
    };
    if let Some(path) = work_dir.to_str() {
        crate::core::mcp_scanner::ensure_local_exclude(path, ".kronn/bin/");
    }
    let current = command
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value.map(|value| value.to_os_string()))
        .or_else(|| crate::core::child_env::var_os("PATH"));
    command.env("PATH", path_with_front(&dir, current.as_deref()));
}

#[cfg(not(unix))]
pub fn apply(_command: &mut std::process::Command, _work_dir: &Path) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Output};

    fn git(dir: &Path, args: &[&str]) -> Output {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    struct Fixture {
        _root: tempfile::TempDir,
        main: PathBuf,
        linked: PathBuf,
        shims: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let main = base.join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "one").unwrap();
        git(&main, &["add", "a.txt"]);
        git(&main, &["commit", "-q", "-m", "one"]);
        let linked = base.join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
            ],
        );
        let shims = install(&base).unwrap();
        Fixture {
            _root: root,
            main,
            linked,
            shims,
        }
    }

    /// Run `git <args>` through the shim from `dir`, the way an agent would.
    fn guarded(fx: &Fixture, dir: &Path, args: &[&str], confirmed: bool) -> Output {
        let mut command = Command::new(fx.shims.join("git"));
        command
            .args(args)
            .current_dir(dir)
            .env(
                "PATH",
                path_with_front(&fx.shims, std::env::var_os("PATH").as_deref()),
            )
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env_remove(CONFIRM_ENV);
        if confirmed {
            command.env(CONFIRM_ENV, "1");
        }
        command.output().expect("shim runs")
    }

    fn head(dir: &Path) -> String {
        String::from_utf8_lossy(&git(dir, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string()
    }

    fn symbolic_head(dir: &Path) -> String {
        let out = Command::new("git")
            .args(["symbolic-ref", "-q", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn refused(out: &Output) -> String {
        assert!(!out.status.success(), "the command must be refused");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    #[test]
    fn checkout_detach_in_the_main_checkout_is_refused_with_a_clear_message() {
        let fx = fixture();
        let before = head(&fx.main);

        let out = guarded(&fx, &fx.main, &["checkout", "--detach"], false);

        let message = refused(&out);
        assert!(message.contains("refused 'git checkout'"), "{message}");
        assert!(message.contains("main checkout"), "{message}");
        assert!(message.contains("worktree"), "{message}");
        assert_eq!(
            symbolic_head(&fx.main),
            "refs/heads/main",
            "HEAD must not move"
        );
        assert_eq!(head(&fx.main), before);
    }

    #[test]
    fn switch_reset_and_stash_are_refused_in_the_main_checkout() {
        let fx = fixture();
        std::fs::write(fx.main.join("a.txt"), "uncommitted").unwrap();

        for args in [
            vec!["switch", "-c", "other"],
            vec!["reset", "--hard"],
            vec!["reset"],
            vec!["stash"],
            vec!["stash", "push", "-u"],
            vec!["stash", "pop"],
            vec!["checkout", "--", "a.txt"],
        ] {
            let out = guarded(&fx, &fx.main, &args, false);
            let message = refused(&out);
            assert!(
                message.contains(&format!("'git {}'", args[0])),
                "{args:?}: {message}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(fx.main.join("a.txt")).unwrap(),
            "uncommitted",
            "the owner's uncommitted work must survive"
        );
        assert_eq!(symbolic_head(&fx.main), "refs/heads/main");
    }

    #[test]
    fn global_options_and_subdirectories_do_not_hide_the_subcommand() {
        let fx = fixture();
        std::fs::create_dir_all(fx.main.join("sub")).unwrap();
        let main = fx.main.to_str().unwrap();

        for (dir, args) in [
            (fx.main.clone(), vec!["-C", ".", "checkout", "--detach"]),
            (fx.linked.clone(), vec!["-C", main, "checkout", "--detach"]),
            (
                fx.main.clone(),
                vec!["-c", "color.ui=never", "--no-pager", "reset", "--hard"],
            ),
            (fx.main.join("sub"), vec!["checkout", "--detach"]),
        ] {
            let out = guarded(&fx, &dir, &args, false);
            refused(&out);
            assert_eq!(symbolic_head(&fx.main), "refs/heads/main", "{args:?}");
        }
    }

    #[test]
    fn a_linked_worktree_is_not_guarded() {
        let fx = fixture();

        let out = guarded(
            &fx,
            &fx.linked,
            &["checkout", "-q", "-b", "agent-work"],
            false,
        );

        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(symbolic_head(&fx.linked), "refs/heads/agent-work");
        assert_eq!(symbolic_head(&fx.main), "refs/heads/main");
    }

    #[test]
    fn read_only_and_unrelated_commands_pass_through_in_the_main_checkout() {
        let fx = fixture();

        for args in [
            vec!["status", "--short"],
            vec!["log", "--oneline"],
            vec!["diff"],
            vec!["stash", "list"],
            vec!["branch", "--show-current"],
            vec!["add", "a.txt"],
        ] {
            let out = guarded(&fx, &fx.main, &args, false);
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn the_humans_confirmation_lets_the_command_through() {
        let fx = fixture();

        let out = guarded(&fx, &fx.main, &["checkout", "-q", "--detach"], true);

        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            symbolic_head(&fx.main),
            "",
            "HEAD is detached once confirmed"
        );
    }

    #[test]
    fn outside_a_repository_the_real_git_answers() {
        let fx = fixture();
        let elsewhere = tempfile::tempdir().unwrap();

        let out = guarded(&fx, elsewhere.path(), &["checkout", "main"], false);

        let message = refused(&out);
        assert!(message.contains("not a git repository"), "{message}");
        assert!(!message.contains("kronn: refused"), "{message}");
    }

    #[test]
    fn apply_puts_the_shim_first_on_the_launch_path_and_keeps_the_rest() {
        let work = tempfile::tempdir().unwrap();
        let mut command = Command::new("true");
        command.env("PATH", "/usr/bin:/bin");

        apply(&mut command, work.path());

        let path = command
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value.map(|v| v.to_os_string()))
            .unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(entries[0], shim_dir(work.path()));
        assert_eq!(
            entries[1..],
            [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
        assert!(shim_dir(work.path()).join("git").is_file());
    }

    /// Run `args` through `path` (shims first), killing the process after a few
    /// seconds: two guards that hand a command to each other never return.
    fn run_through(path: &std::ffi::OsStr, dir: &Path, args: &[&str]) -> Option<Output> {
        let first = std::env::split_paths(path)
            .next()
            .expect("a shim directory comes first");
        let mut child = Command::new(first.join("git"))
            .args(args)
            .current_dir(dir)
            .env("PATH", path)
            .env_remove(CONFIRM_ENV)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("shim runs");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                return Some(child.wait_with_output().unwrap());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        None
    }

    #[test]
    fn two_stacked_shims_look_past_each_other_to_the_real_git() {
        let fx = fixture();
        let other_work = tempfile::tempdir().unwrap();
        let outer = install(other_work.path()).unwrap();
        // An agent in worktree A launched another in worktree B: B's shim is
        // first, A's second, the real git last.
        let path = std::env::join_paths([outer.clone(), fx.shims.clone()].into_iter().chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();

        let status = run_through(&path, &fx.main, &["status", "--short"])
            .expect("stacked shims must not loop");
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );

        let refused_checkout = run_through(&path, &fx.main, &["checkout", "--detach"])
            .expect("stacked shims must not loop");
        let message = refused(&refused_checkout);
        assert!(message.contains("refused 'git checkout'"), "{message}");
        assert_eq!(symbolic_head(&fx.main), "refs/heads/main");

        // And the other way round.
        let reversed = std::env::join_paths([fx.shims.clone(), outer].into_iter().chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let again = run_through(&reversed, &fx.main, &["reset", "--hard"])
            .expect("stacked shims must not loop");
        refused(&again);
    }

    #[test]
    fn a_shim_alone_on_the_path_reports_that_no_real_git_exists() {
        let fx = fixture();

        let out = run_through(fx.shims.as_os_str(), &fx.main, &["status"])
            .expect("a lone shim must not loop");

        assert_eq!(out.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&out.stderr).contains("no other git on PATH"));
    }

    #[test]
    fn a_launch_leaves_the_projects_git_status_clean() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().canonicalize().unwrap();
        git(&project, &["init", "-q", "-b", "main"]);
        std::fs::write(project.join("a.txt"), "one").unwrap();
        git(&project, &["add", "a.txt"]);
        git(&project, &["commit", "-q", "-m", "one"]);
        let mut command = Command::new("true");

        apply(&mut command, &project);
        apply(&mut command, &project);

        assert!(
            shim_dir(&project).join("git").is_file(),
            "the shim is there"
        );
        let status = git(
            &project,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        assert_eq!(
            String::from_utf8_lossy(&status.stdout).trim(),
            "",
            "an untracked .kronn/bin/git would dirty the user's checkout"
        );
    }

    #[test]
    fn a_launch_in_a_linked_worktree_leaves_it_clean_too() {
        let fx = fixture();
        let mut command = Command::new("true");

        apply(&mut command, &fx.linked);

        assert!(shim_dir(&fx.linked).join("git").is_file());
        let status = git(
            &fx.linked,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");
    }

    #[test]
    fn install_is_idempotent_and_repairs_a_stale_shim() {
        let work = tempfile::tempdir().unwrap();
        let dir = install(work.path()).unwrap();
        std::fs::write(dir.join("git"), "#!/bin/sh\nexit 0\n").unwrap();

        install(work.path()).unwrap();
        install(work.path()).unwrap();

        assert_eq!(std::fs::read_to_string(dir.join("git")).unwrap(), SHIM);
    }
}
