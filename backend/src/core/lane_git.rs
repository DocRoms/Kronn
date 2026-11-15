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
    /// Where the commands start: `work_dir`, or a directory inside it.
    start: PathBuf,
    pinned: Option<Pinned>,
    /// For a submodule of a managed worktree: the worktree it lives in, whose
    /// registration and pointer vouch for it.
    parent: Option<Box<LaneGit>>,
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

/// Prefix of the error a command refused because a nested repository of the
/// worktree (a submodule) carries a configuration the worker may have written.
pub const NESTED_PREFIX: &str = "NestedRepoUnsafe";

/// `filter.*`, `merge.*` and `diff.*` program values of the registered
/// repository's configuration (and the user's), cached briefly.
fn registered_programs(common_dir: &Path) -> Vec<(String, String)> {
    type Cache = Mutex<HashMap<PathBuf, (std::time::Instant, Vec<(String, String)>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, known)) = cache.lock().ok().and_then(|c| c.get(common_dir).cloned()) {
        if at.elapsed() < std::time::Duration::from_secs(30) {
            return known;
        }
    }
    // lane-git-allow: reads the registered repository's configuration, no worktree.
    let output = crate::core::cmd::project_git_cmd()
        .arg(format!("--git-dir={}", common_dir.display()))
        .args([
            "config",
            "--null",
            "--get-regexp",
            r"^(filter|merge|diff)\.",
        ])
        .output();
    let mut found = Vec::new();
    if let Ok(output) = output {
        for entry in String::from_utf8_lossy(&output.stdout).split('\0') {
            if let Some((key, value)) = entry.split_once('\n') {
                found.push((key.to_string(), value.to_string()));
            }
        }
    }
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            common_dir.to_path_buf(),
            (std::time::Instant::now(), found.clone()),
        );
    }
    found
}

/// Whether the program `value` of a filter, merge driver or textconv would run
/// something that lives in the worktree: a relative path, or an argument naming
/// a file the worktree holds (`sh tools/clean.sh`, `sh clean.sh`).
fn names_worktree_content(value: &str, work_dir: &Path) -> bool {
    let mut tokens = value.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if first.contains('/') && !Path::new(first).is_absolute() {
        return true;
    }
    tokens.any(|token| {
        let token = token.trim_matches(|c| c == '"' || c == '\'');
        if token.is_empty() || token.starts_with('-') || token.starts_with('%') {
            return false;
        }
        if Path::new(token).is_absolute() {
            return false;
        }
        token.contains('/') || work_dir.join(token).exists()
    })
}

/// `-c` settings that turn off the registered configuration's filter, merge
/// driver and textconv programs that resolve inside the worktree. The registered
/// configuration is the owner's, but a RELATIVE program in it (`sh tools/clean.sh`)
/// runs whatever the worker wrote at that path. Reported once per key.
fn neutralised_programs(common_dir: &Path, work_dir: &Path) -> Vec<String> {
    static REPORTED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let mut settings = Vec::new();
    for (key, value) in registered_programs(common_dir) {
        if !names_worktree_content(&value, work_dir) {
            continue;
        }
        let key = key.to_lowercase();
        let mut turned_off = Vec::new();
        if let Some(name) = key.strip_prefix("filter.") {
            for suffix in [".clean", ".smudge", ".process"] {
                if let Some(driver) = name.strip_suffix(suffix) {
                    turned_off.push(format!("filter.{driver}.clean="));
                    turned_off.push(format!("filter.{driver}.smudge="));
                    turned_off.push(format!("filter.{driver}.process="));
                    turned_off.push(format!("filter.{driver}.required=false"));
                }
            }
        } else if let Some(name) = key.strip_prefix("merge.") {
            if let Some(driver) = name.strip_suffix(".driver") {
                turned_off.push(format!("merge.{driver}.driver=false"));
            }
        } else if let Some(name) = key.strip_prefix("diff.") {
            for suffix in [".textconv", ".command"] {
                if let Some(driver) = name.strip_suffix(suffix) {
                    turned_off.push(format!("diff.{driver}{suffix}="));
                }
            }
        }
        if turned_off.is_empty() {
            continue;
        }
        let first_time = REPORTED
            .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
            .lock()
            .map(|mut seen| seen.insert(format!("{}|{key}", common_dir.display())))
            .unwrap_or(false);
        if first_time {
            tracing::warn!(
                "lane git: `{key}` runs a program that lives in the worker's worktree ({value}); \
                 it is turned off for Kronn's own git commands on worker worktrees"
            );
        }
        settings.extend(turned_off);
    }
    settings.sort();
    settings.dedup();
    settings
}

/// Configuration keys a nested repository Kronn trusts may carry. Anything else
/// (`filter.*`, `core.fsmonitor`, `core.hooksPath`, `include.path`, `gpg.*`,
/// `diff.*`, `merge.*`, `url.*`) is something a worker could have added.
fn nested_key_is_plain(key: &str) -> bool {
    const CORE: [&str; 8] = [
        "core.repositoryformatversion",
        "core.filemode",
        "core.bare",
        "core.logallrefupdates",
        "core.ignorecase",
        "core.precomposeunicode",
        "core.symlinks",
        "core.worktree",
    ];
    if CORE.contains(&key) || matches!(key, "user.name" | "user.email" | "extensions.objectformat")
    {
        return true;
    }
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["remote", .., "url" | "fetch"] | ["branch", .., "remote" | "merge"] => parts.len() >= 3,
        _ => false,
    }
}

/// The keys of a git configuration file's text, lower-cased
/// (`section.subsection.name`, section and name case-insensitive).
fn config_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let mut rest = line;
        if let Some(header) = line.strip_prefix('[') {
            let Some(end) = header.find(']') else {
                keys.push(format!("?malformed:{line}"));
                continue;
            };
            let head = &header[..end];
            section = match head.split_once('"') {
                Some((name, sub)) => format!(
                    "{}.{}",
                    name.trim().to_lowercase(),
                    sub.trim_end_matches('"')
                ),
                None => head.trim().to_lowercase(),
            };
            rest = header[end + 1..].trim();
            if rest.is_empty() {
                continue;
            }
        }
        let name = rest.split('=').next().unwrap_or_default().trim();
        if !name.is_empty() {
            keys.push(format!("{section}.{}", name.to_lowercase()));
        }
    }
    keys
}

/// The gitlink paths of the worktree's index, cached by the index's size and
/// modification time. Listing the index runs no program.
fn gitlink_paths(lane: &LaneGit, pinned: &Pinned) -> Vec<String> {
    type Known = (Option<std::time::SystemTime>, u64, Vec<String>);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Known>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let stamp = std::fs::metadata(pinned.git_dir.join("index")).ok();
    let key = (
        stamp.as_ref().and_then(|m| m.modified().ok()),
        stamp.as_ref().map_or(0, |m| m.len()),
    );
    if let Some(known) = cache
        .lock()
        .ok()
        .and_then(|c| c.get(&pinned.git_dir).cloned())
    {
        if (known.0, known.1) == key {
            return known.2;
        }
    }
    // From the worktree's root, whatever directory `lane` started in: `ls-files`
    // lists what is below where it runs.
    let output = lane
        .command()
        .current_dir(&lane.work_dir)
        .args(["ls-files", "--stage", "-z"])
        .output();
    let mut paths = Vec::new();
    if let Ok(output) = output {
        for entry in output.stdout.split(|byte| *byte == 0) {
            let entry = String::from_utf8_lossy(entry);
            if let Some((meta, path)) = entry.split_once('\t') {
                if meta.starts_with("160000 ") {
                    paths.push(path.to_string());
                }
            }
        }
    }
    if let Ok(mut cache) = cache.lock() {
        cache.insert(pinned.git_dir.clone(), (key.0, key.1, paths.clone()));
    }
    paths
}

/// The closest directory from `start` up to (not including) `root` that holds a
/// `.git` entry: the repository git would discover from `start` instead of the
/// worktree.
fn nested_repo_root(root: &Path, start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .take_while(|dir| *dir != root)
        .find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

/// The git directory of the submodule at `nested`: its `.git` directory, or what
/// its `.git` file points at, which must be inside the registered repository's own
/// git directory.
fn nested_git_dir(nested: &Path, pinned: &Pinned) -> Result<PathBuf, String> {
    let dot_git = nested.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).map_err(|error| {
        format!(
            "{NESTED_PREFIX}: cannot read {}: {error}",
            dot_git.display()
        )
    })?;
    if metadata.is_dir() {
        return Ok(canonical(&dot_git));
    }
    let text = if metadata.is_file() {
        std::fs::read_to_string(&dot_git).unwrap_or_default()
    } else {
        String::new()
    };
    let target = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .unwrap_or_default();
    let target = PathBuf::from(target);
    let target = canonical(&if target.is_absolute() {
        target
    } else {
        nested.join(target)
    });
    if target.starts_with(&pinned.common_dir) {
        Ok(target)
    } else {
        Err(format!(
            "{NESTED_PREFIX}: the `.git` of {} does not point into the repository's own git directory",
            nested.display()
        ))
    }
}

impl LaneGit {
    /// Every submodule of the worktree is a repository Kronn would trust the
    /// configuration of when a command recurses into it (`add`, `commit -a`,
    /// `diff-index` override `diff.ignoreSubmodules`). Refuse while one carries a
    /// configuration the worker could have written: a nested `.git` directory
    /// with a key outside [`nested_key_is_plain`], or a `.git` file that does not
    /// point into the registered repository's own git directory.
    fn check_nested_repos(&self, pinned: &Pinned) -> Result<(), String> {
        for path in gitlink_paths(self, pinned) {
            let nested = self.work_dir.join(&path).join(".git");
            let Ok(metadata) = std::fs::symlink_metadata(&nested) else {
                continue; // not materialised: nothing to recurse into
            };
            let refuse = |why: String| {
                tracing::warn!(
                    "{NESTED_PREFIX}: submodule `{path}` of {} — {why}",
                    self.work_dir.display()
                );
                Err(format!(
                    "{NESTED_PREFIX}: the submodule `{path}` of {} {why}. Kronn refuses to run git \
                     where it would read that repository's configuration; restore the submodule \
                     (remove `{path}/.git` entries the task did not need) and retry.",
                    self.work_dir.display()
                ))
            };
            if metadata.is_file() {
                let text = std::fs::read_to_string(&nested).unwrap_or_default();
                let target = text
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("gitdir:"))
                    .map(str::trim)
                    .unwrap_or_default();
                let target = PathBuf::from(target);
                let target = canonical(&if target.is_absolute() {
                    target
                } else {
                    nested.parent().unwrap_or(&self.work_dir).join(target)
                });
                if !target.starts_with(&pinned.common_dir) {
                    return refuse(format!(
                        "has a `.git` file pointing at {}, outside the repository's own git directory",
                        target.display()
                    ));
                }
            } else if metadata.is_dir() {
                let config = nested.join("config");
                let text = match std::fs::symlink_metadata(&config) {
                    Ok(m) if m.is_file() => std::fs::read_to_string(&config).unwrap_or_default(),
                    Ok(_) => {
                        return refuse("has a `.git/config` that is not a regular file".into())
                    }
                    Err(_) => String::new(),
                };
                let unknown: Vec<String> = config_keys(&text)
                    .into_iter()
                    .filter(|key| !nested_key_is_plain(key))
                    .collect();
                if !unknown.is_empty() {
                    return refuse(format!(
                        "has a `.git/config` with settings Kronn does not trust: {}",
                        unknown.join(", ")
                    ));
                }
            } else {
                return refuse("has a `.git` that is neither a directory nor a file".into());
            }
        }
        Ok(())
    }

    /// How to run git on `work_dir`. A managed worktree is pinned to its
    /// registration; one that is not registered, or whose `.git` pointer was
    /// redirected, is refused.
    pub fn resolve(work_dir: &Path) -> Result<Self, String> {
        let start = canonical(work_dir);
        // A directory INSIDE a managed worktree is still the worker's worktree:
        // git would find the worker-written `.git` pointer from there.
        let Some((work_dir, repo)) = start
            .ancestors()
            .find_map(|dir| managed_parts(dir).map(|(repo, _lane)| (dir.to_path_buf(), repo)))
        else {
            return Ok(Self {
                work_dir: start.clone(),
                start,
                pinned: None,
                parent: None,
            });
        };
        let common_dir = registered_common_dir(&repo)?;
        let git_dir = registered_admin_dir(&common_dir, &work_dir)?;
        let lane = Self {
            work_dir,
            start: start.clone(),
            pinned: Some(Pinned {
                repo,
                common_dir,
                git_dir,
            }),
            parent: None,
        };
        lane.verify()?;
        // A directory inside a submodule is the submodule's, not the worktree's:
        // git finds the closest `.git`, and so does the HEAD a caller asks for.
        match nested_repo_root(&lane.work_dir, &start) {
            None => Ok(lane),
            Some(nested) => lane.into_nested(nested, start),
        }
    }

    /// The lane of the submodule `nested` of this worktree: its own git
    /// directory, vouched for by this worktree's registration and by
    /// [`Self::check_nested_repos`] (which `verify` ran). A repository that is not
    /// a gitlink of the worktree's index is one the worker made: Kronn runs
    /// nothing in it.
    fn into_nested(self, nested: PathBuf, start: PathBuf) -> Result<Self, String> {
        let Some(pinned) = self.pinned.clone() else {
            return Ok(self);
        };
        let relative = nested
            .strip_prefix(&self.work_dir)
            .unwrap_or(&nested)
            .to_string_lossy()
            .replace('\\', "/");
        if !gitlink_paths(&self, &pinned).contains(&relative) {
            return Err(format!(
                "{NESTED_PREFIX}: `{relative}` of {} is a repository that is not a submodule of \
                 the worktree; Kronn runs no git command in it",
                self.work_dir.display()
            ));
        }
        let git_dir = nested_git_dir(&nested, &pinned)?;
        Ok(Self {
            work_dir: nested,
            start,
            pinned: Some(Pinned { git_dir, ..pinned }),
            parent: Some(Box::new(self)),
        })
    }

    /// The pointer of a pinned worktree still reads the registered git
    /// directory. Cheap: one `lstat` and one small read.
    pub fn verify(&self) -> Result<(), String> {
        let Some(pinned) = &self.pinned else {
            return Ok(());
        };
        if let Some(parent) = &self.parent {
            // The worktree's pointer and every submodule's configuration first,
            // then that this submodule still reads the git directory seen at resolve.
            parent.verify()?;
            return match nested_git_dir(&self.work_dir, pinned) {
                Ok(dir) if dir == pinned.git_dir => Ok(()),
                _ => Err(format!(
                    "{NESTED_PREFIX}: the `.git` of {} changed while Kronn was using it",
                    self.work_dir.display()
                )),
            };
        }
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
        self.check_nested_repos(pinned)
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

    /// The `-c` settings that turn off everything the worktree or the worker
    /// could name for git to run.
    fn settings(&self) -> Vec<String> {
        let mut settings = vec![
            format!("core.hooksPath={NO_HOOKS}"),
            "core.fsmonitor=false".to_string(),
            "commit.gpgSign=false".to_string(),
            "tag.gpgSign=false".to_string(),
            // A submodule is a repository of its own, with a configuration the
            // worker may have written: git must not descend into it to look for
            // changes (`status`, `diff`), where its filters would run. `dirty`
            // still shows a moved gitlink (a pointer change is work to keep) but
            // never runs `git status` inside the submodule. Commands that
            // override this (`add`, `commit -a`, `diff-index`) are covered by
            // `check_nested_repos`.
            "diff.ignoreSubmodules=dirty".to_string(),
            "submodule.recurse=false".to_string(),
            "status.submoduleSummary=false".to_string(),
        ];
        if let Some(pinned) = &self.pinned {
            settings.extend(neutralised_programs(&pinned.common_dir, &self.work_dir));
        }
        settings
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
        for setting in self.settings() {
            command.arg("-c").arg(setting);
        }
        command
            .env("GIT_ATTR_NOSYSTEM", "1")
            // A partial clone's missing object is fetched with the remote's
            // transport program (`core.sshCommand`): never from a Kronn command.
            .env("GIT_NO_LAZY_FETCH", "1")
            .env_remove("GIT_EXTERNAL_DIFF")
            .current_dir(&self.start);
        command
    }
}

/// `git` on the worktree `work_dir`, pinned, hardened and verified. See the
/// module documentation; this is the entry point of every call site.
pub fn git(work_dir: &Path) -> Result<Command, String> {
    LaneGit::resolve(work_dir).map(|lane| lane.command())
}

/// Pin a program that starts git ITSELF in `work_dir` (`gh`, `glab`: `gh pr create`
/// runs `git status`, which runs `core.fsmonitor`) to the same registered
/// repository, with the same switches, through the environment. In a managed
/// worktree whose pointer was redirected, git is aimed at a directory that does
/// not exist: the program fails and runs nothing. The owner's own checkout is left
/// alone.
pub fn pin_foreign_command(command: &mut Command, work_dir: &Path) {
    match LaneGit::resolve(work_dir) {
        Ok(lane) => {
            let Some(pinned) = lane.pinned() else {
                return;
            };
            command
                .env("GIT_DIR", &pinned.git_dir)
                .env("GIT_WORK_TREE", lane.work_dir())
                .env("GIT_ATTR_NOSYSTEM", "1")
                .env("GIT_NO_LAZY_FETCH", "1")
                .env_remove("GIT_EXTERNAL_DIFF");
            let settings = lane.settings();
            command.env("GIT_CONFIG_COUNT", settings.len().to_string());
            for (index, setting) in settings.iter().enumerate() {
                let (key, value) = setting.split_once('=').unwrap_or((setting, ""));
                command
                    .env(format!("GIT_CONFIG_KEY_{index}"), key)
                    .env(format!("GIT_CONFIG_VALUE_{index}"), value);
            }
        }
        Err(error) => {
            tracing::warn!("lane git refused {}: {error}", work_dir.display());
            command.env("GIT_DIR", "/dev/null/kronn-refused");
        }
    }
}

/// [`git`] for tokio.
pub fn async_git(work_dir: &Path) -> Result<tokio::process::Command, String> {
    git(work_dir).map(tokio::process::Command::from)
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
    command.current_dir(&lane.start);
    Ok(command)
}

/// [`git_keeping_owner_hooks`] for a read-only helper whose signature cannot carry
/// an error (`Option`, `String`): when the worktree is refused (tampered pointer,
/// nested repository, not registered) the command it returns is aimed at a git
/// directory that does not exist, so it runs nothing and fails like any other
/// unusable repository. The refusal is logged.
pub fn git_or_refuse(work_dir: &Path) -> Command {
    match git_keeping_owner_hooks(work_dir) {
        Ok(command) => command,
        Err(error) => {
            tracing::warn!("lane git refused {}: {error}", work_dir.display());
            // lane-git-allow: aimed at a git directory that does not exist; runs nothing.
            let mut command = crate::core::cmd::git_cmd();
            command
                .arg("--git-dir=/dev/null/kronn-refused")
                .current_dir(work_dir);
            command
        }
    }
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
