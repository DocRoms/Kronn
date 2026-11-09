//! Filesystem write primitives for routes that write inside a project (Codex A2,
//! KT-1595).
//!
//! `assert_contained_no_symlink` is an lstat walk: a point-in-time refusal with
//! a readable message. It guarantees nothing once it returns — a parent can be
//! swapped for a link right after — so a caller that checks and then writes
//! the path itself is NOT covered. Every write goes through a `guarded_*`
//! function below instead.
//!
//! Unix: the `guarded_*` writes use the rooted primitive
//! (`core::rooted_fd`, shared with the run artifacts): the root is opened once
//! as a directory fd (the root itself is trusted — a path Kronn chose — and
//! may be reached through links), then each component below it is created
//! with `mkdirat` and opened with `openat(O_NOFOLLOW | O_DIRECTORY)` from the
//! previous fd. File names are only ever resolved against the held parent fd
//! (`openat`, `linkat`, `renameat`, `unlinkat`). Guarantees, stated honestly:
//! - a symlink anywhere below the root (dangling included) is never
//!   followed — whether it pre-exists or is swapped in for a parent after
//!   the walk; a parent MOVED (not replaced by a link) inside the root
//!   mid-operation still receives the write, because the fd holds the
//!   original directory, which is under the root;
//! - create-only writes (`guarded_write_new`, `guarded_copy_new`) write a
//!   temp sibling and publish it no-clobber (`linkat`): a file or symlink
//!   appearing between check and publish is never overwritten nor followed —
//!   the call quietly reports `Ok(false)`;
//! - replacing writes (`guarded_replace*`) write a temp sibling and
//!   `renameat` it over the name, so a link planted at the name is replaced,
//!   never written through. A regular file keeps its permissions and a new
//!   file gets 0666 under the umask (`fs::write` parity). Only the `rwx` bits
//!   travel (`0o777`): setuid, setgid and sticky are never carried to written
//!   content, a copy (`guarded_copy_new`) included;
//! - the temp is compared with its name (device and inode) right before
//!   publication, and the published entry is compared with the temp right
//!   after. A temp swapped for a link or another file in the instant between
//!   the first check and the publication is DETECTED after the fact and the
//!   call fails. That is detection only: nothing is unlinked by name after the
//!   publication (the entry could by then be a concurrent writer's file, or
//!   our own good one), so a link or foreign file published inside that window
//!   stays inside the root and is reported. Nothing outside the root is ever
//!   written: every step runs against the held fds;
//! - `guarded_replace_if_unchanged_keeping_mode` is optimistic, not an atomic
//!   compare-and-swap: it holds one handle on the file, checks right before
//!   the `renameat` that the name is still that file and that its bytes are
//!   still the ones observed, and a write landing after that check is lost;
//! - `guarded_move_new` opens the source `O_NOFOLLOW`, links it no-clobber and
//!   unlinks the source only while its name still is the opened file (device
//!   and inode); a source swapped meanwhile is left alone and the move is
//!   reported as failed or incomplete. `guarded_remove_file_if_ours` does the
//!   same for a file we created (`guarded_write_new_identified` returns its
//!   identity). Both are a stat followed by an `unlinkat`, not an atomic
//!   compare-and-unlink: they keep a rollback from deleting what someone put
//!   at the name BEFORE the check;
//! - `guarded_read` reads a regular file through a handle opened against the
//!   held parent fd, `O_NOFOLLOW`: a link at the name or in a parent is
//!   refused. It is for decisions a caller takes about files it is about to
//!   write; other reads in the code base are not covered (below);
//! - a failure before publication leaves NO destination (the temp is removed
//!   by best-effort RAII); durability is `fs::write` parity (no fsync),
//!   except `guarded_replace_like`, which syncs.
//!
//! Not covered, on any platform: a caller that obtains a path from
//! `assert_contained_no_symlink` / `docs_location::contained_dir` /
//! `docs_location::guard_write` and writes it by itself (the read-only ones
//! are fine); READS (`resolve_contained_read` canonicalizes, a swap after it
//! can still be read through); the files a spawned `git` or an agent process
//! writes by path; and other by-path writers that never call this module
//! (`root_agent_files::inject_or_update`, `td_decisions`, `adr`, `skills`,
//! `profiles`, and in `worktree` the copy of the untracked files into a new
//! checkout, the `info/exclude` append and the submodule parent folders).
//!
//! Windows (residual risk, owner decision amending the KT-1595 DoD): there
//! is no directory-fd API in use here, so every `guarded_*` write keeps the
//! previous behaviour — the lstat walk, then the write by path. A parent
//! directory swapped for a link between the walk and the write is therefore
//! NOT excluded. Creating a symlink there needs elevated rights or developer
//! mode, which is why this is accepted instead of failing closed (worktree
//! creation, template installs, native files and seeding must keep working
//! on the desktop build). The follow-up is KT-1609 (a Windows rooted
//! primitive). See docs/operations/fs-guard-rooted-writes.md.
//!
//! Any other non-unix, non-Windows target FAILS CLOSED: every `guarded_*`
//! write is refused and nothing is created.
//!
//! A crashed process may leave an orphan `.kronn-tmp-*` sibling; it is inert
//! and never auto-deleted here.

use std::path::{Component, Path, PathBuf};

/// Verify `path` is lexically under `root` and that no component from
/// `root` (exclusive) down to `path` (inclusive) is a symlink. Missing
/// components are fine — they're what we're about to create.
pub fn assert_contained_no_symlink(root: &Path, path: &Path) -> Result<(), String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| format!("{} escapes the root {}", path.display(), root.display()))?;
    if rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{} contains a parent traversal", path.display()));
    }
    let mut cur = PathBuf::from(root);
    for comp in rel.components() {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink — refusing to write through it",
                    cur.display()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Test seam for the callers' tests (KT-1595): a one-shot swap of a parent
/// folder for a link, fired exactly when a `guarded_*` call holds that
/// folder as an fd — after every path check and walk of the caller. The state
/// is shared by every thread (a route runs its filesystem work on a blocking
/// thread), keyed by the folder, which each test takes from its own tempdir.
#[cfg(all(test, unix))]
pub(crate) mod swap_probe {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Probe {
        armed: HashMap<PathBuf, PathBuf>,
        fired: HashSet<PathBuf>,
    }

    static PROBE: Mutex<Option<Probe>> = Mutex::new(None);

    thread_local! {
        /// The folder this test thread armed last, for [`fired`].
        static MINE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    fn with<R>(f: impl FnOnce(&mut Probe) -> R) -> R {
        let mut guard = PROBE.lock().unwrap_or_else(|e| e.into_inner());
        f(guard.get_or_insert_with(Probe::default))
    }

    /// The next guarded call whose parent folder is `dir` finds it renamed to
    /// `<dir>.moved` and replaced by a symlink to `external`.
    pub(crate) fn arm(dir: &Path, external: &Path) {
        with(|probe| {
            probe.fired.remove(dir);
            probe
                .armed
                .insert(dir.to_path_buf(), external.to_path_buf());
        });
        MINE.with(|mine| *mine.borrow_mut() = Some(dir.to_path_buf()));
    }

    /// Whether the swap happened (and disarms the probe).
    pub(crate) fn fired() -> bool {
        let Some(dir) = MINE.with(|mine| mine.borrow().clone()) else {
            return false;
        };
        with(|probe| {
            probe.armed.remove(&dir);
            probe.fired.contains(&dir)
        })
    }

    pub(super) fn after_walk(parent: Option<&Path>) {
        let Some(parent) = parent else { return };
        let external = with(|probe| probe.armed.remove(parent));
        if let Some(external) = external {
            let mut moved = parent.to_path_buf().into_os_string();
            moved.push(".moved");
            std::fs::rename(parent, PathBuf::from(moved)).unwrap();
            std::os::unix::fs::symlink(external, parent).unwrap();
            with(|probe| probe.fired.insert(parent.to_path_buf()));
        }
    }
}

#[cfg(unix)]
pub use imp::guarded_set_mode;
pub use imp::{
    guarded_copy_new, guarded_create_dir_all, guarded_move_new, guarded_read, guarded_remove_dir,
    guarded_remove_file, guarded_remove_file_if_ours, guarded_remove_tree, guarded_rename,
    guarded_replace, guarded_replace_if_unchanged_keeping_mode, guarded_replace_like,
    guarded_write_new, guarded_write_new_identified, FileId,
};

#[cfg(unix)]
mod imp {
    use super::assert_contained_no_symlink;
    use crate::core::rooted_fd::{Dir, Identity};
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// Who a file we created is (device, inode, ctime, size), taken from its
    /// handle once nothing more of ours touches it: a later removal checks
    /// the name is still that very file. The handle is kept open so the inode
    /// number cannot be handed to a recreated file meanwhile.
    #[derive(Debug, Clone)]
    pub struct FileId {
        identity: Identity,
        _pin: std::sync::Arc<File>,
    }

    impl FileId {
        fn of(file: File) -> Result<Self, String> {
            let identity = Identity::of(&file).map_err(|e| format!("stat handle: {e}"))?;
            Ok(Self {
                identity,
                _pin: std::sync::Arc::new(file),
            })
        }
    }

    /// The permission bits of `meta` without setuid, setgid and sticky: those
    /// never travel to content we write.
    fn plain_permissions(meta: &std::fs::Metadata) -> std::fs::Permissions {
        std::fs::Permissions::from_mode(meta.permissions().mode() & 0o777)
    }

    /// `path` split into its parent (relative to `root`) and final name.
    fn split_under<'a>(root: &Path, path: &'a Path) -> Result<(PathBuf, &'a OsStr), String> {
        let rel = path
            .strip_prefix(root)
            .map_err(|_| format!("{} escapes the root {}", path.display(), root.display()))?;
        let name = rel
            .file_name()
            .ok_or_else(|| format!("{} has no file name", path.display()))?;
        Ok((
            rel.parent().map(Path::to_path_buf).unwrap_or_default(),
            name,
        ))
    }

    /// The directory holding `path` as an fd, walked without following a
    /// link, and the final name. `None` when a part is missing and not
    /// `create`d. Runs the lstat walk first for its readable refusal.
    fn parent_of<'a>(
        root: &Path,
        path: &'a Path,
        create: bool,
    ) -> Result<Option<(Dir, &'a OsStr)>, String> {
        assert_contained_no_symlink(root, path)?;
        let (parent_rel, name) = split_under(root, path)?;
        let parent = Dir::open_path(root, &parent_rel, create)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(test)]
        super::swap_probe::after_walk(path.parent());
        Ok(parent.map(|dir| (dir, name)))
    }

    /// [`parent_of`] for an entry that may itself be a link (a rename moves the
    /// link, never what it points at): only the folders above it are checked.
    fn parent_of_entry<'a>(
        root: &Path,
        path: &'a Path,
        create: bool,
    ) -> Result<Option<(Dir, &'a OsStr)>, String> {
        assert_contained_no_symlink(root, path.parent().unwrap_or(root))?;
        let (parent_rel, name) = split_under(root, path)?;
        let parent = Dir::open_path(root, &parent_rel, create)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(test)]
        super::swap_probe::after_walk(path.parent());
        Ok(parent.map(|dir| (dir, name)))
    }

    /// [`parent_of`] with the missing parents created.
    fn parent_created<'a>(root: &Path, path: &'a Path) -> Result<(Dir, &'a OsStr), String> {
        parent_of(root, path, true)?
            .ok_or_else(|| format!("{}: its folder cannot be created", path.display()))
    }

    /// `create_dir_all` through the rooted primitive: each component is
    /// created with `mkdirat` and opened `O_NOFOLLOW` from the previous fd, so
    /// a parent swapped to a symlink mid-walk is refused, never followed.
    pub fn guarded_create_dir_all(root: &Path, dir: &Path) -> Result<(), String> {
        assert_contained_no_symlink(root, dir)?;
        let rel = dir
            .strip_prefix(root)
            .map_err(|_| format!("{} escapes the root {}", dir.display(), root.display()))?;
        #[cfg(test)]
        super::swap_probe::after_walk(dir.parent());
        Dir::open_path(root, rel, true)
            .map(|_| ())
            .map_err(|e| format!("mkdir {}: {e}", dir.display()))
    }

    /// Best-effort RAII cleanup for the temp sibling on non-publish exit
    /// paths (a successful publish disarms it and unlinks explicitly). An
    /// unlink failure cannot be compensated here — it is logged, and the
    /// destination-never-published guarantee is unaffected.
    struct TempGuard<'a> {
        dir: &'a Dir,
        name: OsString,
        armed: bool,
    }
    impl Drop for TempGuard<'_> {
        fn drop(&mut self) {
            if self.armed {
                if let Err(e) = self.dir.unlink_file(&self.name) {
                    if e.kind() != io::ErrorKind::NotFound {
                        tracing::warn!("temp cleanup failed {:?}: {e}", self.name);
                    }
                }
            }
        }
    }

    /// A fresh temp file in `parent`, with its cleanup armed.
    fn temp_in(parent: &Dir, mode: u32) -> Result<(File, TempGuard<'_>), String> {
        let (file, name) = parent
            .create_temp(mode)
            .map_err(|e| format!("create temp: {e}"))?;
        Ok((
            file,
            TempGuard {
                dir: parent,
                name,
                armed: true,
            },
        ))
    }

    /// The temp name is checked against the open handle (device and inode)
    /// right before publication: a temp swapped for a link or for another
    /// file is refused, never published.
    fn still_ours(guard: &TempGuard<'_>, file: &File) -> Result<(), String> {
        match guard.dir.names_file(&guard.name, file) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "temp {:?} was replaced before publication — refused",
                guard.name
            )),
            Err(e) => Err(format!("stat temp {:?}: {e}", guard.name)),
        }
    }

    /// The entry just published at `name` must be the file written through
    /// `file`. The check before publication leaves a window (temp name
    /// checked, then linked or renamed): a temp swapped inside it would
    /// publish a link or a foreign file. This is detection after the fact and
    /// nothing more: the mismatch is reported and NOTHING is unlinked, because
    /// whatever sits at the name now may be a concurrent writer's legitimate
    /// file, or ours.
    pub(super) fn published_is_ours(parent: &Dir, name: &OsStr, file: &File) -> Result<(), String> {
        match parent.names_file(name, file) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "{name:?} was replaced during publication: reported, left in place"
            )),
            Err(e) => Err(format!(
                "cannot verify {name:?} after publication ({e}): left in place"
            )),
        }
    }

    /// Two-phase create-only publication of `name` inside the held `parent`.
    /// `write` fills the temp handle (payload + any handle-level
    /// permissions); the temp is then published atomically no-clobber via
    /// `linkat`, both against the parent fd — a swap of the parent's path
    /// after the walk is invisible here:
    /// - anything occupying `name` at publish time (file, symlink even
    ///   dangling — never followed) ⇒ `Ok(false)`, best-effort temp cleanup,
    ///   dst intact;
    /// - the temp itself swapped before publication ⇒ `Err`, nothing
    ///   published;
    /// - any error before publish ⇒ `Err`, best-effort temp cleanup, dst
    ///   NEVER existed;
    /// - `Unsupported`/`PermissionDenied` on the link are explicit `Err`s —
    ///   no rename fallback (rename overwrites);
    /// - after a successful link the destination is live: a failed unlink of
    ///   the temp downgrades to a warning, never to a false `Err`.
    ///
    /// `pre_publish` runs between "temp fully written" and the publish —
    /// production passes a no-op, tests inject deterministic races: one code
    /// path, no hidden state.
    pub(super) fn create_new_via_temp(
        parent: &Dir,
        name: &OsStr,
        write: impl FnOnce(&mut File) -> Result<(), String>,
        pre_publish: impl FnOnce(),
    ) -> Result<Option<FileId>, String> {
        // Mode 0666 & umask — fs::write parity for guarded_write_new;
        // guarded_copy_new overrides via the handle.
        let (mut tmp, mut guard) = temp_in(parent, 0o666)?;
        write(&mut tmp)?;
        tmp.flush()
            .map_err(|e| format!("flush {:?}: {e}", guard.name))?;
        pre_publish();
        still_ours(&guard, &tmp)?;
        match parent.link_new(&guard.name, name) {
            Ok(()) => {
                guard.armed = false;
                let published = published_is_ours(parent, name, &tmp);
                if let Err(e) = parent.unlink_file(&guard.name) {
                    tracing::warn!("temp cleanup failed after publish {:?}: {e}", guard.name);
                }
                published?;
                // After the temp unlink: dropping a link changes the ctime.
                Ok(Some(FileId::of(tmp)?))
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(e) => Err(format!("publish {:?} -> {name:?}: {e}", guard.name)),
        }
    }

    /// Write `bytes` at `path` ONLY if nothing exists there (a dangling
    /// symlink is "something"). Returns whether a file was written.
    /// Two-phase publish: see `create_new_via_temp` for the race contract.
    pub fn guarded_write_new(root: &Path, path: &Path, bytes: &[u8]) -> Result<bool, String> {
        guarded_write_new_hooked(root, path, bytes, || {}, || {})
    }

    /// [`guarded_write_new`] that also says who the created file is (`None`
    /// when something already occupied the name), so a rollback can remove it
    /// with [`guarded_remove_file_if_ours`].
    pub fn guarded_write_new_identified(
        root: &Path,
        path: &Path,
        bytes: &[u8],
    ) -> Result<Option<FileId>, String> {
        write_new_inner(root, path, bytes, || {}, || {})
    }

    /// Full wrapper with test seams: `after_parents` runs once the parent
    /// directory is held as an fd — the exact window where, before KT-1595, a
    /// parent swapped to a symlink was no longer re-checked; `pre_publish`
    /// runs between the written temp and the publish (final-collision
    /// injection). Production goes through this exact path with no-ops.
    pub(super) fn guarded_write_new_hooked(
        root: &Path,
        path: &Path,
        bytes: &[u8],
        after_parents: impl FnOnce(),
        pre_publish: impl FnOnce(),
    ) -> Result<bool, String> {
        write_new_inner(root, path, bytes, after_parents, pre_publish).map(|id| id.is_some())
    }

    fn write_new_inner(
        root: &Path,
        path: &Path,
        bytes: &[u8],
        after_parents: impl FnOnce(),
        pre_publish: impl FnOnce(),
    ) -> Result<Option<FileId>, String> {
        let (parent, name) = parent_created(root, path)?;
        if parent
            .exists(name)
            .map_err(|e| format!("stat {name:?}: {e}"))?
        {
            return Ok(None);
        }
        after_parents();
        create_new_via_temp(
            &parent,
            name,
            |tmp| {
                tmp.write_all(bytes)
                    .map_err(|e| format!("write {}: {e}", path.display()))
            },
            pre_publish,
        )
    }

    /// Copy `src` to `dst` ONLY if nothing exists at `dst`. Returns whether a
    /// file was created. Two-phase publish; the src mode (fstat on the open
    /// src handle) is applied to the temp handle BEFORE publication — what
    /// `fs::copy` guaranteed before the streamed rewrite. `src` is read, not
    /// written: it is a template path outside the root.
    pub fn guarded_copy_new(root: &Path, src: &Path, dst: &Path) -> Result<bool, String> {
        assert_contained_no_symlink(root, dst)?;
        if let Some((parent, name)) = parent_of(root, dst, false)? {
            if parent
                .exists(name)
                .map_err(|e| format!("stat {name:?}: {e}"))?
            {
                return Ok(false);
            }
        }
        // Input first (msg 213 pipeline): no destination mutation happens for
        // an unreadable source.
        let mut input = File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
        let (parent, name) = parent_created(root, dst)?;
        let created = create_new_via_temp(
            &parent,
            name,
            |tmp| {
                io::copy(&mut input, tmp)
                    .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
                // Flush BEFORE the mode lands on the handle (msg 213 order);
                // the caller's post-write flush is then a benign second flush.
                tmp.flush()
                    .map_err(|e| format!("flush temp for {}: {e}", dst.display()))?;
                let perms = plain_permissions(
                    &input
                        .metadata()
                        .map_err(|e| format!("metadata {}: {e}", src.display()))?,
                );
                tmp.set_permissions(perms)
                    .map_err(|e| format!("chmod temp for {}: {e}", dst.display()))
            },
            || {},
        )?;
        Ok(created.is_some())
    }

    /// Replaces `name` in the held `parent` with what `prepare` writes into a
    /// fresh temp: `renameat` over the name, so a link planted
    /// there is replaced, never written through. `before_rename` runs after
    /// the temp is complete and may refuse (the unchanged-content check) or,
    /// in tests, swap things; the temp is then verified to still be ours.
    fn replace_in(
        parent: &Dir,
        name: &OsStr,
        create_mode: u32,
        prepare: impl FnOnce(&mut File) -> Result<(), String>,
        before_rename: impl FnOnce(&Dir) -> Result<(), String>,
    ) -> Result<(), String> {
        let (mut tmp, mut guard) = temp_in(parent, create_mode)?;
        prepare(&mut tmp)?;
        tmp.flush()
            .map_err(|e| format!("flush {:?}: {e}", guard.name))?;
        before_rename(parent)?;
        still_ours(&guard, &tmp)?;
        parent
            .rename_from(parent, &guard.name, name)
            .map_err(|e| format!("rename {:?} -> {name:?}: {e}", guard.name))?;
        guard.armed = false;
        published_is_ours(parent, name, &tmp)
    }

    /// Writes `bytes` at `path`, replacing what is there (a link included,
    /// never followed), creating the missing folders. A regular file keeps its
    /// permissions; a new file (or a replaced link) gets 0666 under the umask,
    /// like a plain `fs::write`.
    pub fn guarded_replace(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
        guarded_replace_hooked(root, path, bytes, || {}, || {})
    }

    /// Test seams as in `guarded_write_new_hooked`.
    pub(super) fn guarded_replace_hooked(
        root: &Path,
        path: &Path,
        bytes: &[u8],
        after_parents: impl FnOnce(),
        before_rename: impl FnOnce(),
    ) -> Result<(), String> {
        let (parent, name) = parent_created(root, path)?;
        after_parents();
        let mode = parent
            .regular_mode(name)
            .map_err(|e| format!("stat {}: {e}", path.display()))?;
        replace_in(
            &parent,
            name,
            0o666,
            |tmp| {
                tmp.write_all(bytes)
                    .map_err(|e| format!("write {}: {e}", path.display()))?;
                if let Some(mode) = mode {
                    tmp.set_permissions(std::fs::Permissions::from_mode(mode))
                        .map_err(|e| format!("keep permissions of {}: {e}", path.display()))?;
                }
                Ok(())
            },
            |_| {
                before_rename();
                Ok(())
            },
        )
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Writes `bytes` at `path` like a plain file write would: an existing
    /// regular file keeps its permissions, a new one gets 0666 under the
    /// umask; the data is synced before the `renameat`. A link at the name,
    /// like any entry that is not a regular file, is refused.
    pub fn guarded_replace_like(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let (parent, name) = parent_created(root, path)?;
        let permissions = parent
            .open_regular(name)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .map(|existing| existing.metadata().map(|m| plain_permissions(&m)))
            .transpose()
            .map_err(|e| format!("stat {}: {e}", path.display()))?;
        replace_in(
            &parent,
            name,
            0o666,
            |tmp| {
                tmp.write_all(bytes)
                    .map_err(|e| format!("write {}: {e}", path.display()))?;
                if let Some(permissions) = permissions {
                    tmp.set_permissions(permissions)
                        .map_err(|e| format!("keep permissions of {}: {e}", path.display()))?;
                }
                tmp.sync_all()
                    .map_err(|e| format!("sync {}: {e}", path.display()))
            },
            |_| Ok(()),
        )
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Replaces the regular file at `path` with `content` only if it still
    /// holds exactly `observed`, keeping its permissions. One handle is held
    /// from the start: right before the `renameat` the name must still be that
    /// very file (device and inode; an editor's replace-by-rename changes
    /// them) and its bytes, read again through the handle, must still be
    /// `observed`. This is an optimistic check, NOT an atomic compare-and-swap:
    /// a write landing between that last read and the `renameat` is lost.
    /// A link, a missing file or anything but a regular file is refused.
    pub fn guarded_replace_if_unchanged_keeping_mode(
        root: &Path,
        path: &Path,
        content: &str,
        observed: &[u8],
    ) -> Result<(), String> {
        guarded_replace_if_unchanged_hooked(root, path, content, observed, || {})
    }

    /// Test seam as in `guarded_write_new_hooked`: `before_check` runs after
    /// the temp is complete, right before the last comparison.
    pub(super) fn guarded_replace_if_unchanged_hooked(
        root: &Path,
        path: &Path,
        content: &str,
        observed: &[u8],
        before_check: impl FnOnce(),
    ) -> Result<(), String> {
        let Some((parent, name)) = parent_of(root, path, false)? else {
            return Err(format!("{} is missing; refusing the write", path.display()));
        };
        let mut existing = parent
            .open_regular(name)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .ok_or_else(|| format!("{} is missing; refusing the write", path.display()))?;
        let permissions = plain_permissions(
            &existing
                .metadata()
                .map_err(|e| format!("stat {}: {e}", path.display()))?,
        );
        replace_in(
            &parent,
            name,
            0o600,
            |tmp| {
                tmp.write_all(content.as_bytes())
                    .map_err(|e| format!("write {}: {e}", path.display()))?;
                tmp.set_permissions(permissions)
                    .map_err(|e| format!("keep permissions of {}: {e}", path.display()))
            },
            |dir| {
                before_check();
                let changed = || {
                    format!(
                        "{} changed concurrently; refusing overwrite",
                        path.display()
                    )
                };
                match dir.names_file(name, &existing) {
                    Ok(true) => {}
                    Ok(false) => return Err(changed()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Err(format!("{} vanished; refusing the write", path.display()))
                    }
                    Err(e) => return Err(format!("stat {}: {e}", path.display())),
                }
                let mut current = Vec::new();
                existing
                    .seek(SeekFrom::Start(0))
                    .and_then(|_| existing.read_to_end(&mut current))
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                if current == observed {
                    Ok(())
                } else {
                    Err(changed())
                }
            },
        )
    }

    /// `chmod` of the regular file at `path` through its handle (a link is
    /// refused, never followed).
    pub fn guarded_set_mode(root: &Path, path: &Path, mode: u32) -> Result<(), String> {
        let (parent, name) = parent_of(root, path, false)?
            .ok_or_else(|| format!("{} is missing", path.display()))?;
        parent
            .set_mode(name, mode)
            .map_err(|e| format!("chmod {}: {e}", path.display()))
    }

    fn refused(message: String) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, message)
    }

    /// `unlinkat` of the file (or link) at `path`; a missing one is `NotFound`.
    pub fn guarded_remove_file(root: &Path, path: &Path) -> io::Result<()> {
        match parent_of(root, path, false).map_err(refused)? {
            Some((parent, name)) => parent.unlink_file(name),
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    /// `unlinkat` of the file at `path` only if the name still is the file `id`
    /// describes (device, inode, ctime and size, checked against the held
    /// parent fd); a name that is now a link, another file or gone is left
    /// alone and refused. The
    /// check and the `unlinkat` are two steps: this keeps a rollback from
    /// deleting what someone else put there BEFORE the check, it is not an
    /// atomic compare-and-unlink.
    pub fn guarded_remove_file_if_ours(root: &Path, path: &Path, id: &FileId) -> io::Result<()> {
        let Some((parent, name)) = parent_of(root, path, false).map_err(refused)? else {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        };
        if !parent.names_identity(name, &id.identity)? {
            return Err(refused(format!(
                "{} is no longer the file we created: left in place",
                path.display()
            )));
        }
        parent.unlink_file(name)
    }

    /// The bytes of the regular file at `path`, read through a handle opened
    /// `O_NOFOLLOW` against the held parent fd (a link at the name or in a
    /// parent is refused, never followed); `None` when missing.
    pub fn guarded_read(root: &Path, path: &Path) -> io::Result<Option<Vec<u8>>> {
        let Some((parent, name)) = parent_of(root, path, false).map_err(refused)? else {
            return Ok(None);
        };
        let Some(mut file) = parent.open_regular(name)? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// `unlinkat(AT_REMOVEDIR)` of the empty folder at `path`.
    pub fn guarded_remove_dir(root: &Path, path: &Path) -> io::Result<()> {
        match parent_of(root, path, false).map_err(refused)? {
            Some((parent, name)) => parent.remove_empty_dir(name),
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    /// Removes the folder (or file) at `path` with everything below it,
    /// opening each level `O_NOFOLLOW`: a link inside is unlinked as an entry,
    /// never followed. Missing is fine.
    pub fn guarded_remove_tree(root: &Path, path: &Path) -> io::Result<()> {
        match parent_of(root, path, false).map_err(refused)? {
            Some((parent, name)) => parent.remove_child(name),
            None => Ok(()),
        }
    }

    /// `renameat` of `from` to `to` (both under `root`, folders not created).
    /// `from` may be a link: it is moved as an entry, never followed.
    pub fn guarded_rename(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
        let (from_dir, from_name) = parent_of_entry(root, from, false)?
            .ok_or_else(|| format!("{} is missing", from.display()))?;
        let (to_dir, to_name) = parent_of(root, to, false)?
            .ok_or_else(|| format!("{}: its folder is missing", to.display()))?;
        to_dir
            .rename_from(&from_dir, from_name, to_name)
            .map_err(|e| format!("rename {} -> {}: {e}", from.display(), to.display()))
    }

    /// Moves the regular file `from` to `to` WITHOUT replacing: the source is
    /// opened `O_NOFOLLOW` first, a hard link (`linkat`, which refuses an
    /// occupied name) is made, and the source is unlinked only if its name is
    /// still the file we opened (device and inode), all against the held parent
    /// fds. If the name was swapped before the link, `to` is reported as not
    /// being the opened file and nothing is removed; if it was swapped after
    /// the link, the new entry at `from` is left alone and the move is
    /// reported as incomplete. The folder of `to` is created.
    pub fn guarded_move_new(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
        guarded_move_new_hooked(root, from, to, || {}, || {})
    }

    /// Test seams as in `guarded_write_new_hooked`: `after_open` runs once the
    /// source is held, `after_link` right after the hard link.
    pub(super) fn guarded_move_new_hooked(
        root: &Path,
        from: &Path,
        to: &Path,
        after_open: impl FnOnce(),
        after_link: impl FnOnce(),
    ) -> Result<(), String> {
        let (from_dir, from_name) = parent_of(root, from, false)?
            .ok_or_else(|| format!("{} is missing", from.display()))?;
        let source = from_dir
            .open_regular(from_name)
            .map_err(|e| format!("{}: {e}", from.display()))?
            .ok_or_else(|| format!("{} is missing", from.display()))?;
        let (to_dir, to_name) = parent_created(root, to)?;
        after_open();
        to_dir
            .link_from(&from_dir, from_name, to_name)
            .map_err(|e| format!("link {} -> {}: {e}", from.display(), to.display()))?;
        after_link();
        // `source` stays open, so its inode number cannot be reused meanwhile.
        match to_dir.names_file(to_name, &source) {
            Ok(true) => {}
            Ok(false) => {
                return Err(format!(
                "{} was replaced before the move: {} is not the file we opened, nothing removed",
                from.display(),
                to.display()
            ))
            }
            Err(e) => return Err(format!("stat {}: {e}", to.display())),
        }
        match from_dir.names_file(from_name, &source) {
            Ok(true) => from_dir
                .unlink_file(from_name)
                .map_err(|e| format!("remove {}: {e}", from.display())),
            Ok(false) => Err(format!(
                "{} was replaced during the move: left in place, {} holds the original",
                from.display(),
                to.display()
            )),
            // Gone already: the move is done.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("stat {}: {e}", from.display())),
        }
    }
}

/// Any target that is neither unix nor Windows: no rooted primitive and no
/// accepted residual risk, so every write is refused (fail closed) and
/// nothing is created.
#[cfg(not(any(unix, windows)))]
mod imp {
    use std::io;
    use std::path::Path;

    /// Nothing is ever created here, so there is nothing to identify.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FileId;

    const REFUSED: &str = "writes inside a project are not supported on this platform: \
        no rooted filesystem primitive is available (fail closed)";

    fn refused<T>() -> Result<T, String> {
        Err(REFUSED.to_string())
    }

    fn refused_io<T>() -> io::Result<T> {
        Err(io::Error::new(io::ErrorKind::Unsupported, REFUSED))
    }

    pub fn guarded_create_dir_all(_: &Path, _: &Path) -> Result<(), String> {
        refused()
    }
    pub fn guarded_write_new(_: &Path, _: &Path, _: &[u8]) -> Result<bool, String> {
        refused()
    }
    pub fn guarded_write_new_identified(
        _: &Path,
        _: &Path,
        _: &[u8],
    ) -> Result<Option<FileId>, String> {
        refused()
    }
    pub fn guarded_remove_file_if_ours(_: &Path, _: &Path, _: &FileId) -> io::Result<()> {
        refused_io()
    }
    pub fn guarded_read(_: &Path, _: &Path) -> io::Result<Option<Vec<u8>>> {
        refused_io()
    }
    pub fn guarded_copy_new(_: &Path, _: &Path, _: &Path) -> Result<bool, String> {
        refused()
    }
    pub fn guarded_replace(_: &Path, _: &Path, _: &[u8]) -> Result<(), String> {
        refused()
    }
    pub fn guarded_replace_like(_: &Path, _: &Path, _: &[u8]) -> Result<(), String> {
        refused()
    }
    pub fn guarded_replace_if_unchanged_keeping_mode(
        _: &Path,
        _: &Path,
        _: &str,
        _: &[u8],
    ) -> Result<(), String> {
        refused()
    }
    pub fn guarded_remove_file(_: &Path, _: &Path) -> io::Result<()> {
        refused_io()
    }
    pub fn guarded_remove_dir(_: &Path, _: &Path) -> io::Result<()> {
        refused_io()
    }
    pub fn guarded_remove_tree(_: &Path, _: &Path) -> io::Result<()> {
        refused_io()
    }
    pub fn guarded_rename(_: &Path, _: &Path, _: &Path) -> Result<(), String> {
        refused()
    }
    pub fn guarded_move_new(_: &Path, _: &Path, _: &Path) -> Result<(), String> {
        refused()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_write_is_refused() {
            let p = Path::new("x");
            assert!(guarded_create_dir_all(p, p).is_err());
            assert!(guarded_write_new(p, p, b"x").is_err());
            assert!(guarded_replace(p, p, b"x").is_err());
            assert!(guarded_remove_file(p, p).is_err());
            assert!(guarded_move_new(p, p, p).is_err());
        }
    }
}

#[cfg(windows)]
mod imp {
    //! Windows: the lstat walk, then the write by path — the previous
    //! behaviour, kept on purpose (see the module header: residual risk,
    //! KT-1609).

    use super::assert_contained_no_symlink;
    use std::io;
    use std::path::{Path, PathBuf};

    /// Nothing to identify a file by without fds: always matches.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FileId;

    fn refused(message: String) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, message)
    }

    pub fn guarded_create_dir_all(root: &Path, dir: &Path) -> Result<(), String> {
        assert_contained_no_symlink(root, dir)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))
    }

    /// Best-effort RAII cleanup for the temp sibling on non-publish exit
    /// paths (a successful publish disarms it and unlinks explicitly).
    struct TempGuard {
        path: PathBuf,
        armed: bool,
    }
    impl Drop for TempGuard {
        fn drop(&mut self) {
            if self.armed {
                if let Err(e) = std::fs::remove_file(&self.path) {
                    if e.kind() != io::ErrorKind::NotFound {
                        tracing::warn!("temp cleanup failed {}: {e}", self.path.display());
                    }
                }
            }
        }
    }

    /// Two-phase create-only publication, no-clobber through `hard_link`:
    /// anything occupying `dst` at publish time ⇒ `Ok(false)`; an error before
    /// publish leaves no destination.
    fn create_new_via_temp(
        dst: &Path,
        write: impl FnOnce(&mut std::fs::File) -> Result<(), String>,
    ) -> Result<bool, String> {
        use std::io::Write;
        let parent = dst
            .parent()
            .ok_or_else(|| format!("{}: no parent directory", dst.display()))?;
        let tmp_path = parent.join(format!(".kronn-tmp-{}", uuid::Uuid::new_v4()));
        let mut tmp = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| format!("create temp {}: {e}", tmp_path.display()))?;
        let mut guard = TempGuard {
            path: tmp_path,
            armed: true,
        };
        write(&mut tmp)?;
        tmp.flush()
            .map_err(|e| format!("flush {}: {e}", guard.path.display()))?;
        drop(tmp); // close before linking (Windows requirement)
        match std::fs::hard_link(&guard.path, dst) {
            Ok(()) => {
                guard.armed = false;
                if let Err(e) = std::fs::remove_file(&guard.path) {
                    tracing::warn!(
                        "temp cleanup failed after publish {}: {e}",
                        guard.path.display()
                    );
                }
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(format!(
                "publish {} -> {}: {e}",
                guard.path.display(),
                dst.display()
            )),
        }
    }

    pub fn guarded_write_new(root: &Path, path: &Path, bytes: &[u8]) -> Result<bool, String> {
        use std::io::Write;
        assert_contained_no_symlink(root, path)?;
        if std::fs::symlink_metadata(path).is_ok() {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            guarded_create_dir_all(root, parent)?;
        }
        create_new_via_temp(path, |tmp| {
            tmp.write_all(bytes)
                .map_err(|e| format!("write {}: {e}", path.display()))
        })
    }

    pub fn guarded_write_new_identified(
        root: &Path,
        path: &Path,
        bytes: &[u8],
    ) -> Result<Option<FileId>, String> {
        guarded_write_new(root, path, bytes).map(|created| created.then_some(FileId))
    }

    pub fn guarded_remove_file_if_ours(root: &Path, path: &Path, _: &FileId) -> io::Result<()> {
        guarded_remove_file(root, path)
    }

    pub fn guarded_read(root: &Path, path: &Path) -> io::Result<Option<Vec<u8>>> {
        assert_contained_no_symlink(root, path).map_err(refused)?;
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn guarded_copy_new(root: &Path, src: &Path, dst: &Path) -> Result<bool, String> {
        use std::io::Write;
        assert_contained_no_symlink(root, dst)?;
        if std::fs::symlink_metadata(dst).is_ok() {
            return Ok(false);
        }
        let mut input =
            std::fs::File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
        if let Some(parent) = dst.parent() {
            guarded_create_dir_all(root, parent)?;
        }
        create_new_via_temp(dst, |tmp| {
            io::copy(&mut input, tmp)
                .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
            tmp.flush()
                .map_err(|e| format!("flush temp for {}: {e}", dst.display()))?;
            let perms = input
                .metadata()
                .map_err(|e| format!("metadata {}: {e}", src.display()))?
                .permissions();
            tmp.set_permissions(perms)
                .map_err(|e| format!("chmod temp for {}: {e}", dst.display()))
        })
    }

    pub fn guarded_replace(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
        assert_contained_no_symlink(root, path)?;
        if let Some(parent) = path.parent() {
            guarded_create_dir_all(root, parent)?;
        }
        crate::core::mcp_scanner::atomic_write_bytes(path, bytes)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    pub fn guarded_replace_like(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        assert_contained_no_symlink(root, path)?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("{}: no parent directory", path.display()))?;
        guarded_create_dir_all(root, parent)?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let tmp = parent.join(format!(".{name}.kronn-replace"));
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("write {}: {e}", path.display()));
        }
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("replace {}: {e}", path.display())
        })
    }

    pub fn guarded_replace_if_unchanged_keeping_mode(
        root: &Path,
        path: &Path,
        content: &str,
        observed: &[u8],
    ) -> Result<(), String> {
        assert_contained_no_symlink(root, path)?;
        crate::core::mcp_scanner::atomic_write_if_unchanged_preserving_permissions(
            path, content, observed,
        )
    }

    pub fn guarded_remove_file(root: &Path, path: &Path) -> io::Result<()> {
        assert_contained_no_symlink(root, path).map_err(refused)?;
        std::fs::remove_file(path)
    }

    pub fn guarded_remove_dir(root: &Path, path: &Path) -> io::Result<()> {
        assert_contained_no_symlink(root, path).map_err(refused)?;
        std::fs::remove_dir(path)
    }

    pub fn guarded_remove_tree(root: &Path, path: &Path) -> io::Result<()> {
        assert_contained_no_symlink(root, path).map_err(refused)?;
        match std::fs::remove_dir_all(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    pub fn guarded_rename(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
        assert_contained_no_symlink(root, from.parent().unwrap_or(root))?;
        assert_contained_no_symlink(root, to)?;
        std::fs::rename(from, to)
            .map_err(|e| format!("rename {} -> {}: {e}", from.display(), to.display()))
    }

    pub fn guarded_move_new(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
        assert_contained_no_symlink(root, from)?;
        if let Some(parent) = to.parent() {
            guarded_create_dir_all(root, parent)?;
        }
        assert_contained_no_symlink(root, to)?;
        std::fs::hard_link(from, to)
            .map_err(|e| format!("link {} -> {}: {e}", from.display(), to.display()))?;
        std::fs::remove_file(from).map_err(|e| format!("remove {}: {e}", from.display()))
    }
}

/// Where a READ of `path` lands, refused when it leaves `root`. Relative paths
/// are taken from `root`; symlinks are followed (a link inside the project
/// pointing outside is refused); a missing tail is resolved lexically from its
/// deepest existing ancestor, and a `..` in that tail is refused.
pub fn resolve_contained_read(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve the project root {}: {e}", root.display()))?;
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut existing = joined.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let resolved = loop {
        match existing.canonicalize() {
            Ok(real) => break real,
            Err(_) => {
                let Some(name) = existing.file_name() else {
                    return Err(format!("{} cannot be resolved", path.display()));
                };
                tail.push(name);
                existing = existing
                    .parent()
                    .ok_or_else(|| format!("{} cannot be resolved", path.display()))?;
            }
        }
    };
    // `file_name` is `None` on a `..` component, so the tail never holds one.
    let mut resolved = resolved;
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(format!("{} is outside the project", path.display()))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::imp::{create_new_via_temp, guarded_replace_hooked, guarded_write_new_hooked};
    #[cfg(unix)]
    use super::swap_probe;
    use super::*;
    #[cfg(unix)]
    use crate::core::rooted_fd::Dir;

    #[test]
    fn a_read_inside_the_project_resolves_and_one_outside_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("secret"), "x").unwrap();

        assert!(resolve_contained_read(&root, Path::new("src/main.rs")).is_ok());
        assert!(resolve_contained_read(&root, Path::new("src/not-yet.rs")).is_ok());
        assert!(resolve_contained_read(&root, Path::new(".")).is_ok());
        assert!(resolve_contained_read(&root, Path::new("../secret")).is_err());
        assert!(resolve_contained_read(&root, Path::new("../../x")).is_err());
        assert!(resolve_contained_read(&root, Path::new("src/../../secret")).is_err());
        assert!(resolve_contained_read(&root, Path::new("missing/../../secret")).is_err());
        assert!(resolve_contained_read(&root, Path::new("/etc/passwd")).is_err());
        assert!(resolve_contained_read(&root, &dir.path().join("secret")).is_err());
        // Unicode names stay ordinary paths.
        assert!(resolve_contained_read(&root, Path::new("src/é漢.md")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_leaving_the_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(dir.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("link")).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("etc")).unwrap();
        std::fs::write(root.join("inside"), "y").unwrap();
        std::os::unix::fs::symlink(root.join("inside"), root.join("ok-link")).unwrap();

        assert!(resolve_contained_read(&root, Path::new("link")).is_err());
        assert!(resolve_contained_read(&root, Path::new("etc/passwd")).is_err());
        assert!(resolve_contained_read(&root, Path::new("ok-link")).is_ok());
    }

    #[cfg(unix)]
    fn no_temp_left(dir: &Path) -> bool {
        // An unreadable dir or entry must FAIL the check, never pass it.
        std::fs::read_dir(dir)
            .expect("test dir must be readable")
            .map(|e| e.expect("test dir entry must be readable"))
            .all(|e| !e.file_name().to_string_lossy().starts_with(".kronn-tmp-"))
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_regular_file_at_publish_is_never_clobbered() {
        // The Copilot round-3 finding, made deterministic: a rival file
        // lands EXACTLY between "temp written" and the publish.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let dst = proj.join("docs/target.md");
        let rival = dst.clone();
        let created = guarded_write_new_hooked(
            &proj,
            &dst,
            b"kronn content",
            || {},
            move || std::fs::write(&rival, "SENTINEL").unwrap(),
        )
        .unwrap();
        assert!(!created, "losing the race must report false, not clobber");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "SENTINEL");
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_appearing_at_publish_is_never_followed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let dst = proj.join("docs/target.md");
        let link = dst.clone();
        let nowhere = tmp.path().join("nowhere");
        let nowhere_hook = nowhere.clone();
        let created = guarded_write_new_hooked(
            &proj,
            &dst,
            b"kronn content",
            || {},
            move || std::os::unix::fs::symlink(&nowhere_hook, &link).unwrap(),
        )
        .unwrap();
        assert!(!created);
        let meta = std::fs::symlink_metadata(&dst).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "the late link stays byte-intact"
        );
        assert!(!nowhere.exists(), "the link target was never created");
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn name_max_basename_still_publishes() {
        // The temp name is basename-independent — a destination close to
        // NAME_MAX must not make the temp impossible.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let long = format!("{}.md", "a".repeat(251));
        let dst = proj.join("docs").join(&long);
        assert!(guarded_write_new(&proj, &dst, b"content").unwrap());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "content");
    }

    #[cfg(unix)]
    #[test]
    fn injected_write_failure_leaves_no_destination_and_no_temp() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let parent = Dir::open_path(&proj, Path::new("docs"), false)
            .unwrap()
            .unwrap();
        let dst = proj.join("docs/target.md");
        let err = create_new_via_temp(
            &parent,
            std::ffi::OsStr::new("target.md"),
            |f| {
                use std::io::Write;
                f.write_all(b"partial prefix").unwrap();
                Err("injected failure after a partial write".into())
            },
            || {},
        )
        .unwrap_err();
        assert!(err.contains("injected"), "{err}");
        assert!(
            std::fs::symlink_metadata(&dst).is_err(),
            "a pre-publish failure must never leave a destination"
        );
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn write_new_mode_matches_fs_write_parity() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let dst = proj.join("docs/a.md");
        assert!(guarded_write_new(&proj, &dst, b"x").unwrap());
        let control = proj.join("control.md");
        std::fs::write(&control, "x").unwrap();
        assert_eq!(
            std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            std::fs::metadata(&control).unwrap().permissions().mode() & 0o777,
            "guarded_write_new keeps fs::write mode parity (0666 & umask)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_swapped_to_symlink_after_walk_is_refused() {
        // Real regression harness (KT-1595): every path-based check and the
        // parent walk pass on a clean `docs`, THEN the directory is swapped
        // for a symlink to an EXTERNAL dir — between the walk and the write.
        // The temp open and the publish go through the held parent fd, so
        // nothing may ever land outside the root.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        let docs = proj.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let docs_hook = docs.clone();
        let external_hook = external.clone();
        let res = guarded_write_new_hooked(
            &proj,
            &proj.join("docs/escape.md"),
            b"x",
            move || {
                std::fs::remove_dir_all(&docs_hook).unwrap();
                std::os::unix::fs::symlink(&external_hook, &docs_hook).unwrap();
            },
            || {},
        );
        assert!(
            res.is_err() && !external.join("escape.md").exists(),
            "a parent swapped to a symlink after the walk must be refused, \
             and nothing may be created outside the root"
        );
        assert!(
            std::fs::read_dir(&external).unwrap().next().is_none(),
            "nothing, temp included, may appear outside the root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_moved_and_replaced_by_symlink_never_writes_outside() {
        // The non-trivial swap: the directory still holds data, so it is
        // renamed away (inside the root) and a symlink takes its name. The
        // held fd keeps pointing at the ORIGINAL directory, which is under
        // the root — the write lands there, never through the link.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        let nested = proj.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("keep.md"), "keep").unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let (a, moved, ext) = (proj.join("a"), proj.join("a-moved"), external.clone());
        let created = guarded_write_new_hooked(
            &proj,
            &proj.join("a/b/new.md"),
            b"x",
            move || {
                std::fs::rename(&a, &moved).unwrap();
                std::os::unix::fs::symlink(&ext, &a).unwrap();
            },
            || {},
        )
        .unwrap();
        assert!(created);
        assert!(
            std::fs::read_dir(&external).unwrap().next().is_none(),
            "nothing may appear in the external dir"
        );
        assert_eq!(
            std::fs::read_to_string(proj.join("a-moved/b/new.md")).unwrap(),
            "x"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_swapped_between_temp_and_publish_never_writes_outside() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        let docs = proj.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let (d, moved, ext) = (docs.clone(), proj.join("docs-moved"), external.clone());
        let created = guarded_write_new_hooked(
            &proj,
            &docs.join("late.md"),
            b"x",
            || {},
            move || {
                std::fs::rename(&d, &moved).unwrap();
                std::os::unix::fs::symlink(&ext, &d).unwrap();
            },
        )
        .unwrap();
        assert!(created);
        assert!(std::fs::read_dir(&external).unwrap().next().is_none());
        assert!(proj.join("docs-moved/late.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_all_refuses_a_link_in_the_middle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        std::os::unix::fs::symlink(&external, proj.join("link")).unwrap();
        let err = guarded_create_dir_all(&proj, &proj.join("link/sub")).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(std::fs::read_dir(&external).unwrap().next().is_none());
        // A clean nested path is created, and re-running it is a no-op.
        guarded_create_dir_all(&proj, &proj.join("x/y/z")).unwrap();
        guarded_create_dir_all(&proj, &proj.join("x/y/z")).unwrap();
        assert!(proj.join("x/y/z").is_dir());
    }

    /// Windows keeps the previous behaviour (module header): the writes work,
    /// behind the lstat walk.
    #[cfg(windows)]
    #[test]
    fn writes_keep_the_lstat_walk_behaviour_on_windows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        assert!(guarded_write_new(&proj, &proj.join("d/a.md"), b"x").unwrap());
        assert!(!guarded_write_new(&proj, &proj.join("d/a.md"), b"y").unwrap());
        assert_eq!(std::fs::read_to_string(proj.join("d/a.md")).unwrap(), "x");
        guarded_replace(&proj, &proj.join("e/b.md"), b"1").unwrap();
        guarded_replace(&proj, &proj.join("e/b.md"), b"2").unwrap();
        assert_eq!(std::fs::read_to_string(proj.join("e/b.md")).unwrap(), "2");
        guarded_move_new(&proj, &proj.join("e/b.md"), &proj.join("f/c.md")).unwrap();
        assert!(!proj.join("e/b.md").exists() && proj.join("f/c.md").is_file());
        guarded_remove_file(&proj, &proj.join("f/c.md")).unwrap();
        guarded_replace_like(&proj, &proj.join("g/d.md"), b"1").unwrap();
        guarded_replace_like(&proj, &proj.join("g/d.md"), b"2").unwrap();
        assert_eq!(std::fs::read_to_string(proj.join("g/d.md")).unwrap(), "2");
        guarded_create_dir_all(&proj, &proj.join("h/i")).unwrap();
        guarded_remove_tree(&proj, &proj.join("g")).unwrap();
        assert!(!proj.join("g").exists());
        assert!(guarded_write_new(&tmp.path().join("other"), &proj.join("z.md"), b"x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_component_and_dangling_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let external = root.join("external");
        std::fs::create_dir_all(&external).unwrap();
        let proj = root.join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        std::os::unix::fs::symlink(&external, proj.join("docs/conventions")).unwrap();
        // Through a symlinked dir component -> refusal.
        let err =
            guarded_write_new(&proj, &proj.join("docs/conventions/spec.md"), b"x").unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(std::fs::read_dir(&external).unwrap().next().is_none());
        // Dangling symlink target -> no write, link intact.
        std::os::unix::fs::symlink(root.join("nowhere"), proj.join("docs/index.md")).unwrap();
        let err = guarded_write_new(&proj, &proj.join("docs/index.md"), b"x").unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(std::fs::symlink_metadata(proj.join("docs/index.md"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_src_mode_and_stays_create_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let src = tmp.path().join("template.sh");
        std::fs::write(&src, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dst = proj.join("docs/run.sh");
        assert!(guarded_copy_new(&proj, &src, &dst).unwrap());
        assert_eq!(
            std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o755,
            "the streamed create-new must re-apply the src mode like fs::copy did"
        );
        // Second copy: dst exists — untouched (create-only holds end to end).
        std::fs::write(&dst, "user edit").unwrap();
        assert!(!guarded_copy_new(&proj, &src, &dst).unwrap());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "user edit");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_escape_and_creates_cleanly_inside() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        assert!(assert_contained_no_symlink(&proj, &tmp.path().join("outside.md")).is_err());
        assert!(guarded_write_new(&proj, &proj.join("docs/new.md"), b"hello").unwrap());
        assert_eq!(
            std::fs::read_to_string(proj.join("docs/new.md")).unwrap(),
            "hello"
        );
        // Second write: target exists -> untouched.
        assert!(!guarded_write_new(&proj, &proj.join("docs/new.md"), b"clobber").unwrap());
        assert_eq!(
            std::fs::read_to_string(proj.join("docs/new.md")).unwrap(),
            "hello"
        );
    }

    #[cfg(unix)]
    fn project_and_external() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        (tmp, proj, external)
    }

    #[cfg(unix)]
    fn nothing_in(dir: &Path) -> bool {
        std::fs::read_dir(dir).unwrap().next().is_none()
    }

    #[cfg(unix)]
    #[test]
    fn a_temp_swapped_for_a_link_before_publish_is_refused() {
        // The temp is replaced by a link to an outside file right before the
        // publication: linkat would publish the link. The device/inode check
        // refuses it, nothing is published, the outside file stays intact.
        let (tmp, proj, _external) = project_and_external();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "OUTSIDE").unwrap();
        let docs = proj.join("docs");
        let (docs_hook, outside_hook) = (docs.clone(), outside.clone());
        let res = guarded_write_new_hooked(
            &proj,
            &docs.join("new.md"),
            b"x",
            || {},
            move || {
                let temp = std::fs::read_dir(&docs_hook)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".kronn-tmp-")
                    })
                    .expect("the temp exists");
                std::fs::remove_file(&temp).unwrap();
                std::os::unix::fs::symlink(&outside_hook, &temp).unwrap();
            },
        );
        assert!(res.unwrap_err().contains("replaced before publication"));
        assert!(std::fs::symlink_metadata(docs.join("new.md")).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "OUTSIDE");
        assert!(no_temp_left(&docs), "the swapped-in link is cleaned up too");
    }

    #[cfg(unix)]
    #[test]
    fn replace_swaps_atomically_and_creates_the_folders() {
        let (_tmp, proj, _external) = project_and_external();
        let path = proj.join("a/b/file.md");
        guarded_replace(&proj, &path, b"one").unwrap();
        guarded_replace(&proj, &path, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert!(no_temp_left(&proj.join("a/b")));
        // A planted link at the name is refused by the early walk.
        let (tmp2, proj2, external2) = project_and_external();
        std::fs::write(external2.join("target"), "KEEP").unwrap();
        std::os::unix::fs::symlink(external2.join("target"), proj2.join("docs/l.md")).unwrap();
        assert!(guarded_replace(&proj2, &proj2.join("docs/l.md"), b"x").is_err());
        assert_eq!(
            std::fs::read_to_string(external2.join("target")).unwrap(),
            "KEEP"
        );
        drop(tmp2);
    }

    #[cfg(unix)]
    #[test]
    fn replace_over_a_link_planted_at_the_last_moment_replaces_the_link() {
        let (_tmp, proj, external) = project_and_external();
        std::fs::write(external.join("target"), "KEEP").unwrap();
        let link = proj.join("docs/late.md");
        let (link_hook, target) = (link.clone(), external.join("target"));
        guarded_replace_hooked(
            &proj,
            &link,
            b"mine",
            || {},
            move || {
                std::os::unix::fs::symlink(&target, &link_hook).unwrap();
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(external.join("target")).unwrap(),
            "KEEP"
        );
        assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "mine");
    }

    #[cfg(unix)]
    #[test]
    fn replace_with_a_parent_swapped_after_the_walk_never_writes_outside() {
        for at_rename in [false, true] {
            let (_tmp, proj, external) = project_and_external();
            let docs = proj.join("docs");
            let swap = {
                let (docs, moved, external) =
                    (docs.clone(), proj.join("docs-moved"), external.clone());
                move || {
                    std::fs::rename(&docs, &moved).unwrap();
                    std::os::unix::fs::symlink(&external, &docs).unwrap();
                }
            };
            if at_rename {
                guarded_replace_hooked(&proj, &docs.join("f.md"), b"x", || {}, swap).unwrap();
            } else {
                guarded_replace_hooked(&proj, &docs.join("f.md"), b"x", swap, || {}).unwrap();
            }
            assert!(nothing_in(&external), "at_rename={at_rename}");
            assert_eq!(
                std::fs::read_to_string(proj.join("docs-moved/f.md")).unwrap(),
                "x"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn replace_if_unchanged_keeps_the_mode_and_refuses_a_concurrent_edit() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, proj, external) = project_and_external();
        let path = proj.join("docs/entry.md");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        guarded_replace_if_unchanged_keeping_mode(&proj, &path, "new", b"old").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // Edited since it was read: refused, content intact, no temp left.
        let err = guarded_replace_if_unchanged_keeping_mode(&proj, &path, "x", b"old").unwrap_err();
        assert!(err.contains("changed concurrently"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(no_temp_left(&proj.join("docs")));
        // A link, or a missing file, is refused.
        std::fs::write(external.join("t"), "KEEP").unwrap();
        std::os::unix::fs::symlink(external.join("t"), proj.join("docs/l.md")).unwrap();
        assert!(guarded_replace_if_unchanged_keeping_mode(
            &proj,
            &proj.join("docs/l.md"),
            "x",
            b"KEEP"
        )
        .is_err());
        assert!(guarded_replace_if_unchanged_keeping_mode(
            &proj,
            &proj.join("docs/none.md"),
            "x",
            b""
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(external.join("t")).unwrap(), "KEEP");
    }

    #[cfg(unix)]
    #[test]
    fn set_mode_remove_rename_and_move_stay_under_the_root() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, proj, external) = project_and_external();
        let file = proj.join("docs/a.md");
        std::fs::write(&file, "a").unwrap();
        guarded_set_mode(&proj, &file, 0o755).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o755
        );

        // A link planted as a parent: every operation refuses it.
        std::fs::write(external.join("victim"), "KEEP").unwrap();
        std::os::unix::fs::symlink(&external, proj.join("linked")).unwrap();
        let through = proj.join("linked/victim");
        assert!(guarded_set_mode(&proj, &through, 0o777).is_err());
        assert!(guarded_remove_file(&proj, &through).is_err());
        assert!(guarded_remove_tree(&proj, &proj.join("linked/sub")).is_err());
        assert!(guarded_rename(&proj, &through, &proj.join("docs/v")).is_err());
        assert!(guarded_rename(&proj, &file, &proj.join("linked/v")).is_err());
        assert!(guarded_move_new(&proj, &through, &proj.join("docs/v")).is_err());
        assert!(guarded_move_new(&proj, &file, &proj.join("linked/v")).is_err());
        assert_eq!(
            std::fs::read_to_string(external.join("victim")).unwrap(),
            "KEEP"
        );
        assert_eq!(std::fs::read_dir(&external).unwrap().count(), 1);

        // Plain use: rename (moves a link as an entry), move without
        // clobbering, then removals.
        std::os::unix::fs::symlink(external.join("victim"), proj.join("docs/ln")).unwrap();
        guarded_rename(&proj, &proj.join("docs/ln"), &proj.join("docs/ln2")).unwrap();
        assert!(std::fs::symlink_metadata(proj.join("docs/ln2"))
            .unwrap()
            .is_symlink());
        assert!(
            guarded_remove_file(&proj, &proj.join("docs/ln2")).is_err(),
            "a removal never goes through a link, even the last component"
        );
        std::fs::remove_file(proj.join("docs/ln2")).unwrap();
        assert_eq!(
            std::fs::read_to_string(external.join("victim")).unwrap(),
            "KEEP"
        );
        std::fs::write(proj.join("docs/b.md"), "b").unwrap();
        let err = guarded_move_new(&proj, &file, &proj.join("docs/b.md")).unwrap_err();
        assert!(err.contains("link"), "{err}");
        assert!(file.exists(), "a refused move keeps the source");
        guarded_move_new(&proj, &file, &proj.join("new/dir/c.md")).unwrap();
        assert!(!file.exists() && proj.join("new/dir/c.md").is_file());
        guarded_remove_file(&proj, &proj.join("new/dir/c.md")).unwrap();
        guarded_remove_dir(&proj, &proj.join("new/dir")).unwrap();
        assert!(
            guarded_remove_dir(&proj, &proj.join("docs")).is_err(),
            "not empty"
        );
        guarded_remove_tree(&proj, &proj.join("docs")).unwrap();
        assert!(!proj.join("docs").exists());
        guarded_remove_tree(&proj, &proj.join("docs")).unwrap();
        assert!(guarded_remove_file(&proj, &proj.join("gone.md")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replace_if_unchanged_with_a_parent_swapped_after_the_walk_never_writes_outside() {
        let (_tmp, proj, external) = project_and_external();
        let path = proj.join("docs/entry.md");
        std::fs::write(&path, "old").unwrap();
        std::fs::write(external.join("entry.md"), "KEEP").unwrap();
        swap_probe::arm(&proj.join("docs"), &external);
        guarded_replace_if_unchanged_keeping_mode(&proj, &path, "new", b"old").unwrap();
        assert!(swap_probe::fired(), "the swap really happened");
        assert_eq!(
            std::fs::read_to_string(external.join("entry.md")).unwrap(),
            "KEEP"
        );
        assert_eq!(
            std::fs::read_to_string(proj.join("docs.moved/entry.md")).unwrap(),
            "new"
        );
        assert!(no_temp_left(&proj.join("docs.moved")));
    }

    #[cfg(unix)]
    #[test]
    fn remove_and_set_mode_with_a_parent_swapped_after_the_walk_spare_the_outside() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, proj, external) = project_and_external();
        std::fs::write(proj.join("docs/v.md"), "inside").unwrap();
        std::fs::write(external.join("v.md"), "KEEP").unwrap();
        std::fs::set_permissions(
            external.join("v.md"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        swap_probe::arm(&proj.join("docs"), &external);
        guarded_set_mode(&proj, &proj.join("docs/v.md"), 0o777).unwrap();
        assert!(swap_probe::fired(), "the swap really happened");
        assert_eq!(
            std::fs::metadata(external.join("v.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the outside file keeps its mode"
        );
        assert_eq!(
            std::fs::metadata(proj.join("docs.moved/v.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o777
        );
        // Removal: the outside twin survives, the inside file goes.
        let (_tmp2, proj2, external2) = project_and_external();
        std::fs::write(proj2.join("docs/v.md"), "inside").unwrap();
        std::fs::write(external2.join("v.md"), "KEEP").unwrap();
        swap_probe::arm(&proj2.join("docs"), &external2);
        guarded_remove_file(&proj2, &proj2.join("docs/v.md")).unwrap();
        assert!(swap_probe::fired());
        assert_eq!(
            std::fs::read_to_string(external2.join("v.md")).unwrap(),
            "KEEP"
        );
        assert!(!proj2.join("docs.moved/v.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn move_new_with_a_parent_swapped_after_the_walk_never_writes_outside() {
        let (_tmp, proj, external) = project_and_external();
        std::fs::write(proj.join("docs/from.md"), "payload").unwrap();
        std::fs::create_dir_all(proj.join("dest")).unwrap();
        swap_probe::arm(&proj.join("dest"), &external);
        guarded_move_new(&proj, &proj.join("docs/from.md"), &proj.join("dest/to.md")).unwrap();
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(nothing_in(&external));
        assert_eq!(
            std::fs::read_to_string(proj.join("dest.moved/to.md")).unwrap(),
            "payload"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_published_entry_that_is_not_the_temp_is_reported_never_deleted() {
        // The window between the check of the temp and its publication: the
        // temp name was swapped for a link, which got published. Detected
        // right after, the write fails — and nothing is unlinked by name: the
        // entry could be a concurrent writer's legitimate file.
        let (tmp, proj, _external) = project_and_external();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "OUTSIDE").unwrap();
        let docs = Dir::open_path(&proj, Path::new("docs"), false)
            .unwrap()
            .unwrap();
        let (file, name) = docs.create_temp(0o600).unwrap();
        std::os::unix::fs::symlink(&outside, proj.join("docs/entry.md")).unwrap();
        let err = super::imp::published_is_ours(&docs, std::ffi::OsStr::new("entry.md"), &file)
            .unwrap_err();
        assert!(err.contains("replaced during publication"), "{err}");
        assert!(
            std::fs::symlink_metadata(proj.join("docs/entry.md")).is_ok(),
            "the entry at the final name is left alone"
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "OUTSIDE");
        docs.unlink_file(&name).unwrap();
        // A concurrent writer's regular file at the name is never deleted.
        std::fs::write(proj.join("docs/theirs.md"), "THEIRS").unwrap();
        let (mine, my_temp) = docs.create_temp(0o600).unwrap();
        let err = super::imp::published_is_ours(&docs, std::ffi::OsStr::new("theirs.md"), &mine)
            .unwrap_err();
        assert!(err.contains("left in place"), "{err}");
        assert_eq!(
            std::fs::read_to_string(proj.join("docs/theirs.md")).unwrap(),
            "THEIRS"
        );
        docs.unlink_file(&my_temp).unwrap();
        // A name that vanished is reported, not recreated nor "cleaned".
        let (gone, gone_temp) = docs.create_temp(0o600).unwrap();
        assert!(
            super::imp::published_is_ours(&docs, std::ffi::OsStr::new("absent.md"), &gone).is_err()
        );
        docs.unlink_file(&gone_temp).unwrap();
        // The genuine entry passes.
        std::fs::write(proj.join("docs/real.md"), "x").unwrap();
        let real = std::fs::File::open(proj.join("docs/real.md")).unwrap();
        super::imp::published_is_ours(&docs, std::ffi::OsStr::new("real.md"), &real).unwrap();
        assert!(proj.join("docs/real.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn replace_if_unchanged_refuses_a_file_replaced_or_edited_under_it() {
        use super::imp::guarded_replace_if_unchanged_hooked;
        let (_tmp, proj, _external) = project_and_external();
        let path = proj.join("docs/entry.md");
        // Replaced by another file with the very same bytes: another inode.
        std::fs::write(&path, "old").unwrap();
        let (swap_from, swap_to) = (proj.join("docs/other.md"), path.clone());
        let err = guarded_replace_if_unchanged_hooked(&proj, &path, "new", b"old", move || {
            std::fs::write(&swap_from, "old").unwrap();
            std::fs::rename(&swap_from, &swap_to).unwrap();
        })
        .unwrap_err();
        assert!(err.contains("changed concurrently"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        // Edited in place: the same inode, other bytes.
        let edited = path.clone();
        let err = guarded_replace_if_unchanged_hooked(&proj, &path, "new", b"old", move || {
            std::fs::write(&edited, "edited").unwrap();
        })
        .unwrap_err();
        assert!(err.contains("changed concurrently"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edited");
        // Removed: refused as well.
        let gone = path.clone();
        let err = guarded_replace_if_unchanged_hooked(&proj, &path, "new", b"edited", move || {
            std::fs::remove_file(&gone).unwrap();
        })
        .unwrap_err();
        assert!(err.contains("vanished"), "{err}");
        assert!(no_temp_left(&proj.join("docs")));
        // Untouched: the replacement goes through.
        std::fs::write(&path, "old").unwrap();
        guarded_replace_if_unchanged_hooked(&proj, &path, "new", b"old", || {}).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn replace_keeps_the_mode_and_creates_new_files_like_a_plain_write() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, proj, external) = project_and_external();
        let mode_of = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        // An existing file keeps its mode, whatever it is.
        let path = proj.join("docs/entry.md");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        guarded_replace(&proj, &path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(mode_of(&path), 0o640);
        // A new file gets what `fs::write` would give: not owner-only.
        let plain = proj.join("docs/plain.md");
        std::fs::write(&plain, "x").unwrap();
        let created = proj.join("deep/er/new.md");
        guarded_replace(&proj, &created, b"1").unwrap();
        assert_eq!(mode_of(&created), mode_of(&plain));
        // A link at the name is refused up front, its target neither read nor
        // changed (a link planted after that check is replaced, see the
        // forced-race tests).
        std::fs::write(external.join("t"), "KEEP").unwrap();
        std::fs::set_permissions(external.join("t"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        let linked = proj.join("docs/l.md");
        std::os::unix::fs::symlink(external.join("t"), &linked).unwrap();
        assert!(guarded_replace(&proj, &linked, b"x").is_err());
        assert_eq!(std::fs::read_to_string(external.join("t")).unwrap(), "KEEP");
        assert_eq!(mode_of(&external.join("t")), 0o600);
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn replace_like_keeps_the_mode_and_refuses_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, proj, external) = project_and_external();
        let path = proj.join("docs/entry.md");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        guarded_replace_like(&proj, &path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // A new file is created like a plain write would, folders included.
        let created = proj.join("deep/er/new.md");
        guarded_replace_like(&proj, &created, b"1").unwrap();
        assert_eq!(std::fs::read_to_string(&created).unwrap(), "1");
        assert_eq!(
            std::fs::metadata(&created).unwrap().permissions().mode() & 0o600,
            0o600
        );
        // A link at the name is refused, never written through.
        std::fs::write(external.join("t"), "KEEP").unwrap();
        let linked = proj.join("docs/l.md");
        std::os::unix::fs::symlink(external.join("t"), &linked).unwrap();
        assert!(guarded_replace_like(&proj, &linked, b"x").is_err());
        assert_eq!(std::fs::read_to_string(external.join("t")).unwrap(), "KEEP");
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn replace_like_with_a_parent_swapped_after_the_walk_never_writes_outside() {
        let (_tmp, proj, external) = project_and_external();
        let docs = proj.join("docs");
        swap_probe::arm(&docs, &external);
        guarded_replace_like(&proj, &docs.join("f.md"), b"x").unwrap();
        assert!(swap_probe::fired());
        assert!(nothing_in(&external));
        assert_eq!(
            std::fs::read_to_string(proj.join("docs.moved/f.md")).unwrap(),
            "x"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_all_with_a_parent_swapped_after_the_walk_never_creates_outside() {
        let (_tmp, proj, external) = project_and_external();
        swap_probe::arm(&proj.join("docs"), &external);
        let err = guarded_create_dir_all(&proj, &proj.join("docs/sub")).unwrap_err();
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(err.contains("mkdir"), "{err}");
        assert!(nothing_in(&external));
        assert!(!proj.join("docs.moved/sub").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_tree_with_a_parent_swapped_after_the_walk_spares_the_outside() {
        let (_tmp, proj, external) = project_and_external();
        std::fs::create_dir_all(proj.join("docs/tree/sub")).unwrap();
        std::fs::write(proj.join("docs/tree/sub/f"), "x").unwrap();
        std::fs::create_dir_all(external.join("tree/sub")).unwrap();
        std::fs::write(external.join("tree/sub/f"), "KEEP").unwrap();
        swap_probe::arm(&proj.join("docs"), &external);
        guarded_remove_tree(&proj, &proj.join("docs/tree")).unwrap();
        assert!(swap_probe::fired());
        assert_eq!(
            std::fs::read_to_string(external.join("tree/sub/f")).unwrap(),
            "KEEP"
        );
        assert!(!proj.join("docs.moved/tree").exists());
    }
    #[cfg(unix)]
    #[test]
    fn written_content_never_carries_setuid_setgid_or_sticky_bits() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, proj, _external) = project_and_external();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        let target = proj.join("docs/tool.sh");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o6755)).unwrap();
        guarded_replace(&proj, &target, b"new").unwrap();
        assert_eq!(
            mode(&target),
            0o755,
            "the plain bits stay, the special ones go"
        );
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o4750)).unwrap();
        guarded_replace_like(&proj, &target, b"newer").unwrap();
        assert_eq!(mode(&target), 0o750);
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o2755)).unwrap();
        guarded_replace_if_unchanged_keeping_mode(&proj, &target, "last", b"newer").unwrap();
        assert_eq!(mode(&target), 0o755);
        let template = proj.join("docs/template.sh");
        std::fs::write(&template, "t").unwrap();
        std::fs::set_permissions(&template, std::fs::Permissions::from_mode(0o4755)).unwrap();
        guarded_copy_new(&proj, &template, &proj.join("docs/copy.sh")).unwrap();
        assert_eq!(mode(&proj.join("docs/copy.sh")), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn remove_if_ours_spares_a_file_that_is_no_longer_the_one_we_created() {
        let (_tmp, proj, external) = project_and_external();
        let path = proj.join("docs/created.md");
        let id = guarded_write_new_identified(&proj, &path, b"mine")
            .unwrap()
            .expect("created");
        // Someone replaces it with their own file of the same size before the
        // rollback (ext4 and tmpfs would hand it the freed inode number).
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "THEM").unwrap();
        let err = guarded_remove_file_if_ours(&proj, &path, &id).unwrap_err();
        assert!(
            err.to_string().contains("no longer the file we created"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "THEM");
        // ... or with a link.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(external.join("t"), "OUT").unwrap();
        std::os::unix::fs::symlink(external.join("t"), &path).unwrap();
        assert!(guarded_remove_file_if_ours(&proj, &path, &id).is_err());
        assert!(std::fs::symlink_metadata(&path).is_ok(), "the link is left");
        assert_eq!(std::fs::read_to_string(external.join("t")).unwrap(), "OUT");
        // The genuine file goes; an occupied name reports no identity.
        std::fs::remove_file(&path).unwrap();
        let id = guarded_write_new_identified(&proj, &path, b"mine")
            .unwrap()
            .expect("created");
        assert!(guarded_write_new_identified(&proj, &path, b"again")
            .unwrap()
            .is_none());
        guarded_remove_file_if_ours(&proj, &path, &id).unwrap();
        assert!(!path.exists());
        assert_eq!(
            guarded_remove_file_if_ours(&proj, &path, &id)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn guarded_read_refuses_a_link_and_never_reads_through_a_swapped_parent() {
        let (_tmp, proj, external) = project_and_external();
        std::fs::write(proj.join("docs/a.md"), "inside").unwrap();
        assert_eq!(
            guarded_read(&proj, &proj.join("docs/a.md"))
                .unwrap()
                .as_deref(),
            Some(b"inside".as_slice())
        );
        assert!(guarded_read(&proj, &proj.join("docs/none.md"))
            .unwrap()
            .is_none());
        std::fs::write(external.join("secret"), "OUT").unwrap();
        std::os::unix::fs::symlink(external.join("secret"), proj.join("docs/l.md")).unwrap();
        assert!(guarded_read(&proj, &proj.join("docs/l.md")).is_err());
        std::fs::write(external.join("a.md"), "OUT").unwrap();
        swap_probe::arm(&proj.join("docs"), &external);
        // The folder is held as an fd before the swap: the read serves the
        // original, in-root file, never the outside one behind the new link.
        let read = guarded_read(&proj, &proj.join("docs/a.md")).unwrap();
        assert!(swap_probe::fired(), "the swap really happened");
        assert_eq!(read.as_deref(), Some(b"inside".as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn move_new_removes_the_source_only_while_it_is_still_the_file_we_opened() {
        use super::imp::guarded_move_new_hooked;
        let (_tmp, proj, _external) = project_and_external();
        let from = proj.join("docs/from.md");
        // Swapped after the link: the new entry at the source stays.
        std::fs::write(&from, "ORIGINAL").unwrap();
        let err = guarded_move_new_hooked(
            &proj,
            &from,
            &proj.join("docs/to.md"),
            || {},
            || {
                std::fs::remove_file(proj.join("docs/from.md")).unwrap();
                std::fs::write(proj.join("docs/from.md"), "REPLACEMENT").unwrap();
            },
        )
        .unwrap_err();
        assert!(err.contains("replaced during the move"), "{err}");
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "REPLACEMENT");
        assert_eq!(
            std::fs::read_to_string(proj.join("docs/to.md")).unwrap(),
            "ORIGINAL"
        );
        // Swapped before the link: the destination is not the opened file and
        // nothing is removed.
        std::fs::write(&from, "ORIGINAL2").unwrap();
        let err = guarded_move_new_hooked(
            &proj,
            &from,
            &proj.join("docs/to2.md"),
            || {
                std::fs::remove_file(proj.join("docs/from.md")).unwrap();
                std::fs::write(proj.join("docs/from.md"), "REPLACEMENT2").unwrap();
            },
            || {},
        )
        .unwrap_err();
        assert!(err.contains("replaced before the move"), "{err}");
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "REPLACEMENT2");
        // A link as the source is refused, never followed.
        let link = proj.join("docs/link.md");
        std::os::unix::fs::symlink(proj.join("docs/to.md"), &link).unwrap();
        assert!(guarded_move_new(&proj, &link, &proj.join("docs/moved.md")).is_err());
        assert!(std::fs::symlink_metadata(&link).is_ok());
        assert!(!proj.join("docs/moved.md").exists());
    }
}
