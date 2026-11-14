//! Git hooks of Kronn's own commits and merges for a worker (KT-1745).
//!
//! Kronn commits and merges for a worker itself, in its own process tree: outside
//! the worker's OS sandbox, with the host user's rights and credentials. A
//! repository whose `core.hooksPath` is relative (husky: `.husky`, `.husky/_`)
//! resolves its hooks inside the worktree, which the worker writes — so letting
//! git run them is running worker-authored code unconfined.
//!
//! The rule: git never runs a hook for a Kronn-run command on a worker's
//! worktree. Every such command is built by [`crate::core::lane_git`] (pinned
//! repository, `core.hooksPath=/dev/null`), and the one hook that gates a commit
//! (`pre-commit`, or `pre-merge-commit` for a merge) is run by Kronn itself
//! ([`run_gating_hook`]) according to the project's [`WorkerCommitHooks`] policy,
//! so that a refusal is its own outcome ([`HookRefusal`]) with the hook's output
//! instead of a bare git failure. No other hook runs under any policy: a
//! `commit-msg` check (commitlint) is not run for a worker's commit.
//!
//! The DCO check needs no `commit-msg` hook: Kronn signs every commit off itself
//! (`git commit -s`).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::core::child_env::ChildRoute;
use crate::core::lane_git::LaneGit;
use crate::models::WorkerCommitHooks;

/// Longest a gating hook may run, on the host or confined.
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
/// hook (`project_git_cmd`). Only for a checkout that is not a managed worktree.
fn rev_parse_path(work_dir: &Path, flag: &str) -> Option<PathBuf> {
    // lane-git-allow: an unmanaged checkout (the owner's own), never a lane worktree.
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
    Some(absolute_in(work_dir, PathBuf::from(line)))
}

/// `core.hooksPath` as `dir`'s own configuration sets it, as a path (git has
/// expanded `~`). Plain `git config` runs no hook. `dir` is the registered
/// project checkout for a managed worktree: never read through a worktree.
fn configured_hooks_path(dir: &Path) -> Option<PathBuf> {
    // lane-git-allow: asked of the registered project checkout / an unmanaged checkout.
    crate::core::cmd::git_cmd()
        .args(["config", "--type=path", "--get", "core.hooksPath"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn absolute_in(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// The directory the gating hook of `policy` is looked up in, `None` when the
/// policy runs none.
///
/// For a managed worktree it comes from the REGISTERED project checkout alone
/// (its configuration and its common git directory), never from what the
/// worktree's `.git` pointer leads to:
///
/// - `main_checkout`: the hooks of that checkout, resolved against it — the
///   owner's files, outside every path the worker writes;
/// - `sandboxed`: what the repository configures, resolved against the worktree
///   as git would (possibly the worker's own `.husky`) — run confined.
fn gating_hooks_dir(lane: &LaneGit, policy: WorkerCommitHooks) -> Option<PathBuf> {
    let work_dir = lane.work_dir();
    match (policy, lane.pinned()) {
        (WorkerCommitHooks::Off, _) => None,
        (WorkerCommitHooks::MainCheckout, Some(pinned)) => Some(
            configured_hooks_path(&pinned.repo)
                .map(|path| absolute_in(&pinned.repo, path))
                .unwrap_or_else(|| pinned.common_dir.join("hooks")),
        ),
        (WorkerCommitHooks::Sandboxed, Some(pinned)) => Some(
            configured_hooks_path(&pinned.repo)
                .map(|path| absolute_in(work_dir, path))
                .unwrap_or_else(|| pinned.common_dir.join("hooks")),
        ),
        // Not a managed worktree: the owner's own checkout. A plain checkout is
        // its own main checkout, so nothing under its tree is trusted.
        (WorkerCommitHooks::MainCheckout, None) => {
            let hooks = canonical(&rev_parse_path(work_dir, "--git-common-dir")?).join("hooks");
            (!hooks.starts_with(work_dir)).then_some(hooks)
        }
        (WorkerCommitHooks::Sandboxed, None) => configured_hooks_path(work_dir)
            .map(|path| absolute_in(work_dir, path))
            .or_else(|| rev_parse_path(work_dir, "--git-common-dir").map(|dir| dir.join("hooks"))),
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
/// - [`WorkerCommitHooks::MainCheckout`]: the hook of the registered main
///   checkout, as the host user (it is the owner's own hook, not worker-written).
/// - [`WorkerCommitHooks::Sandboxed`]: the hook the repository configures
///   (possibly inside the worktree), under the lane's OS sandbox profile. A host
///   without a usable sandbox refuses: worker-written code is never run
///   unconfined.
///
/// A worktree whose `.git` pointer was redirected is refused
/// ([`crate::core::lane_git::TAMPER_PREFIX`]) before anything runs.
pub fn run_gating_hook(
    work_dir: &Path,
    policy: WorkerCommitHooks,
    name: &str,
    data_dir_lock: Option<&File>,
) -> Result<(), HookRefusal> {
    if policy == WorkerCommitHooks::Off {
        return Ok(());
    }
    let refuse = |output: String| HookRefusal {
        hook: name.to_string(),
        output,
    };
    let lane = LaneGit::resolve(work_dir).map_err(refuse)?;
    let Some(dir) = gating_hooks_dir(&lane, policy) else {
        return Ok(());
    };
    let hook = dir.join(name);
    if !is_runnable(&hook) {
        return Ok(());
    }
    match policy {
        WorkerCommitHooks::Off => Ok(()),
        WorkerCommitHooks::MainCheckout => {
            run_on_host(&hook, &lane, data_dir_lock).map_err(|output| refuse(first_lines(&output)))
        }
        WorkerCommitHooks::Sandboxed => {
            run_confined(&hook, &lane).map_err(|output| refuse(first_lines(&output)))
        }
    }
}

/// What a hook's own `git` is pinned to: the registered repository.
fn pinned_git_env(lane: &LaneGit) -> Vec<(&'static str, std::ffi::OsString)> {
    match lane.pinned() {
        Some(pinned) => vec![
            ("GIT_DIR", pinned.git_dir.clone().into_os_string()),
            ("GIT_WORK_TREE", lane.work_dir().as_os_str().to_os_string()),
        ],
        None => Vec::new(),
    }
}

/// The hook as the host user, in the worktree, keeping the data-directory lock
/// alive in it and its descendants like the commit it gates, and bounded by
/// [`HOOK_TIMEOUT`] like the confined path.
fn run_on_host(hook: &Path, lane: &LaneGit, data_dir_lock: Option<&File>) -> Result<(), String> {
    let child_lock = data_dir_lock
        .map(crate::core::config::inherit_data_dir_lock_for_child)
        .transpose()
        .map_err(|error| format!("cannot inherit the data-directory lock: {error}"))?;
    let mut command = crate::core::cmd::sync_cmd(hook, ChildRoute::Git);
    command
        .current_dir(lane.work_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in pinned_git_env(lane) {
        command.env(name, value);
    }
    #[cfg(unix)]
    if let Some(child_lock) = child_lock.as_ref() {
        crate::core::config::inherit_data_dir_lock_on_command(&mut command, child_lock);
    }
    #[cfg(windows)]
    if let Some(child_lock) = child_lock.as_ref() {
        crate::core::config::inherit_data_dir_lock_on_command(&mut command, child_lock)
            .map_err(|error| format!("cannot attach the data-directory lock: {error}"))?;
    }
    let result = run_bounded(command, hook, HOOK_TIMEOUT);
    drop(child_lock);
    result
}

/// Run `command` to its end or kill it after `limit`; `Err` carries its output.
fn run_bounded(
    mut command: std::process::Command,
    hook: &Path,
    limit: Duration,
) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", hook.display()))?;
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "{} did not finish within {} s and was stopped",
                    hook.display(),
                    limit.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => break Err(format!("cannot wait for {}: {error}", hook.display())),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    match status? {
        status if status.success() => Ok(()),
        _ => Err(hook_output(&stdout, &stderr)),
    }
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
/// lane's roots (never its own `.git` pointer), and has no network.
fn run_confined(hook: &Path, lane: &LaneGit) -> Result<(), String> {
    use crate::core::task_worker_env::{ProbeCommand, ProbeRunner, ProcessProbeRunner};
    let work_dir = lane.work_dir();
    let confinement = crate::core::task_worker_sandbox::Confinement::for_worktree(work_dir)
        .map_err(|reason| format!("not run: {reason}"))?;
    let probe = ProbeCommand {
        program: hook.to_string_lossy().into_owned(),
        args: Vec::new(),
        cwd: work_dir.to_path_buf(),
        env: pinned_git_env(lane)
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
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
