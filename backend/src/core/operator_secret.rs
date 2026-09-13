//! KT-619 — handing the bootstrap secret to the operator, and to nobody else.
//!
//! The secret exists so a human can authorise the first enrolment. It is
//! therefore useless if the operator cannot read it, and dangerous if anyone
//! else can — which makes the delivery, not the minting, the interesting part.
//!
//! Two rules shape everything here:
//!
//! **The file is written before the row is committed.** A crash between the two
//! must never leave a secret in the database that the operator has no copy of:
//! that install would be permanently unable to enrol, with a hash claiming
//! otherwise. The reverse — a file whose secret authenticates nothing — is
//! merely untidy, and is cleaned up.
//!
//! **The path comes from server configuration, never from a request.** An API
//! that accepted a destination would let a caller aim the secret at somewhere it
//! can read.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Name of the file inside the operator's private directory. Fixed: there is no
/// search, no fallback, and no second place to look.
const SECRET_FILE: &str = "human-admin-secret";

/// Where the secret goes: the same private directory the database lives in, on
/// the host or in the Docker volume. `config_dir()` already resolves both.
pub fn secret_path() -> Result<PathBuf> {
    Ok(crate::core::config::config_dir()?.join(SECRET_FILE))
}

/// Write the secret so that only its owner can read it, and so that a reader
/// either sees the whole thing or nothing.
///
/// Written to a temporary file in the same directory, fsynced, then renamed:
/// a rename within a directory is atomic, so no reader ever observes a partial
/// secret. The mode is set at creation rather than after, so the file is never
/// briefly world-readable.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    let dir = path
        .parent()
        .context("the operator secret path has no parent directory")?;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;

    let temporary = dir.join(format!(".{SECRET_FILE}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("cannot create {}", temporary.display()))?;
    file.write_all(contents.as_bytes())?;
    file.write_all(b"\n")?;
    // Durable before the rename, or a crash could leave an empty file with the
    // right name — which reads as "delivered" and is not.
    file.sync_all()?;
    drop(file);

    fs::rename(&temporary, path).with_context(|| format!("cannot place {}", path.display()))?;
    // Fsync the directory so the rename itself survives a crash.
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// Refuse a file that is not exactly what we wrote: a real file, owned by this
/// process's user, readable by nobody else.
///
/// A symlink is refused rather than followed — otherwise the "private" file
/// could point anywhere, and its permissions would be somebody else's.
fn verify_private(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("cannot inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "{} is a symlink; the operator secret is never followed through one",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "{} is readable beyond its owner (mode {mode:o}); refusing to treat it as private",
                path.display()
            );
        }
        // Owner check: a file this process cannot have created is not ours to
        // trust, whatever its mode says.
        use std::os::unix::fs::MetadataExt;
        let us = unsafe { libc::geteuid() };
        if metadata.uid() != us {
            bail!(
                "{} belongs to uid {} rather than {us}",
                path.display(),
                metadata.uid()
            );
        }
    }
    Ok(())
}

/// Outcome of a bootstrap attempt, for the caller to report without ever
/// carrying the secret itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub path: PathBuf,
}

/// Mint the bootstrap secret and hand it to the operator.
///
/// File first, row second. If the row cannot be committed the file is removed,
/// because a secret nobody can use is better than a secret that exists only in
/// a hash. If the file cannot be written, nothing is committed at all.
pub fn bootstrap(conn: &rusqlite::Connection) -> Result<Delivered> {
    if crate::db::human_credentials::admin_secret_exists(conn)? {
        bail!("the admin secret already exists; rotate it rather than minting a second");
    }
    let path = secret_path()?;
    // A leftover from a failed attempt must not be mistaken for a live secret.
    if path.exists() {
        verify_private(&path)
            .context("an operator secret file already exists and is not private")?;
    }

    let transaction = conn.unchecked_transaction()?;
    let secret =
        crate::db::human_credentials::create_admin_secret(&transaction, &path.to_string_lossy())?;

    write_private(&path, secret.expose())
        .context("the admin secret was not written; nothing was committed")?;

    if let Err(error) = transaction.commit() {
        // The operator has a file whose secret authenticates nothing. Remove it
        // rather than leave something that looks delivered.
        let _ = fs::remove_file(&path);
        return Err(error).context("the admin secret was rolled back and its file removed");
    }

    verify_private(&path)?;
    Ok(Delivered { path })
}

/// Mint the bootstrap if this install has none, and say where it landed.
///
/// Idempotent, and deliberately quiet on the common path: `None` means one
/// already exists. Called at boot so a fresh install delivers the secret
/// without the operator having to know a command — the feature was unusable
/// while this had no caller, and a mechanism nobody can start is not a
/// mechanism.
pub fn ensure_bootstrap(conn: &rusqlite::Connection) -> Result<Option<PathBuf>> {
    if crate::db::human_credentials::admin_secret_exists(conn)? {
        return Ok(None);
    }
    Ok(Some(bootstrap(conn)?.path))
}

/// Rotate the bootstrap secret and deliver the new one.
///
/// Rotation and recovery are the SAME operation. Whether the operator still
/// holds the old secret and wants a new one, or lost the file entirely, the only
/// way forward is a fresh secret written to the one place they can read. Every
/// credential already enrolled keeps working: only the power to enrol NEW ones
/// moves.
///
/// Where `bootstrap` DELETES the file if the row cannot be committed, this puts
/// the previous one back. There was nothing to lose before; here there is, and
/// a failed rotation that takes the operator's only copy with it turns a
/// recoverable install into a dead one.
pub fn rotate(conn: &rusqlite::Connection) -> Result<Delivered> {
    if !crate::db::human_credentials::admin_secret_exists(conn)? {
        bail!("there is no admin secret to rotate; a boot with none mints one");
    }
    let path = secret_path()?;

    // Keep the current bytes only if the file is genuinely ours and private.
    // A secret already readable by others is already given away, and restoring
    // it after a failed rotation would restore exactly what was being rotated
    // away from. An absent file lands here too, which is the recovery case.
    let previous = verify_private(&path)
        .ok()
        .and_then(|()| fs::read_to_string(&path).ok());

    let transaction = conn.unchecked_transaction()?;
    let secret =
        crate::db::human_credentials::rotate_admin_secret(&transaction, &path.to_string_lossy())?;

    write_private(&path, secret.expose())
        .context("the rotated admin secret was not written; nothing was committed")?;

    if let Err(error) = transaction.commit() {
        match previous.as_deref().map(str::trim) {
            Some(old) if !old.is_empty() => {
                let _ = write_private(&path, old);
            }
            // Nothing usable to restore: leaving the new secret in place would
            // look delivered while authenticating nothing.
            _ => {
                let _ = fs::remove_file(&path);
            }
        }
        return Err(error).context("the admin secret rotation was rolled back");
    }

    verify_private(&path)?;
    Ok(Delivered { path })
}

/// The file an operator creates to ask for a new admin secret.
///
/// Creating it proves write access to the private directory — the same access
/// that already owns the database and the secret itself. That is the right
/// proof for a recovery: it is available precisely to the person who can no
/// longer authenticate, and to nobody who cannot already take everything.
const RECOVERY_REQUEST_FILE: &str = "recover-admin-secret";

pub fn recovery_request_path() -> Result<PathBuf> {
    Ok(crate::core::config::config_dir()?.join(RECOVERY_REQUEST_FILE))
}

/// Honour a recovery request left in the private directory, and consume it.
///
/// This is the way back from a lost secret: the HTTP surface cannot help there
/// by design — a route that re-delivers the admin secret to whoever asks is the
/// hole this whole lot exists to close — so the way in is the filesystem.
///
/// The request is removed only once the new secret is committed and on disk. A
/// failed attempt is therefore retried on the next boot rather than silently
/// swallowed; the cost is that a crash between the commit and the removal
/// rotates a second time, which delivers another usable secret rather than
/// losing one.
pub fn recover_if_requested(conn: &rusqlite::Connection) -> Result<Option<PathBuf>> {
    let request = recovery_request_path()?;
    let Ok(metadata) = fs::symlink_metadata(&request) else {
        return Ok(None);
    };
    if !metadata.file_type().is_file() {
        bail!(
            "{} is not a regular file; refusing to read it as a recovery request",
            request.display()
        );
    }

    // An install that never bootstrapped has nothing to rotate, and recovery is
    // then simply the bootstrap it never got.
    let delivered = if crate::db::human_credentials::admin_secret_exists(conn)? {
        rotate(conn)?
    } else {
        bootstrap(conn)?
    };

    fs::remove_file(&request).with_context(|| {
        format!(
            "the admin secret was rotated but {} could not be removed; the next boot will rotate again",
            request.display()
        )
    })?;
    Ok(Some(delivered.path))
}

/// Read the secret back, for an operator-side check that delivery worked.
/// Refuses a file that is not private, rather than returning what it holds.
pub fn read_delivered() -> Result<crate::db::human_credentials::Secret> {
    let path = secret_path()?;
    verify_private(&path)?;
    let contents =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        bail!("{} is empty", path.display());
    }
    Ok(crate::db::human_credentials::Secret::new(trimmed))
}

#[cfg(test)]
mod tests;
