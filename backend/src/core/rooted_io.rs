//! Worktree-rooted file reads and writes for the SubWorkflow foreach engine.
//!
//! On Unix every component below the root is opened with `openat` and
//! `O_NOFOLLOW` from the previous directory handle, so a symlink planted (or
//! swapped in) anywhere on the path makes the call fail instead of leaving the
//! worktree. The root itself is trusted (a path Kronn chose) and may be reached
//! through symlinks.
//!
//! - A read must land on a regular file with a single link, and the name must
//!   still point at that same file after it was opened: a hard link (or one
//!   removed right after the open) would reach an outside file.
//! - A write never opens the existing file. It writes a fresh file under a
//!   short random name (owner-only, then the replaced file's permission
//!   bits), syncs it and renames it over the name, which replaces a planted
//!   link instead of writing through it; the directory is then synced. Only a
//!   regular file or a link is replaced: sockets, FIFOs, devices and
//!   directories are refused.
//!
//! Windows: the same replace-by-rename write and a no-follow, single-link
//! read through the opened handle, but the path is walked by name after an
//! `fs_guard` check, so a directory swapped in between check and open is not
//! excluded, and directories cannot be synced (best effort).
//!
//! Known residual (KT-1055, 0.15): a read can still be fooled by a process
//! that mutates the worktree namespace while it runs (a hard link unlinked
//! and relinked between the checks on Unix, deletion of a hard link while
//! the file is open on Windows). Closing it needs OS isolation or protected
//! staging, i.e. the rooted primitive planned in KT-1055.

use std::io;
use std::path::{Component, Path};

/// Normal components of `rel`; anything that could leave the root is refused.
fn components(rel: &Path) -> io::Result<Vec<&std::ffi::OsStr>> {
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
    if parts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty relative path",
        ));
    }
    Ok(parts)
}

fn refused(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_string())
}

#[cfg(windows)]
fn refused_owned(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what)
}

/// Attempts at an exclusive temporary name before giving up.
const TEMP_ATTEMPTS: usize = 8;

/// A short random sibling name, independent of the destination's (a name
/// near the length limit stays writable). Created exclusively, retried on a
/// collision.
fn temp_name() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!(".kronn-tmp-{}", &id[..16])
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn cstring(bytes: &[u8]) -> io::Result<CString> {
        CString::new(bytes).map_err(|_| super::refused("NUL in path"))
    }

    fn check(fd: libc::c_int) -> io::Result<OwnedFd> {
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a non-negative descriptor just returned by open/openat is
        // owned by nobody else.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn open_root(root: &Path) -> io::Result<OwnedFd> {
        let path = cstring(root.as_os_str().as_bytes())?;
        // SAFETY: valid NUL-terminated path; the result is checked.
        check(unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })
    }

    fn open_at(dir: &OwnedFd, name: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
        open_at_mode(dir, name, flags, 0o600)
    }

    fn open_at_mode(
        dir: &OwnedFd,
        name: &CStr,
        flags: libc::c_int,
        mode: libc::c_uint,
    ) -> io::Result<OwnedFd> {
        // SAFETY: `dir` is an open directory and `name` NUL-terminated; the
        // mode is read only with O_CREAT.
        check(unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        })
    }

    /// The directory holding the last component, walked without following
    /// any symlink, and that component's name. Missing directories are
    /// created when `create` is set.
    fn parent_dir(root: &Path, rel: &Path, create: bool) -> io::Result<(OwnedFd, CString)> {
        let parts = super::components(rel)?;
        let mut dir = open_root(root)?;
        for part in &parts[..parts.len() - 1] {
            let name = cstring(part.as_bytes())?;
            dir = match open_at(&dir, &name, libc::O_RDONLY | libc::O_DIRECTORY) {
                Ok(next) => next,
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                    // SAFETY: as in open_at.
                    let made = unsafe {
                        libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755 as libc::mode_t)
                    };
                    if made != 0 {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::AlreadyExists {
                            return Err(error);
                        }
                    } else {
                        sync_dir(&dir)?;
                    }
                    open_at(&dir, &name, libc::O_RDONLY | libc::O_DIRECTORY)?
                }
                Err(error) => return Err(error),
            };
        }
        let last: &OsStr = parts[parts.len() - 1];
        Ok((dir, cstring(last.as_bytes())?))
    }

    /// Makes a directory's new entries durable.
    fn sync_dir(dir: &OwnedFd) -> io::Result<()> {
        #[cfg(test)]
        super::tests::DIR_SYNCS.with(|count| count.set(count.get() + 1));
        // SAFETY: `dir` is an open descriptor.
        if unsafe { libc::fsync(dir.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// `lstat` of `name` in `dir`.
    fn stat_at(dir: &OwnedFd, name: &CStr) -> io::Result<libc::stat> {
        // SAFETY: zeroed stat is a valid out-buffer; arguments as in open_at.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(st)
    }

    pub fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
        let (dir, name) = parent_dir(root, rel, false)?;
        // O_NONBLOCK: opening a FIFO must fail or return, never hang.
        let mut file = File::from(open_at(&dir, &name, libc::O_RDONLY | libc::O_NONBLOCK)?);
        #[cfg(test)]
        super::tests::AFTER_OPEN.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let meta = file.metadata()?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(super::refused("not a regular file with a single link"));
        }
        // The name must still be this very file: a hard link unlinked right
        // after the open would otherwise pass the link count.
        let entry = stat_at(&dir, &name)?;
        // Casts to the libc aliases: field widths differ by platform.
        if entry.st_dev != meta.dev() as libc::dev_t
            || entry.st_ino != meta.ino() as libc::ino_t
            || entry.st_nlink != 1
        {
            return Err(super::refused("the file changed while it was opened"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// A fresh temporary file in `dir`, created exclusively under a random
    /// name; a collision picks another name and never touches the existing one.
    fn create_temp(dir: &OwnedFd, mode: libc::c_uint) -> io::Result<(File, CString)> {
        for _ in 0..super::TEMP_ATTEMPTS {
            let tmp = cstring(super::temp_name().as_bytes())?;
            match open_at_mode(
                dir,
                &tmp,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                mode,
            ) {
                Ok(fd) => return Ok((File::from(fd), tmp)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free temporary name",
        ))
    }

    pub fn write(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let (dir, name) = parent_dir(root, rel, true)?;
        let existing = match stat_at(&dir, &name) {
            Ok(st) => Some(st),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        // Only a regular file or a link is replaced: a socket, FIFO, device
        // or directory at the destination is refused, never destroyed.
        if let Some(st) = existing {
            let kind = st.st_mode & libc::S_IFMT;
            if kind != libc::S_IFREG && kind != libc::S_IFLNK {
                return Err(super::refused("the destination is not a regular file"));
            }
        }
        // A replaced regular file keeps its permission bits: the temporary
        // file starts owner-only and gets them before it is published. A new
        // file (or one replacing a link) is created 0666 under the umask, like
        // any file the process writes.
        let kept: Option<libc::mode_t> = match existing {
            Some(st) if (st.st_mode & libc::S_IFMT) == libc::S_IFREG => Some(st.st_mode & 0o777),
            _ => None,
        };
        let create_mode = if kept.is_some() { 0o600 } else { 0o666 };
        // Cleanup is armed only once the temporary file is ours.
        let (mut file, tmp) = create_temp(&dir, create_mode)?;
        let written = (|| {
            if let Some(mode) = kept {
                // SAFETY: the descriptor is open and owned by `file`.
                if unsafe { libc::fchmod(file.as_raw_fd(), mode) } != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            file.write_all(bytes)?;
            // Content first, then the entry: a crash never publishes a
            // half-written file over the previous one.
            file.sync_all()?;
            // SAFETY: as in open_at; renameat replaces the entry, never
            // writes through what it pointed at.
            if unsafe {
                libc::renameat(
                    dir.as_raw_fd(),
                    tmp.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        match written {
            Ok(()) => sync_dir(&dir),
            Err(error) => {
                // SAFETY: as in open_at.
                unsafe { libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0) };
                Err(error)
            }
        }
    }

    pub fn remove(root: &Path, rel: &Path) -> io::Result<()> {
        let (dir, name) = parent_dir(root, rel, false)?;
        // SAFETY: as in open_at. Unlinking a link removes the link only.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    /// Opens a reparse point (symlink, junction) itself instead of its target.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    fn guarded(root: &Path, rel: &Path) -> io::Result<PathBuf> {
        super::components(rel)?;
        let path = root.join(rel);
        crate::core::fs_guard::assert_contained_no_symlink(root, &path)
            .map_err(super::refused_owned)?;
        Ok(path)
    }

    fn link_count(file: &File) -> io::Result<u32> {
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        // SAFETY: zeroed info is a valid out-buffer; the handle is open.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.nNumberOfLinks)
    }

    pub fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
        let path = guarded(root, rel)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.file_type().is_symlink() || link_count(&file)? != 1 {
            return Err(super::refused("not a regular file with a single link"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn write(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let path = guarded(root, rel)?;
        let parent = path
            .parent()
            .ok_or_else(|| super::refused("no parent directory"))?;
        std::fs::create_dir_all(parent)?;
        guarded(root, rel)?;
        let existing = std::fs::symlink_metadata(&path).ok();
        // Only a regular file or a link is replaced, never a directory or
        // device-like entry.
        if existing
            .as_ref()
            .is_some_and(|meta| !meta.is_file() && !meta.file_type().is_symlink())
        {
            return Err(super::refused("the destination is not a regular file"));
        }
        // Cleanup is armed only once the temporary file is ours.
        let (mut file, tmp) = create_temp(parent)?;
        let written = (|| {
            file.write_all(bytes)?;
            // A replaced regular file keeps its attributes (read-only).
            if let Some(meta) = existing.as_ref().filter(|meta| meta.is_file()) {
                file.set_permissions(meta.permissions())?;
            }
            // std cannot sync a directory on Windows: the content is synced,
            // the renamed entry relies on the file system's journal.
            file.sync_all()?;
            drop(file);
            // Replaces the directory entry (a planted link included).
            std::fs::rename(&tmp, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    /// A fresh temporary file in `dir` under a random name, retried on a
    /// collision without touching the existing entry.
    fn create_temp(dir: &Path) -> io::Result<(File, PathBuf)> {
        for _ in 0..super::TEMP_ATTEMPTS {
            let tmp = dir.join(super::temp_name());
            match OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Ok(file) => return Ok((file, tmp)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free temporary name",
        ))
    }

    pub fn remove(root: &Path, rel: &Path) -> io::Result<()> {
        std::fs::remove_file(guarded(root, rel)?)
    }
}

/// Reads `rel` under `root`: a regular, single-link file reached without
/// following any link below the root.
pub fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
    imp::read(root, rel)
}

/// Replaces `rel` under `root` with `bytes` (fresh file renamed over it),
/// creating missing parents.
pub fn write(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    imp::write(root, rel, bytes)
}

/// Appends by rewriting: the existing content is read with [`read`]'s checks
/// and replaced like [`write`]. Meant for small journals.
pub fn append(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut content = match imp::read(root, rel) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    content.extend_from_slice(bytes);
    imp::write(root, rel, &content)
}

/// Removes `rel` under `root` (a link there is removed, not followed).
pub fn remove(root: &Path, rel: &Path) -> io::Result<()> {
    imp::remove(root, rel)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    thread_local! {
        /// Directory syncs made by this thread.
        pub(super) static DIR_SYNCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        /// Runs once inside the next read, right after its open.
        pub(super) static AFTER_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
    }

    #[test]
    fn plain_files_read_write_append_and_remove() {
        let (root, _) = dirs();
        write(root.path(), Path::new(".kronn/a/b.json"), b"[1]").unwrap();
        write(root.path(), Path::new(".kronn/a/b.json"), b"[2]").unwrap();
        assert_eq!(
            read(root.path(), Path::new(".kronn/a/b.json")).unwrap(),
            b"[2]"
        );
        append(root.path(), Path::new("notes/d.md"), b"x").unwrap();
        append(root.path(), Path::new("notes/d.md"), b"y").unwrap();
        assert_eq!(read(root.path(), Path::new("notes/d.md")).unwrap(), b"xy");
        remove(root.path(), Path::new(".kronn/a/b.json")).unwrap();
        assert!(!root.path().join(".kronn/a/b.json").exists());
    }

    #[test]
    fn escaping_paths_are_refused() {
        let (root, _) = dirs();
        for rel in ["/etc/hosts", "../x", "a/../../x", ""] {
            assert!(read(root.path(), Path::new(rel)).is_err(), "{rel}");
            assert!(write(root.path(), Path::new(rel), b"x").is_err(), "{rel}");
        }
    }

    #[test]
    fn a_symlinked_file_is_not_read_and_a_write_replaces_the_link() {
        let (root, outside) = dirs();
        let target = outside.path().join("secret");
        std::fs::write(&target, b"keep").unwrap();
        symlink(&target, root.path().join("link")).unwrap();
        assert!(read(root.path(), Path::new("link")).is_err());
        write(root.path(), Path::new("link"), b"new").unwrap();
        append(root.path(), Path::new("link"), b"+").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        let replaced = std::fs::symlink_metadata(root.path().join("link")).unwrap();
        assert!(replaced.file_type().is_file());
        assert_eq!(read(root.path(), Path::new("link")).unwrap(), b"new+");
    }

    #[test]
    fn a_symlinked_directory_is_not_walked() {
        let (root, outside) = dirs();
        std::fs::write(outside.path().join("f.json"), b"keep").unwrap();
        symlink(outside.path(), root.path().join(".kronn")).unwrap();
        assert!(read(root.path(), Path::new(".kronn/f.json")).is_err());
        assert!(write(root.path(), Path::new(".kronn/f.json"), b"").is_err());
        assert!(write(root.path(), Path::new(".kronn/new.json"), b"x").is_err());
        assert_eq!(
            std::fs::read(outside.path().join("f.json")).unwrap(),
            b"keep"
        );
        assert!(!outside.path().join("new.json").exists());
    }

    /// A hard link to an outside file, when the filesystem allows one here.
    fn hard_link(root: &Path, outside: &Path) -> Option<std::path::PathBuf> {
        let target = outside.join("secret");
        std::fs::write(&target, b"keep").unwrap();
        std::fs::hard_link(&target, root.join("hard")).ok()?;
        Some(target)
    }

    #[test]
    fn a_hard_link_is_not_read_and_a_write_leaves_its_target_alone() {
        let (root, outside) = dirs();
        let Some(target) = hard_link(root.path(), outside.path()) else {
            return; // different filesystems: no hard link possible
        };
        assert!(read(root.path(), Path::new("hard")).is_err());
        write(root.path(), Path::new("hard"), b"").unwrap();
        append(root.path(), Path::new("hard"), b"x").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn a_hard_link_unlinked_right_after_the_open_is_not_read() {
        let (root, outside) = dirs();
        let Some(target) = hard_link(root.path(), outside.path()) else {
            return;
        };
        let entry = root.path().join("hard");
        AFTER_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || std::fs::remove_file(&entry).unwrap()));
        });
        assert!(read(root.path(), Path::new("hard")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn an_existing_temporary_name_is_never_removed() {
        let (root, _) = dirs();
        let occupied = root.path().join(".kronn-tmp-occupied");
        std::fs::write(&occupied, b"someone else").unwrap();
        write(root.path(), Path::new("f"), b"x").unwrap();
        assert_eq!(std::fs::read(&occupied).unwrap(), b"someone else");
        assert!(super::temp_name().starts_with(".kronn-tmp-"));
        assert_ne!(super::temp_name(), super::temp_name());
    }

    #[test]
    fn a_name_at_the_length_limit_stays_writable() {
        let (root, _) = dirs();
        let long = "n".repeat(255);
        write(root.path(), Path::new(&long), b"x").unwrap();
        assert_eq!(read(root.path(), Path::new(&long)).unwrap(), b"x");
    }

    #[test]
    fn a_replaced_file_keeps_its_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let (root, _) = dirs();
        let mode = |name: &str| {
            std::fs::metadata(root.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        for (name, bits) in [("secret", 0o600), ("tool.sh", 0o755)] {
            std::fs::write(root.path().join(name), b"old").unwrap();
            std::fs::set_permissions(
                root.path().join(name),
                std::fs::Permissions::from_mode(bits),
            )
            .unwrap();
            write(root.path(), Path::new(name), b"new").unwrap();
            append(root.path(), Path::new(name), b"+").unwrap();
            assert_eq!(mode(name), bits, "{name}");
        }
    }

    #[test]
    fn special_destinations_are_refused_and_kept() {
        use std::os::unix::fs::FileTypeExt;
        let (root, _) = dirs();
        let socket = root.path().join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let fifo = std::ffi::CString::new(root.path().join("pipe").as_os_str().as_encoded_bytes())
            .unwrap();
        // SAFETY: valid path; test-only.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        for name in ["s.sock", "pipe"] {
            assert!(write(root.path(), Path::new(name), b"x").is_err(), "{name}");
            assert!(
                append(root.path(), Path::new(name), b"x").is_err(),
                "{name}"
            );
        }
        assert!(std::fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_socket());
        assert!(std::fs::symlink_metadata(root.path().join("pipe"))
            .unwrap()
            .file_type()
            .is_fifo());
    }

    #[test]
    fn a_write_syncs_its_directory_and_every_created_parent() {
        let (root, _) = dirs();
        DIR_SYNCS.with(|count| count.set(0));
        write(root.path(), Path::new("a/b/f.json"), b"x").unwrap();
        // root (gains a/), a/ (gains b/), b/ (gains f.json).
        assert_eq!(DIR_SYNCS.with(|count| count.get()), 3);
        DIR_SYNCS.with(|count| count.set(0));
        write(root.path(), Path::new("a/b/f.json"), b"y").unwrap();
        assert_eq!(DIR_SYNCS.with(|count| count.get()), 1);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let (root, _) = dirs();
        std::fs::create_dir(root.path().join("taken")).unwrap();
        assert!(write(root.path(), Path::new("taken"), b"x").is_err());
        let names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["taken"]);
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let (root, _) = dirs();
        let fifo = std::ffi::CString::new(root.path().join("pipe").as_os_str().as_encoded_bytes())
            .unwrap();
        // SAFETY: valid path; test-only.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        assert!(read(root.path(), Path::new("pipe")).is_err());
    }
}
