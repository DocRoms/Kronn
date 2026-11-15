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

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::lane_git::LaneGit;
use crate::models::WorkerCommitHooks;

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
/// worktree's `.git` pointer leads to. It is resolved against the worktree as git
/// would (possibly the worker's own `.husky`): the hook then runs confined, never
/// as the host user.
fn gating_hooks_dir(lane: &LaneGit, policy: WorkerCommitHooks) -> Option<PathBuf> {
    let work_dir = lane.work_dir();
    match (policy, lane.pinned()) {
        (WorkerCommitHooks::Off, _) => None,
        (WorkerCommitHooks::Sandboxed, Some(pinned)) => Some(
            configured_hooks_path(&pinned.repo)
                .map(|path| absolute_in(work_dir, path))
                .unwrap_or_else(|| pinned.common_dir.join("hooks")),
        ),
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
/// - [`WorkerCommitHooks::Sandboxed`]: the hook the repository configures
///   (possibly inside the worktree), under the lane's OS sandbox profile. A host
///   without a usable sandbox refuses. A hook is never run unconfined, not even
///   the owner's own: a real hook executes worktree content
///   (`PATH=node_modules/.bin:$PATH`, lint-staged, make).
///
/// A worktree whose `.git` pointer was redirected is refused
/// ([`crate::core::lane_git::TAMPER_PREFIX`]) before anything runs.
pub fn run_gating_hook(
    work_dir: &Path,
    policy: WorkerCommitHooks,
    name: &str,
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
    run_confined(&hook, &lane).map_err(|output| refuse(first_lines(&output)))
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

/// How the confined hook is started (`sh -c <this> <hook>`): both of its streams
/// are kept, in order, bounded to their last 8 KiB, and handed back on STDOUT with
/// the line breaks written as [`LINE_BREAK`]: the probe runner reports one stream
/// of a failure (stderr first), with its whitespace collapsed, which would drop a
/// hook's lint errors printed on the other stream and flatten the rest. The
/// hook's exit status is the wrapper's.
const HOOK_WRAPPER: &str =
    "nocapture=; out=$(mktemp \"${TMPDIR:-/tmp}/kronn-hook.XXXXXX\" 2>/dev/null) || { out=/dev/null; nocapture=1; }; \
\"$0\" >\"$out\" 2>&1; code=$?; \
tail -c 8192 \"$out\" 2>/dev/null | tr '\\n' '\\036'; \
[ -z \"$nocapture\" ] || printf 'kronn could not capture the hook output (mktemp failed in %s)' \"${TMPDIR:-/tmp}\"; \
[ -n \"$nocapture\" ] || rm -f \"$out\" 2>/dev/null; exit $code";

/// What stands for a line break in a confined hook's output (ASCII record
/// separator: not whitespace to Rust, so the runner's collapsing keeps it).
const LINE_BREAK: char = '\u{1e}';

/// The output of [`HOOK_WRAPPER`] as the hook printed it, line breaks restored.
fn decode_hook_output(detail: &str) -> String {
    detail.replace(LINE_BREAK, "\n").trim().to_string()
}

/// The hook under the lane's OS sandbox: it reads the filesystem, writes only the
/// lane's roots (never its own `.git` pointer), and has no network.
fn run_confined(hook: &Path, lane: &LaneGit) -> Result<(), String> {
    use crate::core::task_worker_env::{ProbeCommand, ProbeRunner, ProcessProbeRunner};
    let work_dir = lane.work_dir();
    let confinement = crate::core::task_worker_sandbox::Confinement::for_worktree(work_dir)
        .map_err(|reason| format!("not run: {reason}"))?;
    let probe = ProbeCommand {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            HOOK_WRAPPER.to_string(),
            hook.to_string_lossy().into_owned(),
        ],
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
        Err(decode_hook_output(&outcome.detail))
    }
}

#[cfg(test)]
#[path = "commit_hooks_test.rs"]
mod commit_hooks_test;
