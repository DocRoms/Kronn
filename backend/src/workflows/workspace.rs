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

/// A workspace lifecycle hook: user shell in the workspace, with the same
/// built environment as an Exec step (KT-1006).
fn hook_command(cmd: &str, dir: &std::path::Path) -> tokio::process::Command {
    let mut command = async_cmd("sh", crate::core::child_env::ChildRoute::WorkflowExec);
    // A workspace carries no project id, so a hook gets no GitHub token.
    crate::core::child_env::isolate_with_github(
        command.as_std_mut(),
        crate::core::child_env::ChildRoute::WorkflowExec,
        &[],
    );
    command
        .args(["-c", cmd])
        .current_dir(dir)
        .kill_on_drop(true);
    command
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
    main_tree_wait_from(
        crate::core::child_env::var(MAIN_TREE_WAIT_ENV)
            .ok()
            .as_deref(),
    )
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
/// the run's branch held local commits not present on a known base —
/// the runner records it on `WorkflowRun.produced_branches` so the run
/// detail UI can show "commit produit, push bloqué — branche `X` preservée".
#[derive(Debug, Clone, Default)]
pub struct CleanupOutcome {
    pub preserved: Option<PreservedBranch>,
    /// Set when the checkout held work and was left in place (KT-1096).
    pub kept: Option<KeptCheckout>,
}

/// A checkout a stopped run left in place, with its branch, because it held
/// work: found again through the run's `workspace_path`.
#[derive(Debug, Clone)]
pub struct KeptCheckout {
    pub path: PathBuf,
    pub reason: String,
}

/// Why a checkout, or a worktree registered inside it, must not be removed
/// (uncommitted work, a detached HEAD or a state git cannot read), or `None`
/// when the whole set is clean. Kronn's own `.kronn/` files are not work.
pub async fn work_to_keep(repo_path: &Path, worktree: &Path) -> Option<String> {
    // After the agents ran: git's agent-writable pointers are not followed.
    if let Err(error) = crate::core::worktree::ensure_local_git_exclude_pinned(repo_path, ".kronn/")
    {
        tracing::warn!(path = %repo_path.display(), "cannot exclude .kronn/: {error}");
    }
    let mut checkouts = nested_worktrees(repo_path, worktree).await;
    checkouts.push(worktree.to_path_buf());
    for checkout in checkouts {
        // A nested worktree whose directory is gone holds nothing.
        if checkout != worktree && !checkout.exists() {
            continue;
        }
        let inspected = if checkout == worktree {
            inspect_run_checkout(repo_path, &checkout).await
        } else {
            inspect_interrupted_checkout(&checkout).await
        };
        let found = match inspected {
            Ok(InterruptedCheckout::Removable { .. } | InterruptedCheckout::DetachedReachable) => {
                continue
            }
            Ok(InterruptedCheckout::Dirty { entries }) => {
                format!("{entries} uncommitted change(s)")
            }
            Ok(InterruptedCheckout::Detached) => "a detached HEAD".into(),
            Err(error) => format!("a state git cannot read ({error})"),
        };
        return Some(if checkout == worktree {
            found
        } else {
            format!("{found} in its nested worktree `{}`", checkout.display())
        });
    }
    None
}

/// The agent configs Kronn copied into `worktree` that are still as copied,
/// with nothing in the index on their path beyond the committed version.
async fn copied_agent_configs(repo_path: &Path, worktree: &Path) -> Vec<PathBuf> {
    let (repo, tree) = (repo_path.to_path_buf(), worktree.to_path_buf());
    let unchanged = tokio::task::spawn_blocking(move || {
        crate::core::worktree::unchanged_agent_configs(&repo, &tree)
    })
    .await
    .unwrap_or_default();
    let mut copies = Vec::new();
    for relative in unchanged {
        let name = relative.to_string_lossy();
        // Untracked, ignored, or modified in the work tree only. Anything
        // the index says (staged, added, unmerged), or an error: the run's.
        // Not trimmed: the first column of a porcelain line is the index.
        let status = match crate::core::lane_git::async_git(worktree) {
            Ok(mut git) => git
                .args(["status", "--porcelain", "--untracked-files=normal", "--"])
                .arg(format!(":(top,literal){name}"))
                .output()
                .await
                .ok(),
            Err(_) => None,
        };
        let index_untouched = status.is_some_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .all(|line| line.starts_with("??") || line.starts_with(" M"))
        });
        if index_untouched {
            copies.push(relative);
        }
    }
    copies
}

/// Takes those copies back, once the removal is decided: the tracked ones
/// restored, the others removed. Git would refuse the removal over them.
/// Returns the ones taken.
async fn drop_copied_agent_configs(repo_path: &Path, worktree: &Path) -> Vec<PathBuf> {
    let unchanged = copied_agent_configs(repo_path, worktree).await;
    for relative in &unchanged {
        let name = relative.to_string_lossy();
        let tracked = git_text_output(worktree, &["ls-files", "--error-unmatch", "--", &name])
            .await
            .is_some();
        if tracked {
            let _ = git_text_output(worktree, &["checkout", "--", &name]).await;
        } else {
            let _ = std::fs::remove_file(worktree.join(relative));
        }
    }
    unchanged
}

/// Puts back the copies `drop_copied_agent_configs` took, and only those:
/// any other config is as the run left it.
fn restore_copied_agent_configs(repo_path: &Path, worktree: &Path, taken: &[PathBuf]) {
    let Ok(root) = worktree.canonicalize() else {
        return;
    };
    for relative in taken {
        let target = worktree.join(relative);
        // Never through a link, in the file or above it: the copy taken was
        // a plain file at its own path.
        let own_place = target
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .zip(relative.parent())
            .is_some_and(|(found, parent)| found == root.join(parent));
        let linked =
            std::fs::symlink_metadata(&target).is_ok_and(|found| !found.file_type().is_file());
        if !own_place || linked {
            tracing::warn!(
                path = %target.display(),
                "agent config not put back in the kept worktree: its place changed"
            );
            continue;
        }
        if let Err(error) = std::fs::copy(repo_path.join(relative), &target) {
            tracing::warn!(
                path = %target.display(),
                "agent config not put back in the kept worktree: {error}"
            );
        }
    }
}

async fn nested_worktrees(repo_path: &Path, parent: &Path) -> Vec<PathBuf> {
    let (repo, parent) = (repo_path.to_path_buf(), parent.to_path_buf());
    tokio::task::spawn_blocking(move || crate::core::worktree::nested_worktrees(&repo, &parent))
        .await
        .unwrap_or_default()
}

#[cfg(test)]
tokio::task_local! {
    /// Runs where a refused removal used to be forced: lets a test write work
    /// after the last inspection.
    pub(crate) static AFTER_REFUSAL: std::sync::Arc<dyn Fn(&Path) + Send + Sync>;
}

/// Removes a checkout found clean, nested worktrees first, never with
/// `--force`: no re-read can exclude a write landing just before a forced
/// removal, so a refusal (late work, a lock) keeps the set and says why
/// (`Ok(Some(reason))`). The one forced removal is a checkout with initialised
/// submodules, which git never removes otherwise, re-inspected first (KT-1809).
pub(crate) async fn remove_inspected_checkout(
    repo_path: &Path,
    worktree: &Path,
) -> Result<Option<String>> {
    let mut targets = nested_worktrees(repo_path, worktree).await;
    targets.push(worktree.to_path_buf());
    let mut dropped: Vec<PathBuf> = Vec::new();
    for target in targets {
        if !target.exists() {
            continue;
        }
        if target == worktree {
            dropped = drop_copied_agent_configs(repo_path, worktree).await;
        }
        let Err(refusal) = remove_clean_checkout(repo_path, &target).await else {
            continue;
        };
        #[cfg(test)]
        let _ = AFTER_REFUSAL.try_with(|hook| hook(&target));
        let Err(held) = remove_checkout_with_submodules(repo_path, &target).await else {
            continue;
        };
        let found = held.unwrap_or_else(|| refusal.to_string());
        let which = if target == worktree {
            String::new()
        } else {
            format!(" (nested worktree `{}`)", target.display())
        };
        // Kept after all, and maybe resumed: it gets those configs back.
        restore_copied_agent_configs(repo_path, worktree, &dropped);
        return Ok(Some(format!("{found}{which}; kept")));
    }
    Ok(None)
}

/// An initialised submodule, as `git submodule foreach` reports it.
struct SubmoduleState {
    path: String,
    git_dir: PathBuf,
    changes: usize,
}

/// Every initialised submodule of `checkout`, nested ones included.
/// `None`: git could not say.
async fn submodule_states(checkout: &Path) -> Option<Vec<SubmoduleState>> {
    let report = git_text_output(
        checkout,
        &[
            "submodule",
            "foreach",
            "--quiet",
            "--recursive",
            "printf '@%s\\n' \"$displaypath\" \
             && printf '>%s\\n' \"$(git rev-parse --absolute-git-dir)\" \
             && git status --porcelain --untracked-files=normal",
        ],
    )
    .await?;
    let mut states: Vec<SubmoduleState> = Vec::new();
    for line in report.lines() {
        if let Some(path) = line.strip_prefix('@') {
            states.push(SubmoduleState {
                path: path.to_string(),
                git_dir: PathBuf::new(),
                changes: 0,
            });
            continue;
        }
        let state = states.last_mut()?;
        match line.strip_prefix('>') {
            Some(dir) if state.git_dir.as_os_str().is_empty() => state.git_dir = dir.into(),
            _ => state.changes += 1,
        }
    }
    Some(states)
}

/// Every submodule clone the worktree owns, deinitialised ones included:
/// they live under its git directory and go with a forced removal.
fn submodule_clones(modules: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(modules) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("HEAD").is_file() && path.join("objects").is_dir() {
            submodule_clones(&path.join("modules"), found)?;
            found.push(path);
        } else {
            submodule_clones(&path, found)?;
        }
    }
    Ok(())
}

/// The first repository kept inside `dir` itself (a `.git` directory, where
/// a worktree or an absorbed submodule has a file): a forced removal takes it.
fn embedded_repository(dir: &Path, top: bool) -> std::io::Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if entry.file_name() == ".git" {
            if top {
                continue;
            }
            return Ok(Some(dir.to_path_buf()));
        }
        if let Some(found) = embedded_repository(&entry.path(), false)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// What the disk says of the submodules of `checkout`: the canonical
/// `modules` directory, its clones, and a repository embedded in the tree.
type SubmoduleDisk = (PathBuf, Vec<PathBuf>, Option<PathBuf>);

fn submodule_disk(checkout: &Path, git_dir: &Path) -> std::io::Result<SubmoduleDisk> {
    let modules = git_dir.canonicalize()?.join("modules");
    let mut clones = Vec::new();
    submodule_clones(&modules, &mut clones)?;
    let embedded = embedded_repository(checkout, true)?;
    Ok((modules, clones, embedded))
}

/// Whether `checkout` has submodules a forced removal would take, all clean
/// (`Ok(false)`: it has none). `Err` says what the removal would lose: a
/// change, a repository kept inside the worktree, or a commit no remote branch
/// of its submodule holds. Anything git or the disk cannot say counts as work.
async fn submodules_hold_nothing(checkout: &Path) -> std::result::Result<bool, String> {
    let unreadable = || "submodules git cannot read".to_string();
    let git_dir = git_text_output(checkout, &["rev-parse", "--absolute-git-dir"])
        .await
        .ok_or_else(unreadable)?;
    let states = submodule_states(checkout).await.ok_or_else(unreadable)?;
    let (root, git_dir) = (checkout.to_path_buf(), PathBuf::from(git_dir));
    let (modules, clones, embedded) =
        tokio::task::spawn_blocking(move || submodule_disk(&root, &git_dir))
            .await
            .map_err(|_| unreadable())?
            .map_err(|error| format!("submodules the disk cannot read ({error})"))?;
    if let Some(embedded) = embedded {
        let at = embedded.strip_prefix(checkout).unwrap_or(&embedded);
        return Err(format!(
            "a repository kept inside the worktree, at `{}`",
            at.display()
        ));
    }
    for state in &states {
        if state.changes > 0 {
            return Err(format!(
                "{} uncommitted change(s) in its submodule `{}`",
                state.changes, state.path
            ));
        }
        let owned = state
            .git_dir
            .canonicalize()
            .is_ok_and(|dir| dir.starts_with(&modules));
        if !owned {
            return Err(format!(
                "the repository of its submodule `{}`, kept outside its git directory",
                state.path
            ));
        }
    }
    for clone in &clones {
        let name = clone.strip_prefix(&modules).unwrap_or(clone).display();
        // A worktree of the submodule's own repository lives in its clone.
        let linked = std::fs::read_dir(clone.join("worktrees"))
            .is_ok_and(|mut entries| entries.next().is_some());
        if linked {
            return Err(format!(
                "its submodule `{name}`, which has worktrees of its own"
            ));
        }
        // Its recorded work tree may be gone (deinitialised, removed).
        let unpushed = crate::core::cmd::async_git_cmd() // lane-git-allow: a submodule clone of a workflow run's worktree, never a worker lane
            .arg("--git-dir")
            .arg(clone)
            .arg("--work-tree")
            .arg(clone)
            .args(["rev-list", "-n1", "HEAD", "--all", "--not", "--remotes"])
            .output()
            .await;
        match unpushed {
            Ok(output) if output.status.success() && output.stdout.is_empty() => {}
            Ok(output) if output.status.success() => {
                let commit = String::from_utf8_lossy(&output.stdout);
                return Err(format!(
                    "commit {} in its submodule `{name}`, which no remote branch of that \
                     submodule holds",
                    commit.trim().get(..12).unwrap_or(commit.trim())
                ));
            }
            _ => return Err(format!("its submodule `{name}`, which git cannot read")),
        }
    }
    Ok(!states.is_empty() || !clones.is_empty())
}

/// KT-1809 — git never removes, without force, a worktree with an initialised
/// submodule. Forced only once a fresh inspection, made after git's refusal,
/// finds the checkout clean and `submodules_hold_nothing`. `Err(Some(_))` says
/// what a submodule holds; `Err(None)`: the refusal was not about submodules.
pub(crate) async fn remove_checkout_with_submodules(
    repo_path: &Path,
    checkout: &Path,
) -> std::result::Result<(), Option<String>> {
    let late = match inspect_interrupted_checkout(checkout).await {
        Ok(InterruptedCheckout::Removable { .. } | InterruptedCheckout::DetachedReachable) => None,
        Ok(InterruptedCheckout::Dirty { entries }) => Some(format!(
            "{entries} uncommitted change(s) written since the inspection"
        )),
        Ok(InterruptedCheckout::Detached) => Some("a HEAD detached since the inspection".into()),
        Err(error) => Some(format!(
            "a state git cannot read since the inspection ({error})"
        )),
    };
    if late.is_some() {
        return Err(late);
    }
    if !submodules_hold_nothing(checkout).await.map_err(Some)? {
        return Err(None);
    }
    let removed = crate::core::cmd::async_git_cmd() // lane-git-allow: the worktree of a workflow run, removed from the registered checkout
        .args(["worktree", "remove", "--force"])
        .arg(checkout)
        .current_dir(repo_path)
        .output()
        .await;
    if removed.is_ok_and(|output| output.status.success()) {
        Ok(())
    } else {
        Err(None)
    }
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

/// Decide whether the branch backing a worktree should outlive cleanup.
///
/// Judged on the branch itself, from any checkout of the repository: HEAD may
/// be detached, and the worktree directory may already be gone (KT-1806).
/// Returns `Some(PreservedBranch)` when the branch holds commits not on any
/// known base ref (upstream → origin/main → origin/master → main → master).
/// Returns `None` only when it is fully synced with one of those bases, or
/// does not exist (nothing to salvage). When no base ref resolves, or git
/// cannot read the branch, we return `Some` with `ahead=0` to err on the side
/// of preservation — losing real work is much worse than leaving a stale
/// empty branch behind.
async fn check_branch_for_preservation(checkout: &Path, branch: &str) -> Option<PreservedBranch> {
    let tip = format!("refs/heads/{branch}");
    // The branch tip is the anchor we want to be able to recover.
    let head_sha = match branch_tip(checkout, &tip).await {
        BranchTip::At(sha) => sha,
        BranchTip::Absent => return None,
        BranchTip::Unreadable => {
            return Some(PreservedBranch {
                branch_name: branch.to_string(),
                head_sha: String::new(),
                ahead: 0,
                pushed_upstream: false,
            })
        }
    };

    // Was an upstream set on the branch ? Tells us whether the agent tried
    // to push at all. `@{u}` resolves only when set.
    let upstream = format!("{branch}@{{u}}");
    let pushed_upstream = git_text_output(checkout, &["rev-parse", "--abbrev-ref", &upstream])
        .await
        .is_some();

    // Walk through plausible bases in order of relevance. The first one
    // that resolves wins — count commits ahead.
    let mut bases: Vec<&str> = Vec::with_capacity(5);
    if pushed_upstream {
        bases.push(&upstream);
    }
    bases.extend(["origin/main", "origin/master", "main", "master"]);

    for base in bases {
        let count_str = git_text_output(
            checkout,
            &["rev-list", "--count", &format!("{base}..{tip}")],
        )
        .await;
        if let Some(count_str) = count_str {
            // A count git did not give is no proof the branch is synced.
            let Ok(ahead) = count_str.trim().parse::<u32>() else {
                continue;
            };
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

enum BranchTip {
    At(String),
    Absent,
    /// Git could not say: never a reason to delete the branch.
    Unreadable,
}

async fn branch_tip(checkout: &Path, tip: &str) -> BranchTip {
    let Ok(mut git) = crate::core::lane_git::async_git(checkout) else {
        return BranchTip::Unreadable;
    };
    match git
        .args(["rev-parse", "--verify", "--quiet", tip])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            BranchTip::At(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        // `--verify --quiet` exits 1 on a ref that does not exist.
        Ok(out) if out.status.code() == Some(1) => BranchTip::Absent,
        _ => BranchTip::Unreadable,
    }
}

/// Run `git <args>` in `cwd`, return trimmed stdout if exit was 0.
async fn git_text_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = crate::core::lane_git::async_git(cwd)
        .ok()?
        .args(args)
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

pub(crate) async fn fetch_remote_branch(
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
    let mut command = crate::core::cmd::async_git_cmd();
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
    /// HEAD is on no branch and no ref holds its commit: removing the
    /// worktree could strand it.
    Detached,
    /// HEAD is on no branch, but a ref already holds its commit (a PR checkout
    /// of `origin/x`, a tag): nothing would be stranded.
    DetachedReachable,
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
    inspect_checkout(worktree, &[]).await
}

/// `inspect_interrupted_checkout` for a run's own worktree: the agent configs
/// Kronn copied into it, still as copied, are not changes. Reads only.
pub(crate) async fn inspect_run_checkout(
    repo_path: &Path,
    worktree: &Path,
) -> std::result::Result<InterruptedCheckout, String> {
    let copies = copied_agent_configs(repo_path, worktree).await;
    inspect_checkout(worktree, &copies).await
}

async fn inspect_checkout(
    worktree: &Path,
    not_changes: &[PathBuf],
) -> std::result::Result<InterruptedCheckout, String> {
    let mut paths: Vec<String> = Vec::new();
    if !not_changes.is_empty() {
        paths.extend(["--".to_string(), ":(top)".to_string()]);
        paths.extend(
            not_changes
                .iter()
                .map(|path| format!(":(top,exclude,literal){}", path.to_string_lossy())),
        );
    }
    let status = crate::core::lane_git::async_git(worktree)?
        // On the command line: a config hiding untracked files hides work.
        .args([
            "status",
            "--porcelain",
            "--ignore-submodules=none",
            "--untracked-files=normal",
        ])
        .args(&paths)
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
        let holder = git_text_output(
            worktree,
            &[
                "for-each-ref",
                "--contains",
                "HEAD",
                "--count=1",
                "--format=%(refname)",
                // Not `refs/worktree`, `refs/bisect`: they go with the worktree.
                "refs/heads",
                "refs/tags",
                "refs/remotes",
            ],
        )
        .await;
        // No output, or an error: nothing proves a ref holds it.
        return Ok(match holder {
            Some(holder) if !holder.is_empty() => InterruptedCheckout::DetachedReachable,
            _ => InterruptedCheckout::Detached,
        });
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
    let output = crate::core::cmd::async_git_cmd()
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
    // `update-ref` would leave a checkout that is on it without a branch.
    let listed = crate::core::cmd::async_git_cmd()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .await
        .context("Failed to run git worktree list")?;
    if !listed.status.success() {
        anyhow::bail!("branch `{branch}` not deleted: the worktrees cannot be listed");
    }
    let checked_out = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .any(|line| line.strip_prefix("branch refs/heads/") == Some(branch));
    if checked_out {
        anyhow::bail!("branch `{branch}` not deleted: a checkout is on it");
    }
    let output = crate::core::cmd::async_git_cmd()
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

async fn remove_nested_worktrees(repo_path: &Path, parent: &Path) -> usize {
    let (repo, parent) = (repo_path.to_path_buf(), parent.to_path_buf());
    tokio::task::spawn_blocking(move || {
        crate::core::worktree::remove_nested_worktrees(&repo, &parent)
    })
    .await
    .unwrap_or(0)
}

async fn prune_kronn_worktrees(repo_path: &Path) -> usize {
    let repo = repo_path.to_path_buf();
    tokio::task::spawn_blocking(move || crate::core::worktree::prune_kronn_worktrees(&repo))
        .await
        .unwrap_or(0)
}

/// Boot pass: drops the admin entries of Kronn worktrees whose checkout is
/// gone, in every project repository. Returns how many were pruned.
pub async fn prune_orphan_worktrees(db: &crate::db::Database) -> usize {
    let projects = match db.with_conn(crate::db::projects::list_projects).await {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!("Orphan worktree prune skipped: {error}");
            return 0;
        }
    };
    let mut pruned = 0;
    for project in projects {
        let repo = crate::core::scanner::resolve_host_path(&project.path);
        if repo.join(".kronn").is_dir() && repo.join(".git").exists() {
            pruned += prune_kronn_worktrees(&repo).await;
        }
    }
    pruned
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
        let ws =
            Self::create_before_hooks(repo_path, workflow_name, run_id, hooks, base_ref).await?;
        ws.after_create().await?;
        Ok(ws)
    }

    /// [`Self::create`] without its `after_create` hook, so the caller can act
    /// on the fresh checkout (record its state) before project commands run.
    pub async fn create_before_hooks(
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
        // Docker only, and never twice: a native run must not grow ~/.gitconfig.
        crate::core::git_global_config::ensure_safe_directory_async(repo_path.to_path_buf()).await;

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
        if let Some(p) = repo_path.to_str() {
            let p = p.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                crate::core::mcp_scanner::ensure_local_exclude(&p, ".kronn/")
            })
            .await;
        }
        let worktree_path = worktree_base.join(build_worktree_dir_name(workflow_name, run_id));

        crate::core::git_global_config::ensure_safe_directory_async(worktree_path.clone()).await;

        // Create the worktree with a new branch. A SHA start point sets up no
        // tracking, so `@{u}` keeps meaning "the agent pushed".
        let mut add = crate::core::cmd::async_git_cmd();
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

        // Same agent configs as a discussion worktree, so an Agent step keeps
        // the project's MCP servers and its strict MCP config.
        crate::core::worktree::copy_agent_configs(repo_path, &worktree_path);

        Ok(Self {
            path: worktree_path,
            branch,
            repo_path: repo_path.to_path_buf(),
            hooks,
        })
    }

    /// Run the after_create hook.
    pub async fn after_create(&self) -> Result<()> {
        self.run_hook("after_create").await
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
    ///   - The branch is preserved (left alive in the parent repo) when it
    ///     has local commits not present on any known base
    ///     ref. Returned in `CleanupOutcome.preserved`. The runner records
    ///     it on the run so the UI can surface "commit available here".
    ///   - The branch is deleted when the worktree is fully synced with a
    ///     known base — nothing of value would survive its deletion.
    ///
    /// Failures: best-effort. If the preserve check itself errors out, we
    /// default to preserving the branch (safer than silently dropping work).
    ///
    /// Forced: test fixtures only. A finished run uses `cleanup_keeping_work`
    /// (KT-1806).
    #[cfg(test)]
    pub async fn cleanup(self) -> Result<CleanupOutcome> {
        self.cleanup_with(false).await
    }

    /// `cleanup` for a finished run, successful or not: a checkout (nested
    /// worktrees included) holding work is left in place with its branch and
    /// reported in `kept`; a clean one is removed without `--force`.
    pub async fn cleanup_keeping_work(self) -> Result<CleanupOutcome> {
        self.cleanup_with(true).await
    }

    async fn cleanup_with(self, keep_work: bool) -> Result<CleanupOutcome> {
        // A kept checkout keeps its branch: its commits ahead are reported too.
        let kept = |reason: String, preserved: Option<PreservedBranch>| CleanupOutcome {
            preserved,
            kept: Some(KeptCheckout {
                path: self.path.clone(),
                reason,
            }),
        };
        if keep_work && self.path.exists() {
            if let Some(reason) = work_to_keep(&self.repo_path, &self.path).await {
                let preserved = check_branch_for_preservation(&self.repo_path, &self.branch).await;
                return Ok(kept(reason, preserved));
            }
        }
        // A failing or hung teardown hook must not leave the checkout behind.
        if let Err(error) = self.run_hook("before_remove").await {
            tracing::warn!(
                path = %self.path.display(),
                "before_remove hook failed, removing the worktree anyway: {error:#}"
            );
        }

        // Read from the repository: the worktree may be detached, or gone.
        let preserve = check_branch_for_preservation(&self.repo_path, &self.branch).await;

        if keep_work {
            if let Some(reason) = remove_inspected_checkout(&self.repo_path, &self.path).await? {
                return Ok(kept(reason, preserve));
            }
        } else {
            self.force_remove().await?;
        }
        prune_kronn_worktrees(&self.repo_path).await;

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
                    kept: None,
                }
            } else {
                // Fully synced — safe to drop the branch ref, where it was
                // read: a commit landed since keeps it.
                let tip = format!("refs/heads/{}", self.branch);
                if let BranchTip::At(sha) = branch_tip(&self.repo_path, &tip).await {
                    let late = check_branch_for_preservation(&self.repo_path, &self.branch).await;
                    if late.is_some() {
                        return Ok(CleanupOutcome {
                            preserved: late,
                            kept: None,
                        });
                    }
                    if let Err(error) = delete_branch_at(&self.repo_path, &self.branch, &sha).await
                    {
                        tracing::warn!("{error:#}");
                    }
                }
                CleanupOutcome::default()
            };

        tracing::info!("Cleaned up worktree: {}", self.path.display());
        Ok(outcome)
    }

    async fn force_remove(&self) -> Result<()> {
        remove_nested_worktrees(&self.repo_path, &self.path).await;
        let output = crate::core::cmd::async_git_cmd()
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
        Ok(())
    }

    /// Purge a worktree left behind by an already-terminal run after a crash.
    /// Unlike `cleanup`, this never runs user hooks and never deletes the
    /// branch ref: the original runner may already have persisted/pushed it,
    /// and boot recovery must remove discoverable checkout data only.
    /// `Ok(false)`: the checkout holds work and is kept (KT-1096).
    pub async fn purge_terminal_checkout(repo_path: &Path, worktree_path: &Path) -> Result<bool> {
        if let Some(reason) = work_to_keep(repo_path, worktree_path).await {
            tracing::warn!(
                path = %worktree_path.display(),
                "terminal workflow worktree kept with its branch: it holds {reason}"
            );
            return Ok(false);
        }
        let kept = remove_inspected_checkout(repo_path, worktree_path).await;
        prune_kronn_worktrees(repo_path).await;
        if let Some(reason) = kept? {
            tracing::warn!(
                path = %worktree_path.display(),
                "terminal workflow worktree kept with its branch: it holds {reason}"
            );
            return Ok(false);
        }
        Ok(true)
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
            #[cfg(not(test))]
            const HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
            #[cfg(test)]
            const HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
            let mut command = hook_command(cmd, &self.path);
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

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::{main_tree_wait_from, MainTreeGuard, MainTreeRefusal};
    use std::time::Duration;

    #[test]
    fn a_workspace_hook_gets_a_built_environment() {
        let dir = tempfile::TempDir::new().unwrap();
        let command = crate::core::child_env::with_parent_env(
            &[
                ("PATH", "/usr/bin"),
                ("KRONN_AUTH_TOKEN", "admin"),
                ("OPENAI_API_KEY", "sk"),
            ],
            || super::hook_command("npm ci", dir.path()),
        );
        let env: Vec<String> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_some())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert!(env.contains(&"PATH".to_string()));
        assert!(!env.contains(&"KRONN_AUTH_TOKEN".to_string()));
        assert!(!env.contains(&"OPENAI_API_KEY".to_string()));
    }

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
        let _ = crate::core::cmd::async_git_cmd()
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["config", "user.email", "test@kronn.local"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["config", "user.name", "test"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
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
        let result = check_branch_for_preservation(&repo, "main").await;
        assert!(result.is_none(), "a synced branch should NOT be preserved");
        let absent = check_branch_for_preservation(&repo, "no-such-branch").await;
        assert!(absent.is_none(), "no branch, nothing to preserve");
    }

    #[tokio::test]
    async fn preserve_when_branch_ahead_of_main_returns_some() {
        // Mirror the production scenario: a worktree branched off main
        // gets its own commit, leaving main behind by 1.
        let (_dir, repo) = make_test_repo().await;
        let _ = crate::core::cmd::async_git_cmd()
            .args(["checkout", "-q", "-b", "kronn/test"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        std::fs::write(repo.join("CHANGELOG.md"), "v1\n").unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
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
        let _ = crate::core::cmd::async_git_cmd()
            .args(["init", "-q", "-b", "weird-default"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["config", "user.email", "test@kronn.local"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["config", "user.name", "test"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        std::fs::write(repo.join("README.md"), "x\n").unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&repo)
            .output()
            .await
            .unwrap();

        let result = check_branch_for_preservation(&repo, "weird-default").await;
        let preserved = result.expect("no resolvable base → defensive preserve");
        assert_eq!(preserved.ahead, 0, "ahead unknown → 0 (still preserve)");
    }

    // ─── operator checkout stays untouched (KT-1125) ───────────────────

    async fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = crate::core::cmd::async_git_cmd()
            .args(args)
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    async fn assert_operator_untouched(repo: &Path, gitignore_before: Option<Vec<u8>>) {
        assert_eq!(git_out(repo, &["status", "--porcelain"]).await, "");
        assert_eq!(
            std::fs::read(repo.join(".gitignore")).ok(),
            gitignore_before
        );
        let ignored = crate::core::cmd::async_git_cmd()
            .args(["check-ignore", "-q", ".kronn/"])
            .current_dir(repo)
            .status()
            .await
            .unwrap();
        assert!(ignored.success(), ".kronn/ must be ignored");
    }

    #[tokio::test]
    async fn worktree_creation_leaves_an_absent_gitignore_absent() {
        let (_dir, repo) = make_test_repo().await;
        assert!(!repo.join(".gitignore").exists());
        let ws = Workspace::create(&repo, "wf", "11111111-a", None, None)
            .await
            .unwrap();
        assert_operator_untouched(&repo, None).await;
        let _ = ws.cleanup().await;
    }

    #[tokio::test]
    async fn worktree_creation_leaves_a_tracked_gitignore_byte_identical() {
        let (_dir, repo) = make_test_repo().await;
        let body = b"target/\nno-trailing-newline".to_vec();
        std::fs::write(repo.join(".gitignore"), &body).unwrap();
        git_out(&repo, &["add", ".gitignore"]).await;
        git_out(&repo, &["commit", "-q", "-m", "gitignore"]).await;
        let ws = Workspace::create(&repo, "wf", "22222222-a", None, None)
            .await
            .unwrap();
        assert_operator_untouched(&repo, Some(body)).await;
        let _ = ws.cleanup().await;
    }

    #[tokio::test]
    async fn worktree_creation_keeps_an_existing_ignore_rule_untouched() {
        let (_dir, repo) = make_test_repo().await;
        let body = b".kronn/\n".to_vec();
        std::fs::write(repo.join(".gitignore"), &body).unwrap();
        git_out(&repo, &["add", ".gitignore"]).await;
        git_out(&repo, &["commit", "-q", "-m", "gitignore"]).await;
        let ws = Workspace::create(&repo, "wf", "33333333-a", None, None)
            .await
            .unwrap();
        assert_operator_untouched(&repo, Some(body)).await;
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default();
        assert!(!exclude.lines().any(|l| l == ".kronn/"));
        let _ = ws.cleanup().await;
    }

    #[tokio::test]
    async fn local_exclude_is_idempotent_and_works_from_a_linked_worktree() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "wf", "44444444-a", None, None)
            .await
            .unwrap();
        let again = Workspace::create(&repo, "wf", "55555555-a", None, None)
            .await
            .unwrap();
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|l| *l == ".kronn/").count(), 1);

        // From a linked worktree the rule lands in the shared common dir.
        crate::core::mcp_scanner::ensure_local_exclude(&ws.path.to_string_lossy(), ".kronn-extra/");
        crate::core::mcp_scanner::ensure_local_exclude(&ws.path.to_string_lossy(), ".kronn-extra/");
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|l| *l == ".kronn-extra/").count(), 1);
        assert!(!ws.path.join(".git/info").exists());
        assert_eq!(git_out(&ws.path, &["status", "--porcelain"]).await, "");
        let _ = again.cleanup().await;
        let _ = ws.cleanup().await;
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
        let _ = crate::core::cmd::async_git_cmd()
            .args(["add", "."])
            .current_dir(&ws.path)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
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
        let branch_check = crate::core::cmd::async_git_cmd()
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
        let _ = crate::core::cmd::async_git_cmd()
            .args(["add", "."])
            .current_dir(&child_ws.path)
            .output()
            .await
            .unwrap();
        let _ = crate::core::cmd::async_git_cmd()
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
    async fn an_isolated_worktree_gets_the_project_agent_configs() {
        let (_dir, repo) = make_test_repo().await;
        let mcp = r#"{"mcpServers":{"kronn-internal":{"command":"kronn-mcp"}}}"#;
        std::fs::write(repo.join(".mcp.json"), mcp).unwrap();
        std::fs::create_dir_all(repo.join(".gemini")).unwrap();
        std::fs::write(repo.join(".gemini/settings.json"), "{}").unwrap();

        let ws = Workspace::create(&repo, "mcp-wf", "0a0b0c0d-run", None, None)
            .await
            .expect("create worktree");

        assert_eq!(
            std::fs::read_to_string(ws.path.join(".mcp.json")).unwrap(),
            mcp,
            "the agent launcher reads `.mcp.json` from its working directory"
        );
        assert!(ws.path.join(".gemini/settings.json").is_file());
        assert!(!ws.path.join(".vibe/config.toml").exists());
        ws.cleanup().await.expect("cleanup");
    }

    #[tokio::test]
    async fn a_hung_before_remove_hook_does_not_keep_the_worktree() {
        let (_dir, repo) = make_test_repo().await;
        let hooks = WorkspaceHooks {
            after_create: None,
            before_run: None,
            after_run: None,
            before_remove: Some("sleep 30".into()),
        };
        let ws = Workspace::create(&repo, "hung-hook", "deadbeef-hook", Some(hooks), None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();

        ws.cleanup()
            .await
            .expect("cleanup goes on after the hook timeout");

        assert!(!path.exists(), "the worktree is removed anyway");
    }

    async fn porcelain(repo: &std::path::Path) -> String {
        let out = crate::core::cmd::async_git_cmd()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo)
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    async fn add_nested_pr_worktree(run_worktree: &std::path::Path) -> std::path::PathBuf {
        let out = crate::core::cmd::async_git_cmd()
            .args(["worktree", "add", "-q", "-b", "pr-1995", ".kronn/pr-1995"])
            .current_dir(run_worktree)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        run_worktree.join(".kronn/pr-1995")
    }

    #[tokio::test]
    async fn cleanup_removes_a_worktree_nested_in_the_run_worktree() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "pr-review", "0d9e49ec-run", None, None)
            .await
            .expect("create worktree");
        let nested = add_nested_pr_worktree(&ws.path).await;
        assert!(porcelain(&repo).await.contains("pr-1995"));

        ws.cleanup().await.expect("cleanup");

        let listed = porcelain(&repo).await;
        assert!(!nested.exists());
        assert!(!listed.contains("prunable"), "{listed}");
        assert!(!listed.contains("pr-1995"), "{listed}");
    }

    #[tokio::test]
    async fn the_boot_purge_keeps_a_terminal_checkout_holding_uncommitted_work() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "boot-keep", "11223344-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        drop(ws);
        std::fs::write(path.join("wip.txt"), "work\n").unwrap();

        let removed = Workspace::purge_terminal_checkout(&repo, &path)
            .await
            .expect("purge");
        assert!(!removed);
        assert!(path.join("wip.txt").exists());

        std::fs::remove_file(path.join("wip.txt")).unwrap();
        let removed = Workspace::purge_terminal_checkout(&repo, &path)
            .await
            .expect("purge");
        assert!(removed, "once clean, a later boot removes it");
        assert!(!path.exists());
    }

    /// A nested worktree with a modified tracked file and an untracked one.
    async fn dirty_nested(run_worktree: &std::path::Path) -> std::path::PathBuf {
        let nested = add_nested_pr_worktree(run_worktree).await;
        std::fs::write(nested.join("README.md"), "changed\n").unwrap();
        std::fs::write(nested.join("wip.txt"), "work\n").unwrap();
        nested
    }

    #[tokio::test]
    async fn a_stopped_run_keeps_a_dirty_nested_worktree_and_its_parent() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "nested-keep", "aa11bb22-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        let nested = dirty_nested(&parent).await;
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome.kept.expect("kept");
        assert!(kept.reason.contains("nested worktree"), "{}", kept.reason);
        assert!(nested.join("wip.txt").exists());
        assert_eq!(
            std::fs::read_to_string(nested.join("README.md")).unwrap(),
            "changed\n"
        );
    }

    #[tokio::test]
    async fn the_boot_purge_keeps_a_dirty_nested_worktree() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "nested-boot", "cc33dd44-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        drop(ws);
        let nested = dirty_nested(&parent).await;
        let removed = Workspace::purge_terminal_checkout(&repo, &parent)
            .await
            .expect("purge");
        assert!(!removed);
        assert!(nested.join("wip.txt").exists());
    }

    #[tokio::test]
    async fn a_stopped_run_still_removes_a_clean_nested_worktree() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "nested-clean", "ee55ff66-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        let nested = add_nested_pr_worktree(&parent).await;
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(outcome.kept.is_none());
        assert!(!nested.exists() && !parent.exists());
        let listed = porcelain(&repo).await;
        assert!(!listed.contains("pr-1995"), "{listed}");
    }

    /// KT-1806 — a PR checkout on a detached commit a ref already holds strands
    /// nothing: it must not keep a finished run's worktree forever.
    #[tokio::test]
    async fn a_detached_nested_checkout_of_a_known_commit_is_not_work() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "nested-detached", "a1b2c3d4-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        let out = crate::core::cmd::async_git_cmd()
            .args(["worktree", "add", "-q", "--detach", ".kronn/pr-7", "HEAD"])
            .current_dir(&parent)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(
            outcome.kept.is_none(),
            "{:?}",
            outcome.kept.map(|kept| kept.reason)
        );
        assert!(!parent.exists());
    }

    /// A detached HEAD whose commit no ref holds is still work.
    #[tokio::test]
    async fn a_detached_checkout_of_an_unreferenced_commit_is_kept() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "detached-orphan", "e5f6a7b8-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        for args in [
            &["checkout", "-q", "--detach"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "orphan",
            ][..],
        ] {
            let out = crate::core::cmd::async_git_cmd()
                .args(args)
                .current_dir(&parent)
                .output()
                .await
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome.kept.expect("an unreferenced commit is kept");
        assert!(kept.reason.contains("detached"), "{}", kept.reason);
        assert!(parent.exists());
    }

    /// KT-1806 — a kept checkout still records the commits its branch holds
    /// beyond the base, as a removed one does.
    #[tokio::test]
    async fn a_kept_checkout_still_reports_its_branch_commits() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "kept-ahead", "c9d8e7f6-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        let out = crate::core::cmd::async_git_cmd()
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "ahead",
            ])
            .current_dir(&parent)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::write(parent.join("wip.txt"), "work\n").unwrap();

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(outcome.kept.is_some());
        assert_eq!(
            outcome
                .preserved
                .expect("the commit ahead is reported")
                .ahead,
            1
        );
    }

    async fn branch_tip(repo: &Path, branch: &str) -> Option<String> {
        git_text_output(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .await
    }

    /// KT-1806 — the branch is judged on its own commits: a HEAD detached on a
    /// commit a ref holds says nothing of what the run committed before.
    #[tokio::test]
    async fn a_branch_ahead_survives_a_head_detached_on_a_known_commit() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "detached-ahead", "0a1b2c3d-run", None, None)
            .await
            .expect("create worktree");
        let (parent, branch) = (ws.path.clone(), ws.branch.clone());
        git(
            &parent,
            &["commit", "-q", "--allow-empty", "-m", "the run's work"],
        );
        let work = branch_tip(&repo, &branch).await.expect("the branch");
        git(&parent, &["checkout", "-q", "--detach", "main"]);

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(
            outcome.kept.is_none(),
            "{:?}",
            outcome.kept.map(|kept| kept.reason)
        );
        let preserved = outcome.preserved.expect("the branch holds a commit");
        assert_eq!(
            (preserved.ahead, preserved.head_sha.as_str()),
            (1, work.as_str())
        );
        assert_eq!(
            branch_tip(&repo, &branch).await.as_deref(),
            Some(work.as_str())
        );
    }

    /// A worktree directory already gone leaves the branch as the only record.
    #[tokio::test]
    async fn a_branch_ahead_survives_a_worktree_directory_already_gone() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "gone-ahead", "4e5f6a7b-run", None, None)
            .await
            .expect("create worktree");
        let (parent, branch) = (ws.path.clone(), ws.branch.clone());
        git(
            &parent,
            &["commit", "-q", "--allow-empty", "-m", "the run's work"],
        );
        let work = branch_tip(&repo, &branch).await.expect("the branch");
        std::fs::remove_dir_all(&parent).unwrap();

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert_eq!(
            outcome.preserved.expect("the branch holds a commit").ahead,
            1
        );
        assert_eq!(
            branch_tip(&repo, &branch).await.as_deref(),
            Some(work.as_str())
        );
    }

    /// Work written between the inspection and the removal is never forced
    /// away: git refuses, the fresh inspection sees it, the set is kept.
    #[tokio::test]
    async fn work_written_after_the_inspection_is_kept() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "barrier", "77889900-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        let nested = add_nested_pr_worktree(&parent).await;
        drop(ws);
        assert_eq!(work_to_keep(&repo, &parent).await, None);
        // The barrier: written after the inspection said clean.
        std::fs::write(nested.join("late.txt"), "late\n").unwrap();
        std::fs::write(parent.join("late.txt"), "late\n").unwrap();
        let kept = remove_inspected_checkout(&repo, &parent)
            .await
            .expect("removal");
        assert!(kept.is_some());
        assert!(nested.join("late.txt").exists());
        assert!(parent.join("late.txt").exists());
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(["-c", "protocol.file.allow=always"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A worktree with an initialised submodule, which `git worktree remove`
    /// refuses without `--force` even when nothing in it changed.
    async fn worktree_with_submodule(
        repo: &std::path::Path,
        run_id: &str,
    ) -> (Workspace, tempfile::TempDir) {
        let source = tempfile::TempDir::new().unwrap();
        git(source.path(), &["init", "-q", "-b", "main"]);
        git(source.path(), &["config", "user.email", "t@kronn.local"]);
        git(source.path(), &["config", "user.name", "t"]);
        std::fs::write(source.path().join("lib.txt"), "lib\n").unwrap();
        git(source.path(), &["add", "."]);
        git(source.path(), &["commit", "-q", "-m", "lib"]);
        git(
            repo,
            &[
                "submodule",
                "add",
                "-q",
                &source.path().to_string_lossy(),
                "deps/lib",
            ],
        );
        git(repo, &["commit", "-q", "-m", "submodule"]);
        let ws = Workspace::create(repo, "submodule", run_id, None, None)
            .await
            .expect("create worktree");
        git(&ws.path, &["submodule", "update", "-q", "--init"]);
        (ws, source)
    }

    /// The window a forced fallback left open: work written after the last
    /// inspection, where `--force` used to run after a submodule refusal.
    #[tokio::test]
    async fn a_refused_removal_is_never_forced_over_late_work() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, _source) = worktree_with_submodule(&repo, "5b5b5b5b-run").await;
        let path = ws.path.clone();
        drop(ws);
        assert_eq!(
            work_to_keep(&repo, &path).await,
            None,
            "clean before removal"
        );
        let late = path.join("late-user-work.txt");
        let hook_late = late.clone();
        let kept = AFTER_REFUSAL
            .scope(
                std::sync::Arc::new(move |_: &std::path::Path| {
                    std::fs::write(&hook_late, "late\n").unwrap();
                }),
                remove_inspected_checkout(&repo, &path),
            )
            .await
            .expect("removal");
        let reason = kept.expect("kept");
        assert!(
            reason.contains("since the inspection") && reason.contains("kept"),
            "{reason}"
        );
        assert!(late.exists(), "the late work survives");

        // The boot purge keeps it as well.
        assert!(!Workspace::purge_terminal_checkout(&repo, &path)
            .await
            .expect("purge"));
        assert!(late.exists());
    }

    /// `status.showUntrackedFiles=no`, in the user's or the repository's
    /// config, must not hide a new file from the inspection.
    #[tokio::test]
    async fn a_config_hiding_untracked_files_does_not_hide_work() {
        let (_dir, repo) = make_test_repo().await;
        git(&repo, &["config", "status.showUntrackedFiles", "no"]);
        let ws = Workspace::create(&repo, "hidden", "a1a1a1a1-run", None, None)
            .await
            .expect("create worktree");
        std::fs::write(ws.path.join("notes.md"), "draft\n").unwrap();
        let found = work_to_keep(&repo, &ws.path).await;
        assert!(found.is_some_and(|reason| reason.contains("uncommitted")));
    }

    /// A nested worktree whose directory is gone holds nothing.
    #[tokio::test]
    async fn a_nested_worktree_already_deleted_keeps_nothing() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "gone", "b2b2b2b2-run", None, None)
            .await
            .expect("create worktree");
        assert_eq!(work_to_keep(&repo, &ws.path).await, None);
        let nested = ws.path.join(".kronn/pr-7");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &nested.to_string_lossy(),
                "HEAD",
            ],
        );
        std::fs::remove_dir_all(&nested).unwrap();
        assert_eq!(work_to_keep(&repo, &ws.path).await, None);
    }

    /// The agent configs Kronn copies into a worktree are not the run's work,
    /// tracked or not, as long as they still equal the main checkout's.
    #[tokio::test]
    async fn an_agent_config_kronn_copied_is_not_work() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::write(repo.join(".mcp.json"), "{}\n").unwrap();
        std::fs::create_dir_all(repo.join(".gemini")).unwrap();
        std::fs::write(repo.join(".gemini/settings.json"), "{}\n").unwrap();
        git(&repo, &["add", ".gemini/settings.json"]);
        git(&repo, &["commit", "-q", "-m", "tracked agent config"]);
        // A whole directory git does not know.
        std::fs::create_dir_all(repo.join(".vibe")).unwrap();
        std::fs::write(repo.join(".vibe/config.toml"), "a = 1\n").unwrap();
        // Edited locally since: the copy differs from the branch's version.
        std::fs::write(repo.join(".gemini/settings.json"), "{\"local\":1}\n").unwrap();
        let ws = Workspace::create(&repo, "configs", "c3c3c3c3-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        assert_eq!(work_to_keep(&repo, &path).await, None);
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(outcome.kept.is_none());
        assert!(!path.exists());
    }

    /// A config reached through a tracked link is the repository's file,
    /// not a copy Kronn made: it stays where it is.
    #[cfg(unix)] // creates symlinks through std::os::unix
    #[tokio::test]
    async fn an_agent_config_behind_a_tracked_link_is_left_alone() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::create_dir_all(repo.join("conf")).unwrap();
        std::fs::write(repo.join("conf/config.toml"), "a = 1\n").unwrap();
        std::os::unix::fs::symlink("conf", repo.join(".vibe")).unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "linked agent config"]);
        let ws = Workspace::create(&repo, "linked", "b8b8b8b8-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        assert_eq!(work_to_keep(&repo, &path).await, None);
        assert!(path.join("conf/config.toml").exists());
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(
            outcome.kept.is_none(),
            "{:?}",
            outcome.kept.map(|kept| kept.reason)
        );
    }

    /// Looking at a worktree changes nothing in it, and one git refuses to
    /// remove gets its agent configs back: it may be resumed.
    #[tokio::test]
    async fn a_kept_worktree_keeps_its_agent_configs() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::write(repo.join(".mcp.json"), "{}\n").unwrap();
        let ws = Workspace::create(&repo, "resumable", "c9c9c9c9-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        drop(ws);
        assert_eq!(work_to_keep(&repo, &path).await, None);
        assert!(
            path.join(".mcp.json").exists(),
            "an inspection is read-only"
        );
        git(&repo, &["worktree", "lock", &path.to_string_lossy()]);
        let kept = remove_inspected_checkout(&repo, &path)
            .await
            .expect("removal");
        assert!(kept.is_some(), "a locked worktree is kept");
        assert!(path.join(".mcp.json").exists());
    }

    /// Only the copies taken are put back: a config the run edited, ignored
    /// by git, is the run's and stays as the run left it.
    #[tokio::test]
    async fn a_kept_worktree_keeps_the_configs_the_run_edited() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::write(repo.join(".gitignore"), ".mcp.json\n.vibe/\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(&repo, &["commit", "-q", "-m", "ignore the agent configs"]);
        std::fs::write(repo.join(".mcp.json"), "{}\n").unwrap();
        std::fs::create_dir_all(repo.join(".vibe")).unwrap();
        std::fs::write(repo.join(".vibe/config.toml"), "a = 1\n").unwrap();
        let ws = Workspace::create(&repo, "edited", "d0d0d0d0-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        drop(ws);
        std::fs::write(path.join(".vibe/config.toml"), "a = 2\n").unwrap();
        git(&repo, &["worktree", "lock", &path.to_string_lossy()]);
        let kept = remove_inspected_checkout(&repo, &path)
            .await
            .expect("removal");
        assert!(kept.is_some(), "a locked worktree is kept");
        assert!(path.join(".mcp.json").exists());
        assert_eq!(
            std::fs::read_to_string(path.join(".vibe/config.toml")).unwrap(),
            "a = 2\n"
        );
    }

    /// A staged change on a config path is the run's, whatever the file
    /// on disk says.
    #[tokio::test]
    async fn a_staged_change_on_an_agent_config_is_work() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::write(repo.join(".mcp.json"), "{}\n").unwrap();
        git(&repo, &["add", ".mcp.json"]);
        git(&repo, &["commit", "-q", "-m", "tracked agent config"]);
        let ws = Workspace::create(&repo, "staged", "e1e1e1e1-run", None, None)
            .await
            .expect("create worktree");
        std::fs::write(ws.path.join(".mcp.json"), "{\"mine\":1}\n").unwrap();
        git(&ws.path, &["add", ".mcp.json"]);
        std::fs::write(ws.path.join(".mcp.json"), "{}\n").unwrap();
        assert!(work_to_keep(&repo, &ws.path).await.is_some());
    }

    /// A directory replaced by a link after the copies were taken: nothing
    /// is written through it.
    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn a_config_is_never_put_back_through_a_linked_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let (repo, tree, outside) = (
            dir.path().join("repo"),
            dir.path().join("tree"),
            dir.path().join("outside"),
        );
        for made in [&repo.join(".gemini"), &tree, &outside] {
            std::fs::create_dir_all(made).unwrap();
        }
        std::fs::write(repo.join(".gemini/settings.json"), "project\n").unwrap();
        std::fs::write(outside.join("settings.json"), "theirs\n").unwrap();
        std::os::unix::fs::symlink(&outside, tree.join(".gemini")).unwrap();

        restore_copied_agent_configs(&repo, &tree, &[PathBuf::from(".gemini/settings.json")]);

        assert_eq!(
            std::fs::read_to_string(outside.join("settings.json")).unwrap(),
            "theirs\n"
        );
    }

    /// `git add -N` leaves nothing staged for `diff --cached`, yet the path
    /// is in the index: the run's.
    #[tokio::test]
    async fn an_agent_config_the_run_added_to_the_index_is_work() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::write(repo.join(".mcp.json"), "{}\n").unwrap();
        let ws = Workspace::create(&repo, "intent", "f2f2f2f2-run", None, None)
            .await
            .expect("create worktree");
        git(&ws.path, &["add", "-N", ".mcp.json"]);
        assert!(work_to_keep(&repo, &ws.path).await.is_some());
    }

    #[tokio::test]
    async fn an_agent_config_the_run_changed_is_work() {
        let (_dir, repo) = make_test_repo().await;
        std::fs::write(repo.join(".mcp.json"), "{}\n").unwrap();
        let ws = Workspace::create(&repo, "configs", "d4d4d4d4-run", None, None)
            .await
            .expect("create worktree");
        std::fs::write(ws.path.join(".mcp.json"), "{\"mine\":1}\n").unwrap();
        assert!(work_to_keep(&repo, &ws.path).await.is_some());
        assert!(ws.path.join(".mcp.json").exists());
    }

    /// A synced run branch someone checked out elsewhere is not deleted
    /// under their feet.
    #[tokio::test]
    async fn a_synced_branch_checked_out_elsewhere_is_not_deleted() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "elsewhere", "e5e5e5e5-run", None, None)
            .await
            .expect("create worktree");
        let branch = ws.branch.clone();
        git(&ws.path, &["checkout", "-q", "--detach"]);
        git(&repo, &["checkout", "-q", &branch]);
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(outcome.kept.is_none());
        assert!(branch_tip(&repo, &branch).await.is_some());
    }

    /// A submodule removed by a commit leaves its clone under the git
    /// directory, with a work tree that no longer exists: still readable.
    #[tokio::test]
    async fn a_checkout_whose_submodule_a_commit_removed_is_removed() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, _source) = worktree_with_submodule(&repo, "f6f6f6f6-run").await;
        let path = ws.path.clone();
        git(&path, &["rm", "-q", "deps/lib"]);
        git(&path, &["commit", "-q", "-m", "drop the submodule"]);
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(
            outcome.kept.is_none(),
            "{:?}",
            outcome.kept.map(|kept| kept.reason)
        );
        assert!(!path.exists());
    }

    /// A worktree of the submodule's own repository lives in its clone.
    #[tokio::test]
    async fn a_linked_worktree_of_a_submodule_keeps_the_checkout() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, _source) = worktree_with_submodule(&repo, "a7a7a7a7-run").await;
        let side = tempfile::TempDir::new().unwrap();
        let target = side.path().join("side");
        git(
            &ws.path.join("deps/lib"),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &target.to_string_lossy(),
            ],
        );
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome.kept.expect("kept: the submodule has a worktree");
        assert!(kept.reason.contains("worktree"), "{}", kept.reason);
    }

    /// KT-1809 — git's refusal of an initialised submodule is no reason to
    /// keep a clean checkout for ever.
    #[tokio::test]
    async fn a_clean_checkout_with_an_initialised_submodule_is_removed() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, _source) = worktree_with_submodule(&repo, "6c6c6c6c-run").await;
        let path = ws.path.clone();
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(
            outcome.kept.is_none(),
            "{:?}",
            outcome.kept.map(|kept| kept.reason)
        );
        assert!(!path.exists());
        let listed = porcelain(&repo).await;
        assert!(!listed.contains("6c6c6c6c"), "{listed}");
    }

    /// A commit made in the submodule lives only in the worktree's own copy
    /// of it, even once the run committed the new pointer.
    #[tokio::test]
    async fn a_submodule_commit_no_remote_has_keeps_the_checkout() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, _source) = worktree_with_submodule(&repo, "7d7d7d7d-run").await;
        let path = ws.path.clone();
        let lib = path.join("deps/lib");
        git(
            &lib,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "local",
            ],
        );
        git(&path, &["add", "deps/lib"]);
        git(&path, &["commit", "-q", "-m", "bump the submodule"]);
        assert_eq!(
            work_to_keep(&repo, &path).await,
            None,
            "the checkout itself is clean"
        );

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome.kept.expect("kept: the submodule holds a commit");
        assert!(kept.reason.contains("submodule"), "{}", kept.reason);
        // The reason names where the work is, not only git's refusal.
        assert!(kept.reason.contains("deps/lib"), "{}", kept.reason);
        assert!(lib.join("lib.txt").exists());
    }

    /// A submodule added over an existing clone keeps its repository inside
    /// the worktree, not under the git directory: a forced removal takes it.
    #[tokio::test]
    async fn a_submodule_repository_embedded_in_the_worktree_keeps_the_checkout() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, source) = worktree_with_submodule(&repo, "9f9f9f9f-run").await;
        let path = ws.path.clone();
        let url = source.path().to_string_lossy().to_string();
        git(&path, &["clone", "-q", &url, "vendor/lib"]);
        git(&path, &["submodule", "add", "-q", &url, "vendor/lib"]);
        let vendor = path.join("vendor/lib");
        assert!(vendor.join(".git").is_dir(), "the repository is embedded");
        git(
            &vendor,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "local",
            ],
        );
        git(&path, &["add", "vendor/lib"]);
        git(&path, &["commit", "-q", "-m", "embedded submodule"]);
        assert_eq!(
            work_to_keep(&repo, &path).await,
            None,
            "the checkout itself is clean"
        );

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome.kept.expect("kept: the embedded repository");
        assert!(kept.reason.contains("vendor/lib"), "{}", kept.reason);
        assert!(vendor.join(".git").is_dir());
    }

    /// A deinitialised submodule is out of `git submodule foreach`, but its
    /// clone, with the commit made in it, goes with a forced removal.
    #[tokio::test]
    async fn a_commit_in_a_deinitialised_submodule_keeps_the_checkout() {
        let (_dir, repo) = make_test_repo().await;
        let (ws, _source) = worktree_with_submodule(&repo, "8e8e8e8e-run").await;
        let path = ws.path.clone();
        // A second, untouched submodule keeps git refusing the removal.
        let other = tempfile::TempDir::new().unwrap();
        git(other.path(), &["init", "-q", "-b", "main"]);
        git(
            other.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "other",
            ],
        );
        git(
            &path,
            &[
                "submodule",
                "add",
                "-q",
                &other.path().to_string_lossy(),
                "deps/other",
            ],
        );
        git(&path, &["commit", "-q", "-m", "second submodule"]);
        let lib = path.join("deps/lib");
        git(
            &lib,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "local",
            ],
        );
        git(&path, &["add", "deps/lib"]);
        git(&path, &["commit", "-q", "-m", "bump"]);
        git(&path, &["submodule", "deinit", "-q", "-f", "deps/lib"]);
        assert_eq!(
            work_to_keep(&repo, &path).await,
            None,
            "the checkout itself is clean"
        );

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome
            .kept
            .expect("the deinitialised clone holds a commit");
        assert!(kept.reason.contains("commit"), "{}", kept.reason);
        assert!(path.exists());
    }

    /// A ref of the worktree itself goes with it: it holds nothing.
    #[tokio::test]
    async fn a_detached_commit_only_a_worktree_ref_holds_is_kept() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "wt-ref", "9f9f9f9f-run", None, None)
            .await
            .expect("create worktree");
        let parent = ws.path.clone();
        git(&parent, &["checkout", "-q", "--detach"]);
        git(&parent, &["commit", "-q", "--allow-empty", "-m", "orphan"]);
        git(&parent, &["update-ref", "refs/worktree/keep", "HEAD"]);

        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        let kept = outcome
            .kept
            .expect("only a ref of the worktree holds the commit");
        assert!(kept.reason.contains("detached"), "{}", kept.reason);
    }

    #[tokio::test]
    async fn kronn_files_are_not_work_a_stopped_run_keeps() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "artifacts", "55667788-run", None, None)
            .await
            .expect("create worktree");
        let path = ws.path.clone();
        std::fs::create_dir_all(path.join(".kronn")).unwrap();
        std::fs::write(path.join(".kronn/plan.md"), "plan\n").unwrap();
        let outcome = ws.cleanup_keeping_work().await.expect("cleanup");
        assert!(outcome.kept.is_none());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn the_boot_purge_removes_a_worktree_nested_in_a_terminal_run() {
        let (_dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "pr-review", "65b120da-run", None, None)
            .await
            .expect("create worktree");
        add_nested_pr_worktree(&ws.path).await;
        let path = ws.path.clone();
        drop(ws);

        Workspace::purge_terminal_checkout(&repo, &path)
            .await
            .expect("purge");

        let listed = porcelain(&repo).await;
        assert!(!path.exists());
        assert!(!listed.contains("prunable"), "{listed}");
        assert!(!listed.contains("pr-1995"), "{listed}");
    }

    #[tokio::test]
    async fn orphan_kronn_worktrees_are_pruned_at_startup_and_user_ones_are_kept() {
        let (dir, repo) = make_test_repo().await;
        let ws = Workspace::create(&repo, "orphan", "abcdef01-run", None, None)
            .await
            .expect("create worktree");
        add_nested_pr_worktree(&ws.path).await;
        // A user's own worktree outside `.kronn/`, on a drive not mounted now.
        let outside = dir.path().join("été-outside");
        let out = crate::core::cmd::async_git_cmd()
            .args(["worktree", "add", "-q", "-b", "user-branch"])
            .arg(&outside)
            .current_dir(&repo)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::remove_dir_all(&ws.path).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
        assert_eq!(porcelain(&repo).await.matches("prunable").count(), 3);

        let db = crate::db::Database::open_in_memory().unwrap();
        let now = chrono::Utc::now();
        let project: crate::models::Project = serde_json::from_value(serde_json::json!({
            "id": "proj-orphans", "name": "Orphans",
            "path": repo.to_string_lossy(),
            "repo_url": null, "token_override": null,
            "ai_config": {"detected": false, "configs": []},
            "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(),
        }))
        .unwrap();
        db.with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
            .await
            .unwrap();

        assert_eq!(prune_orphan_worktrees(&db).await, 2);
        let listed = porcelain(&repo).await;
        assert_eq!(listed.matches("prunable").count(), 1, "{listed}");
        assert!(listed.contains("user-branch"), "{listed}");
        assert_eq!(prune_orphan_worktrees(&db).await, 0, "idempotent");
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
        let out = crate::core::cmd::async_git_cmd()
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
