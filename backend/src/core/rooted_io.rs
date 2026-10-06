//! Worktree-rooted file reads and writes for the SubWorkflow foreach engine.
//!
//! On Unix every component below the root is opened with `openat` and
//! `O_NOFOLLOW` from the previous directory handle, so a symlink planted (or
//! swapped in) anywhere on the path makes the call fail instead of leaving the
//! worktree. The opened file must be a regular file with a single link: a hard
//! link would reach an outside file without any symlink. The root itself is
//! trusted (a path Kronn chose) and may be reached through symlinks.
//!
//! Windows: best effort only. The path is checked with `fs_guard` and then
//! opened by name, so a swap between check and open is not excluded. The
//! rooted primitive for every platform is KT-1055.

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

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn cstring(bytes: &[u8]) -> io::Result<CString> {
        CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
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
        // SAFETY: `dir` is an open directory and `name` NUL-terminated; the
        // mode is read only with O_CREAT.
        check(unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o644 as libc::c_uint,
            )
        })
    }

    /// The directory holding the last component, walked without following
    /// any symlink. Missing directories are created when `create` is set.
    fn parent_dir(root: &Path, parts: &[&OsStr], create: bool) -> io::Result<OwnedFd> {
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
                    }
                    open_at(&dir, &name, libc::O_RDONLY | libc::O_DIRECTORY)?
                }
                Err(error) => return Err(error),
            };
        }
        Ok(dir)
    }

    /// Refuses anything but a regular file with one link.
    fn single_regular_file(fd: OwnedFd) -> io::Result<File> {
        let file = File::from(fd);
        let meta = file.metadata()?;
        use std::os::unix::fs::MetadataExt;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file with a single link",
            ));
        }
        Ok(file)
    }

    fn open_file(root: &Path, rel: &Path, flags: libc::c_int, create: bool) -> io::Result<File> {
        let parts = super::components(rel)?;
        let dir = parent_dir(root, &parts, create)?;
        let name = cstring(parts[parts.len() - 1].as_bytes())?;
        // O_NONBLOCK: opening a FIFO must fail or return, never hang.
        single_regular_file(open_at(&dir, &name, flags | libc::O_NONBLOCK)?)
    }

    pub fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
        let mut file = open_file(root, rel, libc::O_RDONLY, false)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn write(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        // Truncated only once the file is known to be ours.
        let mut file = open_file(root, rel, libc::O_WRONLY | libc::O_CREAT, true)?;
        file.set_len(0)?;
        file.write_all(bytes)
    }

    pub fn append(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = open_file(
            root,
            rel,
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
            true,
        )?;
        file.write_all(bytes)
    }

    pub fn remove(root: &Path, rel: &Path) -> io::Result<()> {
        let parts = super::components(rel)?;
        let dir = parent_dir(root, &parts, false)?;
        let name = cstring(parts[parts.len() - 1].as_bytes())?;
        // SAFETY: as in open_at. Unlinking a symlink removes the link only.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::io;
    use std::path::Path;

    fn guarded(root: &Path, rel: &Path) -> io::Result<std::path::PathBuf> {
        super::components(rel)?;
        let path = root.join(rel);
        crate::core::fs_guard::assert_contained_no_symlink(root, &path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        Ok(path)
    }

    pub fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(guarded(root, rel)?)
    }

    pub fn write(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let path = guarded(root, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)
    }

    pub fn append(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let path = guarded(root, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(bytes)
    }

    pub fn remove(root: &Path, rel: &Path) -> io::Result<()> {
        std::fs::remove_file(guarded(root, rel)?)
    }
}

/// Reads `rel` under `root` without following any symlink below the root.
pub fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
    imp::read(root, rel)
}

/// Replaces the content of `rel` under `root`, creating missing parents.
pub fn write(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    imp::write(root, rel, bytes)
}

/// Appends to `rel` under `root`, creating it and missing parents.
pub fn append(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    imp::append(root, rel, bytes)
}

/// Removes `rel` under `root` (a symlink there is removed, not followed).
pub fn remove(root: &Path, rel: &Path) -> io::Result<()> {
    imp::remove(root, rel)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

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
    fn a_symlinked_file_is_neither_read_nor_truncated() {
        let (root, outside) = dirs();
        let target = outside.path().join("secret");
        std::fs::write(&target, b"keep").unwrap();
        symlink(&target, root.path().join("link")).unwrap();
        assert!(read(root.path(), Path::new("link")).is_err());
        assert!(write(root.path(), Path::new("link"), b"").is_err());
        assert!(append(root.path(), Path::new("link"), b"x").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
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

    #[test]
    fn a_hard_link_to_an_outside_file_is_refused() {
        let (root, outside) = dirs();
        let target = outside.path().join("secret");
        std::fs::write(&target, b"keep").unwrap();
        if std::fs::hard_link(&target, root.path().join("hard")).is_err() {
            return; // different filesystems: no hard link possible
        }
        assert!(read(root.path(), Path::new("hard")).is_err());
        assert!(write(root.path(), Path::new("hard"), b"").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
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
