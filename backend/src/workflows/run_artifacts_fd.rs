//! KT-1585 — the run artifacts root, held as a directory file descriptor.
//!
//! Steps write under the root while Kronn creates and deletes there, so a
//! path checked once can be swapped for a link before it is used. Every
//! operation here goes through a directory fd: each component is opened with
//! `O_NOFOLLOW | O_DIRECTORY`, children are created with `mkdirat`, files
//! with `openat(O_CREAT | O_EXCL | O_NOFOLLOW)` then `renameat`, and a
//! deletion is a `unlinkat` walk that opens each subdirectory with
//! `O_NOFOLLOW` and unlinks a link as an entry. No link is ever followed.
//! Names are raw bytes end to end, and walks use an explicit stack capped at
//! [`MAX_DEPTH`]: a deeper tree is refused, never followed. Such a tree is
//! never deleted by Kronn either: the sweep warns at each start and the
//! operator removes it by hand (docs/operations/workflow-read-only-repos.md).
//! Off unix there is no such API: every operation is refused, so nothing is
//! created, deleted or moved there.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// Deepest tree a walk enters; each level holds an fd (two for a copy), so
/// a copy stays well under macOS's default limit of 256 open files.
pub const MAX_DEPTH: usize = 64;

#[cfg(unix)]
pub use unix::Dir;

#[cfg(not(unix))]
pub use fallback::Dir;

fn too_deep() -> io::Error {
    io::Error::other(format!(
        "deeper than {MAX_DEPTH} levels: refused, nothing below is followed"
    ))
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    const DIR_FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    /// An open directory; every name below it is resolved against the fd.
    pub struct Dir {
        fd: OwnedFd,
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a single path component: {name:?}"),
            ));
        }
        CString::new(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    fn check(result: libc::c_int) -> io::Result<libc::c_int> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    fn missing_ok(result: io::Result<libc::c_int>) -> io::Result<()> {
        match result {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Whether an `openat(O_NOFOLLOW | O_DIRECTORY)` failure means "not a
    /// directory": a link (ELOOP, or ENOTDIR/EMLINK on some systems) or a file.
    fn not_a_directory(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(libc::ELOOP) | Some(libc::ENOTDIR) | Some(libc::EMLINK)
        )
    }

    /// `readdir` only tells an error from the end through errno; the `errno`
    /// crate knows its location on every unix target.
    fn clear_errno() {
        errno::set_errno(errno::Errno(0));
    }

    #[cfg(test)]
    type RemoveHook = Option<Box<dyn FnMut(&OsStr)>>;

    #[cfg(test)]
    thread_local! {
        /// Runs after a directory's entries are listed and before they are
        /// removed: a test swaps an entry for a link there.
        pub static BEFORE_REMOVE: std::cell::RefCell<RemoveHook> =
            const { std::cell::RefCell::new(None) };
    }

    fn listed_hook(_name: &OsStr) {
        #[cfg(test)]
        BEFORE_REMOVE.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook(_name);
            }
        });
    }

    impl Dir {
        fn wrap(fd: libc::c_int) -> Dir {
            Dir {
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
            }
        }

        /// Opens `base` (a link there is the operator's choice), then each of
        /// `parts` below it without following a link, creating the missing
        /// ones when `create`. `None` when a part is missing and not created.
        pub fn open_under(base: &Path, parts: &[&str], create: bool) -> io::Result<Option<Dir>> {
            if create {
                std::fs::create_dir_all(base)?;
            }
            let path = CString::new(base.as_os_str().as_bytes())
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
            let mut dir = match check(unsafe { libc::open(path.as_ptr(), flags) }) {
                Ok(fd) => Dir::wrap(fd),
                Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                    return Ok(None)
                }
                Err(error) => return Err(error),
            };
            for part in parts {
                if create {
                    dir.make_dir(OsStr::new(part))?;
                }
                let child = dir.open_child(part)?;
                match child {
                    Some(child) => dir = child,
                    None => return Ok(None),
                }
            }
            Ok(Some(dir))
        }

        /// The child directory `name`, never through a link; `None` if missing.
        pub fn open_child(&self, name: impl AsRef<OsStr>) -> io::Result<Option<Dir>> {
            let name = c_name(name.as_ref())?;
            match check(unsafe { libc::openat(self.fd.as_raw_fd(), name.as_ptr(), DIR_FLAGS) }) {
                Ok(fd) => Ok(Some(Dir::wrap(fd))),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                // `O_NOFOLLOW | O_DIRECTORY` on a link says ENOTDIR on some
                // systems and ELOOP on others: look at the entry itself, so a
                // link is always ELOOP and ENOTDIR only means a file.
                Err(error) if error.raw_os_error() == Some(libc::ENOTDIR) => {
                    Err(if self.is_symlink(&name) {
                        io::Error::from_raw_os_error(libc::ELOOP)
                    } else {
                        error
                    })
                }
                Err(error) => Err(error),
            }
        }

        /// Whether the entry `name` is itself a link (`fstatat`, not followed).
        fn is_symlink(&self, name: &CStr) -> bool {
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            let seen = unsafe {
                libc::fstatat(
                    self.fd.as_raw_fd(),
                    name.as_ptr(),
                    &mut stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            seen == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
        }

        /// Creates (or opens) the owner-only child directory `name`; a link
        /// planted there is refused, never adopted.
        pub fn create_child(&self, name: impl AsRef<OsStr>) -> io::Result<Dir> {
            let name = name.as_ref();
            self.make_dir(name)?;
            let child = self
                .open_child(name)?
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            check(unsafe { libc::fchmod(child.fd.as_raw_fd(), 0o700) })?;
            Ok(child)
        }

        /// `mkdirat` owner-only (umask only removes bits); an existing entry
        /// is left as it is and checked by the `O_NOFOLLOW` open that follows.
        fn make_dir(&self, name: &OsStr) -> io::Result<()> {
            let c = c_name(name)?;
            match check(unsafe { libc::mkdirat(self.fd.as_raw_fd(), c.as_ptr(), 0o700) }) {
                Ok(_) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        }

        /// The raw names of the entries, `.` and `..` left out. A read error
        /// is an error, never taken for the end of the listing.
        pub fn names(&self) -> io::Result<Vec<OsString>> {
            let dup = check(unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
            let stream = unsafe { libc::fdopendir(dup) };
            if stream.is_null() {
                let error = io::Error::last_os_error();
                unsafe { libc::close(dup) };
                return Err(error);
            }
            unsafe { libc::rewinddir(stream) };
            let mut names = Vec::new();
            let result = loop {
                clear_errno();
                let entry = unsafe { libc::readdir(stream) };
                if entry.is_null() {
                    let error = io::Error::last_os_error();
                    break match error.raw_os_error() {
                        Some(0) | None => Ok(()),
                        Some(_) => Err(error),
                    };
                }
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name != b"." && name != b".." {
                    names.push(OsString::from_vec(name.to_vec()));
                }
            };
            unsafe { libc::closedir(stream) };
            result.map(|()| names)
        }

        fn unlink(&self, name: &OsStr, flags: libc::c_int) -> io::Result<()> {
            let c = c_name(name)?;
            missing_ok(check(unsafe {
                libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), flags)
            }))
        }

        /// Removes the entry `name`: a directory with everything below it,
        /// anything else (a link included) as one entry. Missing is fine.
        /// An explicit stack, one fd per level, closed on the way up.
        pub fn remove_child(&self, name: impl AsRef<OsStr>) -> io::Result<()> {
            let name = name.as_ref();
            let top = match self.open_child(name) {
                Ok(Some(dir)) => dir,
                Ok(None) => return Ok(()),
                Err(error) if not_a_directory(&error) => return self.unlink(name, 0),
                Err(error) => return Err(error),
            };
            let pending = top.names()?;
            listed_hook(name);
            let mut stack: Vec<(Dir, OsString, Vec<OsString>)> =
                vec![(top, name.to_os_string(), pending)];
            while let Some((dir, _, pending)) = stack.last_mut() {
                if let Some(entry) = pending.pop() {
                    match dir.open_child(&entry) {
                        Ok(Some(sub)) => {
                            if stack.len() >= MAX_DEPTH {
                                return Err(too_deep());
                            }
                            let below = sub.names()?;
                            listed_hook(&entry);
                            stack.push((sub, entry, below));
                        }
                        Ok(None) => {}
                        Err(error) if not_a_directory(&error) => dir.unlink(&entry, 0)?,
                        Err(error) => return Err(error),
                    }
                    continue;
                }
                let (done, done_name, _) = stack.pop().expect("a last entry");
                drop(done);
                let parent = stack.last().map_or(self, |(dir, _, _)| dir);
                parent.unlink(&done_name, libc::AT_REMOVEDIR)?;
            }
            Ok(())
        }

        /// `renameat` of `from/name` to `self/to`, on one file system only.
        pub fn rename_from(
            &self,
            from: &Dir,
            name: impl AsRef<OsStr>,
            to: impl AsRef<OsStr>,
        ) -> io::Result<()> {
            let (name, to) = (c_name(name.as_ref())?, c_name(to.as_ref())?);
            check(unsafe {
                libc::renameat(
                    from.fd.as_raw_fd(),
                    name.as_ptr(),
                    self.fd.as_raw_fd(),
                    to.as_ptr(),
                )
            })
            .map(|_| ())
        }

        /// Copies the directory `from/name` into the new directory `self/to`:
        /// directories and regular files only, a link never followed. An
        /// explicit stack of (source, target) fds, capped like a removal.
        pub fn copy_from(
            &self,
            from: &Dir,
            name: impl AsRef<OsStr>,
            to: impl AsRef<OsStr>,
        ) -> io::Result<()> {
            let source = from
                .open_child(name)?
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            let target = self.make_new_dir(to.as_ref())?;
            let pending = source.names()?;
            let mut stack: Vec<(Dir, Dir, Vec<OsString>)> = vec![(source, target, pending)];
            let depth_left = |stack: &Vec<_>| stack.len() < MAX_DEPTH;
            loop {
                let can_descend = depth_left(&stack);
                let Some((source, target, pending)) = stack.last_mut() else {
                    break;
                };
                let Some(entry) = pending.pop() else {
                    stack.pop();
                    continue;
                };
                let next = match source.open_child(&entry) {
                    Ok(Some(sub)) if !can_descend => {
                        drop(sub);
                        return Err(too_deep());
                    }
                    Ok(Some(sub)) => {
                        let made = target.make_new_dir(&entry)?;
                        let below = sub.names()?;
                        Some((sub, made, below))
                    }
                    Ok(None) => None,
                    Err(error) if not_a_directory(&error) => {
                        target.copy_file_from(source, &entry)?;
                        None
                    }
                    Err(error) => return Err(error),
                };
                if let Some(level) = next {
                    stack.push(level);
                }
            }
            Ok(())
        }

        /// A directory that must not exist yet: an existing one, maybe a step's,
        /// is `AlreadyExists`, never adopted.
        pub fn create_new_child(&self, name: impl AsRef<OsStr>) -> io::Result<Dir> {
            self.make_new_dir(name.as_ref())
        }

        fn make_new_dir(&self, name: &OsStr) -> io::Result<Dir> {
            let c = c_name(name)?;
            check(unsafe { libc::mkdirat(self.fd.as_raw_fd(), c.as_ptr(), 0o700) })?;
            self.open_child(name)?
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        /// A regular file only: a link fails `O_NOFOLLOW` and is skipped.
        fn copy_file_from(&self, from: &Dir, name: &OsStr) -> io::Result<()> {
            let c = c_name(name)?;
            let read = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
            let source = match check(unsafe { libc::openat(from.fd.as_raw_fd(), c.as_ptr(), read) })
            {
                Ok(fd) => std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) }),
                Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(()),
                Err(error) => return Err(error),
            };
            if !source.metadata()?.is_file() {
                return Ok(());
            }
            let mut target = self.create_file(&c)?;
            let mut source = source;
            io::copy(&mut source, &mut target).map(|_| ())
        }

        /// The regular file `name` of this directory, opened for reading and
        /// never through a link. `O_NONBLOCK` keeps a FIFO (or a device) from
        /// holding the open forever; the `fstat` that follows refuses it.
        /// `None` when there is no such entry.
        pub fn open_regular(&self, name: impl AsRef<OsStr>) -> io::Result<Option<std::fs::File>> {
            let name = c_name(name.as_ref())?;
            let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
            let fd = match check(unsafe { libc::openat(self.fd.as_raw_fd(), name.as_ptr(), flags) })
            {
                Ok(fd) => fd,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "not a regular file",
                ));
            }
            Ok(Some(file))
        }

        /// Whether `error` is the refusal of a link met on a path walked with
        /// `O_NOFOLLOW` (ELOOP, or EMLINK on some systems).
        pub fn is_link_refusal(error: &io::Error) -> bool {
            matches!(error.raw_os_error(), Some(libc::ELOOP) | Some(libc::EMLINK))
        }

        /// Whether `error` says a part of the path is not a directory (a file
        /// in the middle of `a.png/x`), which is not a link.
        pub fn is_not_a_directory(error: &io::Error) -> bool {
            error.raw_os_error() == Some(libc::ENOTDIR)
        }

        /// `openat(O_CREAT | O_EXCL | O_NOFOLLOW)`, owner-only.
        fn create_file(&self, name: &CStr) -> io::Result<std::fs::File> {
            let write =
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let fd = check(unsafe {
                libc::openat(
                    self.fd.as_raw_fd(),
                    name.as_ptr(),
                    write,
                    0o600 as libc::c_uint,
                )
            })?;
            Ok(std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        /// Writes `name` whole: a new temporary file renamed over it, so a
        /// link planted at `name` is replaced, never written through.
        pub fn write_file(&self, name: impl AsRef<OsStr>, bytes: &[u8]) -> io::Result<()> {
            self.write_from(name, &mut &*bytes)
        }

        /// [`Self::write_file`] for a stream, so a large file is never held
        /// whole in memory: the same new temporary file renamed over `name`.
        pub fn write_from(
            &self,
            name: impl AsRef<OsStr>,
            source: &mut dyn io::Read,
        ) -> io::Result<()> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let name = name.as_ref();
            c_name(name)?;
            let temp = OsString::from(format!(
                ".kronn-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let written = self
                .create_file(&c_name(&temp)?)
                .and_then(|mut file| io::copy(source, &mut file).map(|_| ()))
                .and_then(|()| self.rename_from(self, &temp, name));
            if written.is_err() {
                let _ = self.unlink(&temp, 0);
            }
            written
        }

        /// The path the kernel holds for this directory.
        pub fn path(&self) -> io::Result<PathBuf> {
            #[cfg(target_os = "linux")]
            {
                std::fs::read_link(format!("/proc/self/fd/{}", self.fd.as_raw_fd()))
            }
            #[cfg(target_os = "macos")]
            {
                let mut buffer = vec![0u8; libc::PATH_MAX as usize];
                check(unsafe {
                    libc::fcntl(
                        self.fd.as_raw_fd(),
                        libc::F_GETPATH,
                        buffer.as_mut_ptr() as *mut libc::c_char,
                    )
                })?;
                let end = buffer
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(buffer.len());
                buffer.truncate(end);
                Ok(PathBuf::from(OsString::from_vec(buffer)))
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                Err(io::Error::from(io::ErrorKind::Unsupported))
            }
        }

        /// `(device, inode)` of this directory and of each ancestor up to `/`.
        fn lineage(&self) -> io::Result<Vec<(u64, u64)>> {
            let mut ids = Vec::new();
            let mut current = self.fd.try_clone()?;
            for _ in 0..512 {
                let id = identity(&current)?;
                if ids.last() == Some(&id) {
                    return Ok(ids);
                }
                ids.push(id);
                let parent =
                    check(unsafe { libc::openat(current.as_raw_fd(), c"..".as_ptr(), DIR_FLAGS) })?;
                current = unsafe { OwnedFd::from_raw_fd(parent) };
            }
            Ok(ids)
        }

        /// Refuses a root inside a protected directory or holding one, found
        /// by device and inode, so no path is trusted here. A protected
        /// directory that exists but cannot be opened refuses the root too.
        pub fn check_outside(&self, protected: &[PathBuf]) -> io::Result<()> {
            let own = self.lineage()?;
            for dir in protected {
                let closed = match Dir::open_under(dir, &[], false) {
                    Ok(Some(closed)) => closed,
                    Ok(None) => continue,
                    Err(error) => {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!("cannot check {} ({error}): refused", dir.display()),
                        ))
                    }
                };
                let closed_lineage = closed.lineage()?;
                let closed_id = closed_lineage[0];
                if own.contains(&closed_id) || closed_lineage.contains(&own[0]) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("overlaps {}, which agents cannot read", dir.display()),
                    ));
                }
            }
            Ok(())
        }
    }

    fn identity(fd: &OwnedFd) -> io::Result<(u64, u64)> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        check(unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) })?;
        let stat = unsafe { stat.assume_init() };
        #[allow(clippy::unnecessary_cast)]
        Ok((stat.st_dev as u64, stat.st_ino as u64))
    }
}

#[cfg(not(unix))]
mod fallback {
    use super::*;

    /// No directory fds here: every operation is refused.
    pub struct Dir;

    fn refused() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "run artifacts are only created and removed through directory fds (unix)",
        )
    }

    impl Dir {
        pub fn open_under(_: &Path, _: &[&str], _: bool) -> io::Result<Option<Dir>> {
            Err(refused())
        }
        pub fn open_child(&self, _: impl AsRef<OsStr>) -> io::Result<Option<Dir>> {
            Err(refused())
        }
        pub fn create_child(&self, _: impl AsRef<OsStr>) -> io::Result<Dir> {
            Err(refused())
        }
        pub fn create_new_child(&self, _: impl AsRef<OsStr>) -> io::Result<Dir> {
            Err(refused())
        }
        pub fn names(&self) -> io::Result<Vec<OsString>> {
            Err(refused())
        }
        pub fn remove_child(&self, _: impl AsRef<OsStr>) -> io::Result<()> {
            Err(refused())
        }
        pub fn rename_from(
            &self,
            _: &Dir,
            _: impl AsRef<OsStr>,
            _: impl AsRef<OsStr>,
        ) -> io::Result<()> {
            Err(refused())
        }
        pub fn copy_from(
            &self,
            _: &Dir,
            _: impl AsRef<OsStr>,
            _: impl AsRef<OsStr>,
        ) -> io::Result<()> {
            Err(refused())
        }
        pub fn open_regular(&self, _: impl AsRef<OsStr>) -> io::Result<Option<std::fs::File>> {
            Err(refused())
        }
        pub fn is_link_refusal(_: &io::Error) -> bool {
            false
        }
        pub fn is_not_a_directory(_: &io::Error) -> bool {
            false
        }
        pub fn write_file(&self, _: impl AsRef<OsStr>, _: &[u8]) -> io::Result<()> {
            Err(refused())
        }
        pub fn write_from(&self, _: impl AsRef<OsStr>, _: &mut dyn io::Read) -> io::Result<()> {
            Err(refused())
        }
        pub fn path(&self) -> io::Result<PathBuf> {
            Err(refused())
        }
        pub fn check_outside(&self, _: &[PathBuf]) -> io::Result<()> {
            Err(refused())
        }
    }
}

#[cfg(all(test, unix, feature = "full-tests"))] // exercises the unix-only fd-relative copy (run_artifacts_fd::unix, openat) with symlinks and raw-byte names
mod tests {
    use super::unix::BEFORE_REMOVE;
    use super::*;

    /// A data-dir-like tree: what must survive every test.
    fn vault() -> (tempfile::TempDir, PathBuf) {
        let vault = tempfile::tempdir().unwrap();
        let evidence = vault.path().join("task-evidence");
        std::fs::create_dir_all(evidence.join("a/b")).unwrap();
        std::fs::write(evidence.join("a/b/key"), "secret").unwrap();
        std::fs::write(vault.path().join("encryption_key"), "k").unwrap();
        (vault, evidence)
    }

    fn intact(vault: &Path) {
        assert_eq!(
            std::fs::read_to_string(vault.join("task-evidence/a/b/key")).unwrap(),
            "secret"
        );
        assert!(vault.join("encryption_key").is_file());
    }

    fn root(base: &Path) -> Dir {
        Dir::open_under(base, &["own", "key"], true)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn a_tree_with_links_inside_is_removed_without_following_them() {
        let (vault, evidence) = vault();
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        let run = root.create_child("run").unwrap();
        run.create_child("sub").unwrap();
        let run_path = base.path().join("own/key/run");
        std::fs::write(run_path.join("sub/file"), "x").unwrap();
        std::os::unix::fs::symlink(&evidence, run_path.join("link")).unwrap();
        std::os::unix::fs::symlink(vault.path(), run_path.join("sub/link")).unwrap();
        root.remove_child("run").unwrap();
        assert!(!run_path.exists());
        intact(vault.path());
    }

    /// The swap the sweep fears: between listing and deleting, the run dir
    /// becomes a link into the vault. Only the link goes.
    #[test]
    fn a_run_dir_swapped_for_a_link_after_listing_only_loses_the_link() {
        let (vault, evidence) = vault();
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        root.create_child("run").unwrap();
        assert_eq!(root.names().unwrap(), vec![OsString::from("run")]);
        let run_path = base.path().join("own/key/run");
        std::fs::rename(&run_path, base.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&evidence, &run_path).unwrap();
        root.remove_child("run").unwrap();
        assert!(std::fs::symlink_metadata(&run_path).is_err());
        intact(vault.path());
    }

    /// The same swap inside the walk, injected after a directory's entries
    /// are listed and before they are removed.
    #[test]
    fn a_subdir_swapped_for_a_link_mid_walk_is_unlinked_not_followed() {
        let (vault, evidence) = vault();
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        let run = root.create_child("run").unwrap();
        run.create_child("sub").unwrap();
        let sub = base.path().join("own/key/run/sub");
        let parked = base.path().join("parked");
        let target = evidence.clone();
        BEFORE_REMOVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |at: &OsStr| {
                if at == "run" {
                    std::fs::rename(&sub, &parked).unwrap();
                    std::os::unix::fs::symlink(&target, &sub).unwrap();
                }
            }));
        });
        let removed = root.remove_child("run");
        BEFORE_REMOVE.with(|hook| hook.borrow_mut().take());
        removed.unwrap();
        intact(vault.path());
    }

    #[test]
    fn a_linked_component_is_never_opened() {
        let (vault, _) = vault();
        let base = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(vault.path(), base.path().join("own")).unwrap();
        assert!(Dir::open_under(base.path(), &["own", "key"], true).is_err());
        assert!(Dir::open_under(base.path(), &["own", "key"], false).is_err());
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("own")).unwrap();
        std::os::unix::fs::symlink(vault.path(), other.path().join("own/key")).unwrap();
        assert!(Dir::open_under(other.path(), &["own", "key"], true).is_err());
        assert!(!vault.path().join("key").exists());
        intact(vault.path());
    }

    #[test]
    fn a_planted_link_is_never_adopted_as_a_run_dir() {
        let (vault, evidence) = vault();
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        std::os::unix::fs::symlink(&evidence, base.path().join("own/key/run")).unwrap();
        assert!(root.create_child("run").is_err());
        intact(vault.path());
    }

    #[test]
    fn a_root_inside_or_around_a_protected_dir_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let inside = Dir::open_under(&data, &["own", "key"], true)
            .unwrap()
            .unwrap();
        assert!(inside.check_outside(std::slice::from_ref(&data)).is_err());
        let around = Dir::open_under(home.path(), &[], false).unwrap().unwrap();
        assert!(around.check_outside(std::slice::from_ref(&data)).is_err());
        let elsewhere = tempfile::tempdir().unwrap();
        let fine = root(elsewhere.path());
        fine.check_outside(std::slice::from_ref(&data)).unwrap();
        // A protected dir that exists but cannot be opened refuses the root.
        let file = home.path().join("not-a-dir");
        std::fs::write(&file, "x").unwrap();
        assert!(fine.check_outside(&[file]).is_err());
        fine.check_outside(&[home.path().join("absent")]).unwrap();
    }

    #[test]
    fn a_copy_takes_dirs_and_files_and_skips_links() {
        let (vault, evidence) = vault();
        let from_base = tempfile::tempdir().unwrap();
        let from = Dir::open_under(from_base.path(), &[], false)
            .unwrap()
            .unwrap();
        let run = from.create_child("run").unwrap();
        run.create_child("nested").unwrap();
        std::fs::write(from_base.path().join("run/nested/file"), "data").unwrap();
        std::os::unix::fs::symlink(&evidence, from_base.path().join("run/link")).unwrap();
        std::os::unix::fs::symlink(
            vault.path().join("encryption_key"),
            from_base.path().join("run/key"),
        )
        .unwrap();
        let to_base = tempfile::tempdir().unwrap();
        let to = root(to_base.path());
        to.copy_from(&from, "run", ".tmp-run").unwrap();
        to.rename_from(&to, ".tmp-run", "run").unwrap();
        let copied = to_base.path().join("own/key/run");
        assert_eq!(
            std::fs::read_to_string(copied.join("nested/file")).unwrap(),
            "data"
        );
        assert!(std::fs::symlink_metadata(copied.join("link")).is_err());
        assert!(std::fs::symlink_metadata(copied.join("key")).is_err());
        intact(vault.path());
    }

    /// A name that is not UTF-8 is carried as bytes: listed, copied and
    /// removed, never mistaken for an absent entry. APFS refuses such names.
    #[cfg(target_os = "linux")] // creates a non-UTF-8 name from raw bytes with OsStrExt, and symlinks
    #[test]
    fn a_non_utf8_name_is_copied_and_removed() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"bad-\xff-name");
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        let run = root.create_child("run").unwrap();
        run.write_file(raw, b"payload").unwrap();
        assert!(run.create_child(raw).is_err());
        let sub = run.create_child(OsStr::from_bytes(b"dir-\xfe")).unwrap();
        sub.write_file("f", b"x").unwrap();
        root.copy_from(&root, "run", "copy").unwrap();
        let copied = base.path().join("own/key/copy");
        assert_eq!(std::fs::read(copied.join(raw)).unwrap(), b"payload");
        root.remove_child("run").unwrap();
        root.remove_child("copy").unwrap();
        assert!(root.names().unwrap().is_empty());
    }

    /// A tree deeper than the cap is refused (logged by the caller), never
    /// walked to the bottom; a tree at the cap goes.
    #[test]
    fn a_tree_deeper_than_the_cap_is_refused_not_followed() {
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        let build = |name: &str, depth: usize| {
            let mut dir = root.create_child(name).unwrap();
            for _ in 1..depth {
                dir = dir.create_child("d").unwrap();
            }
        };
        build("deep", MAX_DEPTH + 5);
        let error = root.remove_child("deep").unwrap_err();
        assert!(error.to_string().contains("deeper than"), "{error}");
        assert!(root.open_child("deep").unwrap().is_some());
        build("fits", MAX_DEPTH);
        root.remove_child("fits").unwrap();
        assert!(root.open_child("fits").unwrap().is_none());
        let deep_copy = root.copy_from(&root, "deep", "deep-copy");
        assert!(deep_copy.is_err());
    }

    #[test]
    fn a_file_written_over_a_planted_link_replaces_the_link() {
        let (vault, _) = vault();
        let base = tempfile::tempdir().unwrap();
        let root = root(base.path());
        let link = base.path().join("own/key/out.txt");
        std::os::unix::fs::symlink(vault.path().join("encryption_key"), &link).unwrap();
        root.write_file("out.txt", b"full").unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.path().join("encryption_key")).unwrap(),
            "k"
        );
        assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "full");
        intact(vault.path());
    }

    #[test]
    fn the_path_comes_from_the_fd() {
        let base = tempfile::tempdir().unwrap();
        let dir = root(base.path()).create_child("run").unwrap();
        let path = dir.path().unwrap();
        assert_eq!(
            path,
            base.path().join("own/key/run").canonicalize().unwrap()
        );
    }
}
