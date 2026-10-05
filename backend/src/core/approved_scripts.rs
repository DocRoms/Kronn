//! Repository scripts an Exec step runs, pinned by content hash (KT-918).
//!
//! The step declares its entry script and the modules it loads as paths
//! relative to the repository of the workflow's home project (the carrying
//! repository, ADR-005 §Workflows). Each path carries the approved SHA-256,
//! so the hashes are part of the workflow definition and of its approval
//! fingerprint. Before the command runs, every file is read once from the
//! carrying repository, checked against its hash and written to a copy under
//! the run's artifacts directory; the command then runs from that copy. The
//! bytes checked are the bytes executed, so a change between the check and
//! the interpreter's read cannot slip through.
//!
//! Not a general isolation: a script can still load an absolute path, a path
//! it builds from `KRONN_WORKTREE`, or a package found by the interpreter's
//! own search (Node walks up `node_modules` from the copy, Python reads its
//! site-packages). Only the declared files are verified.

use std::path::{Component, Path, PathBuf};

use crate::models::{ExecScriptFile, ExecScriptFileState, ExecScriptFileStatus};

/// Declared files per step.
pub const MAX_FILES: usize = 64;
/// Largest declared file: scripts and their modules, not data.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Per-launch variable: the run's working directory, for the files the
/// script operates on (its own cwd is the approved copy).
pub const WORKTREE_ENV: &str = "KRONN_WORKTREE";
/// Per-launch variable: the approved copy the command runs from.
pub const SCRIPTS_DIR_ENV: &str = "KRONN_APPROVED_SCRIPTS_DIR";
/// Sub-directory of the run's artifacts directory holding the copies.
const COPY_ROOT: &str = "approved-scripts";

/// The path as stored in the copy: its normal components joined by `/`.
fn normalized(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed != path || path.contains('\\') || path.contains('\0') {
        return Err(format!(
            "script file `{path}`: use a plain repository-relative path with `/` separators"
        ));
    }
    crate::core::repository_resources::validate_relative(path)
        .map_err(|_| format!("script file `{path}` must be relative to the repository"))?;
    let parts: Vec<String> = Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return Err(format!("script file `{path}` names no file"));
    }
    Ok(parts.join("/"))
}

fn valid_hash(hash: &str) -> bool {
    hash.is_empty()
        || (hash.len() == 64
            && hash
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// Checks that need no filesystem: count, path shape, duplicates, hash format.
pub fn validate_declaration(files: &[ExecScriptFile]) -> Result<(), String> {
    if files.len() > MAX_FILES {
        return Err(format!(
            "too many script files ({}, max {MAX_FILES})",
            files.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for file in files {
        let path = normalized(&file.path)?;
        if !seen.insert(path.clone()) {
            return Err(format!("script file `{path}` is declared twice"));
        }
        if !valid_hash(&file.sha256) {
            return Err(format!(
                "script file `{path}`: sha256 must be 64 lowercase hex characters or empty"
            ));
        }
    }
    Ok(())
}

/// Reads one declared file from the carrying repository, refusing a path or
/// symlink that leaves it, a missing file and anything but a regular file.
pub fn read_declared(root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let relative = normalized(path)?;
    let resolved = crate::core::fs_guard::resolve_contained_read(root, Path::new(&relative))
        .map_err(|_| format!("script file `{relative}` resolves outside the repository"))?;
    let metadata = std::fs::metadata(&resolved)
        .map_err(|error| format!("script file `{relative}` cannot be read: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("script file `{relative}` is not a regular file"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "script file `{relative}` is larger than {MAX_FILE_BYTES} bytes"
        ));
    }
    let bytes = std::fs::read(&resolved)
        .map_err(|error| format!("script file `{relative}` cannot be read: {error}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "script file `{relative}` is larger than {MAX_FILE_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

/// Save-time check: every file exists inside the repository; an empty hash is
/// pinned to the current content (the person saving approves it).
pub fn validate_and_pin(root: &Path, files: &mut [ExecScriptFile]) -> Result<(), String> {
    validate_declaration(files)?;
    for file in files.iter_mut() {
        let bytes = read_declared(root, &file.path)?;
        if file.sha256.is_empty() {
            file.sha256 = crate::core::repository_resources::sha256(&bytes);
        }
    }
    Ok(())
}

/// The same checks without approving anything: empty hashes stay empty.
pub fn validate_files(root: &Path, files: &[ExecScriptFile]) -> Result<(), String> {
    validate_declaration(files)?;
    for file in files {
        read_declared(root, &file.path)?;
    }
    Ok(())
}

/// Each declared file against its approved hash, for the step editor.
pub fn status(root: Option<&Path>, files: &[ExecScriptFile]) -> Vec<ExecScriptFileStatus> {
    files
        .iter()
        .map(|file| {
            let read = match root {
                Some(root) => read_declared(root, &file.path),
                None => Err("the workflow has no project, so no repository to read".into()),
            };
            match read {
                Ok(bytes) => {
                    let current = crate::core::repository_resources::sha256(&bytes);
                    let state = if file.sha256.is_empty() {
                        ExecScriptFileState::Pending
                    } else if file.sha256 == current {
                        ExecScriptFileState::Approved
                    } else {
                        ExecScriptFileState::Changed
                    };
                    ExecScriptFileStatus {
                        path: file.path.clone(),
                        state,
                        current_sha256: Some(current),
                        error: None,
                    }
                }
                Err(error) => ExecScriptFileStatus {
                    path: file.path.clone(),
                    state: ExecScriptFileState::Invalid,
                    current_sha256: None,
                    error: Some(error),
                },
            }
        })
        .collect()
}

/// Where one step's approved copy lives inside the run's artifacts directory.
pub fn copy_dir(artifacts_dir: &Path, step_name: &str) -> PathBuf {
    let key = crate::core::repository_resources::sha256(step_name.as_bytes());
    artifacts_dir
        .join(COPY_ROOT)
        .join(key.chars().take(16).collect::<String>())
}

/// Verifies every declared file against its approved hash, then writes the
/// verified bytes to `dest` (replaced). Nothing is written when one file does
/// not match; the error names it.
pub fn prepare_copy(root: &Path, files: &[ExecScriptFile], dest: &Path) -> Result<(), String> {
    validate_declaration(files)?;
    let mut verified = Vec::with_capacity(files.len());
    for file in files {
        let relative = normalized(&file.path)?;
        if file.sha256.is_empty() {
            return Err(format!(
                "`{relative}` has no approved hash; approve it in the step editor and save"
            ));
        }
        let bytes = read_declared(root, &relative)?;
        let current = crate::core::repository_resources::sha256(&bytes);
        if current != file.sha256 {
            return Err(format!(
                "`{relative}` changed since approval (approved {}, now {}); approve the new content in the step editor before running",
                crate::core::repository_resources::fingerprint(&file.sha256),
                crate::core::repository_resources::fingerprint(&current),
            ));
        }
        verified.push((relative, bytes));
    }
    match std::fs::remove_dir_all(dest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "the previous approved copy cannot be removed: {error}"
            ))
        }
    }
    std::fs::create_dir_all(dest)
        .map_err(|error| format!("the approved copy cannot be created: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o700));
    }
    for (relative, bytes) in verified {
        let target = dest.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("`{relative}` cannot be copied: {error}"))?;
        }
        std::fs::write(&target, bytes)
            .map_err(|error| format!("`{relative}` cannot be copied: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "approved_scripts_test.rs"]
mod approved_scripts_test;
