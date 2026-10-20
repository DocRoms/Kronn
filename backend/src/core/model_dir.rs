//! A model folder Kronn downloads into (KT-1381), held by a directory
//! handle so every file operation stays inside it.
//!
//! The folder is opened from the install folder's handle, one plain
//! directory at a time with `O_NOFOLLOW`, so no link anywhere below the
//! install folder is followed and the path is never resolved again. Files
//! are then opened, removed and renamed relative to that handle with
//! `O_NOFOLLOW`:
//! - a `.part` that is not a plain file with a single link (a symbolic link,
//!   a hard link to another file) is removed, the link and never its target,
//!   and the download starts over;
//! - an opened file is checked (`fstat`: regular, one link) before a single
//!   byte is written, and truncated only after that check;
//! - the finished file is renamed within the same handle, then must be the
//!   very file that was hashed.
//!
//! Unix only: elsewhere the card links to the official download instead.

#![cfg(unix)]

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Device and inode of an opened file.
pub type Identity = (u64, u64);

fn cstring(name: &str) -> io::Result<CString> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} is not a plain file name"),
        ));
    }
    CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))
}

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a non-negative descriptor just returned by open/openat is
    // owned by nobody else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn foreign(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// A plain file with a single link: the only kind Kronn writes or renames.
fn plain(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_file() && meta.nlink() == 1
}

pub struct ModelDir {
    fd: OwnedFd,
}

impl ModelDir {
    /// Opens `rel` below `root`: the root (a path Kronn checked, holding no
    /// link), then each component from the previous handle as a plain
    /// directory, never following a link. Anything but plain names (`..`, an
    /// absolute part) is refused.
    pub fn open_under(root: &Path, rel: &Path) -> io::Result<Self> {
        let path = CString::new(root.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: valid NUL-terminated path; the result is checked.
        let mut fd = owned(unsafe { libc::open(path.as_ptr(), flags) })?;
        for component in rel.components() {
            let std::path::Component::Normal(part) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a plain relative path", rel.display()),
                ));
            };
            let name = CString::new(part.as_bytes())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
            // SAFETY: `fd` is an open directory and `name` NUL-terminated.
            fd = owned(unsafe { libc::openat(fd.as_raw_fd(), name.as_ptr(), flags) })?;
        }
        Ok(Self { fd })
    }

    fn stat(&self, name: &str) -> io::Result<Option<libc::stat>> {
        let name = cstring(name)?;
        // SAFETY: zeroed stat is a valid out-buffer; the handle is open and
        // the name NUL-terminated.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(Some(st))
    }

    pub fn remove(&self, name: &str) -> io::Result<()> {
        let c_name = cstring(name)?;
        // SAFETY: as in stat; unlinkat removes the entry, never a link's target.
        if unsafe { libc::unlinkat(self.fd.as_raw_fd(), c_name.as_ptr(), 0) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Bytes already in `name` when it is a plain single-link file; anything
    /// else under that name is removed first, and counts as nothing.
    pub fn part_len(&self, name: &str) -> io::Result<u64> {
        match self.stat(name)? {
            None => Ok(0),
            Some(st) if (st.st_mode & libc::S_IFMT) == libc::S_IFREG && st.st_nlink == 1 => {
                Ok(st.st_size as u64)
            }
            Some(_) => {
                self.remove(name)?;
                Ok(0)
            }
        }
    }

    fn open_at(&self, name: &str, flags: libc::c_int) -> io::Result<File> {
        let c_name = cstring(name)?;
        // SAFETY: as in stat; the mode is read only with O_CREAT.
        let fd = owned(unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                c_name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o644 as libc::c_uint,
            )
        })?;
        let file = File::from(fd);
        if !plain(&file.metadata()?) {
            return Err(foreign(format!(
                "{name} is not a plain file with a single link"
            )));
        }
        Ok(file)
    }

    /// `name` opened for writing, created if needed. Emptied only after the
    /// check, unless `append`.
    pub fn open_part(&self, name: &str, append: bool) -> io::Result<File> {
        let flags = libc::O_WRONLY | libc::O_CREAT | if append { libc::O_APPEND } else { 0 };
        let file = self.open_at(name, flags)?;
        if !append {
            file.set_len(0)?;
        }
        Ok(file)
    }

    pub fn open_read(&self, name: &str) -> io::Result<File> {
        self.open_at(name, libc::O_RDONLY)
    }

    /// Renames `from` to `to` inside the folder; `to` must then be the file
    /// `expected` (device, inode) still, plain and single-linked.
    pub fn rename(&self, from: &str, to: &str, expected: Identity) -> io::Result<()> {
        let (c_from, c_to) = (cstring(from)?, cstring(to)?);
        // SAFETY: both names are resolved relative to the same open handle.
        let rc = unsafe {
            libc::renameat(
                self.fd.as_raw_fd(),
                c_from.as_ptr(),
                self.fd.as_raw_fd(),
                c_to.as_ptr(),
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let landed = self
            .open_read(to)
            .and_then(|file| file_identity(&file))
            .ok();
        if landed != Some(expected) {
            self.remove(to)?;
            return Err(foreign(format!(
                "{from} was replaced before it became {to}; removed, retry"
            )));
        }
        // SAFETY: the handle is an open directory.
        if unsafe { libc::fsync(self.fd.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Device and inode of an opened file.
pub fn file_identity(file: &File) -> io::Result<Identity> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}
