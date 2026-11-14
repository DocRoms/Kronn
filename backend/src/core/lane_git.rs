//! The one way Kronn runs `git` on a worker's worktree (KT-1745).
//!
//! Kronn inspects, stages, commits and merges for a worker itself, in its own
//! process tree: outside the worker's OS sandbox, as the host user, with the
//! user's credentials. The worker writes its worktree, **including the `.git`
//! pointer file** inside it, and every file git reads from there: hooks
//! (`core.hooksPath = .husky`), `.gitattributes`, and — when the pointer is
//! redirected — a whole repository configuration (`filter.*`, `merge.*.driver`,
//! `diff.*.textconv`, `core.fsmonitor`, `gpg.program`, `url.*.insteadOf`).
//! Letting git discover the repository from the worktree is therefore letting
//! the worker pick what Kronn executes.
//!
//! So a command for a worker's worktree is built here, and nowhere else:
//!
//! - **the repository is pinned**, never discovered: `--git-dir=<admin dir>` and
//!   `--work-tree=<worktree>`, where the admin dir is
//!   `<common dir>/worktrees/<lane>` of the REGISTERED project checkout (the
//!   `<repo>` of `<repo>/.kronn/worktrees/<lane>`), found by the registration
//!   git itself wrote (`<admin>/gitdir`), never through the worktree's `.git`
//!   file or a `commondir` the worker could rewrite. Neither lives in a
//!   directory the worker writes;
//! - **the pointer is checked before every command**: if `<worktree>/.git` no
//!   longer reads `gitdir: <that admin dir>`, the operation is refused with
//!   [`TAMPER_PREFIX`] and nothing runs;
//! - git is given no hook (`core.hooksPath=/dev/null`; the gating hook of a
//!   commit is run by `commit_hooks`, under the project's policy), no file-system
//!   monitor, and no commit or tag signing program. Filter, merge-driver and
//!   textconv programs can then only be those the registered repository's own
//!   configuration names, which the worker cannot write.
//!
//! A path that is not a managed worktree (`<repo>/.kronn/worktrees/<lane>`) is
//! the owner's own checkout: it keeps git's discovery and gets the same
//! hardening switches.
//!
//! `lane_git_guard_test.rs` fails when a `git_cmd()` appears in a function that
//! runs on a worker's worktree without going through here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Prefix of the error a command refused because the worktree's `.git` pointer
/// no longer reads what Kronn registered. The worker room is told as is.
pub const TAMPER_PREFIX: &str = "WorktreeTampered";

/// What git is given for `core.hooksPath`: no hook runs.
pub const NO_HOOKS: &str = "/dev/null";

/// The repository a managed worktree belongs to, from the registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    /// The registered project checkout (`<repo>` of `<repo>/.kronn/worktrees/<lane>`).
    pub repo: PathBuf,
    /// Its common git directory.
    pub common_dir: PathBuf,
    /// The worktree's administrative directory under `<common_dir>/worktrees`.
    pub git_dir: PathBuf,
}

/// A way to run `git` on one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneGit {
    work_dir: PathBuf,
    pinned: Option<Pinned>,
}

fn canonical(path: &Path) -> PathBuf {
    crate::core::task_worker_sandbox::real_path(path)
}

/// `(repo, lane)` of `<repo>/.kronn/worktrees/<lane>`.
fn managed_parts(work_dir: &Path) -> Option<(PathBuf, String)> {
    let lane = work_dir.file_name()?.to_str()?.to_string();
    let repo = work_dir
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "worktrees"))?
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == ".kronn"))?
        .parent()?
        .to_path_buf();
    Some((repo, lane))
}

fn common_dirs() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The common git directory of the registered checkout `repo`, absolute. Asked
/// of the owner's checkout, never of a worktree.
fn registered_common_dir(repo: &Path) -> Result<PathBuf, String> {
    if let Some(known) = common_dirs()
        .lock()
        .ok()
        .and_then(|cache| cache.get(repo).cloned())
        .filter(|dir| dir.is_dir())
    {
        return Ok(known);
    }
    // lane-git-allow: the registered project checkout, not a worker worktree.
    let output = crate::core::cmd::project_git_cmd()
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(repo)
        .output()
        .map_err(|error| {
            format!(
                "cannot read the git directory of {}: {error}",
                repo.display()
            )
        })?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().map(str::trim).unwrap_or_default();
    if !output.status.success() || line.is_empty() {
        return Err(format!(
            "{} is not a git checkout: {}",
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let path = PathBuf::from(line);
    let dir = canonical(&if path.is_absolute() {
        path
    } else {
        repo.join(path)
    });
    if let Ok(mut cache) = common_dirs().lock() {
        cache.insert(repo.to_path_buf(), dir.clone());
    }
    Ok(dir)
}

/// The administrative directory git registered for `work_dir`: the entry of
/// `<common>/worktrees` whose `gitdir` file names `<work_dir>/.git`.
fn registered_admin_dir(common_dir: &Path, work_dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(common_dir.join("worktrees")).map_err(|error| {
        format!(
            "no worktree is registered under {}: {error}",
            common_dir.display()
        )
    })?;
    for entry in entries.flatten() {
        let admin = entry.path();
        let Ok(content) = std::fs::read_to_string(admin.join("gitdir")) else {
            continue;
        };
        // Git writes the path as given, absolute or (Kronn's portable worktrees)
        // relative to the administrative directory.
        let named = PathBuf::from(content.trim());
        let named = if named.is_absolute() {
            named
        } else {
            admin.join(named)
        };
        let is_this_one = named.file_name().is_some_and(|name| name == ".git")
            && named
                .parent()
                .is_some_and(|parent| canonical(parent) == work_dir);
        if is_this_one {
            return Ok(canonical(&admin));
        }
    }
    Err(format!(
        "{} is not a worktree registered in {}",
        work_dir.display(),
        common_dir.display()
    ))
}

impl LaneGit {
    /// How to run git on `work_dir`. A managed worktree is pinned to its
    /// registration; one that is not registered, or whose `.git` pointer was
    /// redirected, is refused.
    pub fn resolve(work_dir: &Path) -> Result<Self, String> {
        let work_dir = canonical(work_dir);
        let Some((repo, _lane)) = managed_parts(&work_dir) else {
            return Ok(Self {
                work_dir,
                pinned: None,
            });
        };
        let common_dir = registered_common_dir(&repo)?;
        let git_dir = registered_admin_dir(&common_dir, &work_dir)?;
        let lane = Self {
            work_dir,
            pinned: Some(Pinned {
                repo,
                common_dir,
                git_dir,
            }),
        };
        lane.verify()?;
        Ok(lane)
    }

    /// The pointer of a pinned worktree still reads the registered git
    /// directory. Cheap: one `lstat` and one small read.
    pub fn verify(&self) -> Result<(), String> {
        let Some(pinned) = &self.pinned else {
            return Ok(());
        };
        let pointer = self.work_dir.join(".git");
        let refuse = |why: String| {
            tracing::warn!(
                "{TAMPER_PREFIX}: {} — {why}; no git command runs in it",
                self.work_dir.display()
            );
            Err(format!(
                "{TAMPER_PREFIX}: the `.git` file of {} {why}. Kronn refuses to run git in a \
                 worktree whose repository pointer was changed; restore `gitdir: {}` and retry.",
                self.work_dir.display(),
                pinned.git_dir.display()
            ))
        };
        let metadata = match std::fs::symlink_metadata(&pointer) {
            Ok(metadata) => metadata,
            Err(error) => return refuse(format!("cannot be read ({error})")),
        };
        if !metadata.is_file() {
            return refuse("is not a regular file".to_string());
        }
        let content = match std::fs::read_to_string(&pointer) {
            Ok(content) => content,
            Err(error) => return refuse(format!("cannot be read ({error})")),
        };
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        let target = lines
            .next()
            .and_then(|line| line.strip_prefix("gitdir:"))
            .map(str::trim);
        let Some(target) = target.filter(|_| lines.next().is_none()) else {
            return refuse("is not a single `gitdir:` line".to_string());
        };
        let path = PathBuf::from(target);
        let path = if path.is_absolute() {
            path
        } else {
            self.work_dir.join(path)
        };
        if canonical(&path) != pinned.git_dir {
            return refuse(format!("points at `{target}`"));
        }
        Ok(())
    }

    /// Whether this is a managed worktree pinned to its registration.
    pub fn is_pinned(&self) -> bool {
        self.pinned.is_some()
    }

    /// The registration, for a managed worktree.
    pub fn pinned(&self) -> Option<&Pinned> {
        self.pinned.as_ref()
    }

    /// The directory the commands run in.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// A `git` command for this worktree, in front of which stand the pinned
    /// repository and the switches that turn off everything the worktree or the
    /// worker could name for git to run. Starts in the worktree.
    pub fn command(&self) -> Command {
        // lane-git-allow: this is the builder.
        let mut command = crate::core::cmd::git_cmd();
        if let Some(pinned) = &self.pinned {
            command
                .arg(format!("--git-dir={}", pinned.git_dir.display()))
                .arg(format!("--work-tree={}", self.work_dir.display()));
        }
        for setting in [
            format!("core.hooksPath={NO_HOOKS}"),
            "core.fsmonitor=false".to_string(),
            "commit.gpgSign=false".to_string(),
            "tag.gpgSign=false".to_string(),
        ] {
            command.arg("-c").arg(setting);
        }
        command
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env_remove("GIT_EXTERNAL_DIFF")
            .current_dir(&self.work_dir);
        command
    }
}

/// `git` on the worktree `work_dir`, pinned, hardened and verified. See the
/// module documentation; this is the entry point of every call site.
pub fn git(work_dir: &Path) -> Result<Command, String> {
    LaneGit::resolve(work_dir).map(|lane| lane.command())
}

/// [`git`] for a command the user asked for (a push): in a managed worktree it is
/// the pinned, hook-less command of [`git`]; in the owner's own checkout it is
/// plain git, whose hooks (`pre-push`) are the user's own and keep running.
pub fn git_keeping_owner_hooks(work_dir: &Path) -> Result<Command, String> {
    let lane = LaneGit::resolve(work_dir)?;
    if lane.is_pinned() {
        return Ok(lane.command());
    }
    // lane-git-allow: the owner's own checkout, not a managed worktree.
    let mut command = crate::core::cmd::git_cmd();
    command.current_dir(lane.work_dir());
    Ok(command)
}

/// `args` that keep a diff-like subcommand from running a program the
/// configuration names (`diff.external`, `diff.*.textconv`).
pub fn no_external_diff(subcommand: &str) -> &'static [&'static str] {
    match subcommand {
        "diff" | "log" | "show" | "diff-tree" | "format-patch" => {
            &["--no-ext-diff", "--no-textconv"]
        }
        _ => &[],
    }
}

#[cfg(test)]
#[path = "lane_git_test.rs"]
mod lane_git_test;

#[cfg(test)]
#[path = "lane_git_guard_test.rs"]
mod lane_git_guard_test;
