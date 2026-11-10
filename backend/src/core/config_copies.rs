//! The copies of `config.toml` that Kronn keeps in its data directory
//! (KT-1528): the snapshot the database migrations take, its rotations, the
//! file an unreadable `config.toml` is moved to, and the value a retired key
//! leaves behind.
//!
//! A copy is never kept in clear and never scrubbed by name (a list of
//! credential names is always one name short): it is encrypted whole with the
//! instance key, like `config.toml.pre-credential-store.enc`, so whatever the
//! file held, a credential this build does not know included, leaves no clear
//! value on disk.
//!
//! | copy                          | sealed name                      |
//! |-------------------------------|----------------------------------|
//! | `config.toml.backup`          | `config.toml.backup.enc`         |
//! | `config.toml.backup.<ts>`     | `config.toml.backup.<ts>.enc`    |
//! | `config.toml.corrupt.<ts>`    | `config.toml.corrupt.<ts>.enc`   |
//! | `config.toml.retired-key.<ts>`| `config.toml.retired-key.<ts>.enc`|
//!
//! The ciphertext is bound to the kind of copy it is (backup, corrupt,
//! retired-key) as associated data, so a sealed copy renamed or swapped for
//! another kind does not open. The raw bytes are sealed, not a lossy string.
//!
//! The key lookup has three answers ([`Sealing`]): a key, none in any store
//! (proven), or the stores could not be read (locked keychain, I/O error). Only
//! a proven absence lets a copy be cut down to its `encryption_secret` line (the
//! one thing a later start needs to recover) or dropped; when the stores cannot
//! be read, the copies are left exactly as they are and the next start or unlock
//! retries. A clear copy found at start (written by 0.15.x) is converted once by
//! [`protect_copies`]; a copy that cannot be written is left as it is and
//! retried at the next start, never deleted for it (a key may sit in it).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::{config, crypto, keystore, keyvault};

/// Suffix of a copy encrypted with the instance key.
pub const SEALED_SUFFIX: &str = ".enc";
/// The snapshot taken before migrations (clear name: the sealed one adds `.enc`).
pub const BACKUP_NAME: &str = "config.toml.backup";
/// Prefix of the file an unreadable config.toml is moved to.
pub const CORRUPT_PREFIX: &str = "config.toml.corrupt.";

/// What a copy of config.toml is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `config.toml.backup` and its rotations.
    Backup,
    /// `config.toml.corrupt.<ts>`.
    Corrupt,
    /// `config.toml.retired-key.<ts>`.
    RetiredKey,
}

impl Kind {
    /// The associated data a ciphertext of this kind is bound to.
    fn aad(self) -> &'static [u8] {
        match self {
            Kind::Backup => b"kronn/config-copy/v1/backup",
            Kind::Corrupt => b"kronn/config-copy/v1/corrupt",
            Kind::RetiredKey => b"kronn/config-copy/v1/retired-key",
        }
    }
}

/// A copy found in the data directory.
#[derive(Debug, Clone)]
pub struct ConfigCopy {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
    pub sealed: bool,
}

/// What a file name says about it: `None` when it is no copy of config.toml
/// (nor a temp file of a writer of one).
pub fn classify(name: &str) -> Option<(Kind, bool)> {
    if name.starts_with('.') {
        return None;
    }
    let (stem, sealed) = match name.strip_suffix(SEALED_SUFFIX) {
        Some(stem) => (stem, true),
        None => (name, false),
    };
    let kind = if stem == BACKUP_NAME || stem.starts_with(&format!("{BACKUP_NAME}.")) {
        Kind::Backup
    } else if stem.starts_with(CORRUPT_PREFIX) {
        Kind::Corrupt
    } else if stem.starts_with(keystore::RETIRED_KEY_PREFIX) {
        Kind::RetiredKey
    } else {
        return None;
    };
    Some((kind, sealed))
}

/// Every copy of config.toml in `dir`, sorted by name.
pub fn copies(dir: &Path) -> Vec<ConfigCopy> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<ConfigCopy> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (kind, sealed) = classify(&name)?;
            Some(ConfigCopy {
                path: e.path(),
                name,
                kind,
                sealed,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Names of the sealed copies in `dir`: the files a key change must follow.
pub fn sealed_names(dir: &Path) -> Vec<String> {
    copies(dir)
        .into_iter()
        .filter(|c| c.sealed)
        .map(|c| c.name)
        .collect()
}

/// What the key stores say about the key a copy is sealed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sealing {
    /// A key to seal under (canonical hex).
    Key(String),
    /// Every store was read and none holds a key: none exists.
    Absent,
    /// A store could not be read, or holds something that is not a key: a key
    /// may exist, so nothing may be dropped on the strength of this answer.
    Unavailable(String),
}

/// The key the copies are sealed under when the caller has none: the one the
/// key stores hold now (environment, keychain, key file). With
/// `with_file_key`, the `encryption_secret` config.toml itself holds as well,
/// for a copy of a config.toml that stays where it is.
pub fn sealing(dir: &Path, with_file_key: bool) -> Sealing {
    sealing_in(&keyvault::KeyStore::standard(dir), dir, with_file_key)
}

fn sealing_in(store: &keyvault::KeyStore, dir: &Path, with_file_key: bool) -> Sealing {
    match store.lookup() {
        keyvault::KeyLookup::Found(key, _) => Sealing::Key(key),
        // A store that cannot be read may hold the live key: the key config.toml
        // holds could be an older one, so it does not stand in for it.
        keyvault::KeyLookup::Unreadable(why) => Sealing::Unavailable(why),
        keyvault::KeyLookup::Absent => {
            if with_file_key {
                // Every store was read and is empty, so the file's key is the
                // only one. It stays where it is: a copy sealed under it is as
                // safe as the file. Raw bytes, read line by line when the rest
                // of it is damaged or is not UTF-8.
                match std::fs::read(dir.join("config.toml")) {
                    Ok(bytes) => {
                        if let Some(key) = key_of(&String::from_utf8_lossy(&bytes)) {
                            return Sealing::Key(key);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    // A file that exists but cannot be read may hold the key.
                    Err(e) => return Sealing::Unavailable(format!("config.toml: {e}")),
                }
            }
            Sealing::Absent
        }
    }
}

/// The key of [`sealing`], when there is one.
pub fn sealing_key(dir: &Path, with_file_key: bool) -> Option<String> {
    match sealing(dir, with_file_key) {
        Sealing::Key(key) => Some(key),
        _ => None,
    }
}

/// `bytes` encrypted with `key_hex`, bound to the `kind` of copy.
pub fn seal(bytes: &[u8], key_hex: &str, kind: Kind) -> Result<String> {
    let key = crypto::parse_secret(key_hex).map_err(anyhow::Error::msg)?;
    crypto::encrypt_bytes_aad(bytes, &key, kind.aad()).map_err(anyhow::Error::msg)
}

/// The bytes `encrypted` holds as a `kind` copy, opened with the first of
/// `keys` that reads it.
pub fn open_bytes(encrypted: &str, keys: &[String], kind: Kind) -> Option<Vec<u8>> {
    keys.iter().find_map(|key| {
        let parsed = crypto::parse_secret(key).ok()?;
        crypto::decrypt_bytes_aad(encrypted.trim(), &parsed, kind.aad()).ok()
    })
}

/// The text of [`open_bytes`] (a non-UTF-8 byte reads as U+FFFD: for looking
/// at a copy, never for rewriting it).
pub fn open_text(encrypted: &str, keys: &[String], kind: Kind) -> Option<String> {
    open_bytes(encrypted, keys, kind).map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn kind_of_path(path: &Path) -> Result<Kind> {
    path.file_name()
        .and_then(|n| classify(&n.to_string_lossy()))
        .map(|(kind, _)| kind)
        .with_context(|| format!("{} is not a copy of config.toml", path.display()))
}

/// The bytes of a sealed copy, opened with the first of `keys` that reads it.
pub fn open_sealed_bytes(path: &Path, keys: &[String]) -> Result<Vec<u8>> {
    let kind = kind_of_path(path)?;
    let encrypted =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    open_bytes(&encrypted, keys, kind)
        .with_context(|| format!("{} is not readable with the keys at hand", path.display()))
}

/// The text of a sealed copy, opened with the first of `keys` that reads it.
pub fn open_sealed(path: &Path, keys: &[String]) -> Result<String> {
    open_sealed_bytes(path, keys).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Write `content`, sealed under `key_hex` as the kind of copy `path` names, to
/// `path` (0600, atomic), and read it back: a copy that does not open is no copy.
pub fn write_sealed(
    dir: &Path,
    path: &Path,
    content: impl AsRef<[u8]>,
    key_hex: &str,
) -> Result<()> {
    let content = content.as_ref();
    let kind = kind_of_path(path)?;
    let encrypted = seal(content, key_hex, kind)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp"));
    keyvault::write_private_atomic(&tmp, path, encrypted.as_bytes())
        .with_context(|| format!("write {name}"))?;
    let back = open_sealed_bytes(path, &[key_hex.to_string()])?;
    anyhow::ensure!(back == content, "{name} read back differently");
    Ok(())
}

/// A name for a new copy: `base` when free, else `base-1`, `base-2`, ...
pub fn free_name(dir: &Path, base: &str, suffix: &str) -> String {
    let mut name = format!("{base}{suffix}");
    let mut n = 1;
    // `symlink_metadata`, not `exists`: a dangling symlink holds a name too.
    while dir.join(&name).symlink_metadata().is_ok() {
        name = format!("{base}-{n}{suffix}");
        n += 1;
    }
    name
}

/// A UTC stamp for the name of a new copy.
pub fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// A top-level `encryption_secret` of `text`, whole or salvaged line by line,
/// when it is a real key.
pub fn key_of(text: &str) -> Option<String> {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|t| {
            t.get("encryption_secret")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| config::salvage_key_line(text))
        .and_then(|k| crypto::canonical_secret(&k).ok())
}

/// The only content kept of a copy that cannot be sealed.
fn key_only(key_hex: &str) -> String {
    format!(
        "encryption_secret = {}\n",
        toml::Value::String(key_hex.to_string())
    )
}

/// What the pass did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Protected {
    /// Clear copies now encrypted.
    pub sealed: usize,
    /// Clear copies cut down to their key line, or dropped (no key to seal with).
    pub reduced: usize,
    /// Temp files of an older writer removed.
    pub temps: usize,
    /// Clear copies left as they were because the key stores could not be read
    /// (or the copy holds something only a key can keep): retried at the next
    /// start or unlock.
    pub deferred: usize,
    /// Copies left as they were, with why (retried at the next start).
    pub failures: Vec<String>,
}

/// [`protect_clear_copies`] for what the key stores said: sealed under the key
/// they hold; cut down only when it is proven that no key exists; left exactly
/// as they are when the stores could not be read (a key may exist, and a copy
/// may be the only readable one).
pub fn protect_copies(dir: &Path, sealing: &Sealing) -> Protected {
    match sealing {
        Sealing::Key(key) => protect_clear_copies(dir, Some(key)),
        Sealing::Absent => protect_clear_copies(dir, None),
        Sealing::Unavailable(why) => {
            let held: Vec<String> = copies(dir)
                .into_iter()
                .filter(|c| !c.sealed)
                .map(|c| c.name)
                .collect();
            if !held.is_empty() {
                tracing::warn!(
                    "copies of config.toml are left as they are, not encrypted yet ({why}); \
                     the next start or unlock retries: {}",
                    held.join(", ")
                );
            }
            Protected {
                deferred: held.len(),
                ..Protected::default()
            }
        }
    }
}

/// Convert every clear copy of config.toml in `dir` once, idempotently: sealed
/// under `key_hex` when there is one; else (`None`: the caller PROVED that no
/// key exists, see [`protect_copies`]) cut down to its key line, or dropped when
/// it has none. A copy whose conversion fails is left untouched and reported in
/// `failures`; the next start retries.
pub fn protect_clear_copies(dir: &Path, key_hex: Option<&str>) -> Protected {
    let mut report = Protected::default();
    remove_old_temps(dir, &mut report);
    for copy in copies(dir).into_iter().filter(|c| !c.sealed) {
        match protect_one(dir, &copy, key_hex) {
            Ok(Done::Sealed) => report.sealed += 1,
            Ok(Done::Reduced) => report.reduced += 1,
            Ok(Done::Untouched) => {}
            Ok(Done::Deferred) => {
                tracing::warn!(
                    "{} holds a value that is not a key and no key exists to encrypt it under; \
                     it is kept as it is until one does",
                    copy.name
                );
                report.deferred += 1;
            }
            Err(e) => {
                tracing::warn!(
                    "{} still holds what config.toml held in clear ({e:#}); the next start retries",
                    copy.name
                );
                report.failures.push(format!("{}: {e:#}", copy.name));
            }
        }
    }
    report
}

enum Done {
    Sealed,
    Reduced,
    Untouched,
    Deferred,
}

fn protect_one(dir: &Path, copy: &ConfigCopy, key_hex: Option<&str>) -> Result<Done> {
    let bytes = match std::fs::read(&copy.path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Done::Untouched),
        Err(e) => return Err(e).context("read"),
    };
    // Looked at for a key line only; what is sealed is the raw bytes.
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if let Some(key) = key_hex {
        // The sealed name is free unless a crash left both forms: then the
        // clear one is sealed under a name of its own.
        let sealed_name = if dir
            .join(format!("{}{SEALED_SUFFIX}", copy.name))
            .symlink_metadata()
            .is_ok()
        {
            free_name(dir, &format!("{}.{}", copy.name, stamp()), SEALED_SUFFIX)
        } else {
            format!("{}{SEALED_SUFFIX}", copy.name)
        };
        let target = dir.join(&sealed_name);
        write_sealed(dir, &target, &bytes, key)?;
        remove(&copy.path)?;
        return Ok(Done::Sealed);
    }
    match key_of(&text) {
        Some(key) => {
            let only = key_only(&key);
            if text == only {
                return Ok(Done::Untouched);
            }
            let tmp = dir.join(format!(".{}.tmp", copy.name));
            keyvault::write_private_atomic(&tmp, &copy.path, only.as_bytes())
                .context("cut down to its key line")?;
            Ok(Done::Reduced)
        }
        // A retired-key file holds a value that sat in config.toml, a key or
        // not: it is sealed whole once a key exists, never dropped (only a file
        // proven empty goes).
        None if copy.kind == Kind::RetiredKey && !text.trim().is_empty() => Ok(Done::Deferred),
        // Nothing in it a later start needs: dropped rather than kept in clear.
        None => {
            remove(&copy.path)?;
            Ok(Done::Reduced)
        }
    }
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("remove the clear copy"),
    }
    if let Some(dir) = path.parent() {
        keyvault::sync_dir(dir);
    }
    Ok(())
}

/// Temp files the writers of a copy left behind before they sealed: they may
/// hold a scrubbed-by-name text. The writers' temps now hold only sealed bytes.
fn remove_old_temps(dir: &Path, report: &mut Protected) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_copy_temp = name.ends_with(".tmp")
            && [
                ".config.toml.backup",
                ".config.toml.corrupt",
                ".config.toml.retired-key",
            ]
            .iter()
            .any(|p| name.starts_with(p));
        if is_copy_temp && std::fs::remove_file(entry.path()).is_ok() {
            report.temps += 1;
        }
    }
}

/// An error when `path` is a symbolic link (dangling or not): a copy is never
/// read through, renamed or written over a link. A missing path is fine.
pub fn refuse_symlink(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{} is a symbolic link; it is left where it is",
                path.display()
            ),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Keep the snapshot `config.toml.backup.enc` as a rotation
/// (`config.toml.backup.<ts>.enc`) before a new one replaces it: a rename, so
/// nothing is rewritten and nothing can be lost to a full disk.
pub fn rotate_sealed_backup(dir: &Path) -> std::io::Result<()> {
    let current = dir.join(format!("{BACKUP_NAME}{SEALED_SUFFIX}"));
    // `symlink_metadata`, not `exists`: a dangling link is something there. A
    // link is refused, not renamed into a rotation that would then pass for a
    // real copy (and the new snapshot is not written over it either).
    match std::fs::symlink_metadata(&current) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(_) => refuse_symlink(&current)?,
    }
    let name = free_name(dir, &format!("{BACKUP_NAME}.{}", stamp()), SEALED_SUFFIX);
    std::fs::rename(&current, dir.join(name))?;
    keyvault::sync_dir(dir);
    Ok(())
}

/// What became of the content of an unreadable config.toml.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aside {
    /// Kept whole, encrypted: the file name.
    Sealed(String),
    /// Only its key line kept, in clear (0600): the file name.
    KeyOnly(String),
    /// Nothing worth keeping and no key to keep it under.
    Nothing,
    /// The key stores could not be read, so it is unknown whether a key exists:
    /// the file was moved as it was (clear, 0600) and is encrypted at the next
    /// start, once the stores read. The file name.
    Deferred(String),
}

impl Aside {
    /// What the notice says became of the file, after "config.toml ...".
    pub fn describe(&self) -> String {
        match self {
            Aside::Sealed(name) => format!("was kept encrypted as {name}"),
            Aside::Deferred(name) => format!(
                "was kept as {name}, readable only by you, until the encryption key can be read \
                 and the copy encrypted (at the next start)"
            ),
            Aside::KeyOnly(name) => {
                format!("was removed; only the encryption key line it held is kept as {name}")
            }
            Aside::Nothing => "was removed: no encryption key was available to keep a copy \
                               encrypted"
                .to_string(),
        }
    }
}

/// Keep what an unreadable config.toml held. `key_hex` is a key the stores hold
/// independently of that file; without one the content is not kept (a copy
/// sealed under a key it holds itself would protect nothing): only its key line
/// is, so the key can still be recovered.
/// `content` is sealed as the raw bytes it is.
pub fn set_aside_content(dir: &Path, content: &[u8], key_hex: Option<&str>) -> Result<Aside> {
    let base = format!("{CORRUPT_PREFIX}{}", stamp());
    if let Some(key) = key_hex {
        let name = free_name(dir, &base, SEALED_SUFFIX);
        write_sealed(dir, &dir.join(&name), content, key)?;
        return Ok(Aside::Sealed(name));
    }
    let text = String::from_utf8_lossy(content);
    let Some(key_line) = key_of(&text).map(|k| key_only(&k)) else {
        return Ok(Aside::Nothing);
    };
    let name = free_name(dir, &base, "");
    let tmp = dir.join(format!(".{name}.tmp"));
    keyvault::write_private_atomic(&tmp, &dir.join(&name), key_line.as_bytes())
        .with_context(|| format!("write {name}"))?;
    Ok(Aside::KeyOnly(name))
}

/// Move `path` (an unreadable config.toml) to `config.toml.corrupt.<ts>` as it
/// is, owner-only, for when the key stores could not be read: the file may be
/// the only readable copy, so it is neither dropped nor reduced. The start that
/// finds the stores readable seals it ([`protect_copies`]).
pub fn set_aside_unchanged(dir: &Path, path: &Path) -> Result<Aside> {
    let name = free_name(dir, &format!("{CORRUPT_PREFIX}{}", stamp()), "");
    move_owner_only_no_follow(dir, path, &name)?;
    keyvault::sync_dir(dir);
    Ok(Aside::Deferred(name))
}

// Run by a test between the open of the file and its rename, to swap the path
// the way a racing process could.
#[cfg(all(unix, test))]
thread_local! {
    pub(crate) static BEFORE_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

// Run by a test after a mismatch was found, before the entry is moved back.
#[cfg(all(unix, test))]
thread_local! {
    pub(crate) static BEFORE_MOVE_BACK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// `renameat` that fails with `EEXIST` instead of replacing `to`:
/// `renameat2(RENAME_NOREPLACE)` on Linux, `renameatx_np(RENAME_EXCL)` on macOS.
/// Other unixes have no such call: they fail closed with `ENOTSUP`, no rename.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_noreplace(
    dir_fd: std::os::fd::RawFd,
    from: &std::ffi::CStr,
    to: &std::ffi::CStr,
) -> std::io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    // SAFETY: both names are NUL-terminated and `dir_fd` is an open descriptor
    // for the whole call.
    let rc =
        unsafe { libc::renameat2(dir_fd, from.as_ptr(), dir_fd, to.as_ptr(), RENAME_NOREPLACE) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn rename_noreplace(
    dir_fd: std::os::fd::RawFd,
    from: &std::ffi::CStr,
    to: &std::ffi::CStr,
) -> std::io::Result<()> {
    extern "C" {
        fn renameatx_np(
            fromfd: libc::c_int,
            from: *const libc::c_char,
            tofd: libc::c_int,
            to: *const libc::c_char,
            flags: libc::c_uint,
        ) -> libc::c_int;
    }
    const RENAME_EXCL: libc::c_uint = 0x0000_0004;
    // SAFETY: both names are NUL-terminated and `dir_fd` is an open descriptor
    // for the whole call.
    let rc = unsafe { renameatx_np(dir_fd, from.as_ptr(), dir_fd, to.as_ptr(), RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

// No atomic no-replace rename here, and a check followed by a plain rename is the
// replacing fallback this code refuses: fail closed, nothing is renamed.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn rename_noreplace(
    _dir_fd: std::os::fd::RawFd,
    _from: &std::ffi::CStr,
    _to: &std::ffi::CStr,
) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOTSUP))
}

/// Move `path` to `dir/target_name`, owner-only, without ever following a
/// symbolic link and without a second look-up of a path that could have been
/// swapped:
/// - the parent directory is opened once (`O_DIRECTORY | O_NOFOLLOW`, on its
///   canonical path, so a symlinked data directory still works) and the file is
///   opened relative to it with `O_NOFOLLOW`, then `fchmod`ed on that handle;
/// - the rename is `renameat` against that same directory handle;
/// - the moved entry is compared with the handle (device and inode). If the path
///   was swapped in between, what was moved is not the file that was opened: it
///   is moved back and the call is refused. A link is only ever moved, never
///   followed, so nothing outside the directory is touched.
#[cfg(unix)]
fn move_owner_only_no_follow(dir: &Path, path: &Path, target_name: &str) -> Result<()> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let file_name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?;
    let source = CString::new(file_name.as_bytes()).context("file name")?;
    let target = CString::new(target_name).context("target name")?;

    let real_dir = dir
        .canonicalize()
        .with_context(|| format!("resolve {}", dir.display()))?;
    // Only the file name is used below, so `path` must really sit in `dir`.
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let real_parent = parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?;
    anyhow::ensure!(
        real_parent == real_dir,
        "{} is not in {}; it is left where it is",
        path.display(),
        dir.display()
    );
    let dir_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(&real_dir)
        .with_context(|| format!("open {}", real_dir.display()))?;

    // SAFETY: `source` is a NUL-terminated string and `dir_handle` an open
    // descriptor for the whole call; a returned descriptor is owned by the File.
    let fd = unsafe {
        libc::openat(
            dir_handle.as_raw_fd(),
            source.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ELOOP) {
            anyhow::bail!(
                "{} is a symbolic link; it is left where it is",
                path.display()
            );
        }
        return Err(anyhow::Error::new(e).context(format!("open {}", path.display())));
    }
    // SAFETY: `fd` was just returned by `openat` and nothing else owns it.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file
        .metadata()
        .with_context(|| format!("look at {}", path.display()))?;
    anyhow::ensure!(
        meta.is_file(),
        "{} is not a regular file; it is left where it is",
        path.display()
    );
    // Owner-only BEFORE the move: the moved file is never there with a wider
    // mode, and a failed chmod leaves it unmoved.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("make {} owner-only", path.display()))?;

    #[cfg(test)]
    if let Some(hook) = BEFORE_RENAME.with(|h| h.borrow_mut().take()) {
        hook();
    }

    // Never replaces: an existing `target_name` makes this fail, both files kept.
    rename_noreplace(dir_handle.as_raw_fd(), &source, &target)
        .with_context(|| format!("move it to {target_name}"))?;

    // SAFETY: an all-zero `stat` is a valid out-parameter; the name is
    // NUL-terminated and `dir_handle` is open.
    let mut moved: libc::stat = unsafe { std::mem::zeroed() };
    let looked = unsafe {
        libc::fstatat(
            dir_handle.as_raw_fd(),
            target.as_ptr(),
            &mut moved,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    #[allow(clippy::unnecessary_cast)]
    let same = looked == 0
        && (moved.st_mode & libc::S_IFMT) == libc::S_IFREG
        && moved.st_dev as u64 == meta.dev() as u64
        && moved.st_ino as u64 == meta.ino() as u64;
    if !same {
        #[cfg(test)]
        if let Some(hook) = BEFORE_MOVE_BACK.with(|h| h.borrow_mut().take()) {
            hook();
        }
        // Without replacing: a config.toml created at the original path in the
        // meantime is never overwritten; both files are kept and the error says so.
        if let Err(e) = rename_noreplace(dir_handle.as_raw_fd(), &target, &source) {
            anyhow::bail!(
                "{} changed while it was being set aside and could not be moved back \
                 from {target_name} ({e}); both files are kept, nothing was overwritten",
                path.display()
            );
        }
        anyhow::bail!(
            "{} changed while it was being set aside; it is left where it is",
            path.display()
        );
    }
    Ok(())
}

/// Windows: the same refusals as the unix version (a link, a non-regular file, a
/// path outside `dir`), and a move that never replaces: `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` fails when the destination exists, unlike
/// `std::fs::rename`. Windows has no mode bits to narrow, and its handle-relative
/// rename is not used here, so the gap between the look and the move is narrowed
/// by the no-replace move, not closed.
#[cfg(windows)]
fn move_owner_only_no_follow(dir: &Path, path: &Path, target_name: &str) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, FILE_ATTRIBUTE_REPARSE_POINT};

    let file_name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?;
    let real_dir = dir
        .canonicalize()
        .with_context(|| format!("resolve {}", dir.display()))?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let real_parent = parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?;
    anyhow::ensure!(
        real_parent == real_dir,
        "{} is not in {}; it is left where it is",
        path.display(),
        dir.display()
    );

    let source = real_dir.join(file_name);
    let meta = std::fs::symlink_metadata(&source)
        .with_context(|| format!("look at {}", path.display()))?;
    // `is_symlink` only covers the name-surrogate reparse tags: any reparse point
    // (AppExecLink, cloud placeholder, junction, ...) is refused by its attribute.
    anyhow::ensure!(
        !meta.file_type().is_symlink()
            && meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "{} is a symbolic link or a reparse point; it is left where it is",
        path.display()
    );
    anyhow::ensure!(
        meta.is_file(),
        "{} is not a regular file; it is left where it is",
        path.display()
    );

    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let target = real_dir.join(target_name);
    // SAFETY: both buffers are NUL-terminated UTF-16 and live for the whole call.
    // No flag: an existing destination makes the call fail instead of replacing it.
    let moved = unsafe { MoveFileExW(wide(&source).as_ptr(), wide(&target).as_ptr(), 0) };
    if moved == 0 {
        let e = std::io::Error::last_os_error();
        return Err(anyhow::Error::new(e)
            .context(format!("move it to {target_name}; nothing was overwritten")));
    }
    Ok(())
}

// Neither unix nor windows: no safe move is known, so none is made.
#[cfg(not(any(unix, windows)))]
fn move_owner_only_no_follow(_dir: &Path, path: &Path, _target_name: &str) -> Result<()> {
    anyhow::bail!(
        "no safe way to set {} aside on this platform; it is left where it is",
        path.display()
    )
}

#[cfg(test)]
#[path = "config_copies_tests.rs"]
mod tests;
