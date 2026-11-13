//! KT-1742 — what a repository builds with, found from its manifests, never from
//! the layout of one particular repository.
//!
//! A lane warm-up used to look for a `Cargo.toml` at the worktree root or in
//! `backend/` and to seed from `<repo>/target`. That is Kronn's own layout. This
//! module finds every Rust workspace root anywhere in a checkout (a crate in
//! `redis-flush/`, one in `desktop/src-tauri/`), says where each one's `target/`
//! really is (`cargo metadata`, which honours `[build] target-dir` and
//! `CARGO_TARGET_DIR`), and answers the one question the lane's C-compiler
//! default depends on: does this project build C through the `cc` crate.
//!
//! It is deliberately small and free of lane concepts so another consumer
//! (KT-1747's generic toolchain detection) can reuse [`rust_roots`] as it is.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Directory names a manifest search never enters: dependency and build trees,
/// VCS data, and Kronn's own worktrees.
const SKIPPED_DIRS: [&str; 5] = ["target", "node_modules", "vendor", ".git", ".kronn"];
/// How deep below the repository root a manifest is looked for.
const MAX_DEPTH: usize = 4;
/// A repository with more Rust roots than this is not warmed crate by crate.
const MAX_ROOTS: usize = 8;
/// `cargo metadata --no-deps` reads manifests only; this is a hang guard.
const METADATA_TIMEOUT: Duration = Duration::from_secs(60);

/// A Rust workspace root (or a standalone crate) of a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustRoot {
    /// The directory of its `Cargo.toml`, relative to the repository root
    /// (empty for the root itself).
    pub rel_dir: PathBuf,
    /// The same directory, absolute.
    pub manifest_dir: PathBuf,
}

impl RustRoot {
    /// The relative directory as shown in an event: `.` for the root.
    pub fn label(&self) -> String {
        if self.rel_dir.as_os_str().is_empty() {
            ".".to_string()
        } else {
            self.rel_dir.display().to_string()
        }
    }

    /// The same root in another checkout of the repository (the main checkout
    /// of a worktree).
    pub fn in_checkout(&self, checkout: &Path) -> PathBuf {
        checkout.join(&self.rel_dir)
    }

    /// The SHA-256 of its `Cargo.lock`, when it has one.
    pub fn lock_hash(&self) -> Option<String> {
        file_hash(&self.manifest_dir.join("Cargo.lock"))
    }

    /// Does it have a library target: the default `src/lib.rs`, or a `[lib]`
    /// table (a custom `path`).
    pub fn has_lib_target(&self) -> bool {
        has_lib_target(&self.manifest_dir)
    }
}

/// Every Rust workspace root of the checkout at `repo_root`, shallowest and
/// alphabetically first: a directory with a `Cargo.toml` is a root and is not
/// searched further (its members belong to it), any other directory is.
pub fn rust_roots(repo_root: &Path) -> Vec<RustRoot> {
    let mut roots = Vec::new();
    collect_roots(repo_root, repo_root, 0, &mut roots);
    roots.sort_by(|a, b| a.rel_dir.cmp(&b.rel_dir));
    roots.truncate(MAX_ROOTS);
    roots
}

fn collect_roots(repo_root: &Path, dir: &Path, depth: usize, roots: &mut Vec<RustRoot>) {
    if dir.join("Cargo.toml").is_file() {
        roots.push(RustRoot {
            rel_dir: dir.strip_prefix(repo_root).unwrap_or(dir).to_path_buf(),
            manifest_dir: dir.to_path_buf(),
        });
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref())
        })
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        collect_roots(repo_root, &child, depth + 1, roots);
    }
}

/// Does the package at `manifest_dir` have a library target.
pub fn has_lib_target(manifest_dir: &Path) -> bool {
    manifest_dir.join("src").join("lib.rs").is_file()
        || std::fs::read_to_string(manifest_dir.join("Cargo.toml"))
            .is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "[lib]"))
}

/// Does any Rust root of the checkout list the `cc` crate in its `Cargo.lock`:
/// the project compiles C (or assembly) from a build script, so the C compiler's
/// environment is part of its dependencies' fingerprints.
pub fn builds_c_through_cc(repo_root: &Path) -> bool {
    rust_roots(repo_root).iter().any(|root| {
        std::fs::read_to_string(root.manifest_dir.join("Cargo.lock"))
            .ok()
            .and_then(|text| crate::core::seed_prune::locked_package_names([text.as_str()]))
            .is_some_and(|names| names.contains("cc"))
    })
}

/// The hash of the roots' lock files: the single root's own `Cargo.lock` hash
/// (what a one-crate repository always had), or a hash over every root's
/// `<dir>:<hash>` when there are several. `None` when no root has a lock file.
pub fn combined_lock_hash(roots: &[RustRoot]) -> Option<String> {
    let hashes: Vec<(String, Option<String>)> = roots
        .iter()
        .map(|root| (root.label(), root.lock_hash()))
        .collect();
    if hashes.iter().all(|(_, hash)| hash.is_none()) {
        return None;
    }
    if let [(_, Some(hash))] = hashes.as_slice() {
        return Some(hash.clone());
    }
    let mut digest = Sha256::new();
    for (label, hash) in &hashes {
        digest.update(format!("{label}:{}\n", hash.as_deref().unwrap_or("-")));
    }
    Some(hex(&digest.finalize()))
}

/// SHA-256 of a file, hex, or `None` when it cannot be read.
pub fn file_hash(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| hex(&Sha256::digest(&bytes)))
}

/// SHA-256 of a text, hex.
pub fn text_hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ─── cargo metadata ───────────────────────────────────────────────────────

/// What a crate's build needs to know about where Cargo puts its artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoMetadata {
    /// Where the crate's builds go: `target_directory` of `cargo metadata`.
    pub target_directory: PathBuf,
    pub workspace_root: PathBuf,
}

/// Resolves a crate directory to its [`CargoMetadata`]; injectable so a test
/// needs no Cargo.
pub type MetadataResolver<'a> = &'a dyn Fn(&Path) -> Result<CargoMetadata, String>;

/// Parse the output of `cargo metadata --format-version 1 --no-deps`.
pub fn parse_metadata(json: &[u8]) -> Result<CargoMetadata, String> {
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|error| format!("not JSON: {error}"))?;
    let path = |key: &str| {
        value
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| format!("no `{key}` in the metadata"))
    };
    Ok(CargoMetadata {
        target_directory: path("target_directory")?,
        workspace_root: path("workspace_root")?,
    })
}

/// `cargo metadata --format-version 1 --no-deps --offline` run in `manifest_dir`,
/// without a `CARGO_TARGET_DIR` of this process's own: the answer is the
/// directory the crate's owner builds into.
pub fn host_cargo_metadata(manifest_dir: &Path) -> Result<CargoMetadata, String> {
    let mut command = crate::core::cmd::sync_cmd("cargo", crate::core::child_env::ChildRoute::Tool);
    command
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(manifest_dir)
        .env_remove("CARGO_TARGET_DIR")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_end(&mut bytes);
        }
        bytes
    });
    let deadline = Instant::now() + METADATA_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("cargo metadata timed out".into());
            }
            Err(error) => return Err(format!("cargo metadata: {error}")),
        }
    };
    let stdout = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("cargo metadata failed ({status})"));
    }
    parse_metadata(&stdout)
}

#[cfg(test)]
#[path = "project_toolchains_test.rs"]
mod tests;
