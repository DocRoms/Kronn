//! Atomic file replacement: write a sibling temp file, then rename it over
//! the target, so a reader never sees a half-written file.

use std::path::{Path, PathBuf};

/// Write content to a file atomically: write to a temp sibling then rename.
/// This prevents agents from reading a partially-written config file.
pub fn write_atomic_temp(target: &Path, content: &[u8]) -> Result<PathBuf, String> {
    // A fixed `<target>.tmp` name is unsafe in a user-controlled project: a
    // pre-created symlink at that path makes `fs::write` follow it and mutate
    // an arbitrary file outside the project. Use an unpredictable sibling and
    // `create_new` so an existing filesystem object is never followed.
    let tmp = target.with_extension(format!("kronn-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temp_file = options
        .open(&tmp)
        .map_err(|e| format!("Failed to create temp {}: {e}", tmp.display()))?;
    if let Err(e) = std::io::Write::write_all(&mut temp_file, content) {
        drop(temp_file);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Failed to write temp {}: {e}", tmp.display()));
    }
    drop(temp_file);
    Ok(tmp)
}

fn commit_atomic_temp(tmp: &Path, target: &Path) -> Result<(), String> {
    std::fs::rename(tmp, target).map_err(|e| {
        // Clean up temp file on rename failure
        let _ = std::fs::remove_file(tmp);
        format!(
            "Failed to rename {} → {}: {}",
            tmp.display(),
            target.display(),
            e
        )
    })
}

pub fn atomic_write(target: &Path, content: &str) -> Result<(), String> {
    atomic_write_bytes(target, content.as_bytes())
}

pub fn atomic_write_bytes(target: &Path, content: &[u8]) -> Result<(), String> {
    let tmp = write_atomic_temp(target, content)?;
    commit_atomic_temp(&tmp, target)
}

/// Atomically replace a file only if it is still byte-identical to what the
/// merge parsed (`Some`) or is still absent (`None`). The temp is fully written
/// first, keeping the compare→rename window as small as the portable filesystem
/// API allows. This prevents a normal concurrent CLI/user edit from being
/// silently lost while Kronn is serializing a merged config.
pub fn atomic_write_if_unchanged(
    target: &Path,
    content: &str,
    observed: Option<&[u8]>,
) -> Result<(), String> {
    let tmp = write_atomic_temp(target, content.as_bytes())?;
    let unchanged = match (observed, std::fs::read(target)) {
        (Some(expected), Ok(current)) => current == expected,
        (None, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => true,
        _ => false,
    };
    if !unchanged {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{} changed concurrently while its MCP config was being merged; refusing overwrite",
            target.display()
        ));
    }
    commit_atomic_temp(&tmp, target)
}

/// Atomically replace a file only if it is still byte-identical to `observed`,
/// while preserving the target's existing permissions. Permissions are copied
/// to the temporary sibling *before* the rename, so there is no post-rename
/// window where a normal 0644 document becomes the temp file's defensive 0600.
/// The byte comparison is the final operation before rename, minimizing the
/// remaining portable check-to-commit race window.
#[cfg(any(windows, any(test, feature = "test-support")))] // the unix `fs_guard` replaces through a held directory instead
pub fn atomic_write_if_unchanged_preserving_permissions(
    target: &Path,
    content: &str,
    observed: &[u8],
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(target).map_err(|e| {
        format!(
            "Failed to stat {} before atomic write: {e}",
            target.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is a symlink; refusing atomic permission-preserving write",
            target.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "{} is not a regular file; refusing atomic permission-preserving write",
            target.display()
        ));
    }
    let permissions = metadata.permissions();
    let tmp = write_atomic_temp(target, content.as_bytes())?;
    if let Err(e) = std::fs::set_permissions(&tmp, permissions) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "Failed to preserve permissions for {} before atomic rename: {e}",
            target.display()
        ));
    }
    let current = match std::fs::read(target) {
        Ok(current) => current,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "Failed to read {} before atomic rename: {e}",
                target.display()
            ));
        }
    };
    if current != observed {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{} changed concurrently while its audit artifact was being sanitized; refusing overwrite",
            target.display()
        ));
    }
    commit_atomic_temp(&tmp, target)
}
