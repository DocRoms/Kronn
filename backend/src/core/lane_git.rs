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
    without_verbatim_prefix(crate::core::fs_guard::real_path(path))
}

pub fn git_output(repo: &Path, args: &[&str]) -> Option<Vec<u8>> {
    // KT-1745 — also asked of a worker's worktree (the doc citations gate): the
    // repository is the registered one, never what the worktree's `.git` names.
    let output = git(repo).ok()?.args(args).output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn ref_exists(repo: &Path, refname: &str) -> bool {
    git_output(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{refname}^{{commit}}"),
        ],
    )
    .is_some()
}

/// The full ref the profile is read from, or `None` outside a git repository
/// or when no default branch exists.
pub fn default_branch_ref(repo: &Path) -> Option<String> {
    default_branch_refs(repo).into_iter().next()
}

/// Every ref that currently holds the default branch, the local branch first.
/// Both are consulted to learn whether a commit reached it: after a GitHub
/// merge only the fetched `refs/remotes/origin/<name>` moves, the local branch
/// stays behind until a pull (KT-1519).
pub fn default_branch_refs(repo: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(head) = git_output(
        repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) {
        let head = String::from_utf8_lossy(&head).trim().to_string();
        if let Some(name) = head.strip_prefix("refs/remotes/origin/") {
            names.push(name.to_string());
        }
    }
    for name in ["main", "master"] {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    let refs_of = |name: &String| {
        [
            format!("refs/heads/{name}"),
            format!("refs/remotes/origin/{name}"),
        ]
        .into_iter()
        .filter(|refname| ref_exists(repo, refname))
        .collect::<Vec<_>>()
    };
    names
        .iter()
        .map(refs_of)
        .find(|found| !found.is_empty())
        .unwrap_or_default()
}

/// `path` as git can open it: on Windows `canonicalize` gives `\\?\C:\…`,
/// and git refuses a repository named that way.
pub fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    match path.to_str() {
        Some(text) if text.starts_with(r"\\?\UNC\") => PathBuf::from(format!(r"\\{}", &text[8..])),
        Some(text) if text.starts_with(r"\\?\") => PathBuf::from(&text[4..]),
        _ => path,
    }
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
    let mut command = crate::core::cmd::project_git_cmd();
    command
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(repo);
    let output =
        snapshot::bounded_output(command, "rev-parse --git-common-dir").map_err(|error| {
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

/// The common git directory of the owner's checkout `repo`, canonical (KT-2084).
pub fn common_dir_of(repo: &Path) -> Result<PathBuf, String> {
    registered_common_dir(repo)
}

/// Largest `gitdir` file of a registration read: a path, far below this.
const GITDIR_READ_CAP: u64 = 4096;

/// The registrations of `<common>/worktrees` naming `work_dir` (KT-2084): `exact`, in a form
/// Kronn or git writes, and how many name it only once resolved (through a link, a `..`).
struct Naming {
    exact: Vec<PathBuf>,
    resolved_only: usize,
}

/// `owner` is the owner's checkout (canonical), the only tree a `gitdir` path is resolved in.
/// `Err` also when two registrations name `work_dir` the way git compares them: git's own
/// pruning would delete one of them, whichever a worker chose.
fn registrations_naming(
    owner: &Path,
    common_dir: &Path,
    work_dir: &Path,
) -> Result<Naming, String> {
    let registrations = common_dir.join("worktrees");
    let names = crate::core::fs_guard::guarded_list(common_dir, &registrations)
        .map_err(|error| {
            format!(
                "the worktrees registered in {} cannot be listed: {error}",
                common_dir.display()
            )
        })?
        .unwrap_or_default();
    let ignore_case = ignores_case(common_dir)?;
    let wanted = path_key(&work_dir.join(".git"), ignore_case);
    let mut naming = Naming {
        exact: Vec::new(),
        resolved_only: 0,
    };
    let mut same_for_git: Vec<PathBuf> = Vec::new();
    for name in names {
        let admin = registrations.join(name);
        // Regular files only, never through a link, under a size cap.
        let Ok(Some(content)) = crate::core::fs_guard::guarded_read_head(
            &registrations,
            &admin.join("gitdir"),
            GITDIR_READ_CAP,
        ) else {
            continue;
        };
        if wanted.is_some() && gitdir_key(&admin, &content, ignore_case) == wanted {
            same_for_git.push(admin.clone());
        }
        if gitdir_names(&admin, &content, work_dir) {
            naming.exact.push(admin);
        } else if gitdir_resolves_to(owner, &admin, &content, work_dir) {
            naming.resolved_only += 1;
        }
    }
    if let [first, second, ..] = same_for_git.as_slice() {
        return Err(same_worktree(owner, first, second, work_dir));
    }
    Ok(naming)
}

/// The refusal of two registrations naming the worktree `named`, with the step that repairs
/// it (KT-2084): the registration the worktree's own `.git` names is the one in use, the
/// other is restored for its own worktree when Kronn finds it, else it is the folder to
/// delete. `named` is looked at only inside the owner's checkout `owner`, through no link.
fn same_worktree(owner: &Path, first: &Path, second: &Path, named: &Path) -> String {
    let refusal = format!(
        "the registrations {} and {} both name the worktree {}: git would prune one of them, so \
         Kronn uses neither.",
        first.display(),
        second.display(),
        named.display()
    );
    let never = "Until then, let git clean nothing up in this repository (no prune, no gc): \
                 either may delete the registration still in use.";
    let looked_at = plainly_spelled(named)
        && resolve_inside(owner, named).is_some_and(|(real, present)| present && real == named);
    let in_use = |admin: &Path| looked_at && pointer_problem(named, admin).is_none();
    let (kept, stale) = match (in_use(first), in_use(second)) {
        (true, false) => (first, second),
        (false, true) => (second, first),
        _ => {
            return format!(
                "{refusal} To repair: the `gitdir:` line of {} names the registration to keep; \
                 delete the other folder. {never}",
                named.join(".git").display()
            )
        }
    };
    // The worktree the stale registration belongs to, when it is a checkout of Kronn's that
    // still names it: that record is one line, which the owner writes back by hand.
    let its_worktree = stale
        .file_name()
        .map(|id| owner.join(".kronn").join("worktrees").join(id))
        .filter(|lane| lane != named)
        .filter(|lane| {
            resolve_inside(owner, lane).is_some_and(|(real, present)| present && &real == lane)
                && pointer_problem(lane, stale).is_none()
        });
    match its_worktree {
        // No git command: one that repairs a record also rewrites what other records name.
        Some(lane) => format!(
            "{refusal} {} is the one {} uses; {} belongs to {}. To repair by hand, replace the \
             content of the file{}with this single line:{}{never}",
            kept.display(),
            named.display(),
            stale.display(),
            lane.display(),
            path_as_data(&stale.join("gitdir")),
            path_as_data(&lane.join(".git"))
        ),
        None => format!(
            "{refusal} {} is the one {} uses. To repair, delete the folder {}. {never}",
            kept.display(),
            named.display(),
            stale.display()
        ),
    }
}

/// `path` in single quotes for a command a message prints, only when it is plainly spelled
/// and made of letters, digits, `/`, `.`, `_` and `-` (KT-2084): a lane writes the paths its
/// registrations name, and the owner pastes the command into a shell.
pub fn quoted_for_command(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let inert = |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-');
    (plainly_spelled(path) && text.chars().all(inert)).then(|| format!("'{text}'"))
}

/// `path` on a line of its own in a message, as data: its control characters are escaped,
/// so no part of it reads as another line or as a command.
pub fn path_as_data(path: &Path) -> String {
    let mut shown = String::new();
    for c in path.to_string_lossy().chars() {
        if c.is_control() {
            shown.extend(c.escape_default());
        } else {
            shown.push(c);
        }
    }
    format!("\n    {shown}\n")
}

/// The path a registration's `gitdir` names as git compares two of them when it prunes
/// duplicates: trailing newlines and carriage returns dropped, a relative one taken from the
/// registration, `.` and `..` resolved without a look at the disk, folded to lower case when
/// the repository ignores case (`core.ignorecase`).
fn gitdir_key(admin: &Path, content: &[u8], ignore_case: bool) -> Option<String> {
    let text = std::str::from_utf8(content)
        .ok()?
        .trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return None;
    }
    if Path::new(text).is_absolute() {
        path_key(Path::new(text), ignore_case)
    } else {
        path_key(&admin.join(text), ignore_case)
    }
}

/// `path`, `.` and `..` resolved without a look at the disk, as one string to compare.
fn path_key(path: &Path, ignore_case: bool) -> Option<String> {
    let normal = lexically_normal(path);
    let text = normal.to_str()?;
    Some(if ignore_case {
        text.to_ascii_lowercase()
    } else {
        text.to_string()
    })
}

/// `path` with each `.` dropped and each `..` taking the part before it away, without a look
/// at the disk.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other.as_os_str()),
        }
    }
    normal
}

/// Most links [`resolve_inside`] follows, as `realpath` allows.
const MAX_LINKS: usize = 40;

/// `path` below `owner`, as written: `None` when it is not there. Windows does not tell names
/// apart by their case.
fn below(owner: &Path, path: &Path) -> Option<PathBuf> {
    #[cfg(not(windows))]
    {
        path.strip_prefix(owner).ok().map(Path::to_path_buf)
    }
    #[cfg(windows)]
    {
        let mut parts = path.components();
        for wanted in owner.components() {
            let part = parts.next()?;
            if !part
                .as_os_str()
                .to_str()?
                .eq_ignore_ascii_case(wanted.as_os_str().to_str()?)
            {
                return None;
            }
        }
        Some(parts.as_path().to_path_buf())
    }
}

/// `path` with its links resolved as `realpath` would, as long as each step stays inside
/// `owner` (canonical): the path, and whether all of it exists. `None` once a step leaves
/// `owner`, past [`MAX_LINKS`] links, or when a part cannot be read. Nothing outside `owner` is
/// looked at (KT-2084): a `gitdir` naming `/net/<host>/…` never makes the server look that
/// host up.
pub fn resolve_inside(owner: &Path, path: &Path) -> Option<(PathBuf, bool)> {
    let parts = |path: &Path| -> Vec<std::ffi::OsString> {
        path.components()
            .rev()
            .map(|part| part.as_os_str().to_os_string())
            .collect()
    };
    let mut pending = parts(&below(owner, path)?);
    let mut at = owner.to_path_buf();
    let mut present = true;
    let mut links = 0;
    while let Some(part) = pending.pop() {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if at == owner {
                return None;
            }
            at.pop();
            continue;
        }
        let next = at.join(&part);
        if !present {
            at = next;
            continue;
        }
        match std::fs::symlink_metadata(&next) {
            Ok(meta) if meta.file_type().is_symlink() => {
                links += 1;
                if links > MAX_LINKS {
                    return None;
                }
                let target = std::fs::read_link(&next).ok()?;
                if target.is_absolute() {
                    pending.extend(parts(&below(owner, &target)?));
                    at = owner.to_path_buf();
                } else {
                    pending.extend(parts(&target));
                }
            }
            Ok(_) => at = next,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                present = false;
                at = next;
            }
            Err(_) => return None,
        }
    }
    Some((at, present))
}

/// [`resolve_inside`] the checkout `repo`, for a caller that compares the result with paths
/// spelled from `repo` (canonical as the caller holds it): given back in that spelling.
pub fn resolve_in_checkout(repo: &Path, path: &Path) -> Option<(PathBuf, bool)> {
    let owner = canonical(repo);
    let (real, present) = resolve_inside(&owner, path)?;
    let relative = real.strip_prefix(&owner).ok()?;
    let spelled = if relative.as_os_str().is_empty() {
        repo.to_path_buf()
    } else {
        repo.join(relative)
    };
    Some((spelled, present))
}

/// The path the `gitdir` file of the registration `admin` (below `root`) holds, as written
/// (KT-2084): a regular file of at most [`GITDIR_READ_CAP`] bytes, never read through a link
/// or from a FIFO, one final newline dropped. `None`: missing, larger, or not UTF-8.
pub fn registration_gitdir(root: &Path, admin: &Path) -> Option<String> {
    let content =
        crate::core::fs_guard::guarded_read_head(root, &admin.join("gitdir"), GITDIR_READ_CAP + 1)
            .ok()??;
    if content.len() as u64 > GITDIR_READ_CAP {
        return None;
    }
    let text = String::from_utf8(content).ok()?;
    Some(match text.strip_suffix('\n') {
        Some(path) => path.to_string(),
        None => text,
    })
}

/// Whether the `gitdir` file `content` of the registration `admin` names `<work_dir>/.git` as
/// git writes it (absolute, `work_dir` canonical) or as `fix_worktree_paths` does (`../`
/// steps up from `admin`, then plain names), with one final newline at most. Never resolved
/// on disk.
fn gitdir_names(admin: &Path, content: &[u8], work_dir: &Path) -> bool {
    let Ok(text) = std::str::from_utf8(content) else {
        return false;
    };
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() || text.chars().any(char::is_control) {
        return false;
    }
    let expected = work_dir.join(".git");
    if Path::new(text).is_absolute() {
        return same_spelling(text, &expected);
    }
    let mut resolved = admin.to_path_buf();
    let (mut ups, mut names) = (0, 0);
    for part in text.split('/') {
        if part == ".." && names == 0 {
            if !resolved.pop() {
                return false;
            }
            ups += 1;
        } else if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return false;
        } else {
            resolved.push(part);
            names += 1;
        }
    }
    ups > 0 && names > 0 && resolved == expected
}

#[cfg(not(windows))]
fn same_spelling(text: &str, expected: &Path) -> bool {
    expected.to_str() == Some(text)
}

/// Git writes `C:/…` on Windows, where a name's case does not tell two paths apart.
#[cfg(windows)]
fn same_spelling(text: &str, expected: &Path) -> bool {
    expected.to_str().is_some_and(|expected| {
        expected
            .replace('\\', "/")
            .eq_ignore_ascii_case(&text.replace('\\', "/"))
    })
}

/// Whether `content` names `<work_dir>/.git` once resolved on disk, inside the owner's
/// checkout `owner` only: to say why a registration Kronn does not accept is refused.
fn gitdir_resolves_to(owner: &Path, admin: &Path, content: &[u8], work_dir: &Path) -> bool {
    let Ok(text) = std::str::from_utf8(content) else {
        return false;
    };
    let named = PathBuf::from(text.trim());
    let named = if named.is_absolute() {
        named
    } else {
        admin.join(named)
    };
    named.file_name().is_some_and(|name| name == ".git")
        && named
            .parent()
            .and_then(|parent| resolve_inside(owner, parent))
            .is_some_and(|(real, _)| real == work_dir)
}

/// The one registration of `<common>/worktrees` naming `work_dir` as Kronn or git writes it.
/// Several, or one naming it only through a link or a `..`, is refused: Kronn uses none.
fn registered_admin_dir(
    owner: &Path,
    common_dir: &Path,
    work_dir: &Path,
) -> Result<PathBuf, String> {
    let naming = registrations_naming(owner, common_dir, work_dir)?;
    match naming.exact.as_slice() {
        [admin] => Ok(admin.clone()),
        [] if naming.resolved_only > 0 => {
            Err(format!("{}: {}", work_dir.display(), NAMED_INDIRECTLY))
        }
        [] => Err(format!(
            "{} is not a worktree registered in {}",
            work_dir.display(),
            common_dir.display()
        )),
        [first, second, ..] => Err(same_worktree(owner, first, second, work_dir)),
    }
}

/// Why a worktree whose registration names it only through a link or a `..` is refused.
const NAMED_INDIRECTLY: &str = "it is registered through a link or a `..`, not as git or \
     Kronn writes it: Kronn uses no such registration";

/// Whether the managed worktree `work_dir` (`<repo>/.kronn/worktrees/<lane>`) is still
/// registered with git (KT-2084): `Ok(false)` once its registration was pruned with its
/// directory left. `Err`: not a managed path, its repository cannot be read, or its
/// registration is not one Kronn uses (several, or one naming it through a link).
pub fn is_registered(work_dir: &Path) -> Result<bool, String> {
    let work_dir = canonical(work_dir);
    let (repo, _) = managed_parts(&work_dir)
        .ok_or_else(|| format!("{} is not a managed worktree", work_dir.display()))?;
    let common_dir = registered_common_dir(&repo)?;
    let naming = registrations_naming(&repo, &common_dir, &work_dir)?;
    if naming.exact.is_empty() && naming.resolved_only == 0 {
        return Ok(false);
    }
    registered_admin_dir(&repo, &common_dir, &work_dir).map(|_| true)
}

/// The registration of `repo` (the owner's checkout) naming the worktree `work_dir`
/// (canonical, present or not) exactly, as [`registered_admin_dir`] accepts it (KT-2084).
/// `None`: none names it. Err: several do, or one only through a link or a `..`.
pub fn registration_of(repo: &Path, work_dir: &Path) -> Result<Option<PathBuf>, String> {
    let owner = canonical(repo);
    let common_dir = registered_common_dir(repo)?;
    let naming = registrations_naming(&owner, &common_dir, work_dir)?;
    if naming.exact.is_empty() && naming.resolved_only == 0 {
        return Ok(None);
    }
    registered_admin_dir(&owner, &common_dir, work_dir).map(Some)
}

/// The registration `<common>/worktrees/<id>` of the owner's checkout `repo`, a directory
/// (never a link), and the worktree its `gitdir` names as written (`None`: missing or not
/// readable as a path), never looked at (KT-2084). `None` when there is no such directory.
pub fn registration_by_id(
    repo: &Path,
    id: &std::ffi::OsStr,
) -> Result<Option<(PathBuf, Option<PathBuf>)>, String> {
    let common_dir = registered_common_dir(repo)?;
    let registrations = common_dir.join("worktrees");
    let admin = registrations.join(id);
    match std::fs::symlink_metadata(&admin) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the registration {} cannot be read ({error})",
                admin.display()
            ))
        }
    }
    let named = crate::core::fs_guard::guarded_read_head(
        &registrations,
        &admin.join("gitdir"),
        GITDIR_READ_CAP,
    )
    .ok()
    .flatten()
    .and_then(|content| {
        let text = String::from_utf8(content).ok()?;
        let text = text.strip_suffix('\n').unwrap_or(&text);
        let dot_git = registered_path(&admin, text);
        let worktree = dot_git.parent()?.to_path_buf();
        (dot_git.file_name()? == ".git" && plainly_spelled(&worktree)).then_some(worktree)
    });
    Ok(Some((admin, named)))
}

/// One worktree a repository registers, as `git worktree list` would give it, read from the
/// registration's files without running git (KT-2084).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredWorktree {
    /// The worktree as its `gitdir` file names it, `/.git` left out: absolute as written, or
    /// relative resolved from the registration (`../` steps then plain names; any other
    /// relative form is joined unresolved, so it is never plainly spelled).
    pub path: PathBuf,
    /// The `gitdir` file is UTF-8 with no control character.
    pub readable: bool,
    /// `<common>/worktrees/<id>`; `None` for the main worktree.
    pub admin: Option<PathBuf>,
    /// The branch its `HEAD` is on (`ref: refs/heads/<name>`), if any, and only a name git
    /// would take as a branch ([`check_branch_name`]).
    pub branch: Option<String>,
    /// A `locked` file is in the registration.
    pub locked: bool,
    /// Git would prune it: not locked, inside the owner's checkout, and nothing is at the
    /// `.git` its `gitdir` names.
    pub prunable: bool,
    /// `path` lies, as written and plainly spelled, inside the owner's checkout: the only
    /// worktrees Kronn looks at on disk. Any other is not Kronn's, and is never touched.
    pub inside_checkout: bool,
}

/// Most registrations [`registered_worktrees`] reads.
const MAX_REGISTRATIONS: usize = 10_000;
/// Largest `HEAD` of a registration read.
const REGISTERED_HEAD_CAP: u64 = 4096;

/// The worktrees registered for the owner's checkout `repo`, the main one first (KT-2084).
/// No git runs in a worktree or reads a registration: each `<common>/worktrees/<id>` is read
/// as regular files under a cap, never through a link and never from a FIFO, and a path a
/// registration names is looked at only inside the owner's checkout. `Err` when any
/// registration cannot be read that way, or when two name the same worktree the way git
/// compares them: a caller never takes a failure for "nothing registered". The main worktree
/// is listed unless the repository is bare, with the branch of `<common>/HEAD`: see
/// [`main_worktree_path`] for its path.
pub fn registered_worktrees(repo: &Path) -> Result<Vec<RegisteredWorktree>, String> {
    let common_dir = registered_common_dir(repo)?;
    let flags = repository_flags(&common_dir)?;
    let owner = canonical(repo);
    let mut found = Vec::new();
    if !flags.bare {
        let main = main_worktree_path(repo, &common_dir);
        found.push(RegisteredWorktree {
            readable: main.to_str().is_some(),
            admin: None,
            branch: registered_branch(&common_dir, &common_dir, || {
                snapshot::reftable_repository_refusal(&main)
            })?,
            locked: false,
            prunable: false,
            inside_checkout: true,
            path: main,
        });
    }
    let registrations = common_dir.join("worktrees");
    let names = crate::core::fs_guard::guarded_list(&common_dir, &registrations)
        .map_err(|error| {
            format!(
                "the worktrees registered in {} cannot be listed: {error}",
                common_dir.display()
            )
        })?
        .unwrap_or_default();
    if names.len() > MAX_REGISTRATIONS {
        return Err(format!(
            "{} holds more than {MAX_REGISTRATIONS} registrations: Kronn does not list them",
            registrations.display()
        ));
    }
    let mut named: HashMap<String, PathBuf> = HashMap::new();
    for name in names {
        let admin = registrations.join(name);
        // A stray file (`.DS_Store`, an editor backup) is no registration: git skips it too.
        // A link in place of a registration is refused.
        match std::fs::symlink_metadata(&admin) {
            Ok(meta) if meta.is_dir() => {}
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "the registration {} is a link: Kronn does not read it",
                    admin.display()
                ))
            }
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "the registration {} cannot be read ({error})",
                    admin.display()
                ))
            }
        }
        let unreadable = |what: &str, why: String| {
            format!(
                "the registration {} has a `{what}` Kronn cannot read ({why})",
                admin.display()
            )
        };
        let gitdir = crate::core::fs_guard::guarded_read_head(
            &registrations,
            &admin.join("gitdir"),
            GITDIR_READ_CAP + 1,
        )
        .map_err(|error| unreadable("gitdir", error.to_string()))?;
        // Git lists no registration without a `gitdir` file.
        let Some(gitdir) = gitdir else { continue };
        if gitdir.len() as u64 > GITDIR_READ_CAP {
            return Err(unreadable(
                "gitdir",
                format!("over {GITDIR_READ_CAP} bytes"),
            ));
        }
        let locked = match std::fs::symlink_metadata(admin.join("locked")) {
            Ok(meta) if meta.is_file() => true,
            Ok(_) => return Err(unreadable("locked", "not a regular file".to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(unreadable("locked", error.to_string())),
        };
        let lossy = String::from_utf8_lossy(&gitdir);
        // One final newline, as git writes it: anything else belongs to the path.
        let text = lossy.strip_suffix('\n').unwrap_or(&lossy);
        let readable = std::str::from_utf8(&gitdir).is_ok() && !text.chars().any(char::is_control);
        let dot_git = registered_path(&admin, text);
        let path = match text.strip_suffix("/.git") {
            Some(worktree) => registered_path(&admin, worktree),
            None => dot_git.clone(),
        };
        if let Some(key) = gitdir_key(&admin, &gitdir, flags.ignore_case) {
            if let Some(first) = named.insert(key, admin.clone()) {
                return Err(same_worktree(&owner, &first, &admin, &path));
            }
        }
        let inside_checkout = readable && plainly_spelled(&path) && below(&owner, &path).is_some();
        let prunable = !locked
            && inside_checkout
            && resolve_inside(&owner, &dot_git).is_some_and(|(_, present)| !present);
        let mut reftable = false;
        let branch = registered_branch(&registrations, &admin, || {
            reftable = true;
            snapshot::reftable_refusal(&path)
        })
        .map_err(|why| {
            if reftable {
                why
            } else {
                unreadable("HEAD", why)
            }
        })?;
        found.push(RegisteredWorktree {
            path,
            readable,
            admin: Some(admin),
            branch,
            locked,
            prunable,
            inside_checkout,
        });
    }
    Ok(found)
}

/// A path Kronn may act on as spelled (KT-2084): absolute, no empty, `.` or `..` part and no
/// control character. Refused, never normalised.
pub fn plainly_spelled(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    if !path.is_absolute() || text.chars().any(char::is_control) {
        return false;
    }
    let mut prefix = 0;
    for component in path.components() {
        match component {
            std::path::Component::Prefix(found) => prefix = found.as_os_str().len(),
            std::path::Component::RootDir | std::path::Component::Normal(_) => {}
            _ => return false,
        }
    }
    let Some(rest) = text
        .get(prefix..)
        .and_then(|rest| rest.strip_prefix(std::path::is_separator))
    else {
        return false;
    };
    rest.split(std::path::is_separator)
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Where the main worktree of `common_dir` is: the folder of a `.git` common directory; else
/// `repo` (canonical) when its own `.git` file names the common directory (a separate git
/// directory, a project that is itself a submodule); else the common directory, as git lists
/// it when it cannot tell.
fn main_worktree_path(repo: &Path, common_dir: &Path) -> PathBuf {
    if common_dir.file_name().is_some_and(|name| name == ".git") {
        if let Some(parent) = common_dir.parent() {
            return parent.to_path_buf();
        }
    }
    let repo = canonical(repo);
    let named = read_capped_text(&repo, &repo.join(".git"), POINTER_READ_CAP)
        .ok()
        .and_then(|text| {
            let mut lines = text.lines().filter(|line| !line.trim().is_empty());
            let target = lines.next()?.strip_prefix("gitdir:")?.trim().to_string();
            lines.next().is_none().then_some(target)
        })
        .map(|target| canonical(&repo.join(target)));
    if named.as_deref() == Some(common_dir) {
        repo
    } else {
        common_dir.to_path_buf()
    }
}

/// What the owner's configuration says of its repository (KT-2084).
#[derive(Debug, Clone, Copy)]
struct RepositoryFlags {
    /// `core.bare`: git lists no main worktree.
    bare: bool,
    /// `core.ignorecase`: git compares paths without their case.
    ignore_case: bool,
}

fn known_case_rules() -> &'static Mutex<HashMap<PathBuf, bool>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// [`RepositoryFlags`] of `common_dir`, read in one bounded git. `Err` when the configuration
/// cannot be read in time.
fn repository_flags(common_dir: &Path) -> Result<RepositoryFlags, String> {
    // lane-git-allow: reads the registered repository's configuration, no worktree.
    let mut command = crate::core::cmd::project_git_cmd();
    command
        .arg(format!("--git-dir={}", common_dir.display()))
        .env("GIT_NO_LAZY_FETCH", "1")
        .args([
            "config",
            "--bool",
            "-z",
            "--get-regexp",
            r"^core\.(bare|ignorecase)$",
        ]);
    let output = snapshot::bounded_output(command, "config core.bare core.ignorecase")?;
    let mut flags = RepositoryFlags {
        bare: false,
        ignore_case: false,
    };
    match output.status.code() {
        // The last value of a key wins, as git reads it.
        Some(0) => {
            for entry in output.stdout.split(|byte| *byte == 0) {
                let entry = String::from_utf8_lossy(entry);
                match entry.split_once('\n') {
                    Some(("core.bare", value)) => flags.bare = value.trim() == "true",
                    Some(("core.ignorecase", value)) => flags.ignore_case = value.trim() == "true",
                    _ => {}
                }
            }
        }
        // Neither is set.
        Some(1) => {}
        _ => {
            return Err(format!(
                "the configuration of {} cannot be read: {}",
                common_dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
    if let Ok(mut cache) = known_case_rules().lock() {
        cache.insert(common_dir.to_path_buf(), flags.ignore_case);
    }
    Ok(flags)
}

/// Whether the repository of `common_dir` ignores case, as last read.
fn ignores_case(common_dir: &Path) -> Result<bool, String> {
    let known = known_case_rules()
        .lock()
        .ok()
        .and_then(|cache| cache.get(common_dir).copied());
    match known {
        Some(ignore_case) => Ok(ignore_case),
        None => repository_flags(common_dir).map(|flags| flags.ignore_case),
    }
}

/// The branch the `HEAD` of the git directory `git_dir` (below `root`) is on: read as a
/// regular file under a cap, never through a link or from a FIFO. `Ok(None)`: detached, no
/// `HEAD`, or a name git would not take as a branch ([`check_branch_name`]). The stub `HEAD` of
/// a reftable repository names no branch: `Err(reftable())` (KT-2101).
fn registered_branch(
    root: &Path,
    git_dir: &Path,
    reftable: impl FnOnce() -> String,
) -> Result<Option<String>, String> {
    let head = crate::core::fs_guard::guarded_read_head(
        root,
        &git_dir.join("HEAD"),
        REGISTERED_HEAD_CAP + 1,
    )
    .map_err(|error| error.to_string())?;
    let Some(head) = head else { return Ok(None) };
    if head.len() as u64 > REGISTERED_HEAD_CAP {
        return Err(format!("over {REGISTERED_HEAD_CAP} bytes"));
    }
    if head == snapshot::REFTABLE_STUB {
        return Err(reftable());
    }
    Ok(std::str::from_utf8(&head)
        .ok()
        .and_then(|text| text.strip_prefix("ref: refs/heads/"))
        .map(|name| name.strip_suffix('\n').unwrap_or(name))
        .filter(|name| check_branch_name(name).is_ok())
        .map(str::to_string))
}

/// Whether `name` is a branch name git itself would create: the rules of
/// `git check-ref-format --branch` for `refs/heads/<name>`, checked without running git, and
/// never an option (no leading `-`). A branch name read from a worktree, its registration or
/// its `HEAD` reaches git only once it passes (KT-2084). `Err` says why.
pub fn check_branch_name(name: &str) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "`{}` is not a branch name Kronn passes to git: {why}",
            name.escape_debug()
        ))
    };
    if name.is_empty() {
        return refuse("it is empty");
    }
    if name.starts_with('-') {
        return refuse("it starts with `-`, like an option");
    }
    if name == "@" || name == "HEAD" {
        return refuse("git reserves it");
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return refuse("it holds `..`, `@{` or `//`");
    }
    if name.starts_with('/') || name.ends_with('/') || name.ends_with('.') {
        return refuse("it starts or ends with `/`, or ends with `.`");
    }
    if name.chars().any(|c| {
        c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return refuse("it holds a space, a control character or one of `~^:?*[\\`");
    }
    if name
        .split('/')
        .any(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return refuse("one of its parts starts with `.` or ends with `.lock`");
    }
    Ok(())
}

/// `text`, a path a registration's `gitdir` file gives: absolute as written; relative as
/// `../` steps up from `admin` then plain names, resolved; any other relative form joined to
/// `admin` as it stands.
fn registered_path(admin: &Path, text: &str) -> PathBuf {
    if Path::new(text).is_absolute() {
        return PathBuf::from(text);
    }
    let mut resolved = admin.to_path_buf();
    let mut names = 0;
    for part in text.split('/') {
        if part == ".." && names == 0 {
            if !resolved.pop() {
                return admin.join(text);
            }
        } else if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return admin.join(text);
        } else {
            resolved.push(part);
            names += 1;
        }
    }
    resolved
}

/// The project checkout `work_dir` is a managed worktree of (`<repo>/.kronn/worktrees/<name>`
/// once resolved), read from the path alone.
pub fn managed_worktree_repo(work_dir: &Path) -> Option<PathBuf> {
    managed_parts(&canonical(work_dir)).map(|(repo, _)| repo)
}

/// The registration of the managed worktree `work_dir` itself (not a directory inside it),
/// with its `.git` pointer still naming it, found without running git in the worktree or in
/// its git directory (KT-2084): where a judgement of it starts.
pub fn managed_registration(work_dir: &Path) -> Result<Pinned, String> {
    let work_dir = canonical(work_dir);
    let (repo, _) = managed_parts(&work_dir)
        .ok_or_else(|| format!("{} is not a managed worktree", work_dir.display()))?;
    registered_worktree(&repo, &work_dir)
}

/// [`managed_registration`] of a worktree `repo` (the owner's checkout) registers anywhere,
/// a step's worktree nested in a run's (`<run>/.kronn/pr-N`) included (KT-2084).
pub fn registered_worktree(repo: &Path, work_dir: &Path) -> Result<Pinned, String> {
    let work_dir = canonical(work_dir);
    let common_dir = registered_common_dir(repo)?;
    let git_dir = registered_admin_dir(&canonical(repo), &common_dir, &work_dir)?;
    if let Some(why) = pointer_problem(&work_dir, &git_dir) {
        return Err(tampered(&work_dir, &git_dir, why));
    }
    Ok(Pinned {
        repo: repo.to_path_buf(),
        common_dir,
        git_dir,
    })
}

/// Largest `.git` file read: a `gitdir:` line, far below this.
const POINTER_READ_CAP: u64 = 4096;

/// Why the `.git` pointer of the worktree `work_dir` no longer names `git_dir`, if it does
/// not: read as a regular file under a cap, never through a link or from a FIFO, and compared
/// as written (KT-2084), never resolved on disk: the path an agent writes there is not looked
/// at.
pub fn pointer_problem(work_dir: &Path, git_dir: &Path) -> Option<String> {
    let text = match read_capped_text(work_dir, &work_dir.join(".git"), POINTER_READ_CAP) {
        Ok(text) => text,
        Err(why) => return Some(why),
    };
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let target = lines
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(str::trim);
    let Some(target) = target.filter(|_| lines.next().is_none()) else {
        return Some("is not a single `gitdir:` line".to_string());
    };
    let names_it = pointer_target(work_dir, target)
        .and_then(|path| path.to_str().map(|path| same_spelling(path, git_dir)))
        .unwrap_or(false);
    (!names_it).then(|| format!("points at `{target}`"))
}

/// The id of the registration the `.git` pointer of the worktree `work_dir` names: the last
/// part of its `gitdir:` path, as written and never resolved, read as [`pointer_problem`]
/// reads it (KT-2084). `None`: not a single `gitdir:` line.
pub fn pointer_registration_id(work_dir: &Path) -> Option<String> {
    let text = read_capped_text(work_dir, &work_dir.join(".git"), POINTER_READ_CAP).ok()?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let target = lines.next()?.strip_prefix("gitdir:")?.trim();
    if lines.next().is_some() {
        return None;
    }
    let id = target.rsplit(['/', '\\']).next()?;
    (!id.is_empty() && id != "." && id != "..").then(|| id.to_string())
}

/// The git directory a `.git` pointer's `target` names, read as git or Kronn writes it:
/// absolute and plainly spelled, or `../` steps up from `work_dir` then plain names. `None`
/// for any other form.
fn pointer_target(work_dir: &Path, target: &str) -> Option<PathBuf> {
    if Path::new(target).is_absolute() {
        return plainly_spelled(Path::new(target)).then(|| PathBuf::from(target));
    }
    let mut resolved = work_dir.to_path_buf();
    let mut names = 0;
    for part in target.split('/') {
        if part == ".." && names == 0 {
            if !resolved.pop() {
                return None;
            }
        } else if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return None;
        } else {
            resolved.push(part);
            names += 1;
        }
    }
    (names > 0).then_some(resolved)
}

/// The refusal of a worktree whose pointer no longer names its registration.
fn tampered(work_dir: &Path, git_dir: &Path, why: String) -> String {
    tracing::warn!(
        "{TAMPER_PREFIX}: {} — {why}; no git command runs in it",
        work_dir.display()
    );
    format!(
        "{TAMPER_PREFIX}: the `.git` file of {} {why}. Kronn refuses to run git in a \
         worktree whose repository pointer was changed; restore `gitdir: {}` and retry.",
        work_dir.display(),
        git_dir.display()
    )
}

/// The text of the regular file `path` (below `root`), read without following a link and
/// never from a FIFO, at most `cap` bytes. `Err` says why, to be put after a file name.
fn read_capped_text(root: &Path, path: &Path, cap: u64) -> Result<String, String> {
    let bytes = match crate::core::fs_guard::guarded_read_head(root, path, cap + 1) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err("cannot be read (it is missing)".to_string()),
        Err(error) => return Err(format!("cannot be read ({error})")),
    };
    if bytes.len() as u64 > cap {
        return Err(format!("is over {cap} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| "is not valid UTF-8".to_string())
}

/// A path git reported, relative to the worktree and plain: no root, no prefix, and no
/// empty, `.` or `..` part (a trailing `/` marks a directory).
pub fn plain_relative(path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    !path.is_empty()
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// The directory of the submodule `relative` of the work tree `work_dir` (canonical), as
/// Kronn judges or runs git in it (KT-2084): a plain path, reached through no link, strictly
/// inside the work tree once resolved.
pub fn submodule_dir(work_dir: &Path, relative: &str) -> Result<PathBuf, String> {
    if !plain_relative(relative) {
        return Err(format!(
            "`{relative}` is not a plain path of {}",
            work_dir.display()
        ));
    }
    let nested = work_dir.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(work_dir, &nested)
        .map_err(|why| format!("`{relative}` of {}: {why}", work_dir.display()))?;
    if std::fs::symlink_metadata(&nested).is_ok() {
        let real = canonical(&nested);
        if !(real.starts_with(work_dir) && real != work_dir) {
            return Err(format!(
                "`{relative}` of {} resolves to {}, outside it",
                work_dir.display(),
                real.display()
            ));
        }
    }
    Ok(nested)
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
    let mut command = crate::core::cmd::project_git_cmd();
    command
        .arg(format!("--git-dir={}", common_dir.display()))
        .args([
            "config",
            "--null",
            "--get-regexp",
            r"^(filter|merge|diff)\.",
        ]);
    let output = snapshot::bounded_output(command, "config");
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
pub fn names_worktree_content(value: &str, work_dir: &Path) -> bool {
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
/// (`filter.*`, `core.fsmonitor`, `core.hooksPath`, `core.worktree`, `include.path`,
/// `gpg.*`, `diff.*`, `merge.*`, `url.*`) is something a worker could have added.
pub fn nested_key_is_plain(key: &str) -> bool {
    const CORE: [&str; 7] = [
        "core.repositoryformatversion",
        "core.filemode",
        "core.bare",
        "core.logallrefupdates",
        "core.ignorecase",
        "core.precomposeunicode",
        "core.symlinks",
    ];
    if CORE.contains(&key)
        || matches!(
            key,
            "user.name" | "user.email" | "extensions.objectformat" | "extensions.refstorage"
        )
    {
        return true;
    }
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["remote", .., "url" | "fetch"] | ["branch", .., "remote" | "merge"] => parts.len() >= 3,
        _ => false,
    }
}

/// [`nested_key_is_plain`] for a worktree's own configuration (`config.worktree`): also the
/// switches `git sparse-checkout` writes there and `git worktree add` copies to each new
/// worktree (KT-2084). They name no program and no path.
pub fn worktree_config_key_is_plain(key: &str) -> bool {
    nested_key_is_plain(key) || is_sparse_switch(key)
}

/// A switch that makes git read the worktree's `info/sparse-checkout`.
fn is_sparse_switch(key: &str) -> bool {
    matches!(
        key,
        "core.sparsecheckout" | "core.sparsecheckoutcone" | "index.sparse"
    )
}

/// Most bytes of a worktree's sparse-checkout patterns: a pattern list, never megabytes.
const SPARSE_PATTERNS_CAP: u64 = 1024 * 1024;

/// What `git submodule init` (and `update --init --recursive`) writes into a module's
/// configuration for that module's own submodules (KT-2084): `submodule.<name>.url` and
/// `.active`, and `.update` copied from `.gitmodules`, never a `!command`. Kronn never fetches
/// during a judgement (`GIT_NO_LAZY_FETCH`) nor updates a submodule. `.ignore` is not one: it
/// would hide a moved gitlink.
fn submodule_init_key(key: &str, value: Option<&str>) -> bool {
    let Some((name, setting)) = key
        .strip_prefix("submodule.")
        .and_then(|rest| rest.rsplit_once('.'))
    else {
        return false;
    };
    !name.is_empty()
        && match setting {
            "url" | "active" => true,
            "update" => matches!(value, Some("checkout" | "rebase" | "merge" | "none")),
            _ => false,
        }
}

/// Keys of a partial clone's configuration: its missing objects are fetched on demand.
fn partial_clone_key(key: &str) -> bool {
    key == "extensions.partialclone"
        || (key.starts_with("remote.")
            && (key.ends_with(".promisor") || key.ends_with(".partialclonefilter")))
}

/// The keys a nested repository's configuration file holds, as GIT reads them
/// (`section.subsection.name`, section and name lower-cased), listed by git itself:
/// a line parser here would have to agree with git on every corner of the format
/// (a second `[section]` header on the same line, a quoted sub-section holding `]`
/// or `=`, an old-style `[section.sub]`, a backslash-continued value), and the
/// first place it disagrees is a filter the worker hides from the vetting. Git
/// reads the file alone: no repository (`--git-dir=/dev/null`), no `include`
/// followed (`--no-includes`: the `include.*` key is listed, and is not plain), no
/// system or global configuration. `Err` when git cannot parse the file.
pub fn nested_config_keys(config: &Path) -> Result<Vec<String>, String> {
    Ok(nested_config_entries(config)?
        .into_iter()
        .map(|(key, _)| key)
        .collect())
}

/// [`nested_config_keys`] with each value (`None`: a key written without one).
pub fn nested_config_entries(config: &Path) -> Result<Vec<(String, Option<String>)>, String> {
    // lane-git-allow: reads one configuration file (`config --file`), not a worktree.
    let mut command = crate::core::cmd::git_cmd();
    command
        .arg("--git-dir=/dev/null")
        .args(["config", "--file"])
        .arg(config)
        .args(["--no-includes", "--null", "--list"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .current_dir(config.parent().unwrap_or_else(|| Path::new("/")));
    let output = snapshot::bounded_output(command, "config --list")
        .map_err(|error| format!("could not be listed by git ({error})"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(format!("could not be parsed by git ({})", why.trim()));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            match entry.split_once('\n') {
                Some((key, value)) => (key.to_string(), Some(value.to_string())),
                None => (entry.into_owned(), None),
            }
        })
        .collect())
}

/// The gitlink paths of the worktree's index, cached by the index's size and
/// modification time. Listing the index runs no program. `Err` when git cannot list it
/// (an unreadable index, a timeout): nothing is vouched for then.
fn gitlink_paths(lane: &LaneGit, pinned: &Pinned) -> Result<Vec<String>, String> {
    type Known = (Option<std::time::SystemTime>, u64, Vec<String>);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Known>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let index = pinned.git_dir.join("index");
    let stamp = std::fs::symlink_metadata(&index).ok();
    if stamp.as_ref().is_some_and(|meta| !meta.is_file()) {
        return Err(format!(
            "{NESTED_PREFIX}: the index of {} is not a regular file",
            lane.work_dir.display()
        ));
    }
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
            return Ok(known.2);
        }
    }
    // From the worktree's root, whatever directory `lane` started in: `ls-files`
    // lists what is below where it runs.
    let mut command = lane.command();
    command
        .current_dir(&lane.work_dir)
        .args(["ls-files", "--stage", "-z"]);
    let output = snapshot::bounded_output(command, "ls-files").map_err(|error| {
        format!(
            "{NESTED_PREFIX}: the submodules of {} cannot be listed: {error}",
            lane.work_dir.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "{NESTED_PREFIX}: the submodules of {} cannot be listed: {}",
            lane.work_dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let paths = snapshot::gitlinks_of(&output.stdout)
        .map_err(|why| format!("{NESTED_PREFIX}: {}: {why}", lane.work_dir.display()))?;
    if let Ok(mut cache) = cache.lock() {
        cache.insert(pinned.git_dir.clone(), (key.0, key.1, paths.clone()));
    }
    Ok(paths)
}

/// The directories from `root` (excluded) down to `start` that hold a `.git`
/// entry, outermost first: the repositories git would discover from `start` instead
/// of the worktree, and the chain of them it would cross.
fn nested_repo_roots(root: &Path, start: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = start
        .ancestors()
        .take_while(|dir| *dir != root)
        .filter(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())
        .map(Path::to_path_buf)
        .collect();
    roots.reverse();
    roots
}

/// Largest configuration file of a nested repository Kronn vets.
const NESTED_CONFIG_READ_CAP: u64 = 64 * 1024;

/// A nested repository's file as UTF-8 text, read as a regular file under a cap (never
/// through a link, never from a FIFO). A file git can parse but Kronn cannot read as text
/// is not vetted: it says why, so the caller refuses.
fn read_vetted_text(path: &Path) -> Result<String, String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    read_capped_text(parent, path, NESTED_CONFIG_READ_CAP)
}

/// The git directory a submodule's `.git` FILE names, accepted only where git puts it
/// (KT-2084): `<enclosing>/modules/<plain path>`, `enclosing` being the git directory of the
/// repository that holds the submodule (a worktree's registration, or a submodule's own git
/// directory when nested). Never the common directory itself, never another worktree's
/// registration, and no part below `enclosing` is a link. `nested_dir` is the submodule's
/// directory, reached through no link.
pub fn gitfile_module_dir(nested_dir: &Path, enclosing: &Path) -> Result<PathBuf, String> {
    let text = read_capped_text(nested_dir, &nested_dir.join(".git"), POINTER_READ_CAP)
        .map_err(|why| format!("has a `.git` file that {why}"))?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let target = lines
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|target| !target.is_empty() && lines.next().is_none())
        .ok_or_else(|| "has a `.git` file that is not a single `gitdir:` line".to_string())?;
    let outside = || {
        format!(
            "has a `.git` file naming `{target}`, not a submodule directory of {}",
            enclosing.join("modules").display()
        )
    };
    let named = plainly_resolved(nested_dir, target).ok_or_else(outside)?;
    // Compared as written, never resolved (KT-2084): a path the `.git` file names outside
    // the enclosing git directory is not looked at.
    let canonical_enclosing = canonical(enclosing);
    let below: Vec<std::ffi::OsString> = [canonical_enclosing.as_path(), enclosing]
        .iter()
        .find_map(|base| named.strip_prefix(base).ok())
        .map(|rest| {
            rest.components()
                .map(|part| part.as_os_str().to_os_string())
                .collect()
        })
        .ok_or_else(outside)?;
    let enclosing = canonical_enclosing;
    if below.len() < 2 || below[0] != "modules" {
        return Err(outside());
    }
    let mut dir = enclosing.clone();
    for part in &below {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "has a `.git` file naming `{target}`, which crosses {} (a link or a file)",
                    dir.display()
                ))
            }
            Err(error) => {
                return Err(format!(
                    "has a `.git` file naming `{target}`, which cannot be read ({error})"
                ))
            }
        }
    }
    Ok(dir)
}

/// `target`, a `gitdir:` value, as a path with no `.`, `..` or empty part: absolute and
/// plainly spelled, or relative to `base` as `../` steps then plain names (what git writes).
fn plainly_resolved(base: &Path, target: &str) -> Option<PathBuf> {
    let plain =
        |part: &str| !part.is_empty() && part != "." && part != ".." && !part.contains('\\');
    if Path::new(target).is_absolute() {
        let mut parts = target.split('/');
        let _root = parts.next();
        return parts.all(plain).then(|| PathBuf::from(target));
    }
    let mut resolved = base.to_path_buf();
    let mut names = 0;
    for part in target.split('/') {
        if part == ".." && names == 0 {
            if !resolved.pop() {
                return None;
            }
        } else if plain(part) {
            resolved.push(part);
            names += 1;
        } else {
            return None;
        }
    }
    (names > 0).then_some(resolved)
}

/// Vet the `.git` of the nested repository `nested_dir`: a `.git` file must name
/// `<the enclosing git directory>/modules/<plain path>` (see [`gitfile_module_dir`]); that
/// git directory, or a `.git` directory, must hold only a configuration of plain keys, read
/// as bytes, nothing that makes git read another one (`commondir`, `config.worktree`,
/// `include*`) and no attributes of its own. A module's configuration may also name its own
/// work tree (`core.worktree`, as `git submodule` writes it) and hold what `git submodule init`
/// writes for its own submodules. `Ok` when the submodule is not materialised. The `Err` is
/// the reason.
pub fn vet_nested_dot_git(nested_dir: &Path, enclosing: &Path) -> Result<(), String> {
    vetted_nested_git_dir(nested_dir, enclosing).map(|_| ())
}

/// [`vet_nested_dot_git`], and the git directory it vetted: `None` when the submodule is not
/// materialised.
pub fn vetted_nested_git_dir(
    nested_dir: &Path,
    enclosing: &Path,
) -> Result<Option<PathBuf>, String> {
    let dot_git = nested_dir.join(".git");
    let Ok(metadata) = std::fs::symlink_metadata(&dot_git) else {
        return Ok(None); // not materialised: nothing to recurse into
    };
    if metadata.is_file() {
        let git_dir = gitfile_module_dir(nested_dir, enclosing)?;
        let label = git_dir.display().to_string();
        vet_nested_git_dir(&git_dir, &label, Some(nested_dir))?;
        return Ok(Some(git_dir));
    }
    if !metadata.is_dir() {
        return Err("has a `.git` that is neither a directory nor a file".to_string());
    }
    vet_nested_git_dir(&dot_git, ".git", None)?;
    Ok(Some(dot_git))
}

/// The vetting of [`vet_nested_dot_git`] of the git directory `git_dir`, named `label` in
/// the reason; `work_tree`: the only `core.worktree` its configuration may name.
fn vet_nested_git_dir(git_dir: &Path, label: &str, work_tree: Option<&Path>) -> Result<(), String> {
    // Files that make git read a configuration Kronn does not vet.
    for name in ["commondir", "config.worktree"] {
        if std::fs::symlink_metadata(git_dir.join(name)).is_ok() {
            return Err(format!(
                "has a `{label}/{name}`: git would read a configuration Kronn does not vet"
            ));
        }
    }
    // Attributes name the filters and drivers a command looks for: only an empty file.
    match std::fs::symlink_metadata(git_dir.join("info/attributes")) {
        Ok(m) if m.is_file() && m.len() == 0 => {}
        Ok(_) => {
            return Err(format!(
                "has a `{label}/info/attributes`: git would apply attributes Kronn does not vet"
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "has a `{label}/info/attributes` that cannot be read ({error})"
            ))
        }
    }
    let config = git_dir.join("config");
    let entries = match std::fs::symlink_metadata(&config) {
        Ok(m) if m.is_file() => {
            read_vetted_text(&config)
                .map_err(|why| format!("has a `{label}/config` that {why}"))?;
            nested_config_entries(&config)
                .map_err(|why| format!("has a `{label}/config` that {why}"))?
        }
        Ok(_) => return Err(format!("has a `{label}/config` that is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(format!(
                "has a `{label}/config` that cannot be read ({error})"
            ))
        }
    };
    let names_own_work_tree = |value: &Option<String>| {
        work_tree.is_some_and(|tree| {
            value
                .as_deref()
                .is_some_and(|value| canonical(&git_dir.join(value)) == canonical(tree))
        })
    };
    let unknown: Vec<String> = entries
        .into_iter()
        .filter(|(key, value)| {
            !(nested_key_is_plain(key)
                || submodule_init_key(key, value.as_deref())
                || key == "core.worktree" && names_own_work_tree(value))
        })
        .map(|(key, _)| key)
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else if unknown.iter().any(|key| partial_clone_key(key)) {
        Err(snapshot::partial_clone_refusal(&format!(
            "`{label}/config` declares {}",
            unknown.join(", ")
        )))
    } else {
        Err(format!(
            "has a `{label}/config` with settings Kronn does not trust: {}",
            unknown.join(", ")
        ))
    }
}

/// The git directory of the submodule at `nested`: its `.git` directory, or what its
/// `.git` file names, which must be `<enclosing>/modules/<plain path>` (KT-2084).
fn nested_git_dir(nested: &Path, enclosing: &Path) -> Result<PathBuf, String> {
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
    let outside =
        |why: String| format!("{NESTED_PREFIX}: the `.git` of {} {why}", nested.display());
    if metadata.is_file() {
        gitfile_module_dir(nested, enclosing).map_err(outside)
    } else {
        Err(outside("is neither a directory nor a file".to_string()))
    }
}

/// How many levels of submodules Kronn vets. Deeper ones are refused.
const MAX_NESTING: usize = 6;

/// The `-c` settings that turn off everything the worktree or the worker could name for
/// git to run, before the registered configuration's own relative programs.
fn base_settings() -> Vec<String> {
    vec![
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
        // `diff.submodule=diff` would start a diff inside the submodule.
        "diff.submodule=short".to_string(),
    ]
}

impl LaneGit {
    /// Every submodule of the worktree, and of each submodule in turn, is a
    /// repository whose configuration a command that recurses into it would trust
    /// (`add`, `commit -a` and `diff-index` override `diff.ignoreSubmodules`). Refuse
    /// while one carries a configuration the worker could have written: see
    /// [`vet_nested_dot_git`].
    fn check_nested_repos(&self, pinned: &Pinned) -> Result<(), String> {
        self.check_gitlinks(pinned, 0)
    }

    fn check_gitlinks(&self, pinned: &Pinned, depth: usize) -> Result<(), String> {
        for path in gitlink_paths(self, pinned)? {
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
            // A plain path, through no link, inside the worktree once resolved (KT-2084).
            let nested = match submodule_dir(&self.work_dir, &path) {
                Ok(nested) => nested,
                Err(why) => return refuse(format!("is not a submodule Kronn looks into: {why}")),
            };
            if let Err(why) = vet_nested_dot_git(&nested, &pinned.git_dir) {
                return refuse(why);
            }
            if std::fs::symlink_metadata(nested.join(".git")).is_err() {
                continue; // not materialised
            }
            // The submodules of this submodule are vetted by the same rule.
            if depth + 1 >= MAX_NESTING {
                return refuse(format!("is nested more than {MAX_NESTING} levels deep"));
            }
            let child = self.child(nested.clone(), nested)?;
            if let Some(child_pinned) = child.pinned.clone() {
                child.check_gitlinks(&child_pinned, depth + 1)?;
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
        let managed = start
            .ancestors()
            .find_map(|dir| managed_parts(dir).map(|(repo, _lane)| (dir.to_path_buf(), repo)));
        // As given, under a managed worktree; resolved, elsewhere: a link the worker made
        // leads out of it (KT-2084). Git is never run there, pinned or not.
        let given = work_dir
            .ancestors()
            .find(|dir| managed_parts(dir).is_some())
            .map(canonical);
        if let Some(given) = given {
            if managed.as_ref().map(|(dir, _)| dir) != Some(&given) {
                return Err(format!(
                    "{NESTED_PREFIX}: {} lies in the managed worktree {} but resolves to {}: \
                     Kronn runs no git command there",
                    work_dir.display(),
                    given.display(),
                    start.display()
                ));
            }
        }
        let Some((work_dir, repo)) = managed else {
            return Ok(Self {
                work_dir: start.clone(),
                start,
                pinned: None,
                parent: None,
            });
        };
        let common_dir = registered_common_dir(&repo)?;
        let git_dir = registered_admin_dir(&repo, &common_dir, &work_dir)?;
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
        // Level by level: each root must be a gitlink of the lane enclosing it.
        let mut lane = lane;
        for nested in nested_repo_roots(&lane.work_dir, &start) {
            lane = lane.child(nested, start.clone())?;
        }
        Ok(lane)
    }

    /// The lane of the submodule `nested` of this lane: its own git directory,
    /// vouched for by this lane's registration and by [`Self::check_nested_repos`]
    /// (which `verify` ran). A repository that is not a gitlink of this lane's index
    /// is one the worker made: Kronn runs nothing in it.
    fn child(&self, nested: PathBuf, start: PathBuf) -> Result<Self, String> {
        let Some(pinned) = self.pinned.clone() else {
            return Ok(self.clone());
        };
        let relative = nested
            .strip_prefix(&self.work_dir)
            .unwrap_or(&nested)
            .to_string_lossy()
            .replace('\\', "/");
        if !gitlink_paths(self, &pinned)?.contains(&relative) {
            // A linked worktree of the same repository inside this one (a PR checkout
            // under `<wt>/.kronn/`): git registered it, so it is pinned to its own
            // registration and its own pointer is verified like any managed worktree.
            // A directory the worker made, or one whose registration names another
            // path, is not registered and stays refused.
            if let Ok(git_dir) =
                registered_admin_dir(&canonical(&pinned.repo), &pinned.common_dir, &nested)
            {
                let lane = Self {
                    work_dir: nested,
                    start,
                    pinned: Some(Pinned { git_dir, ..pinned }),
                    parent: None,
                };
                lane.verify()?;
                return Ok(lane);
            }
            return Err(format!(
                "{NESTED_PREFIX}: `{relative}` of {} is a repository that is not a submodule of \
                 the worktree; Kronn runs no git command in it",
                self.work_dir.display()
            ));
        }
        let git_dir = nested_git_dir(&nested, &pinned.git_dir)?;
        Ok(Self {
            work_dir: nested,
            start,
            pinned: Some(Pinned { git_dir, ..pinned }),
            parent: Some(Box::new(self.clone())),
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
            let enclosing = parent
                .pinned
                .as_ref()
                .map_or(&pinned.git_dir, |parent| &parent.git_dir);
            return match nested_git_dir(&self.work_dir, enclosing) {
                Ok(dir) if dir == pinned.git_dir => Ok(()),
                _ => Err(format!(
                    "{NESTED_PREFIX}: the `.git` of {} changed while Kronn was using it",
                    self.work_dir.display()
                )),
            };
        }
        if let Some(why) = pointer_problem(&self.work_dir, &pinned.git_dir) {
            return Err(tampered(&self.work_dir, &pinned.git_dir, why));
        }
        self.check_worktree_config(pinned)?;
        self.check_nested_repos(pinned)
    }

    /// The worktree's own configuration (`<admin>/config.worktree`, read when the
    /// repository enables `extensions.worktreeConfig`) holds plain keys only.
    fn check_worktree_config(&self, pinned: &Pinned) -> Result<(), String> {
        let config = pinned.git_dir.join("config.worktree");
        let why = match std::fs::symlink_metadata(&config) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => format!("cannot be read ({error})"),
            Ok(m) if !m.is_file() => "is not a regular file".to_string(),
            Ok(_) => match read_vetted_text(&config).and_then(|_| nested_config_keys(&config)) {
                Err(why) => why,
                Ok(keys) => {
                    let unknown: Vec<String> = keys
                        .iter()
                        .filter(|key| !worktree_config_key_is_plain(key))
                        .cloned()
                        .collect();
                    if unknown.is_empty() {
                        return self.check_sparse_patterns(pinned, &keys);
                    }
                    format!(
                        "holds settings Kronn does not trust: {}",
                        unknown.join(", ")
                    )
                }
            },
        };
        tracing::warn!(
            "{TAMPER_PREFIX}: {} — config.worktree {why}; no git command runs in it",
            self.work_dir.display()
        );
        Err(format!(
            "{TAMPER_PREFIX}: the `config.worktree` of {} {why}. Kronn refuses to run git with a \
             worktree configuration it does not vet; remove it and retry.",
            self.work_dir.display()
        ))
    }

    /// With a sparse switch in its configuration, git reads the worktree's own
    /// `<admin>/info/sparse-checkout` (KT-2084): absent, or a regular file under a size cap
    /// read through no link; a link or a FIFO there refuses the worktree.
    fn check_sparse_patterns(&self, pinned: &Pinned, keys: &[String]) -> Result<(), String> {
        if !keys.iter().any(|key| is_sparse_switch(key)) {
            return Ok(());
        }
        let patterns = pinned.git_dir.join("info").join("sparse-checkout");
        let read = crate::core::fs_guard::guarded_read_head(
            &pinned.git_dir,
            &patterns,
            SPARSE_PATTERNS_CAP + 1,
        );
        let why = match read {
            Ok(None) => return Ok(()),
            Ok(Some(bytes)) if bytes.len() as u64 <= SPARSE_PATTERNS_CAP => return Ok(()),
            Ok(Some(_)) => format!("is over {SPARSE_PATTERNS_CAP} bytes"),
            Err(error) => format!("is not a regular file Kronn reads ({error})"),
        };
        tracing::warn!(
            "{TAMPER_PREFIX}: {} — {} {why}; no git command runs in it",
            self.work_dir.display(),
            patterns.display()
        );
        Err(format!(
            "{TAMPER_PREFIX}: the sparse-checkout file {} of {} {why}. Kronn refuses to run git \
             where git would read it; remove it and retry.",
            patterns.display(),
            self.work_dir.display()
        ))
    }

    /// Whether this is a managed worktree pinned to its registration.
    pub fn is_pinned(&self) -> bool {
        self.pinned.is_some()
    }

    /// Whether this is a submodule of a managed worktree, not the worktree itself.
    pub fn is_submodule(&self) -> bool {
        self.parent.is_some()
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
        let mut settings = base_settings();
        if let Some(pinned) = &self.pinned {
            settings.extend(neutralised_programs(&pinned.common_dir, &self.work_dir));
        }
        settings
    }

    /// The common directory git must use: the registered one, never what a
    /// `commondir` file in the administrative directory names. A submodule's
    /// repository is its own.
    fn pinned_common_dir(&self) -> Option<&Path> {
        let pinned = self.pinned.as_ref()?;
        Some(if self.parent.is_some() {
            &pinned.git_dir
        } else {
            &pinned.common_dir
        })
    }

    /// The hooks the configuration git reads declares (`hook.<name>.*`), each to be
    /// turned off by name: `core.hooksPath` does not reach them.
    fn hooks_off(&self) -> Vec<std::ffi::OsString> {
        hooks_off_keys(match self.pinned_common_dir() {
            Some(common_dir) => declared_hooks(Some(common_dir), &self.start),
            None => declared_hooks(None, &self.start),
        })
    }

    /// The pinned common directory, the switches, the hooks turned off and the
    /// environment every command of this lane carries.
    fn harden(&self, command: &mut Command) {
        for setting in self.settings() {
            command.arg("-c").arg(setting);
        }
        // `--config-env` splits at the LAST `=`: a hook name holding one stays whole.
        for key in self.hooks_off() {
            let mut arg = std::ffi::OsString::from("--config-env=");
            arg.push(key);
            arg.push("=KRONN_LANE_HOOK_OFF");
            command.arg(arg);
        }
        if let Some(common_dir) = self.pinned_common_dir() {
            command.env("GIT_COMMON_DIR", common_dir);
        }
        command
            .env("KRONN_LANE_HOOK_OFF", "false")
            .env("GIT_ATTR_NOSYSTEM", "1")
            // A partial clone's missing object is fetched with the remote's
            // transport program (`core.sshCommand`): never from a Kronn command.
            .env("GIT_NO_LAZY_FETCH", "1")
            .env_remove("GIT_EXTERNAL_DIFF");
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
        self.harden(&mut command);
        command.current_dir(&self.start);
        command
    }
}

#[cfg(unix)]
fn os_string(bytes: Vec<u8>) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn os_string(bytes: Vec<u8>) -> Option<std::ffi::OsString> {
    String::from_utf8(bytes).ok().map(Into::into)
}

/// The names of the hooks declared in the configuration of the repository at
/// `common_dir` (or the one `start` is in), with the user's, byte for byte; cached
/// briefly. Listing the configuration runs no program.
fn declared_hooks(common_dir: Option<&Path>, start: &Path) -> Vec<Vec<u8>> {
    type Cache = Mutex<HashMap<PathBuf, (std::time::Instant, Vec<Vec<u8>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let key = common_dir.unwrap_or(start).to_path_buf();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, known)) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        if at.elapsed() < std::time::Duration::from_secs(30) {
            return known;
        }
    }
    // lane-git-allow: reads the registered (or the owner's) configuration, no worktree.
    let mut list = crate::core::cmd::project_git_cmd();
    match common_dir {
        Some(dir) => list.arg(format!("--git-dir={}", dir.display())),
        None => list.current_dir(start),
    };
    list.env("GIT_NO_LAZY_FETCH", "1").args([
        "config",
        "--null",
        "--name-only",
        "--get-regexp",
        r"^hook\.",
    ]);
    let output = snapshot::bounded_output(list, "config");
    let names = hook_names(&output.map(|o| o.stdout).unwrap_or_default());
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, (std::time::Instant::now(), names.clone()));
    }
    names
}

/// The hook names of `config --null --name-only --get-regexp ^hook\.` output, once each.
fn hook_names(stdout: &[u8]) -> Vec<Vec<u8>> {
    let mut names: Vec<Vec<u8>> = Vec::new();
    for entry in stdout.split(|b| *b == 0) {
        let Some(rest) = entry.strip_prefix(b"hook.") else {
            continue;
        };
        let Some(last) = rest.iter().rposition(|byte| *byte == b'.') else {
            continue;
        };
        let name = rest[..last].to_vec();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The `hook.<name>.enabled` keys that turn the hooks `names` off.
fn hooks_off_keys(names: Vec<Vec<u8>>) -> Vec<std::ffi::OsString> {
    names
        .into_iter()
        .filter_map(|name| {
            let mut key = b"hook.".to_vec();
            key.extend_from_slice(&name);
            key.extend_from_slice(b".enabled");
            os_string(key)
        })
        .collect()
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
            if let Some(common_dir) = lane.pinned_common_dir() {
                command.env("GIT_COMMON_DIR", common_dir);
            }
            let mut settings: Vec<(std::ffi::OsString, std::ffi::OsString)> = lane
                .settings()
                .iter()
                .map(|setting| {
                    let (key, value) = setting.split_once('=').unwrap_or((setting, ""));
                    (key.into(), value.into())
                })
                .collect();
            settings.extend(
                lane.hooks_off()
                    .into_iter()
                    .map(|key| (key, "false".into())),
            );
            command.env("GIT_CONFIG_COUNT", settings.len().to_string());
            for (index, (key, value)) in settings.into_iter().enumerate() {
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

/// `--ignore-submodules=dirty` for the subcommands that compare a worktree with
/// the index or a commit (`status`, `diff`, `diff-index`) when `work_dir` is (inside)
/// a managed worktree. On the command line it beats a worker-written `.gitmodules`
/// `ignore = none`, so git never starts a `git status` INSIDE a submodule. A pointer
/// that moved is still reported. The owner's own checkout is left alone.
pub fn ignore_submodules(work_dir: &Path, subcommand: &str) -> &'static [&'static str] {
    if !matches!(subcommand, "status" | "diff" | "diff-index") {
        return &[];
    }
    let start = canonical(work_dir);
    if start.ancestors().any(|dir| managed_parts(dir).is_some()) {
        &["--ignore-submodules=dirty"]
    } else {
        &[]
    }
}

/// [`ignore_submodules`] applied to a whole argument list.
pub fn with_ignore_submodules<'a>(work_dir: &Path, args: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::with_capacity(args.len() + 1);
    if let Some((subcommand, rest)) = args.split_first() {
        out.push(*subcommand);
        out.extend_from_slice(ignore_submodules(work_dir, subcommand));
        out.extend_from_slice(rest);
    }
    out
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
#[path = "lane_git_guard_test.rs"]
mod lane_git_guard_test;

#[path = "lane_git_snapshot.rs"]
pub mod snapshot;
