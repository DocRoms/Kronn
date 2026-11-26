//! Kronn's own copy of the git state of a repository an agent can write (KT-2084).
//!
//! A worktree's registration (`<common>/worktrees/<id>`) and any nested `.git` are written by
//! whatever runs in the worktree, at any time. The read-only git commands of a judgement (a
//! delete, a release, a sweep) therefore never read them: they run against a private copy
//! taken here, in Kronn's data directory, removed when dropped (and at boot, when a stopped
//! Kronn left one: [`sweep_leftovers`]):
//!
//! - a linked worktree: its `HEAD` and `index`, read with the registered common directory
//!   (the owner's checkout, out of the agent's reach), and its `config.worktree` only when
//!   the registered configuration enables `extensions.worktreeConfig` and the file holds
//!   plain keys;
//! - a submodule repository: its `HEAD`, `index`, `packed-refs`, `shallow` and loose refs
//!   (`refs/stash` included: its older entries live in its reflog, not copied, and are kept
//!   whenever it is), a configuration Kronn writes (format version and object format,
//!   nothing else), and its objects read through `objects/info/alternates`;
//! - either one's shared index, when its index is split (`core.splitIndex`).
//!
//! Every file is read as a regular file under a cap, never through a link, never from a
//! FIFO, and every command is bounded in time.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use super::{
    base_settings, canonical, declared_hooks, hook_names, hooks_off_keys, managed_registration,
    nested_config_keys, neutralised_programs, registered_worktree, submodule_dir,
    vetted_nested_git_dir, worktree_config_key_is_plain, Pinned, NESTED_PREFIX, TAMPER_PREFIX,
};

/// The folder of Kronn's data directory the copies live in.
pub const SNAPSHOTS_DIR: &str = "git-snapshots";
/// Largest `HEAD` copied.
pub const HEAD_CAP: u64 = 4096;
/// Largest index copied.
pub const INDEX_CAP: u64 = 128 * 1024 * 1024;
/// Largest loose ref copied.
pub const LOOSE_REF_CAP: u64 = 4096;
/// Most loose refs copied from one repository.
pub const MAX_LOOSE_REFS: usize = 10_000;
/// Deepest folder of loose refs.
const MAX_REF_DEPTH: usize = 32;
/// Largest `packed-refs` copied.
pub const PACKED_REFS_CAP: u64 = 64 * 1024 * 1024;
/// Largest `shallow` copied.
pub const SHALLOW_CAP: u64 = 16 * 1024 * 1024;
/// Largest configuration read (`config.worktree`, a submodule's `config`, `commondir`).
pub const CONFIG_CAP: u64 = 64 * 1024;
/// Longest a git command of a judgement may run.
pub const JUDGE_TIMEOUT: Duration = Duration::from_secs(120);
/// The `HEAD` file of a reftable repository, whose real HEAD lives in its tables.
pub const REFTABLE_STUB: &[u8] = b"ref: refs/heads/.invalid\n";
/// Named by every reason Kronn gives for keeping a worktree of a reftable repository.
pub const REFTABLE_FORMAT: &str = "reftable format";

/// Why Kronn keeps `work_dir`, of a repository whose refs are in the reftable format: its copy
/// of the git state cannot hold them. KT-2101 (0.15.5) brings reftable support.
pub fn reftable_refusal(work_dir: &Path) -> String {
    format!(
        "{} belongs to a repository that keeps its refs in the {REFTABLE_FORMAT}, which Kronn \
         cannot judge yet: Kronn leaves it in place; delete it by hand (`git worktree remove`) \
         once nothing in it is needed",
        work_dir.display()
    )
}

/// Why Kronn does not judge a submodule cloned with `--filter`, whose configuration declares
/// `declared`: git fetches its missing objects on demand (KT-2084).
pub fn partial_clone_refusal(declared: &str) -> String {
    format!(
        "is a partial clone ({declared}), whose objects git fetches on demand: Kronn does not \
         judge it; delete the worktree by hand (`git worktree remove --force`) once nothing in \
         it is needed"
    )
}

/// Why Kronn lists no worktree of the reftable repository whose main checkout is `main`
/// (KT-2101): it cannot tell which branch a checkout is on, so it neither creates nor removes
/// a worktree there.
pub fn reftable_repository_refusal(main: &Path) -> String {
    format!(
        "{} keeps its refs in the {REFTABLE_FORMAT}, which Kronn cannot judge yet: Kronn \
         creates no worktree in it and removes none; remove one by hand (`git worktree remove`) \
         once nothing in it is needed",
        main.display()
    )
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

/// A shorter bound for this thread's judgements: a test of a command that never ends.
#[cfg(any(test, feature = "test-support"))]
pub fn set_timeout_for_tests(timeout: Option<Duration>) {
    TIMEOUT.with(|bound| bound.set(timeout));
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TRACE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Git's trace of every command this thread runs against a copy, appended to `file`: a test
/// of which programs git starts.
#[cfg(any(test, feature = "test-support"))]
pub fn set_trace_for_tests(file: Option<PathBuf>) {
    TRACE.with(|trace| *trace.borrow_mut() = file);
}

/// How long a git command of a judgement may run.
pub fn judge_timeout() -> Duration {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(bound) = TIMEOUT.with(std::cell::Cell::get) {
        return bound;
    }
    JUDGE_TIMEOUT
}

/// `command` run to its end within [`judge_timeout`]. `Err` says it timed out, or why it
/// could not start.
pub fn bounded_output(command: Command, what: &str) -> Result<Output, String> {
    bounded_output_with_input(command, &[], what)
}

/// [`bounded_output`] with `input` on the command's standard input.
pub fn bounded_output_with_input(
    mut command: Command,
    input: &[u8],
    what: &str,
) -> Result<Output, String> {
    let timeout = judge_timeout();
    match crate::core::cmd::run_bounded_output(&mut command, input, timeout) {
        Ok(crate::core::cmd::BoundedOutput::Exited(output)) => Ok(output),
        Ok(crate::core::cmd::BoundedOutput::TimedOut) => Err(format!(
            "git {what} did not finish within its {} s timeout",
            timeout.as_secs_f32()
        )),
        Err(error) => Err(format!("git {what} could not be started: {error}")),
    }
}

/// The gitlink paths of `ls-files --stage -z` output, as git lists them: a path that is not
/// UTF-8 is refused, never guessed.
pub fn gitlinks_of(stdout: &[u8]) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for entry in stdout.split(|byte| *byte == 0) {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if !entry.starts_with(b"160000 ") {
            continue;
        }
        let path = std::str::from_utf8(&entry[tab + 1..]).map_err(|_| {
            format!(
                "a submodule path is not UTF-8 ({}): Kronn does not judge it",
                String::from_utf8_lossy(&entry[tab + 1..])
            )
        })?;
        paths.push(path.to_string());
    }
    Ok(paths)
}

/// A private copy of a repository's git state, and how to run git against it.
#[derive(Debug)]
pub struct Snapshot {
    /// Kronn's folder of copies, canonical: the root the copy is removed under.
    holder: PathBuf,
    /// The copy: the git directory every command names.
    dir: PathBuf,
    /// The common directory git is given: the registered one, or the copy itself.
    common_dir: PathBuf,
    /// The files git compares with the copy, if any.
    work_tree: Option<PathBuf>,
    /// The repository's own git directory, canonical: a worktree's registration, a
    /// submodule's repository.
    source: PathBuf,
    settings: Vec<String>,
    hooks_off: Vec<OsString>,
}

/// Removes every entry of Kronn's folder of copies: what a Kronn stopped mid-judgement left
/// behind (KT-2084). Run at boot, before any judgement. Returns how many entries went.
pub fn sweep_leftovers() -> usize {
    match crate::core::config::config_dir() {
        Ok(data_dir) => sweep_leftovers_in(&data_dir),
        Err(_) => 0,
    }
}

/// [`sweep_leftovers`] of the data directory `data_dir`: each entry of its folder of copies
/// is removed through a held directory, a link as a link, never followed.
pub fn sweep_leftovers_in(data_dir: &Path) -> usize {
    let base = canonical(data_dir);
    let holder = base.join(SNAPSHOTS_DIR);
    let names = match crate::core::fs_guard::guarded_list(&base, &holder) {
        Ok(Some(names)) => names,
        Ok(None) => return 0,
        Err(error) => {
            tracing::warn!("git snapshots in {} not swept: {error}", holder.display());
            return 0;
        }
    };
    let mut removed = 0;
    for name in names {
        let entry = holder.join(name);
        match crate::core::fs_guard::guarded_remove_entry(&holder, &entry) {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!("git snapshot {} left: {error}", entry.display()),
        }
    }
    removed
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Err(error) = crate::core::fs_guard::guarded_remove_tree(&self.holder, &self.dir) {
            tracing::warn!("git snapshot {} left behind: {error}", self.dir.display());
        }
    }
}

impl Snapshot {
    /// A new, empty copy in Kronn's data directory.
    fn empty(work_tree: Option<PathBuf>, source: PathBuf) -> Result<Self, String> {
        let base = crate::core::config::config_dir()
            .map_err(|error| format!("Kronn's data directory is unknown: {error}"))?;
        let holder = base.join(SNAPSHOTS_DIR);
        std::fs::create_dir_all(&holder)
            .map_err(|error| format!("cannot create {}: {error}", holder.display()))?;
        let holder = canonical(&holder);
        let dir = holder.join(format!("s-{}", uuid::Uuid::new_v4().simple()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        Ok(Self {
            holder,
            common_dir: dir.clone(),
            dir,
            work_tree,
            source,
            settings: Vec::new(),
            hooks_off: Vec::new(),
        })
    }

    /// The copy of the managed worktree `work_dir` (`<repo>/.kronn/worktrees/<name>`), found
    /// by its registration, whose `.git` pointer must still name it.
    pub fn of_worktree(work_dir: &Path) -> Result<Self, String> {
        Self::of_pinned(managed_registration(work_dir)?, work_dir)
    }

    /// [`Snapshot::of_worktree`] of a worktree `repo` registers anywhere below Kronn's
    /// checkouts: a step's worktree nested in a run's.
    pub fn of_registered_worktree(repo: &Path, work_dir: &Path) -> Result<Self, String> {
        Self::of_pinned(registered_worktree(repo, work_dir)?, work_dir)
    }

    fn of_pinned(pinned: Pinned, work_dir: &Path) -> Result<Self, String> {
        let work_dir = canonical(work_dir);
        check_commondir(&work_dir, &pinned)?;
        let admin = &pinned.git_dir;
        // Read before anything else of the registration: what git reads is this copy.
        let worktree_config = if worktree_config_enabled(&pinned.common_dir) {
            read_capped(
                admin,
                &admin.join("config.worktree"),
                CONFIG_CAP,
                "the worktree configuration",
            )?
        } else {
            None
        };
        let mut snapshot = Self::empty(Some(work_dir.clone()), admin.clone())?;
        snapshot.common_dir = pinned.common_dir.clone();
        // The ref store finds the shared refs through this file, not the environment.
        snapshot.put(
            "commondir",
            format!("{}\n", pinned.common_dir.display()).as_bytes(),
        )?;
        if !snapshot.copy_in(admin, "HEAD", HEAD_CAP, "the HEAD")? {
            return Err(format!(
                "the registration {} has no HEAD: Kronn does not judge it",
                admin.display()
            ));
        }
        // A reftable worktree keeps HEAD in tables the copy does not hold: refused, not guessed
        // (KT-2101).
        if snapshot.head_is_reftable_stub() {
            return Err(reftable_refusal(&work_dir));
        }
        snapshot.copy_in(admin, "index", INDEX_CAP, "the index")?;
        snapshot.copy_shared_index(admin)?;
        if let Some(bytes) = worktree_config {
            snapshot.put("config.worktree", &bytes)?;
            let copy = snapshot.dir.join("config.worktree");
            let unknown: Vec<String> = nested_config_keys(&copy)
                .map_err(|why| worktree_config_refused(&work_dir, &why))?
                .into_iter()
                .filter(|key| !worktree_config_key_is_plain(key))
                .collect();
            if !unknown.is_empty() {
                return Err(worktree_config_refused(
                    &work_dir,
                    &format!(
                        "holds settings Kronn does not trust: {}",
                        unknown.join(", ")
                    ),
                ));
            }
        }
        snapshot.settings = base_settings();
        snapshot
            .settings
            .extend(neutralised_programs(&pinned.common_dir, &work_dir));
        snapshot.settings.extend(judge_settings());
        snapshot.hooks_off = hooks_off_keys(declared_hooks(Some(&pinned.common_dir), &work_dir));
        Ok(snapshot)
    }

    /// The copy of the submodule repository `git_dir` (a real directory below `parent`, as
    /// are its objects), compared with `work_tree` when it has one.
    pub fn of_module(
        git_dir: &Path,
        parent: &Path,
        work_tree: Option<&Path>,
    ) -> Result<Self, String> {
        // A submodule lane whose git directory is itself a linked worktree of a module repository fails closed.
        let objects = git_dir.join("objects");
        for dir in [git_dir, objects.as_path()] {
            crate::core::fs_guard::assert_contained_no_symlink(parent, dir).map_err(|why| {
                format!(
                    "{NESTED_PREFIX}: {} is not a repository Kronn judges: {why}",
                    git_dir.display()
                )
            })?;
            if !std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir()) {
                return Err(format!(
                    "{NESTED_PREFIX}: {} is not a repository Kronn judges: {} is not a directory",
                    git_dir.display(),
                    dir.display()
                ));
            }
        }
        // The copy reads the module's objects through its own `alternates`; git would follow
        // the module's in turn, wherever it points.
        let alternates = objects.join("info").join("alternates");
        match std::fs::symlink_metadata(&alternates) {
            Ok(_) => {
                return Err(format!(
                    "{NESTED_PREFIX}: {} shares another repository's objects (its \
                     `objects/info/alternates`, from `--reference`, `--shared` or \
                     `submodule.alternateLocation`): Kronn does not judge such a submodule; \
                     delete the worktree by hand (`git worktree remove --force`) once nothing \
                     in it is needed",
                    git_dir.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "{NESTED_PREFIX}: {} cannot be judged: its `objects/info/alternates` cannot \
                     be read ({error})",
                    git_dir.display()
                ))
            }
        }
        let config = read_capped(
            git_dir,
            &git_dir.join("config"),
            CONFIG_CAP,
            "the configuration",
        )?;
        let mut snapshot = Self::empty(work_tree.map(Path::to_path_buf), canonical(git_dir))?;
        let (version, format) = match config {
            Some(bytes) => {
                snapshot.put("config.read", &bytes)?;
                let read = snapshot.dir.join("config.read");
                let found = repository_format(&read, git_dir, work_tree.unwrap_or(git_dir));
                let _ = std::fs::remove_file(&read);
                found?
            }
            None => ("0".to_string(), None),
        };
        let mut written =
            format!("[core]\n\trepositoryformatversion = {version}\n\tbare = false\n");
        if let Some(format) = format {
            written.push_str(&format!("[extensions]\n\tobjectFormat = {format}\n"));
        }
        snapshot.put("config", written.as_bytes())?;
        if !snapshot.copy_in(git_dir, "HEAD", HEAD_CAP, "the HEAD")? {
            return Err(format!(
                "{NESTED_PREFIX}: {} has no HEAD: Kronn does not judge it",
                git_dir.display()
            ));
        }
        if snapshot.head_is_reftable_stub() {
            return Err(reftable_refusal(work_tree.unwrap_or(git_dir)));
        }
        snapshot.copy_in(git_dir, "index", INDEX_CAP, "the index")?;
        snapshot.copy_shared_index(git_dir)?;
        snapshot.copy_in(git_dir, "packed-refs", PACKED_REFS_CAP, "the packed refs")?;
        snapshot.copy_in(git_dir, "shallow", SHALLOW_CAP, "the shallow list")?;
        snapshot.copy_refs(git_dir)?;
        snapshot.put(
            "objects/info/alternates",
            format!("{}\n", canonical(&objects).display()).as_bytes(),
        )?;
        snapshot.settings = base_settings();
        snapshot.settings.extend(judge_settings());
        snapshot.hooks_off = hooks_off_keys(snapshot.own_hooks());
        Ok(snapshot)
    }

    /// The copy of the submodule `relative` (a gitlink path) of this copy's work tree: `None`
    /// when it is not populated. Its path is plain, crosses no link and stays inside the work
    /// tree; a `.git` file names `<this repository>/modules/<plain path>`; a `.git` directory
    /// passes the nested-repository vetting first.
    pub fn of_submodule(&self, relative: &str) -> Result<Option<Self>, String> {
        let work_tree = self
            .work_tree
            .as_deref()
            .ok_or_else(|| format!("{} has no work tree", self.source.display()))?;
        let refuse = |why: String| {
            format!(
                "{NESTED_PREFIX}: the submodule `{relative}` of {} {why}: Kronn does not judge it",
                work_tree.display()
            )
        };
        let nested = submodule_dir(work_tree, relative)
            .map_err(|why| refuse(format!("is not a submodule Kronn looks into ({why})")))?;
        let dot_git = nested.join(".git");
        let meta = match std::fs::symlink_metadata(&dot_git) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(refuse(format!("cannot be read ({error})"))),
        };
        if !(meta.is_file() || meta.is_dir()) {
            return Err(refuse(
                "has a `.git` that is neither a directory nor a file".to_string(),
            ));
        }
        // A `.git` file's module repository is vetted like a `.git` directory (KT-2084).
        let Some(git_dir) = vetted_nested_git_dir(&nested, &self.source).map_err(refuse)? else {
            return Ok(None);
        };
        let parent = if meta.is_file() {
            self.source.join("modules")
        } else {
            nested.clone()
        };
        Self::of_module(&git_dir, &parent, Some(&nested)).map(Some)
    }

    /// The files git compares with this copy.
    pub fn work_tree(&self) -> Option<&Path> {
        self.work_tree.as_deref()
    }

    /// The repository's own git directory the copy was taken from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The copy's git directory (Kronn's own, removed with it).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `git` against the copy: the copy as git directory, the switches that turn off what a
    /// repository could name for git to run, no optional lock, no lazy fetch, no implicit
    /// bare repository, C messages.
    // User-global filters (e.g. git-lfs) assigned by in-tree attributes still run; they are the user's own programs.
    pub fn command(&self) -> Command {
        // lane-git-allow: pinned to Kronn's own copy of the repository's git state.
        let mut command = crate::core::cmd::git_cmd();
        command.arg(format!("--git-dir={}", self.dir.display()));
        if let Some(work_tree) = &self.work_tree {
            command.arg(format!("--work-tree={}", work_tree.display()));
        }
        for setting in &self.settings {
            command.arg("-c").arg(setting);
        }
        // `--config-env` splits at the LAST `=`: a hook name holding one stays whole.
        for key in &self.hooks_off {
            let mut arg = OsString::from("--config-env=");
            arg.push(key);
            arg.push("=KRONN_LANE_HOOK_OFF");
            command.arg(arg);
        }
        command
            .env("GIT_COMMON_DIR", &self.common_dir)
            .env("KRONN_LANE_HOOK_OFF", "false")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .env_remove("GIT_EXTERNAL_DIFF")
            .env_remove("GIT_INDEX_FILE")
            .current_dir(self.work_tree.as_deref().unwrap_or(&self.dir));
        #[cfg(any(test, feature = "test-support"))]
        if let Some(file) = TRACE.with(|trace| trace.borrow().clone()) {
            command.env("GIT_TRACE", file);
        }
        command
    }

    /// `git <args>` against the copy, within [`judge_timeout`].
    pub fn output(&self, args: &[&str]) -> Result<Output, String> {
        let mut command = self.command();
        command.args(args);
        bounded_output(command, args.first().copied().unwrap_or_default())
    }

    /// The gitlink paths of the copy's index, as git lists them (not yet judged plain).
    pub fn gitlinks(&self) -> Result<Vec<String>, String> {
        let output = self.output(&["ls-files", "--stage", "-z"])?;
        if !output.status.success() {
            return Err(format!(
                "the submodules of {} cannot be listed: {}",
                self.work_tree.as_deref().unwrap_or(&self.source).display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        gitlinks_of(&output.stdout)
    }

    /// Writes `bytes` at `relative` in the copy, Kronn's own folder.
    fn put(&self, relative: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// Copies the regular file `relative` of `git_dir` into the copy, under `cap`.
    /// `Ok(false)`: there is none.
    fn copy_in(
        &self,
        git_dir: &Path,
        relative: &str,
        cap: u64,
        what: &str,
    ) -> Result<bool, String> {
        match read_capped(git_dir, &git_dir.join(relative), cap, what)? {
            Some(bytes) => self.put(relative, &bytes).map(|()| true),
            None => Ok(false),
        }
    }

    /// Whether the copied `HEAD` is the stub of a reftable repository.
    fn head_is_reftable_stub(&self) -> bool {
        std::fs::read(self.dir.join("HEAD")).is_ok_and(|head| head == REFTABLE_STUB)
    }

    /// Copies the shared index the copied `index` names (`core.splitIndex`):
    /// `sharedindex.<id>` of `git_dir`, read like the index, both within [`INDEX_CAP`].
    fn copy_shared_index(&self, git_dir: &Path) -> Result<(), String> {
        let index = match std::fs::read(self.dir.join("index")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("the copy of the index cannot be read: {error}")),
        };
        let Some(id) = shared_index_id(&index).map_err(|why| {
            format!(
                "the index of {} {why}: Kronn does not judge it",
                git_dir.display()
            )
        })?
        else {
            return Ok(());
        };
        let name = format!("sharedindex.{id}");
        let left = INDEX_CAP.saturating_sub(index.len() as u64);
        match read_capped(git_dir, &git_dir.join(&name), left, "the shared index")? {
            Some(bytes) => self.put(&name, &bytes),
            None => Err(format!(
                "the index of {} names {name}, which is missing: Kronn does not judge it",
                git_dir.display()
            )),
        }
    }

    /// Copies the loose refs of `git_dir` (`refs/**`), plain names only, bounded in count and
    /// depth; a link or anything but a file or a folder refuses.
    fn copy_refs(&self, git_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(self.dir.join("refs")).map_err(|error| {
            format!("cannot create the refs of {}: {error}", self.dir.display())
        })?;
        let refuse = |why: String| {
            format!(
                "{NESTED_PREFIX}: the refs of {} {why}: Kronn does not judge it",
                git_dir.display()
            )
        };
        let mut pending = vec![("refs".to_string(), 0usize)];
        let mut copied = 0usize;
        while let Some((relative, depth)) = pending.pop() {
            let names = crate::core::fs_guard::guarded_list(git_dir, &git_dir.join(&relative))
                .map_err(|error| refuse(format!("cannot be listed ({error})")))?
                .unwrap_or_default();
            for name in names {
                let Some(name) = name.to_str().filter(|name| {
                    !name.is_empty()
                        && *name != "."
                        && *name != ".."
                        && !name.chars().any(|c| c.is_control() || c == '\\')
                }) else {
                    return Err(refuse(format!(
                        "hold a name Kronn does not copy ({name:?})"
                    )));
                };
                let entry = format!("{relative}/{name}");
                let meta = std::fs::symlink_metadata(git_dir.join(&entry))
                    .map_err(|error| refuse(format!("cannot be read ({entry}: {error})")))?;
                if meta.is_dir() {
                    if depth + 1 > MAX_REF_DEPTH {
                        return Err(refuse(format!(
                            "are nested deeper than {MAX_REF_DEPTH} levels"
                        )));
                    }
                    std::fs::create_dir_all(self.dir.join(&entry))
                        .map_err(|error| format!("cannot create {entry}: {error}"))?;
                    pending.push((entry, depth + 1));
                } else if meta.is_file() {
                    copied += 1;
                    if copied > MAX_LOOSE_REFS {
                        return Err(refuse(format!("are more than {MAX_LOOSE_REFS}")));
                    }
                    self.copy_in(git_dir, &entry, LOOSE_REF_CAP, "a ref")?;
                } else {
                    return Err(refuse(format!("hold {entry}, a link or a special file")));
                }
            }
        }
        Ok(())
    }

    /// The hooks the copy's configuration (Kronn's) and the user's declare, read for this
    /// copy only.
    fn own_hooks(&self) -> Vec<Vec<u8>> {
        // lane-git-allow: reads the configuration of Kronn's own copy and the user's.
        let mut list = crate::core::cmd::project_git_cmd();
        list.arg(format!("--git-dir={}", self.dir.display()))
            .env("GIT_NO_LAZY_FETCH", "1")
            .args([
                "config",
                "--null",
                "--name-only",
                "--get-regexp",
                r"^hook\.",
            ])
            .current_dir(&self.dir);
        bounded_output(list, "config")
            .map(|output| hook_names(&output.stdout))
            .unwrap_or_default()
    }
}

/// The hex id of the shared index a split `index` names in its `link` extension; `None` when
/// it names none. `Err`: its layout is not one git writes.
fn shared_index_id(index: &[u8]) -> Result<Option<String>, String> {
    if index.len() < 12 || &index[..4] != b"DIRC" {
        return Err("has no index header".to_string());
    }
    let version = u32::from_be_bytes([index[4], index[5], index[6], index[7]]);
    if !(2..=4).contains(&version) {
        return Err(format!(
            "is in an index version Kronn does not read ({version})"
        ));
    }
    let entries = u32::from_be_bytes([index[8], index[9], index[10], index[11]]) as usize;
    // The object id size is not written in the index: the one whose layout ends exactly at
    // the trailing checksum.
    [20, 32]
        .into_iter()
        .find_map(|hash| link_extension(index, version, entries, hash))
        .ok_or_else(|| "has a layout Kronn does not read".to_string())
}

/// The `link` id of `index` read with `hash`-byte object ids, `Some(None)` when it has none;
/// `None` when the index does not read through that way.
fn link_extension(
    index: &[u8],
    version: u32,
    entries: usize,
    hash: usize,
) -> Option<Option<String>> {
    let end = index.len().checked_sub(hash)?;
    let mut at = 12usize;
    for _ in 0..entries {
        // Stat data (40 bytes), the object id, the flags, the extended flags in v3+.
        let flags_at = at.checked_add(40 + hash)?;
        let flags = index.get(flags_at..flags_at + 2)?;
        let extended = version >= 3 && flags[0] & 0x40 != 0;
        let mut name_at = flags_at + if extended { 4 } else { 2 };
        if version == 4 {
            // How much of the previous name to drop, then the rest, NUL-terminated.
            name_at += varint_length(index.get(name_at..end)?)?;
            let name = index
                .get(name_at..end)?
                .iter()
                .position(|byte| *byte == 0)?;
            at = name_at + name + 1;
        } else {
            // NUL-padded to a multiple of 8 bytes from the entry's start, at least one NUL.
            let name = index
                .get(name_at..end)?
                .iter()
                .position(|byte| *byte == 0)?;
            at = at.checked_add((name_at - at + name + 8) & !7)?;
        }
        if at > end {
            return None;
        }
    }
    let mut link = None;
    while at < end {
        let header = index.get(at..at.checked_add(8)?)?;
        let size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let data = at + 8;
        let next = data.checked_add(size)?;
        if next > end {
            return None;
        }
        if &header[..4] == b"link" {
            let id = index.get(data..data.checked_add(hash)?)?;
            link = Some(id.iter().map(|byte| format!("{byte:02x}")).collect());
        }
        at = next;
    }
    (at == end).then_some(link)
}

/// The length of the varint git writes at the start of `bytes` (index v4).
fn varint_length(bytes: &[u8]) -> Option<usize> {
    let used = bytes.iter().position(|byte| byte & 0x80 == 0)? + 1;
    (used <= 10).then_some(used)
}

/// The switches a judgement adds to the lane's. A copied index's untracked cache would
/// let git skip a folder it records as unchanged: it is never used.
fn judge_settings() -> [String; 2] {
    [
        "safe.bareRepository=explicit".to_string(),
        "core.untrackedCache=false".to_string(),
    ]
}

/// The regular file `path` of `root` under `cap`, read without following a link and never
/// from a FIFO. `None`: there is none.
fn read_capped(root: &Path, path: &Path, cap: u64, what: &str) -> Result<Option<Vec<u8>>, String> {
    match crate::core::fs_guard::guarded_read_head(root, path, cap + 1) {
        Ok(None) => Ok(None),
        Ok(Some(bytes)) if bytes.len() as u64 > cap => Err(format!(
            "{what} {} is over {cap} bytes: Kronn does not judge it",
            path.display()
        )),
        Ok(Some(bytes)) => Ok(Some(bytes)),
        Err(error) => Err(format!(
            "{what} {} cannot be copied ({error}): Kronn does not judge it",
            path.display()
        )),
    }
}

/// A `commondir` in the registration must name the registered common directory (git writes
/// `../..`): one naming another directory, or one that is not a regular file, refuses.
fn check_commondir(work_dir: &Path, pinned: &Pinned) -> Result<(), String> {
    let admin = &pinned.git_dir;
    let Some(bytes) = read_capped(admin, &admin.join("commondir"), CONFIG_CAP, "the commondir")?
    else {
        return Ok(());
    };
    let named = std::str::from_utf8(&bytes)
        .ok()
        .map(str::trim)
        .filter(|named| !named.is_empty())
        .map(PathBuf::from)
        .map(|named| {
            if named.is_absolute() {
                named
            } else {
                admin.join(named)
            }
        });
    match named {
        Some(named) if canonical(&named) == pinned.common_dir => Ok(()),
        _ => Err(format!(
            "{TAMPER_PREFIX}: the registration of {} names another common directory in its \
             `commondir`: Kronn does not judge it",
            work_dir.display()
        )),
    }
}

/// Whether the registered configuration enables `extensions.worktreeConfig`: only then does
/// git read a worktree's `config.worktree`.
fn worktree_config_enabled(common_dir: &Path) -> bool {
    // lane-git-allow: reads the registered repository's configuration, no worktree.
    let mut command = crate::core::cmd::project_git_cmd();
    command
        .arg(format!("--git-dir={}", common_dir.display()))
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["config", "--bool", "--get", "extensions.worktreeConfig"]);
    bounded_output(command, "config")
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn worktree_config_refused(work_dir: &Path, why: &str) -> String {
    tracing::warn!(
        "{TAMPER_PREFIX}: {} — config.worktree {why}; not judged",
        work_dir.display()
    );
    format!(
        "{TAMPER_PREFIX}: the `config.worktree` of {} {why}: Kronn does not judge it",
        work_dir.display()
    )
}

/// The format version and object format the copy of a submodule's configuration `read`
/// declares (`0` or `1`; `sha1` or `sha256`). Any other value, or any other extension, is not
/// judged; a reftable one is named as such for `shown` (KT-2101).
fn repository_format(
    read: &Path,
    git_dir: &Path,
    shown: &Path,
) -> Result<(String, Option<String>), String> {
    let refuse = |why: String| {
        format!(
            "{NESTED_PREFIX}: the configuration of {} {why}: Kronn does not judge it",
            git_dir.display()
        )
    };
    // lane-git-allow: lists Kronn's copy of one configuration file, no repository.
    let mut command = crate::core::cmd::git_cmd();
    command
        .arg("--git-dir=/dev/null")
        .args(["config", "--file"])
        .arg(read)
        .args(["--no-includes", "--null", "--list"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .current_dir(read.parent().unwrap_or_else(|| Path::new("/")));
    let output = bounded_output(command, "config --list").map_err(refuse)?;
    if !output.status.success() {
        return Err(refuse(format!(
            "cannot be parsed by git ({})",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut version = "0".to_string();
    let mut format = None;
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|e| !e.is_empty())
    {
        let entry = String::from_utf8_lossy(entry);
        let (key, value) = entry.split_once('\n').unwrap_or((&entry, ""));
        match key {
            "core.repositoryformatversion" if matches!(value, "0" | "1") => {
                version = value.to_string();
            }
            "core.repositoryformatversion" => {
                return Err(refuse(format!("declares format version {value:?}")))
            }
            "extensions.objectformat" => {
                let value = value.to_ascii_lowercase();
                if !matches!(value.as_str(), "sha1" | "sha256") {
                    return Err(refuse(format!("declares object format {value:?}")));
                }
                format = Some(value);
            }
            "extensions.refstorage" if value.eq_ignore_ascii_case("reftable") => {
                return Err(reftable_refusal(shown));
            }
            "extensions.partialclone" => {
                return Err(format!(
                    "{NESTED_PREFIX}: {} {}",
                    git_dir.display(),
                    partial_clone_refusal("`extensions.partialclone`")
                ));
            }
            other if other.starts_with("extensions.") => {
                return Err(refuse(format!("declares `{other}`")));
            }
            _ => {}
        }
    }
    Ok((version, format))
}
