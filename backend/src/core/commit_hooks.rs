//! Git hooks of Kronn's own commits and merges for a worker (KT-1745).
//!
//! Kronn commits and merges for a worker itself, in its own process tree: outside
//! the worker's OS sandbox, with the host user's rights and credentials. A
//! repository whose `core.hooksPath` is relative (husky: `.husky`, `.husky/_`)
//! resolves its hooks inside the worktree, which the worker writes — so letting
//! git run them is running worker-authored code unconfined.
//!
//! The rule: git never resolves a hook inside the worktree for such a command.
//! Every Kronn-run git command of a worker lane carries
//! `-c core.hooksPath=<absolute dir outside the worktree, or /dev/null>`
//! ([`git_config_args`]), and the one hook that gates a commit (`pre-commit`, or
//! `pre-merge-commit` for a merge) is run by Kronn itself ([`run_gating_hook`])
//! according to the project's [`WorkerCommitHooks`] policy, so that a refusal is
//! its own outcome ([`HookRefusal`]) with the hook's output instead of a bare
//! git failure.
//!
//! The DCO check needs no `commit-msg` hook: Kronn signs every commit off itself
//! (`git commit -s`).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::core::child_env::ChildRoute;
use crate::models::WorkerCommitHooks;

/// What git is given for `core.hooksPath` when no hook may run.
const NO_HOOKS: &str = "/dev/null";
/// Longest a gating hook may run.
const HOOK_TIMEOUT: Duration = Duration::from_secs(600);
/// Lines of a refused hook's output that travel with the refusal.
const REFUSAL_LINES: usize = 20;
/// Characters of a refused hook's output that travel with the refusal.
const REFUSAL_CHARS: usize = 2_000;

/// A gating hook that refused the commit or merge, or could not be run safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRefusal {
    /// The hook's name (`pre-commit`).
    pub hook: String,
    /// The first lines of what it printed, or why it was not run.
    pub output: String,
}

impl std::fmt::Display for HookRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{HOOK_REFUSED_PREFIX} ({}): {}",
            self.hook,
            if self.output.is_empty() {
                "it exited with a failure and printed nothing"
            } else {
                self.output.as_str()
            }
        )
    }
}

/// Prefix of the error string a refused hook becomes where a commit reports its
/// failure as text (`task_exec_commit`).
pub const HOOK_REFUSED_PREFIX: &str = "HookRefused";

/// The first lines of a hook's output, bounded.
pub fn first_lines(output: &str) -> String {
    let mut text: String = output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .take(REFUSAL_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if text.chars().count() > REFUSAL_CHARS {
        text = text.chars().take(REFUSAL_CHARS).collect();
        text.push('…');
    }
    text
}

fn canonical(path: &Path) -> PathBuf {
    crate::core::task_worker_sandbox::real_path(path)
}

/// `git rev-parse <flag>` in `work_dir`, as an absolute path. Reading it runs no
/// hook (`project_git_cmd`).
fn rev_parse_path(work_dir: &Path, flag: &str) -> Option<PathBuf> {
    let output = crate::core::cmd::project_git_cmd()
        .args(["rev-parse", flag])
        .current_dir(work_dir)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let path = PathBuf::from(line);
    Some(if path.is_absolute() {
        path
    } else {
        work_dir.join(path)
    })
}

/// The hooks directory of the main checkout: `<git-common-dir>/hooks`, absolute.
/// `None` when it is not known or lies inside `work_dir` (a plain checkout is
/// its own main checkout: nothing under the tree a worker writes is trusted).
fn main_checkout_hooks(work_dir: &Path) -> Option<PathBuf> {
    let common = rev_parse_path(work_dir, "--git-common-dir")?;
    let hooks = canonical(&common).join("hooks");
    (!hooks.starts_with(canonical(work_dir))).then_some(hooks)
}

/// The `core.hooksPath` a Kronn-run git command of a worker lane gets: an
/// absolute directory that is never under the worktree, or `/dev/null`.
pub fn hooks_path_for_git(work_dir: &Path, policy: WorkerCommitHooks) -> PathBuf {
    match policy {
        // Kronn runs the gating hook itself for the other policies; git runs
        // none: the directory is only what is left for the passive hooks, and
        // those come from the main checkout alone.
        WorkerCommitHooks::MainCheckout => {
            main_checkout_hooks(work_dir).unwrap_or_else(|| PathBuf::from(NO_HOOKS))
        }
        WorkerCommitHooks::Off | WorkerCommitHooks::Sandboxed => PathBuf::from(NO_HOOKS),
    }
}

/// The `-c` arguments that go in front of a Kronn-run `git` subcommand of a
/// worker lane. Also turns the file-system monitor off (a program named by the
/// repository's configuration).
pub fn git_config_args(work_dir: &Path, policy: WorkerCommitHooks) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!(
            "core.hooksPath={}",
            hooks_path_for_git(work_dir, policy).display()
        ),
        "-c".to_string(),
        "core.fsmonitor=false".to_string(),
    ]
}

/// A `git` command for a worker lane, with [`git_config_args`] in front.
pub fn worker_git_cmd(work_dir: &Path, policy: WorkerCommitHooks) -> std::process::Command {
    let mut command = crate::core::cmd::git_cmd();
    command.args(git_config_args(work_dir, policy));
    command
}

/// The directory the gating hook of `policy` is looked up in, `None` when the
/// policy runs none.
fn gating_hooks_dir(work_dir: &Path, policy: WorkerCommitHooks) -> Option<PathBuf> {
    match policy {
        WorkerCommitHooks::Off => None,
        WorkerCommitHooks::MainCheckout => main_checkout_hooks(work_dir),
        WorkerCommitHooks::Sandboxed => {
            // Plain `git config` runs no hook; `project_git_cmd` would override
            // the very value read here.
            let configured = crate::core::cmd::git_cmd()
                .args(["config", "--type=path", "--get", "core.hooksPath"])
                .current_dir(work_dir)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .filter(|value| !value.is_empty());
            match configured {
                // Git resolves a relative value against the directory it runs the
                // hook from: the worktree.
                Some(value) => {
                    let path = PathBuf::from(value);
                    Some(if path.is_absolute() {
                        path
                    } else {
                        work_dir.join(path)
                    })
                }
                None => rev_parse_path(work_dir, "--git-common-dir").map(|c| c.join("hooks")),
            }
        }
    }
}

fn is_runnable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Run the gating hook `name` (`pre-commit`, `pre-merge-commit`) of `work_dir`
/// the way `policy` allows, before Kronn's commit. `Ok` when there is no hook to
/// run or it passed.
///
/// - [`WorkerCommitHooks::Off`]: nothing runs.
/// - [`WorkerCommitHooks::MainCheckout`]: the hook of the main checkout's common
///   git directory, as the host user (it is the owner's own hook, not
///   worker-written).
/// - [`WorkerCommitHooks::Sandboxed`]: the hook the repository configures
///   (possibly inside the worktree), under the lane's OS sandbox profile. A host
///   without a usable sandbox refuses: worker-written code is never run
///   unconfined.
pub fn run_gating_hook(
    work_dir: &Path,
    policy: WorkerCommitHooks,
    name: &str,
    data_dir_lock: Option<&File>,
) -> Result<(), HookRefusal> {
    let Some(dir) = gating_hooks_dir(work_dir, policy) else {
        return Ok(());
    };
    let hook = dir.join(name);
    if !is_runnable(&hook) {
        return Ok(());
    }
    let refuse = |output: String| HookRefusal {
        hook: name.to_string(),
        output,
    };
    match policy {
        WorkerCommitHooks::Off => Ok(()),
        WorkerCommitHooks::MainCheckout => run_on_host(&hook, work_dir, data_dir_lock)
            .map_err(|output| refuse(first_lines(&output))),
        WorkerCommitHooks::Sandboxed => {
            run_confined(&hook, work_dir).map_err(|output| refuse(first_lines(&output)))
        }
    }
}

/// The hook as the host user, in `work_dir`, keeping the data-directory lock
/// alive in it and its descendants like the commit it gates.
fn run_on_host(hook: &Path, work_dir: &Path, data_dir_lock: Option<&File>) -> Result<(), String> {
    let child_lock = data_dir_lock
        .map(crate::core::config::inherit_data_dir_lock_for_child)
        .transpose()
        .map_err(|error| format!("cannot inherit the data-directory lock: {error}"))?;
    let mut command = crate::core::cmd::sync_cmd(hook, ChildRoute::Git);
    command
        .current_dir(work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    if let Some(child_lock) = child_lock.as_ref() {
        crate::core::config::inherit_data_dir_lock_on_command(&mut command, child_lock);
    }
    #[cfg(windows)]
    if let Some(child_lock) = child_lock.as_ref() {
        crate::core::config::inherit_data_dir_lock_on_command(&mut command, child_lock)
            .map_err(|error| format!("cannot attach the data-directory lock: {error}"))?;
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot start {}: {error}", hook.display()))?;
    drop(child_lock);
    if output.status.success() {
        return Ok(());
    }
    Err(hook_output(&output.stdout, &output.stderr))
}

fn hook_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(stderr).into_owned();
    let out = String::from_utf8_lossy(stdout);
    if !out.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&out);
    }
    text
}

/// The hook under the lane's OS sandbox: it reads the filesystem, writes only the
/// lane's roots, and has no network.
fn run_confined(hook: &Path, work_dir: &Path) -> Result<(), String> {
    use crate::core::task_worker_env::{ProbeCommand, ProbeRunner, ProcessProbeRunner};
    let confinement = crate::core::task_worker_sandbox::Confinement::for_worktree(work_dir)
        .map_err(|reason| format!("not run: {reason}"))?;
    let probe = ProbeCommand {
        program: hook.to_string_lossy().into_owned(),
        args: Vec::new(),
        cwd: work_dir.to_path_buf(),
        env: Vec::new(),
        timeout: HOOK_TIMEOUT,
        priority: crate::core::background_priority::Priority::Normal,
        clean_env: false,
    };
    let outcome = ProcessProbeRunner.run(&confinement.wrap(&probe));
    if outcome.success {
        Ok(())
    } else {
        Err(outcome.detail)
    }
}

#[cfg(test)]
#[path = "commit_hooks_test.rs"]
mod commit_hooks_test;
