//! Git worktree workspace management for workflow runs.
//!
//! Each workflow run gets an isolated git worktree so changes don't
//! interfere with the main working tree. Lifecycle hooks are executed
//! at each stage.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::core::cmd::async_cmd;
use crate::models::WorkspaceHooks;

/// TD-20260709 (A) — exclusivity of a project's MAIN checkout across
/// non-isolated runs: two of them cross-contaminate `.kronn/` machine files
/// and each other's edits. Isolated (worktree) runs never take this lock.
pub struct MainTreeGuard {
    key: String,
    ticket: u64,
}

/// Owner of one checkout plus the runs queued behind it, in arrival order.
struct MainTreeSlot {
    holder_ticket: u64,
    holder_run: String,
    waiters: std::collections::VecDeque<MainTreeWaiter>,
}

struct MainTreeWaiter {
    ticket: u64,
    run_id: String,
    wake: tokio::sync::oneshot::Sender<()>,
}

/// Why a run did not get the main checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainTreeRefusal {
    /// `holder` still owned the checkout when the bounded wait ran out.
    Busy {
        holder: String,
        waited: std::time::Duration,
    },
    /// An ancestor run holds it and is waiting on this run: waiting cannot help.
    HeldByAncestor {
        holder: String,
    },
    Cancelled,
}

/// Seconds a non-isolated run waits for the main checkout before refusing.
pub const MAIN_TREE_WAIT_ENV: &str = "KRONN_MAIN_TREE_WAIT_SECS";
pub const DEFAULT_MAIN_TREE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

pub fn main_tree_wait_from(raw: Option<&str>) -> std::time::Duration {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(DEFAULT_MAIN_TREE_WAIT)
}

#[cfg(test)]
tokio::task_local! {
    /// Per-test wait: the env var is process-wide and tests run in parallel.
    pub(crate) static MAIN_TREE_WAIT_FOR_TESTS: std::time::Duration;
}

pub fn main_tree_wait() -> std::time::Duration {
    #[cfg(test)]
    if let Ok(wait) = MAIN_TREE_WAIT_FOR_TESTS.try_with(|wait| *wait) {
        return wait;
    }
    main_tree_wait_from(std::env::var(MAIN_TREE_WAIT_ENV).ok().as_deref())
}

type MainTreeLocks = std::sync::Mutex<std::collections::HashMap<String, MainTreeSlot>>;

fn main_tree_locks(
) -> std::sync::MutexGuard<'static, std::collections::HashMap<String, MainTreeSlot>> {
    static LOCKS: std::sync::OnceLock<MainTreeLocks> = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Canonical key: two spellings of the same checkout (symlink, relative)
/// must not bypass the lock.
fn main_tree_key(project_path: &str) -> String {
    std::fs::canonicalize(project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| project_path.to_string())
}

fn next_main_tree_ticket() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl MainTreeGuard {
    fn claim_free(
        locks: &mut std::collections::HashMap<String, MainTreeSlot>,
        key: String,
        run_id: &str,
    ) -> Self {
        let ticket = next_main_tree_ticket();
        locks.insert(
            key.clone(),
            MainTreeSlot {
                holder_ticket: ticket,
                holder_run: run_id.to_string(),
                waiters: Default::default(),
            },
        );
        Self { key, ticket }
    }

    /// Immediate attempt: `Err(holder run id)` when the checkout is taken.
    pub fn try_acquire(project_path: &str, run_id: &str) -> Result<Self, String> {
        let key = main_tree_key(project_path);
        let mut locks = main_tree_locks();
        if let Some(slot) = locks.get(&key) {
            return Err(slot.holder_run.clone());
        }
        Ok(Self::claim_free(&mut locks, key, run_id))
    }

    /// Queues behind the current holder (FIFO per checkout) for at most
    /// `wait`; the release hands the checkout straight to the next waiter.
    pub async fn acquire(
        project_path: &str,
        run_id: &str,
        ancestors: &[String],
        wait: std::time::Duration,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Self, MainTreeRefusal> {
        let key = main_tree_key(project_path);
        let ticket = next_main_tree_ticket();
        let mut woken = {
            let mut locks = main_tree_locks();
            let Some(slot) = locks.get_mut(&key) else {
                return Ok(Self::claim_free(&mut locks, key, run_id));
            };
            if ancestors.contains(&slot.holder_run) {
                return Err(MainTreeRefusal::HeldByAncestor {
                    holder: slot.holder_run.clone(),
                });
            }
            let (wake, woken) = tokio::sync::oneshot::channel();
            slot.waiters.push_back(MainTreeWaiter {
                ticket,
                run_id: run_id.to_string(),
                wake,
            });
            woken
        };
        let cancelled = tokio::select! {
            biased;
            _ = cancel.cancelled() => true,
            _ = &mut woken => false,
            _ = tokio::time::sleep(wait) => false,
        };
        // Settle under the lock: a hand-off may have raced the timeout or
        // the cancellation, in which case this run already owns the tree.
        let mut locks = main_tree_locks();
        match locks.get_mut(&key) {
            Some(slot) if slot.holder_ticket == ticket => {}
            Some(slot) => {
                slot.waiters.retain(|waiter| waiter.ticket != ticket);
                if cancelled {
                    return Err(MainTreeRefusal::Cancelled);
                }
                return Err(MainTreeRefusal::Busy {
                    holder: slot.holder_run.clone(),
                    waited: wait,
                });
            }
            None if cancelled => return Err(MainTreeRefusal::Cancelled),
            None => return Ok(Self::claim_free(&mut locks, key, run_id)),
        }
        drop(locks);
        let guard = Self { key, ticket };
        if cancelled {
            drop(guard);
            return Err(MainTreeRefusal::Cancelled);
        }
        Ok(guard)
    }
}

impl Drop for MainTreeGuard {
    fn drop(&mut self) {
        let mut locks = main_tree_locks();
        let Some(slot) = locks.get_mut(&self.key) else {
            return;
        };
        if slot.holder_ticket != self.ticket {
            return;
        }
        // A waiter that gave up has dropped its receiver: skip it.
        while let Some(next) = slot.waiters.pop_front() {
            slot.holder_ticket = next.ticket;
            slot.holder_run = next.run_id;
            if next.wake.send(()).is_ok() {
                return;
            }
        }
        locks.remove(&self.key);
    }
}

/// An active workspace (git worktree) for a workflow run.
pub struct Workspace {
    /// Path to the worktree directory
    pub path: PathBuf,
    /// Branch name created for this worktree
    pub branch: String,
    /// The main repo path (for cleanup)
    repo_path: PathBuf,
    /// Optional lifecycle hooks
    hooks: Option<WorkspaceHooks>,
}

/// Snapshot of a branch that the runner kept alive instead of deleting on
/// cleanup. Returned by `Workspace::cleanup` so the caller can persist the
/// info on the run row and surface it in the UI ("commit produit ici").
///
/// Without this, agents that committed locally but failed to push (e.g.
/// pre-push hook blocked, network down, no auth) lose visibility entirely:
/// the worktree gets removed, the branch gets deleted, and the commits
/// drift into git's dangling-object pool until the next `gc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedBranch {
    /// The kept-alive branch name in the parent repo (e.g. `kronn/Autobot/68dccb12`).
    pub branch_name: String,
    /// HEAD SHA at cleanup time. Lets the caller render the commit and
    /// recover even if the branch is later deleted.
    pub head_sha: String,
    /// Commits ahead of the chosen base (upstream / origin/main / main).
    pub ahead: u32,
    /// True if the branch had an upstream tracking ref — i.e. the agent
    /// at least *attempted* to push (and may have partially succeeded).
    /// False = no push was ever attempted.
    pub pushed_upstream: bool,
}

/// Outcome of `Workspace::cleanup`. The branch field is `Some` whenever
/// the worktree's HEAD held local commits not present on a known base —
/// the runner records it on `WorkflowRun.produced_branches` so the run
/// detail UI can show "commit produit, push bloqué — branche `X` preservée".
#[derive(Debug, Clone, Default)]
pub struct CleanupOutcome {
    pub preserved: Option<PreservedBranch>,
}

/// Sanitize a workflow name for use in branch names and directory paths.
/// Keeps alphanumeric, dash, and underscore; replaces everything else with `-`.
pub(crate) fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Decide whether the branch backing this worktree should outlive cleanup.
///
/// Returns `Some(PreservedBranch)` when HEAD holds commits not on any known
/// base ref (upstream → origin/main → origin/master → main → master). Returns
/// `None` only when HEAD is fully synced with one of those bases (nothing to
/// salvage). On total failure (no base ref found, or git errors) we return
/// `Some` with `ahead=0` to err on the side of preservation — losing real
/// work is much worse than leaving a stale empty branch behind.
async fn check_branch_for_preservation(worktree: &Path, branch: &str) -> Option<PreservedBranch> {
    // HEAD sha is the anchor we want to be able to recover via the branch.
    let head_sha = git_text_output(worktree, &["rev-parse", "HEAD"]).await?;

    // Was an upstream set on the worktree's branch ? Tells us whether the
    // agent tried to push at all. `@{u}` resolves only when set.
    let pushed_upstream = git_text_output(worktree, &["rev-parse", "--abbrev-ref", "@{u}"])
        .await
        .is_some();

    // Walk through plausible bases in order of relevance. The first one
    // that resolves wins — count commits ahead.
    let mut bases: Vec<&str> = Vec::with_capacity(5);
    if pushed_upstream {
        bases.push("@{u}");
    }
    bases.extend(["origin/main", "origin/master", "main", "master"]);

    for base in bases {
        let count_str = git_text_output(
            worktree,
            &["rev-list", "--count", &format!("{}..HEAD", base)],
        )
        .await;
        if let Some(count_str) = count_str {
            let ahead: u32 = count_str.trim().parse().unwrap_or(0);
            if ahead == 0 {
                // Synced — drop the branch.
                return None;
            }
            return Some(PreservedBranch {
                branch_name: branch.to_string(),
                head_sha,
                ahead,
                pushed_upstream,
            });
        }
    }

    // No base resolved — preserve defensively.
    Some(PreservedBranch {
        branch_name: branch.to_string(),
        head_sha,
        ahead: 0,
        pushed_upstream,
    })
}

/// Run `git <args>` in `cwd`, return trimmed stdout if exit was 0.
async fn git_text_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = async_cmd("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Upper bound on the fetch that refreshes a run's `base_ref`: an unreachable
/// remote must refuse the run, not pin it.
pub const BASE_REF_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Syntax gate for `workspace_config.base_ref`; returns the trimmed value.
pub fn validate_base_ref(raw: &str) -> Result<&str, String> {
    let value = raw.trim();
    let malformed = value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        });
    if malformed {
        return Err(format!(
            "`workspace_config.base_ref` must name a branch, tag or commit \
             (e.g. `origin/main`, `v1.4.0`, a SHA); got `{value}`"
        ));
    }
    Ok(value)
}

/// Split `origin/main` (or `refs/remotes/origin/main`) on the longest
/// configured remote, so `a/b/c` resolves against remote `a/b` when it exists.
async fn remote_branch_of(repo: &Path, base_ref: &str) -> Option<(String, String)> {
    let short = base_ref.strip_prefix("refs/remotes/").unwrap_or(base_ref);
    let remotes = git_text_output(repo, &["remote"]).await?;
    remotes
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .filter_map(|remote| {
            let branch = short.strip_prefix(remote)?.strip_prefix('/')?;
            (!branch.is_empty()).then(|| (remote.to_string(), branch.to_string()))
        })
        .max_by_key(|(remote, _)| remote.len())
}

/// Wait before retrying a fetch whose ref lock another process held.
const FETCH_LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// One fetch at a time per repository: concurrent fetches of the same ref race
/// on its lock and all but one fail.
fn repo_fetch_lock(repo: &Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    type FetchLocks = std::sync::Mutex<
        std::collections::HashMap<PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>,
    >;
    static LOCKS: std::sync::OnceLock<FetchLocks> = std::sync::OnceLock::new();
    let key = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(key)
        .or_default()
        .clone()
}

async fn fetch_remote_branch(
    repo: &Path,
    remote: &str,
    branch: &str,
    timeout: std::time::Duration,
) -> std::result::Result<(), String> {
    fetch_remote_branch_serialized(repo, remote, branch, timeout, Some(FETCH_LOCK_RETRY_DELAY))
        .await
}

/// `lock_retry`: one more attempt after that delay when the ref lock was held,
/// which only a fetch outside this process can still cause.
async fn fetch_remote_branch_serialized(
    repo: &Path,
    remote: &str,
    branch: &str,
    timeout: std::time::Duration,
    lock_retry: Option<std::time::Duration>,
) -> std::result::Result<(), String> {
    let lock = repo_fetch_lock(repo);
    let _serialized = lock.lock().await;
    match fetch_remote_branch_once(repo, remote, branch, timeout).await {
        Err(reason) if lock_retry.is_some() && reason.contains("cannot lock ref") => {
            tokio::time::sleep(lock_retry.unwrap_or_default()).await;
            fetch_remote_branch_once(repo, remote, branch, timeout).await
        }
        outcome => outcome,
    }
}

async fn fetch_remote_branch_once(
    repo: &Path,
    remote: &str,
    branch: &str,
    timeout: std::time::Duration,
) -> std::result::Result<(), String> {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    let mut command = async_cmd("git");
    command
        .args(["fetch", "--no-tags", "--quiet", remote, &refspec])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Err(_) => {
            return Err(format!(
                "`git fetch {remote} {branch}` did not finish within {timeout:?}"
            ))
        }
        Ok(Err(error)) => return Err(format!("cannot run `git fetch`: {error}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = crate::api::projects::clone::redact_url_credentials(
        String::from_utf8_lossy(&output.stderr).trim(),
    );
    let stderr: String = stderr.chars().take(400).collect();
    Err(format!("`git fetch {remote} {branch}` failed: {stderr}"))
}

/// The commit a run starts from. A `base_ref` naming a remote branch is fetched
/// first: the author asked for that tip, so a failed fetch refuses the run
/// rather than silently starting from a stale copy.
pub(crate) async fn resolve_base_ref(
    repo: &Path,
    base_ref: &str,
    fetch_timeout: std::time::Duration,
) -> Result<String> {
    let base_ref = validate_base_ref(base_ref).map_err(anyhow::Error::msg)?;
    if let Some((remote, branch)) = remote_branch_of(repo, base_ref).await {
        if let Err(reason) = fetch_remote_branch(repo, &remote, &branch, fetch_timeout).await {
            anyhow::bail!(
                "Cannot start this run from `{base_ref}`: {reason}. The run did not fall back \
                 to a stale copy: check network access and credentials for remote `{remote}`, \
                 or remove `workspace_config.base_ref` to start from the checkout's HEAD."
            );
        }
    }
    git_text_output(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base_ref}^{{commit}}"),
        ],
    )
    .await
    .filter(|sha| !sha.is_empty())
    .with_context(|| {
        format!(
            "Cannot start this run from `{base_ref}`: it names no commit in {}. \
             Use an existing branch, tag or commit, e.g. `origin/main`.",
            repo.display()
        )
    })
}

/// What boot found in the checkout of an `Interrupted` run past its lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptedCheckout {
    /// Uncommitted or untracked work: the checkout is kept.
    Dirty { entries: usize },
    /// HEAD is on no branch: removing the worktree could strand its commits.
    Detached,
    Removable {
        branch: String,
        head_sha: String,
        /// `Some` when the branch holds commits no known base has.
        preserve: Option<PreservedBranch>,
    },
}

pub async fn inspect_interrupted_checkout(
    worktree: &Path,
) -> std::result::Result<InterruptedCheckout, String> {
    let status = async_cmd("git")
        .args(["status", "--porcelain", "--ignore-submodules=none"])
        .current_dir(worktree)
        .output()
        .await
        .map_err(|error| format!("cannot run git status: {error}"))?;
    if !status.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    let entries = String::from_utf8_lossy(&status.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    if entries > 0 {
        return Ok(InterruptedCheckout::Dirty { entries });
    }
    let Some(branch) =
        git_text_output(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await
    else {
        return Ok(InterruptedCheckout::Detached);
    };
    let head_sha = git_text_output(worktree, &["rev-parse", "HEAD"])
        .await
        .ok_or_else(|| "cannot read HEAD".to_string())?;
    let preserve = check_branch_for_preservation(worktree, &branch).await;
    Ok(InterruptedCheckout::Removable {
        branch,
        head_sha,
        preserve,
    })
}

/// Remove a checkout already found clean. No `--force`: git refuses if it
/// became dirty or was locked since the inspection.
pub async fn remove_clean_checkout(repo_path: &Path, worktree: &Path) -> Result<()> {
    let output = async_cmd("git")
        .args(["worktree", "remove"])
        .arg(worktree)
        .current_dir(repo_path)
        .output()
        .await
        .context("Failed to run git worktree remove")?;
    if !output.status.success() {
        anyhow::bail!(
            "git worktree remove refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Delete `branch` only while it still points at `expected_sha`.
pub async fn delete_branch_at(repo_path: &Path, branch: &str, expected_sha: &str) -> Result<()> {
    let output = async_cmd("git")
        .args([
            "update-ref",
            "-d",
            &format!("refs/heads/{branch}"),
            expected_sha,
        ])
        .current_dir(repo_path)
        .output()
        .await
        .context("Failed to run git update-ref")?;
    if !output.status.success() {
        anyhow::bail!(
            "branch `{branch}` not deleted: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Build a branch name for a workflow run: `kronn/<sanitized_name>/<run_id_prefix>`.
pub(crate) fn build_branch_name(workflow_name: &str, run_id: &str) -> String {
    let sanitized = sanitize_name(workflow_name);
    format!("kronn/{}/{}", sanitized, &run_id[..8.min(run_id.len())])
}

/// Build the worktree directory name: `<sanitized_name>-<run_id_prefix>`.
pub(crate) fn build_worktree_dir_name(workflow_name: &str, run_id: &str) -> String {
    let sanitized = sanitize_name(workflow_name);
    format!("{}-{}", sanitized, &run_id[..8.min(run_id.len())])
}

impl Workspace {
    /// Create a new workspace via `git worktree add`.
    /// Branch: `kronn/<workflow_name>/<run_id>`, started from `base_ref` when
    /// given (fetched first if it names a remote branch), else from HEAD.
    pub async fn create(
        repo_path: &Path,
        workflow_name: &str,
        run_id: &str,
        hooks: Option<WorkspaceHooks>,
        base_ref: Option<&str>,
    ) -> Result<Self> {
        let _sanitized_name = sanitize_name(workflow_name);

        let branch = build_branch_name(workflow_name, run_id);

        // Mark the repo as a safe directory (needed in Docker where the mounted
        // volume owner differs from the container user) before fetching in it.
        let _ = async_cmd("git")
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &repo_path.to_string_lossy(),
            ])
            .output()
            .await;

        // Resolved before anything is written, so a refused base leaves no trace.
        let start_point = match base_ref {
            Some(base_ref) => {
                Some(resolve_base_ref(repo_path, base_ref, BASE_REF_FETCH_TIMEOUT).await?)
            }
            None => None,
        };

        // Worktree path: alongside the repo, in a .kronn/worktrees directory
        let worktree_base = repo_path.join(".kronn/worktrees");
        std::fs::create_dir_all(&worktree_base)?;
        // Ensure .kronn/worktrees/ is gitignored in the project
        if let Some(p) = repo_path.to_str() {
            crate::core::mcp_scanner::ensure_gitignore_public(p, ".kronn/");
        }
        let worktree_path = worktree_base.join(build_worktree_dir_name(workflow_name, run_id));

        let _ = async_cmd("git")
            .args([
                "config",
                "--global",
                "--add",
                "safe.directory",
                &worktree_path.to_string_lossy(),
            ])
            .output()
            .await;

        // Create the worktree with a new branch. A SHA start point sets up no
        // tracking, so `@{u}` keeps meaning "the agent pushed".
        let mut add = async_cmd("git");
        add.args(["worktree", "add", "-b", &branch])
            .arg(&worktree_path)
            .current_dir(repo_path);
        if let Some(sha) = &start_point {
            add.arg(sha);
        }
        let output = add
            .output()
            .await
            .context("Failed to execute git worktree add")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {}", stderr);
        }

        tracing::info!(
            "Created worktree at {} (branch: {}, base: {})",
            worktree_path.display(),
            branch,
            match (base_ref, &start_point) {
                (Some(base_ref), Some(sha)) => format!("{base_ref} @ {sha}"),
                _ => "HEAD".to_string(),
            }
        );

        let ws = Self {
            path: worktree_path,
            branch,
            repo_path: repo_path.to_path_buf(),
            hooks,
        };

        // Run after_create hook
        ws.run_hook("after_create").await?;

        Ok(ws)
    }

    /// 0.7.0 Phase 4 — attach to a previously-created worktree. Used on
    /// resume from a Gate pause: the worktree already exists on disk
    /// (the agent ran in it before the pause), `before_run` already
    /// fired, and we just want a `Workspace` handle to drive `after_run`
    /// and `cleanup` once the run finishes. No git side-effects: no
    /// `worktree add`, no `safe.directory` config, no hook firing.
    /// The path is taken at face value.
    pub fn attach(
        path: PathBuf,
        repo_path: PathBuf,
        workflow_name: &str,
        run_id: &str,
        hooks: Option<WorkspaceHooks>,
    ) -> Self {
        Self {
            path,
            branch: build_branch_name(workflow_name, run_id),
            repo_path,
            hooks,
        }
    }

    /// Run the before_run hook.
    pub async fn before_run(&self) -> Result<()> {
        self.run_hook("before_run").await
    }

    /// Run the after_run hook.
    pub async fn after_run(&self) -> Result<()> {
        self.run_hook("after_run").await
    }

    /// Clean up the workspace: run before_remove hook, then remove the worktree.
    ///
    /// Branch lifecycle:
    ///   - The branch is preserved (left alive in the parent repo) when the
    ///     worktree's HEAD has local commits not present on any known base
    ///     ref. Returned in `CleanupOutcome.preserved`. The runner records
    ///     it on the run so the UI can surface "commit available here".
    ///   - The branch is deleted when the worktree is fully synced with a
    ///     known base — nothing of value would survive its deletion.
    ///
    /// Failures: best-effort. If the preserve check itself errors out, we
    /// default to preserving the branch (safer than silently dropping work).
    pub async fn cleanup(self) -> Result<CleanupOutcome> {
        self.run_hook("before_remove").await?;

        // Snapshot branch state BEFORE removing the worktree — afterwards
        // the worktree path is gone and `git -C <worktree>` calls fail.
        let preserve = check_branch_for_preservation(&self.path, &self.branch).await;

        // Remove the worktree
        let output = async_cmd("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.repo_path)
            .output()
            .await
            .context("Failed to execute git worktree remove")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                "git worktree remove failed (will try manual cleanup): {}",
                stderr
            );
            // Fallback: remove directory manually
            if self.path.exists() {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        let outcome =
            if let Some(info) = preserve {
                // Keep the branch alive in the parent repo.
                tracing::info!(
                "Preserved branch '{}' (HEAD={}, {} commit(s) ahead of base, upstream_set={}) — \
                 the worktree had local commits the operator may want to recover.",
                info.branch_name, info.head_sha, info.ahead, info.pushed_upstream
            );
                CleanupOutcome {
                    preserved: Some(info),
                }
            } else {
                // Fully synced — safe to drop the branch ref.
                let _ = async_cmd("git")
                    .args(["branch", "-D", &self.branch])
                    .current_dir(&self.repo_path)
                    .output()
                    .await;
                CleanupOutcome::default()
            };

        tracing::info!("Cleaned up worktree: {}", self.path.display());
        Ok(outcome)
    }

    /// Purge a worktree left behind by an already-terminal run after a crash.
    /// Unlike `cleanup`, this never runs user hooks and never deletes the
    /// branch ref: the original runner may already have persisted/pushed it,
    /// and boot recovery must remove discoverable checkout data only.
    pub async fn purge_terminal_checkout(repo_path: &Path, worktree_path: &Path) -> Result<()> {
        let output = async_cmd("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .current_dir(repo_path)
            .output()
            .await
            .context("Failed to purge terminal workflow worktree")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree remove failed: {}", stderr.trim());
        }
        Ok(())
    }

    /// Execute a lifecycle hook shell command in the workspace directory.
    async fn run_hook(&self, hook_name: &str) -> Result<()> {
        let cmd = match (&self.hooks, hook_name) {
            (Some(h), "after_create") => h.after_create.as_deref(),
            (Some(h), "before_run") => h.before_run.as_deref(),
            (Some(h), "after_run") => h.after_run.as_deref(),
            (Some(h), "before_remove") => h.before_remove.as_deref(),
            _ => None,
        };

        if let Some(cmd) = cmd {
            tracing::info!("Running workspace hook '{}': {}", hook_name, cmd);
            // Bounded: a hung hook (e.g. `npm ci` against a dead registry)
            // would otherwise pin the run and its concurrency slot forever —
            // this await is outside the cancel race, so Stop can't reach it.
            // `kill_on_drop` ensures the timed-out child doesn't leak.
            const HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
            let mut command = async_cmd("sh");
            command
                .args(["-c", cmd])
                .current_dir(&self.path)
                .kill_on_drop(true);
            let output = match tokio::time::timeout(HOOK_TIMEOUT, command.output()).await {
                Ok(res) => res.with_context(|| format!("Failed to run {} hook", hook_name))?,
                Err(_) => {
                    tracing::warn!(
                        "Hook '{}' timed out after {}s — killing it",
                        hook_name,
                        HOOK_TIMEOUT.as_secs()
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to run {} hook: timed out after {}s",
                        hook_name,
                        HOOK_TIMEOUT.as_secs()
                    ));
                }
            };

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("Hook '{}' failed: {}", hook_name, stderr);
                // Hooks are best-effort — log but don't fail the workflow
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{main_tree_wait_from, MainTreeGuard, MainTreeRefusal};
    use std::time::Duration;

    #[test]
    fn main_tree_guard_is_exclusive_per_project_and_released_on_drop() {
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = dir_a.path().to_string_lossy().to_string();
        let b = dir_b.path().to_string_lossy().to_string();
        let g1 = MainTreeGuard::try_acquire(&a, "run-1").expect("first acquire");
        let denied = MainTreeGuard::try_acquire(&a, "run-2");
        assert_eq!(
            denied.err().as_deref(),
            Some("run-1"),
            "second run must be refused, naming the holder"
        );
        // A different project is unaffected.
        let _other = MainTreeGuard::try_acquire(&b, "run-3").expect("other project free");
        drop(g1);
        let _g2 = MainTreeGuard::try_acquire(&a, "run-2").expect("released on drop");
    }

    fn fresh_tree() -> (tempfile::TempDir, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        (dir, path)
    }

    #[tokio::test]
    async fn main_tree_waiters_are_served_in_arrival_order() {
        let (_dir, tree) = fresh_tree();
        let never = tokio_util::sync::CancellationToken::new();
        let holder = MainTreeGuard::try_acquire(&tree, "run-holder").unwrap();
        let (order_tx, mut order_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        for run in ["run-a", "run-b", "run-c"] {
            let (tree, never, order_tx) = (tree.clone(), never.clone(), order_tx.clone());
            tasks.push(tokio::spawn(async move {
                let guard =
                    MainTreeGuard::acquire(&tree, run, &[], Duration::from_secs(10), &never)
                        .await
                        .expect("served before the deadline");
                order_tx.send(run).unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                drop(guard);
            }));
            // Let this waiter enqueue before the next one arrives.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(holder);
        for task in tasks {
            task.await.unwrap();
        }
        drop(order_tx);
        let mut order = Vec::new();
        while let Some(run) = order_rx.recv().await {
            order.push(run);
        }
        assert_eq!(order, ["run-a", "run-b", "run-c"]);
        assert!(
            MainTreeGuard::try_acquire(&tree, "run-after").is_ok(),
            "fully released"
        );
    }

    #[tokio::test]
    async fn main_tree_wait_times_out_naming_the_holder() {
        let (_dir, tree) = fresh_tree();
        let never = tokio_util::sync::CancellationToken::new();
        let _holder = MainTreeGuard::try_acquire(&tree, "run-holder").unwrap();
        let started = std::time::Instant::now();
        let refusal =
            MainTreeGuard::acquire(&tree, "run-late", &[], Duration::from_millis(150), &never)
                .await
                .err()
                .expect("the holder never releases");
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "it waited first"
        );
        assert_eq!(
            refusal,
            MainTreeRefusal::Busy {
                holder: "run-holder".into(),
                waited: Duration::from_millis(150),
            }
        );
    }

    #[tokio::test]
    async fn a_cancelled_waiter_leaves_the_queue() {
        let (_dir, tree) = fresh_tree();
        let never = tokio_util::sync::CancellationToken::new();
        let cancel = tokio_util::sync::CancellationToken::new();
        let holder = MainTreeGuard::try_acquire(&tree, "run-holder").unwrap();
        let waiter = {
            let (tree, cancel) = (tree.clone(), cancel.clone());
            tokio::spawn(async move {
                MainTreeGuard::acquire(
                    &tree,
                    "run-cancelled",
                    &[],
                    Duration::from_secs(30),
                    &cancel,
                )
                .await
                .err()
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        assert_eq!(waiter.await.unwrap(), Some(MainTreeRefusal::Cancelled));
        let next = {
            let (tree, never) = (tree.clone(), never.clone());
            tokio::spawn(async move {
                MainTreeGuard::acquire(&tree, "run-next", &[], Duration::from_secs(10), &never)
                    .await
                    .is_ok()
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(holder);
        assert!(
            next.await.unwrap(),
            "the release skips the cancelled waiter"
        );
    }

    #[tokio::test]
    async fn a_run_held_up_by_its_own_ancestor_is_refused_at_once() {
        let (_dir, tree) = fresh_tree();
        let never = tokio_util::sync::CancellationToken::new();
        let _parent = MainTreeGuard::try_acquire(&tree, "run-parent").unwrap();
        let refusal = MainTreeGuard::acquire(
            &tree,
            "run-child",
            &["run-parent".to_string()],
            Duration::from_secs(30),
            &never,
        )
        .await
        .err();
        assert_eq!(
            refusal,
            Some(MainTreeRefusal::HeldByAncestor {
                holder: "run-parent".into()
            })
        );
    }

    #[test]
    fn main_tree_wait_defaults_to_sixty_seconds_and_reads_the_override() {
        assert_eq!(main_tree_wait_from(None), Duration::from_secs(60));
        assert_eq!(
            main_tree_wait_from(Some("not a number")),
            Duration::from_secs(60)
        );
        assert_eq!(main_tree_wait_from(Some(" 5 ")), Duration::from_secs(5));
        assert_eq!(main_tree_wait_from(Some("0")), Duration::ZERO);
    }

    use super::*;

    // ─── sanitize_name ───────────────────────────────────────────────────

    #[test]
    fn sanitize_name_alphanumeric_unchanged() {
        assert_eq!(sanitize_name("my-workflow_v2"), "my-workflow_v2");
    }

    #[test]
    fn sanitize_name_spaces_replaced() {
        assert_eq!(sanitize_name("my workflow"), "my-workflow");
    }

    #[test]
    fn sanitize_name_special_chars_replaced() {
        assert_eq!(sanitize_name("build & deploy!"), "build---deploy-");
    }

    #[test]
    fn sanitize_name_unicode_alphanumeric_preserved() {
        // Rust's char::is_alphanumeric() includes Unicode letters like é
        assert_eq!(sanitize_name("déploiement"), "déploiement");
    }

    #[test]
    fn sanitize_name_empty() {
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn sanitize_name_all_special() {
        assert_eq!(sanitize_name("!@#$%"), "-----");
    }

    // ─── build_branch_name ───────────────────────────────────────────────

    #[test]
    fn build_branch_name_basic() {
        let branch = build_branch_name("my-workflow", "abcdef12-3456-7890");
        assert_eq!(branch, "kronn/my-workflow/abcdef12");
    }

    #[test]
    fn build_branch_name_short_run_id() {
        let branch = build_branch_name("wf", "abc");
        assert_eq!(branch, "kronn/wf/abc");
    }

    #[test]
    fn build_branch_name_sanitizes_workflow_name() {
        let branch = build_branch_name("My Workflow!", "12345678");
        assert_eq!(branch, "kronn/My-Workflow-/12345678");
    }

    #[test]
    fn build_branch_name_exact_8_char_run_id() {
        let branch = build_branch_name("test", "12345678");
        assert_eq!(branch, "kronn/test/12345678");
    }

    // ─── build_worktree_dir_name ─────────────────────────────────────────

    #[test]
    fn build_worktree_dir_basic() {
        let dir = build_worktree_dir_name("deploy", "abcdef12-rest");
        assert_eq!(dir, "deploy-abcdef12");
    }

    #[test]
    fn build_worktree_dir_sanitizes() {
        let dir = build_worktree_dir_name("build & test", "11223344");
        assert_eq!(dir, "build---test-11223344");
    }

    #[test]
    fn build_worktree_dir_short_run_id() {
        let dir = build_worktree_dir_name("wf", "ab");
        assert_eq!(dir, "wf-ab");
    }

    // ─── Worktree base dir ───────────────────────────────────────────────

    #[test]
    fn worktree_base_is_inside_repo() {
        let repo = PathBuf::from("/home/user/project");
        let base = repo.join(".kronn/worktrees");
        assert_eq!(
            base.to_str().unwrap(),
            "/home/user/project/.kronn/worktrees"
        );
    }

    #[test]
    fn worktree_path_combines_base_and_dir() {
        let repo = PathBuf::from("/repos/myapp");
        let base = repo.join(".kronn/worktrees");
        let dir_name = build_worktree_dir_name("audit", "aabbccdd-1234");
        let full = base.join(&dir_name);
        assert_eq!(
            full.to_str().unwrap(),
            "/repos/myapp/.kronn/worktrees/audit-aabbccdd"
        );
    }

    // ─── check_branch_for_preservation (P3 — preserve commits cleanup
    //    would otherwise orphan). Uses real git on a tempdir; serial so
    //    parallel test runs don't trip over each other on shared global
    //    git config. ─────────────────────────────────────────────────────

    /// Set up a tempdir with `git init`, an initial commit on `main`,
    /// then create a branch pointing at HEAD. Returns the repo path so
    /// the test can call our detector against it as the "worktree".
    async fn make_test_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().to_path_buf();

        // Init + minimal user config (commit -m needs an identity).
        let _ = crate::core::cmd::async_cmd("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["config", "user.email", "test@kronn.local"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["config", "user.name", "test"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();

        (dir, repo)
    }

    #[tokio::test]
    async fn preserve_when_branch_synced_with_main_returns_none() {
        // Branch HEAD == main HEAD → nothing to preserve.
        let (_dir, repo) = make_test_repo().await;
        let result = check_branch_for_preservation(&repo, "test-branch").await;
        assert!(result.is_none(), "synced HEAD should NOT be preserved");
    }

    #[tokio::test]
    async fn preserve_when_branch_ahead_of_main_returns_some() {
        // Mirror the production scenario: a worktree branched off main
        // gets its own commit, leaving main behind by 1.
        let (_dir, repo) = make_test_repo().await;
        let _ = crate::core::cmd::async_cmd("git")
            .args(["checkout", "-q", "-b", "kronn/test"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        std::fs::write(repo.join("CHANGELOG.md"), "v1\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["commit", "-q", "-m", "feat: changelog"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();

        let result = check_branch_for_preservation(&repo, "kronn/test").await;
        let preserved = result.expect("branch ahead of main should be preserved");
        assert_eq!(preserved.branch_name, "kronn/test");
        assert!(!preserved.head_sha.is_empty(), "head_sha should be filled");
        assert_eq!(preserved.ahead, 1, "exactly one commit beyond main");
        assert!(!preserved.pushed_upstream, "no remote → upstream_set=false");
    }

    #[tokio::test]
    async fn preserve_when_no_base_resolves_falls_back_to_some() {
        // Fresh repo with no `main` ref and no remotes → detector can't
        // count ahead; it returns Some(ahead=0) defensively rather than
        // None (preferring "preserve too much" over "lose work").
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().to_path_buf();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["init", "-q", "-b", "weird-default"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["config", "user.email", "test@kronn.local"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["config", "user.name", "test"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        std::fs::write(repo.join("README.md"), "x\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();

        let result = check_branch_for_preservation(&repo, "test-branch").await;
        let preserved = result.expect("no resolvable base → defensive preserve");
        assert_eq!(preserved.ahead, 0, "ahead unknown → 0 (still preserve)");
    }

    // ─── Workspace::create + cleanup (E2E worktree lifecycle) ──────────

    #[tokio::test]
    async fn workspace_create_and_cleanup_synced_branch_does_not_preserve() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "test-wf", "abcdef12-rest", None, None)
            .await
            .expect("create worktree");

        assert!(ws.path.exists(), "worktree path should be on disk");
        assert!(ws.path.starts_with(repo.join(".kronn/worktrees")));
        assert_eq!(ws.branch, "kronn/test-wf/abcdef12");

        // No new commits → cleanup() should NOT preserve.
        let outcome = ws.cleanup().await.expect("cleanup");
        assert!(
            outcome.preserved.is_none(),
            "synced branch should not be preserved"
        );
    }

    #[tokio::test]
    async fn workspace_create_and_cleanup_with_new_commit_preserves_branch() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "preservetest", "ffeeddcc-extra", None, None)
            .await
            .expect("create worktree");

        // Make a real commit inside the worktree so HEAD diverges from main.
        std::fs::write(ws.path.join("NEWFILE.md"), "extra\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["add", "."])
            .current_dir(&ws.path)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["commit", "-q", "-m", "feat: extra"])
            .current_dir(&ws.path)
            .output()
            .await
            .unwrap();

        let outcome = ws.cleanup().await.expect("cleanup");
        let preserved = outcome
            .preserved
            .expect("branch with new commit must be preserved");
        assert_eq!(preserved.branch_name, "kronn/preservetest/ffeeddcc");
        assert_eq!(preserved.ahead, 1, "exactly one commit beyond main");
        assert!(!preserved.head_sha.is_empty());
        assert!(!preserved.pushed_upstream, "no remote → no upstream");
    }

    #[tokio::test]
    async fn terminal_purge_removes_checkout_but_preserves_branch_evidence() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "boot-cleanup", "aabbccdd-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        let branch = ws.branch.clone();
        assert!(path.exists());

        // Boot cleanup is intentionally narrower than normal cleanup: it
        // removes only the discoverable checkout and keeps the branch ref as
        // crash evidence, even when that branch is still synced with main.
        drop(ws);
        Workspace::purge_terminal_checkout(&repo, &path)
            .await
            .expect("purge terminal checkout");

        assert!(
            !path.exists(),
            "the stale checkout must no longer be visible"
        );
        let branch_check = crate::core::cmd::async_cmd("git")
            .args(["show-ref", "--verify", &format!("refs/heads/{branch}")])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        assert!(
            branch_check.status.success(),
            "boot cleanup must preserve the branch ref"
        );
    }

    #[tokio::test]
    async fn workspace_attach_uses_provided_path_without_side_effects() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let repo = dir.path().to_path_buf();
        let ws = Workspace::attach(path.clone(), repo.clone(), "wf-name", "aabbccdd-zz", None);
        assert_eq!(ws.path, path);
        assert_eq!(ws.branch, "kronn/wf-name/aabbccdd");
    }

    #[tokio::test]
    async fn phase2_inherited_child_attach_then_drop_without_cleanup_keeps_worktree() {
        // Phase 2 (worktree handoff) invariant: a sub-workflow CHILD that
        // INHERITS the parent's worktree attaches to it, commits there, then
        // is dropped WITHOUT cleanup (the runner skips cleanup for inherited
        // runs). The worktree + the child's commit MUST survive so the PARENT
        // can keep working (e.g. `create_pr` sees the implementation), and the
        // PARENT — not the child — owns final cleanup/branch preservation.
        let (_dir, repo) = make_test_repo().await;

        // Parent creates the worktree (its own branch).
        let parent_ws = Workspace::create(&repo, "ticket-to-pr", "11223344-parent", None, None)
            .await
            .expect("create parent worktree");
        let path = parent_ws.path.clone();
        assert!(path.exists());

        // Child ATTACHES the same path (different run_id → cosmetic branch
        // field; commits land on the branch actually checked out in `path`,
        // i.e. the parent's branch).
        let child_ws = Workspace::attach(
            path.clone(),
            repo.clone(),
            "implement-verify",
            "99887766-child",
            None,
        );
        std::fs::write(child_ws.path.join("IMPL.md"), "child implementation\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["add", "."])
            .current_dir(&child_ws.path)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_cmd("git")
            .args(["commit", "-q", "-m", "feat: child work"])
            .current_dir(&child_ws.path)
            .output()
            .await
            .unwrap();

        // The child is dropped WITHOUT cleanup (the inherited-workspace path
        // in execute_run skips `ws.cleanup()`). No async Drop → no removal.
        drop(child_ws);

        // Worktree + the child's file must still be there for the parent.
        assert!(
            path.exists(),
            "inherited child must NOT remove the shared worktree"
        );
        assert!(
            path.join("IMPL.md").exists(),
            "child's commit/files must survive for the parent"
        );

        // The PARENT owns cleanup → its branch is preserved WITH the child's commit.
        let outcome = parent_ws.cleanup().await.expect("parent cleanup");
        let preserved = outcome
            .preserved
            .expect("parent branch carries the child's commit → preserved");
        assert_eq!(preserved.branch_name, "kronn/ticket-to-pr/11223344");
        assert_eq!(
            preserved.ahead, 1,
            "the single child commit is one ahead of main"
        );
    }

    #[tokio::test]
    async fn workspace_hook_runs_when_command_set() {
        // Hook output goes to a sentinel file we can read back.
        let (_dir, repo) = make_test_repo().await;
        let sentinel = repo.join("sentinel.txt");
        let hooks = WorkspaceHooks {
            after_create: Some(format!("echo touched > {}", sentinel.display())),
            before_run: None,
            after_run: None,
            before_remove: None,
        };
        let ws = Workspace::create(&repo, "hookwf", "11112222-rest", Some(hooks), None)
            .await
            .expect("create worktree");
        // The hook should have written to the sentinel.
        let _ = ws.before_run().await;
        let _ = ws.after_run().await;
        assert!(
            sentinel.exists(),
            "after_create hook should have created sentinel"
        );
        let outcome = ws.cleanup().await.expect("cleanup");
        // No new commits committed → no preserve.
        assert!(outcome.preserved.is_none());
    }

    #[tokio::test]
    async fn workspace_no_hooks_runs_silently() {
        // attach() bypasses git side-effects ; calling before/after/after-remove
        // with no hooks set must be a no-op (no panic, no error).
        let dir = tempfile::TempDir::new().unwrap();
        let ws = Workspace::attach(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            "nohook",
            "abababab-x",
            None,
        );
        ws.before_run().await.unwrap();
        ws.after_run().await.unwrap();
        // No cleanup() — attach doesn't own a real worktree.
    }

    // ─── base_ref: where a fresh isolated run starts ─────────────────────

    async fn git_ok(cwd: &Path, args: &[&str]) -> String {
        let out = crate::core::cmd::async_cmd("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// An upstream repo and a clone of it; upstream then gains a commit the
    /// clone has not fetched, so both `main` and `origin/main` there are behind.
    /// Returns (dirs, clone, upstream tip).
    async fn clone_behind_its_remote() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, String) {
        let (upstream_dir, upstream) = make_test_repo().await;
        let clone_dir = tempfile::TempDir::new().unwrap();
        let clone = clone_dir.path().join("clone");
        git_ok(
            clone_dir.path(),
            &["clone", "-q", &upstream.to_string_lossy(), "clone"],
        )
        .await;
        git_ok(&clone, &["config", "user.email", "test@kronn.local"]).await;
        git_ok(&clone, &["config", "user.name", "test"]).await;
        std::fs::write(upstream.join("REMOTE.md"), "landed upstream\n").unwrap();
        git_ok(&upstream, &["add", "."]).await;
        git_ok(&upstream, &["commit", "-q", "-m", "feat: upstream only"]).await;
        let tip = git_ok(&upstream, &["rev-parse", "HEAD"]).await;
        (upstream_dir, clone_dir, clone, tip)
    }

    #[test]
    fn base_ref_syntax_accepts_refs_and_rejects_options_and_revision_syntax() {
        for good in [
            "origin/main",
            " main ",
            "v1.4.0",
            "refs/remotes/upstream/release/2.0",
        ] {
            assert!(validate_base_ref(good).is_ok(), "{good}");
        }
        assert_eq!(validate_base_ref(" origin/main ").unwrap(), "origin/main");
        for bad in [
            "",
            "   ",
            "--upload-pack=x",
            "main~1",
            "HEAD^",
            "a..b",
            "main@{1}",
            "a b",
            "x:y",
        ] {
            let error = validate_base_ref(bad).unwrap_err();
            assert!(
                error.contains("workspace_config.base_ref"),
                "{bad}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn base_ref_starts_the_worktree_from_the_fetched_remote_tip() {
        let (_upstream, _clone_dir, clone, remote_tip) = clone_behind_its_remote().await;
        let local_main = git_ok(&clone, &["rev-parse", "main"]).await;
        assert_ne!(local_main, remote_tip, "the fixture is behind its remote");

        let ws = Workspace::create(&clone, "based", "abcd1234-run", None, Some("origin/main"))
            .await
            .expect("create from origin/main");

        assert_eq!(
            git_ok(&ws.path, &["rev-parse", "HEAD"]).await,
            remote_tip,
            "the run starts from the remote tip, not the stale local main"
        );
        assert_eq!(
            git_ok(&clone, &["rev-parse", "main"]).await,
            local_main,
            "the checkout's own branch is untouched"
        );
        assert!(
            git_text_output(&ws.path, &["rev-parse", "--abbrev-ref", "@{u}"])
                .await
                .is_none(),
            "no upstream is set, so `@{{u}}` still means the agent pushed"
        );
        let outcome = ws.cleanup().await.expect("cleanup");
        assert!(outcome.preserved.is_none(), "nothing of its own to keep");
    }

    #[tokio::test]
    async fn base_ref_may_name_a_local_tag_without_any_fetch() {
        let (_dir, repo) = make_test_repo().await;
        let tagged = git_ok(&repo, &["rev-parse", "HEAD"]).await;
        git_ok(&repo, &["tag", "v1.0.0"]).await;
        std::fs::write(repo.join("LATER.md"), "later\n").unwrap();
        git_ok(&repo, &["add", "."]).await;
        git_ok(&repo, &["commit", "-q", "-m", "later"]).await;

        let ws = Workspace::create(&repo, "tagged", "feed1234-run", None, Some("v1.0.0"))
            .await
            .expect("create from a tag");
        assert_eq!(git_ok(&ws.path, &["rev-parse", "HEAD"]).await, tagged);
        ws.cleanup().await.expect("cleanup");
    }

    #[tokio::test]
    async fn an_unreachable_remote_refuses_the_run_with_an_actionable_message() {
        let (_upstream, clone_dir, clone, _tip) = clone_behind_its_remote().await;
        let gone = clone_dir.path().join("moved-away");
        git_ok(
            &clone,
            &["remote", "set-url", "origin", &gone.to_string_lossy()],
        )
        .await;

        let error = Workspace::create(&clone, "offline", "deadbeef-run", None, Some("origin/main"))
            .await
            .err()
            .expect("a failed fetch must refuse the run")
            .to_string();

        assert!(error.contains("`origin/main`"), "{error}");
        assert!(error.contains("git fetch origin main"), "{error}");
        assert!(error.contains("remote `origin`"), "{error}");
        assert!(error.contains("workspace_config.base_ref"), "{error}");
        assert!(
            !clone.join(".kronn/worktrees").exists(),
            "nothing is written before the base is known"
        );
        assert!(
            git_text_output(
                &clone,
                &["rev-parse", "--verify", "--quiet", "kronn/offline/deadbeef"]
            )
            .await
            .is_none(),
            "no branch either"
        );
    }

    /// Moves the clone's upstream on by one commit and returns its new tip.
    async fn advance_upstream(clone: &Path, round: usize) -> String {
        let upstream = PathBuf::from(git_ok(clone, &["remote", "get-url", "origin"]).await);
        std::fs::write(upstream.join("ROUND.md"), format!("{round}\n")).unwrap();
        git_ok(&upstream, &["add", "."]).await;
        git_ok(
            &upstream,
            &["commit", "-q", "-m", &format!("round {round}")],
        )
        .await;
        git_ok(&upstream, &["rev-parse", "HEAD"]).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_base_ref_fetches_in_one_repo_all_start_from_the_remote_tip() {
        // Parallel foreach items each create a worktree from the same base.
        let (_upstream, _clone_dir, clone, _tip) = clone_behind_its_remote().await;
        for round in 0..3 {
            let tip = advance_upstream(&clone, round).await;
            let mut runs = tokio::task::JoinSet::new();
            for _ in 0..8 {
                let clone = clone.clone();
                runs.spawn(async move {
                    resolve_base_ref(&clone, "origin/main", BASE_REF_FETCH_TIMEOUT).await
                });
            }
            while let Some(resolved) = runs.join_next().await {
                let resolved = resolved.unwrap().map_err(|error| error.to_string());
                assert_eq!(resolved.as_deref(), Ok(tip.as_str()), "round {round}");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fetches_into_one_repo_are_serialized_without_needing_a_retry() {
        let (_upstream, _clone_dir, clone, _tip) = clone_behind_its_remote().await;
        for round in 0..3 {
            advance_upstream(&clone, round).await;
            let mut fetches = tokio::task::JoinSet::new();
            for _ in 0..8 {
                let clone = clone.clone();
                fetches.spawn(async move {
                    fetch_remote_branch_serialized(
                        &clone,
                        "origin",
                        "main",
                        BASE_REF_FETCH_TIMEOUT,
                        None,
                    )
                    .await
                });
            }
            while let Some(fetched) = fetches.join_next().await {
                assert_eq!(fetched.unwrap(), Ok(()), "round {round}");
            }
        }
    }

    /// The lock file git takes on `refs/remotes/origin/main` in `clone`.
    async fn remote_ref_lock(clone: &Path) -> PathBuf {
        let lock = clone.join(
            git_ok(
                clone,
                &["rev-parse", "--git-path", "refs/remotes/origin/main.lock"],
            )
            .await,
        );
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        lock
    }

    #[tokio::test]
    async fn a_ref_lock_held_by_another_fetch_is_waited_out_once() {
        let (_upstream, _clone_dir, clone, tip) = clone_behind_its_remote().await;
        let lock = remote_ref_lock(&clone).await;
        std::fs::write(&lock, "").unwrap();
        // The user's own fetch lets go shortly after ours first tries.
        let released = {
            let lock = lock.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::fs::remove_file(lock).unwrap();
            })
        };

        let resolved = resolve_base_ref(&clone, "origin/main", BASE_REF_FETCH_TIMEOUT).await;

        released.await.unwrap();
        assert_eq!(resolved.map_err(|error| error.to_string()), Ok(tip));
    }

    #[tokio::test]
    async fn a_ref_lock_still_held_after_the_retry_refuses_the_run() {
        let (_upstream, _clone_dir, clone, _tip) = clone_behind_its_remote().await;
        std::fs::write(remote_ref_lock(&clone).await, "").unwrap();

        let started = std::time::Instant::now();
        let error = resolve_base_ref(&clone, "origin/main", BASE_REF_FETCH_TIMEOUT)
            .await
            .expect_err("a ref lock nobody releases")
            .to_string();

        assert!(error.contains("cannot lock ref"), "{error}");
        assert!(
            started.elapsed() >= FETCH_LOCK_RETRY_DELAY,
            "it waited once before refusing"
        );
    }

    #[tokio::test]
    async fn a_hung_fetch_is_cut_by_its_timeout() {
        let (_upstream, _clone_dir, clone, _tip) = clone_behind_its_remote().await;
        // The local transport runs this through a shell before answering.
        git_ok(
            &clone,
            &[
                "config",
                "remote.origin.uploadpack",
                "sleep 5; git-upload-pack",
            ],
        )
        .await;

        let started = std::time::Instant::now();
        let error = resolve_base_ref(&clone, "origin/main", Duration::from_secs(1))
            .await
            .expect_err("a fetch that never answers must not pin the run")
            .to_string();

        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        assert!(error.contains("did not finish within 1s"), "{error}");
    }

    #[tokio::test]
    async fn a_base_ref_naming_no_commit_is_refused() {
        let (_dir, repo) = make_test_repo().await;
        let error = resolve_base_ref(&repo, "release/9.9", BASE_REF_FETCH_TIMEOUT)
            .await
            .expect_err("unknown ref")
            .to_string();
        assert!(error.contains("names no commit"), "{error}");
    }
}
