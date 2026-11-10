//! Directory file descriptors, the one rooted primitive (KT-1585, KT-1595).
//!
//! Shared by the run artifacts root (`workflows/run_artifacts`) and by
//! `core::fs_guard`, so the two security primitives cannot drift apart.
//!
//! Steps write under the root while Kronn creates and deletes there, so a
//! path checked once can be swapped for a link before it is used. Every
//! operation here goes through a directory fd: each component is opened with
//! `O_NOFOLLOW | O_DIRECTORY`, children are created with `mkdirat`, files
//! with `openat(O_CREAT | O_EXCL | O_NOFOLLOW)` then `renameat` or `linkat`,
//! and a deletion is a `unlinkat` walk that opens each subdirectory with
//! `O_NOFOLLOW` and unlinks a link as an entry. No link is ever followed.
//! Names are raw bytes end to end, and walks use an explicit stack capped at
//! [`MAX_DEPTH`]: a deeper tree is refused, never followed. Such a tree is
//! never deleted by Kronn either: the sweep warns at each start and the
//! operator removes it by hand (docs/operations/workflow-read-only-repos.md).
//! Off unix there is no such API: every operation is refused, so nothing is
//! created, deleted or moved there (`core::fs_guard` keeps its own
//! documented path-based behaviour on Windows).

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// Deepest tree a walk enters; each level holds an fd (two for a copy), so
/// a copy stays well under macOS's default limit of 256 open files.
pub const MAX_DEPTH: usize = 64;

#[cfg(unix)]
pub use unix::{Dir, Identity};

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
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Component;

    const DIR_FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    /// Who a regular file is: device, inode, inode change time and size. ext4
    /// and tmpfs reuse a freed inode number at once, so device and inode alone
    /// cannot tell a recreated file from the original; any change to the file
    /// (content, mode, links) also changes its ctime and makes it a stranger.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Identity {
        dev: u64,
        ino: u64,
        ctime: i64,
        ctime_nsec: i64,
        size: u64,
    }

    impl Identity {
        /// The identity of the file behind `file`, as of now.
        pub fn of(file: &std::fs::File) -> io::Result<Self> {
            let meta = file.metadata()?;
            Ok(Self {
                dev: meta.dev(),
                ino: meta.ino(),
                ctime: meta.ctime(),
                ctime_nsec: meta.ctime_nsec(),
                size: meta.size(),
            })
        }

        // Field widths differ between unix targets.
        #[allow(clippy::unnecessary_cast)]
        fn of_stat(stat: &libc::stat) -> Self {
            Self {
                dev: stat.st_dev as u64,
                ino: stat.st_ino as u64,
                ctime: stat.st_ctime as i64,
                ctime_nsec: stat.st_ctime_nsec as i64,
                size: stat.st_size as u64,
            }
        }
    }

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

    /// A walk stopped at `part`; keeps the errno so [`Dir::is_link_refusal`]
    /// and [`Dir::is_not_a_directory`] still classify it.
    #[derive(Debug)]
    struct WalkRefusal {
        part: OsString,
        errno: i32,
    }

    impl std::fmt::Display for WalkRefusal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "{:?} is a symlink or not a directory — refusing to write through it",
                self.part
            )
        }
    }

    impl std::error::Error for WalkRefusal {}

    /// The errno of `error`, also when a walk wrapped it in a [`WalkRefusal`].
    fn errno_of(error: &io::Error) -> Option<i32> {
        error.raw_os_error().or_else(|| {
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<WalkRefusal>())
                .map(|refusal| refusal.errno)
        })
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
        /// ones (owner-only) when `create`. `None` when a part is missing and
        /// not created.
        pub fn open_under(base: &Path, parts: &[&str], create: bool) -> io::Result<Option<Dir>> {
            Dir::open_parts(base, parts.iter().map(OsStr::new), create, 0o700)
        }

        /// [`Dir::open_under`] for a relative path (plain names and `.`
        /// only); the directories it creates get mode 0777 under the umask,
        /// like `create_dir_all`.
        pub fn open_path(base: &Path, rel: &Path, create: bool) -> io::Result<Option<Dir>> {
            let mut parts = Vec::new();
            for component in rel.components() {
                match component {
                    Component::Normal(name) => parts.push(name),
                    Component::CurDir => {}
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{} is not a plain relative path", rel.display()),
                        ))
                    }
                }
            }
            Dir::open_parts(base, parts, create, 0o777)
        }

        fn open_parts<'a>(
            base: &Path,
            parts: impl IntoIterator<Item = &'a OsStr>,
            create: bool,
            mode: libc::mode_t,
        ) -> io::Result<Option<Dir>> {
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
                    dir.make_dir(part, mode)?;
                }
                let child = dir
                    .open_child(part)
                    .map_err(|error| match error.raw_os_error() {
                        Some(errno) if not_a_directory(&error) => io::Error::new(
                            error.kind(),
                            WalkRefusal {
                                part: part.to_os_string(),
                                errno,
                            },
                        ),
                        _ => error,
                    })?;
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
            self.make_dir(name, 0o700)?;
            let child = self
                .open_child(name)?
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            check(unsafe { libc::fchmod(child.fd.as_raw_fd(), 0o700) })?;
            Ok(child)
        }

        /// `mkdirat` with `mode` (the umask only removes bits); an existing entry
        /// is left as it is and checked by the `O_NOFOLLOW` open that follows.
        fn make_dir(&self, name: &OsStr, mode: libc::mode_t) -> io::Result<()> {
            let c = c_name(name)?;
            match check(unsafe { libc::mkdirat(self.fd.as_raw_fd(), c.as_ptr(), mode) }) {
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
            let name = name.as_ref();
            let c = c_name(name)?;
            let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
            let fd = match check(unsafe { libc::openat(self.fd.as_raw_fd(), c.as_ptr(), flags) }) {
                Ok(fd) => fd,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name:?} is not a regular file"),
                ));
            }
            Ok(Some(file))
        }

        /// Whether `error` is the refusal of a link met on a path walked with
        /// `O_NOFOLLOW` (ELOOP, or EMLINK on some systems).
        pub fn is_link_refusal(error: &io::Error) -> bool {
            matches!(errno_of(error), Some(libc::ELOOP) | Some(libc::EMLINK))
        }

        /// Whether `error` says a part of the path is not a directory (a file
        /// in the middle of `a.png/x`), which is not a link.
        pub fn is_not_a_directory(error: &io::Error) -> bool {
            errno_of(error) == Some(libc::ENOTDIR)
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

        /// Whether `name` exists here; a link counts (dangling included) and is
        /// never followed.
        pub fn exists(&self, name: &OsStr) -> io::Result<bool> {
            match self.stat(name) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error),
            }
        }

        /// `fstatat(AT_SYMLINK_NOFOLLOW)` of `name`.
        fn stat(&self, name: &OsStr) -> io::Result<libc::stat> {
            let c = c_name(name)?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            check(unsafe {
                libc::fstatat(
                    self.fd.as_raw_fd(),
                    c.as_ptr(),
                    stat.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            })?;
            Ok(unsafe { stat.assume_init() })
        }

        /// A fresh file `name` (`O_CREAT | O_EXCL | O_NOFOLLOW`) with `mode`
        /// under the umask; anything already there, a link included, is
        /// `AlreadyExists`.
        pub fn create_excl(&self, name: &OsStr, mode: u32) -> io::Result<std::fs::File> {
            let c = c_name(name)?;
            let write =
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let fd = check(unsafe {
                libc::openat(self.fd.as_raw_fd(), c.as_ptr(), write, mode as libc::c_uint)
            })?;
            Ok(std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        /// A fresh file with `mode` under a random `.kronn-tmp-*` name, short
        /// whatever its destination (a target near NAME_MAX stays writable).
        /// A collision picks another name and never touches the existing one.
        pub fn create_temp(&self, mode: u32) -> io::Result<(std::fs::File, OsString)> {
            for _ in 0..8 {
                let name = OsString::from(format!(".kronn-tmp-{}", uuid::Uuid::new_v4()));
                match self.create_excl(&name, mode) {
                    Ok(file) => return Ok((file, name)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "no free temporary name",
            ))
        }

        /// Whether the entry `name` is, right now, the regular file behind
        /// `file` (same device and inode): a name swapped for a link or for
        /// another file since `file` was opened is `false`. The open handle
        /// keeps its inode allocated, so no other file can reuse the number.
        pub fn names_file(&self, name: &OsStr, file: &std::fs::File) -> io::Result<bool> {
            let meta = file.metadata()?;
            let stat = self.stat(name)?;
            #[allow(clippy::unnecessary_cast)]
            Ok(stat.st_mode & libc::S_IFMT == libc::S_IFREG
                && stat.st_dev as u64 == meta.dev()
                && stat.st_ino as u64 == meta.ino())
        }

        /// Whether the entry `name` is, right now, a regular file with exactly
        /// this identity (a link or another file is `false`; a missing name is
        /// a `NotFound` error).
        pub fn names_identity(&self, name: &OsStr, id: &Identity) -> io::Result<bool> {
            let stat = self.stat(name)?;
            Ok(stat.st_mode & libc::S_IFMT == libc::S_IFREG && Identity::of_stat(&stat) == *id)
        }

        /// The permission bits (`0o777` only: setuid, setgid and sticky never
        /// travel to new content) of `name` when it is a regular file here;
        /// `None` for a missing name, a link or anything else (a link is never
        /// followed, so its target's mode is never read).
        // st_mode is u16 on macOS and u32 on Linux: the conversion is needed on one.
        #[allow(clippy::useless_conversion)]
        pub fn regular_mode(&self, name: &OsStr) -> io::Result<Option<u32>> {
            match self.stat(name) {
                Ok(stat) if stat.st_mode & libc::S_IFMT == libc::S_IFREG => {
                    Ok(Some(u32::from(stat.st_mode & 0o777)))
                }
                Ok(_) => Ok(None),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }

        /// `chmod` of the regular file `name`, through its handle: a link is
        /// refused, never followed.
        pub fn set_mode(&self, name: &OsStr, mode: u32) -> io::Result<()> {
            let file = self
                .open_regular(name)?
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            file.set_permissions(std::fs::Permissions::from_mode(mode))
        }

        /// Hard-links `from` to `to` in this directory: `AlreadyExists` when
        /// `to` is anything, a link included, and it is never followed.
        pub fn link_new(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
            self.link_from(self, from, to)
        }

        /// Hard-links `from_dir/from` to `self/to`, no-clobber like
        /// [`Dir::link_new`]; neither name is re-resolved from a path.
        pub fn link_from(&self, from_dir: &Dir, from: &OsStr, to: &OsStr) -> io::Result<()> {
            let (from, to) = (c_name(from)?, c_name(to)?);
            check(unsafe {
                libc::linkat(
                    from_dir.fd.as_raw_fd(),
                    from.as_ptr(),
                    self.fd.as_raw_fd(),
                    to.as_ptr(),
                    0,
                )
            })
            .map(|_| ())
        }

        /// `unlinkat` of the file or link `name`; missing is `NotFound`.
        pub fn unlink_file(&self, name: &OsStr) -> io::Result<()> {
            let c = c_name(name)?;
            check(unsafe { libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), 0) }).map(|_| ())
        }

        /// `unlinkat(AT_REMOVEDIR)` of the empty directory `name`.
        pub fn remove_empty_dir(&self, name: &OsStr) -> io::Result<()> {
            let c = c_name(name)?;
            check(unsafe { libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), libc::AT_REMOVEDIR) })
                .map(|_| ())
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
