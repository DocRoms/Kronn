//! KT-1742 — what a lane's warm-up learned about each Rust root, kept per root.
//!
//! A repository may hold several Rust workspace roots (Kronn's own has
//! `backend/` and `desktop/src-tauri/`). They do not warm alike: one builds from
//! the seed in minutes, another needs system libraries the host does not have.
//! A single lane-wide marker made the second one's failure erase the first one's
//! success and made every relaunch (and every new lane) run it again.
//!
//! Two records fix that, both keyed on **the root, its `Cargo.lock` hash and the
//! lane's compiler environment** (the three things that change what a build of
//! that root does):
//!
//! * a [`RootMark`] in the lane's own marker (`.kronn-warm.json`): the root was
//!   built (or failed) for that key in this lane, so a relaunch leaves it alone;
//! * a failure cache beside the seed ([`FAILURES_FILE`], next to the seed's
//!   stamp): a root that FAILED for a key is not tried again by any later lane of
//!   the project until its lock or its environment changes. A timeout and a
//!   cancellation are never cached: they say nothing about the root.
//!
//! Only Kronn writes the cache, outside every worker's write roots.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::project_toolchains::text_hash;

/// The failure cache, in the seed's target directory (not in `debug/`, which a
/// lane clones).
pub const FAILURES_FILE: &str = ".kronn-warm-failures.json";

/// How a root ended in a lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RootState {
    Built,
    Failed { reason: String },
}

/// One root's entry of a lane's marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootMark {
    /// The root's directory relative to the repository (`.`), or `command`.
    pub path: String,
    /// [`root_key`] at the time.
    pub key: String,
    #[serde(flatten)]
    pub state: RootState,
    pub at_unix: u64,
}

impl RootMark {
    pub fn new(path: &str, key: &str, state: RootState) -> Self {
        Self {
            path: path.to_string(),
            key: key.to_string(),
            state,
            at_unix: unix_now(),
        }
    }
}

/// What a root's build depends on: where it is, the lock it builds from and the
/// compiler environment of the lane (sorted, so the order is not a difference).
pub fn root_key(label: &str, lock_hash: Option<&str>, compiler_env: &[(String, String)]) -> String {
    let mut env = compiler_env.to_vec();
    env.sort();
    let env: Vec<String> = env
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    text_hash(&format!(
        "{label}\n{}\n{}",
        lock_hash.unwrap_or("-"),
        env.join("\n")
    ))
}

/// Replace (by path) or add `mark` in `marks`.
pub fn upsert(marks: &mut Vec<RootMark>, mark: RootMark) {
    match marks.iter_mut().find(|known| known.path == mark.path) {
        Some(known) => *known = mark,
        None => marks.push(mark),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedFailure {
    key: String,
    reason: String,
    at_unix: u64,
}

type Failures = BTreeMap<String, CachedFailure>;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn read(seed_target: &Path) -> Failures {
    std::fs::read(seed_target.join(FAILURES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(seed_target: &Path, failures: &Failures) -> io::Result<()> {
    let path = seed_target.join(FAILURES_FILE);
    if failures.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let staging = seed_target.join(format!("{FAILURES_FILE}.tmp"));
    std::fs::write(
        &staging,
        serde_json::to_vec(failures).map_err(io::Error::other)?,
    )?;
    std::fs::rename(staging, path)
}

/// Why `label` is not tried again for `key`: it failed for exactly this key
/// before.
pub fn cached_failure(seed_target: &Path, label: &str, key: &str) -> Option<String> {
    read(seed_target)
        .remove(label)
        .filter(|failure| failure.key == key)
        .map(|failure| failure.reason)
}

/// Remember that `label` failed for `key`. Never raises; the seed's directory
/// is not created for it (a root with no seed has nowhere to be remembered, and
/// its lane's own mark still keeps a relaunch from retrying).
pub fn remember_failure(seed_target: &Path, label: &str, key: &str, reason: &str) {
    if !seed_target.is_dir() {
        return;
    }
    let mut failures = read(seed_target);
    failures.insert(
        label.to_string(),
        CachedFailure {
            key: key.to_string(),
            reason: reason.to_string(),
            at_unix: unix_now(),
        },
    );
    if let Err(error) = write(seed_target, &failures) {
        tracing::warn!(seed = %seed_target.display(), "warm failure not cached: {error}");
    }
}

/// A root that built has no failure to remember any more.
pub fn forget_failure(seed_target: &Path, label: &str) {
    let mut failures = read(seed_target);
    if failures.remove(label).is_some() {
        if let Err(error) = write(seed_target, &failures) {
            tracing::warn!(seed = %seed_target.display(), "warm failure not cleared: {error}");
        }
    }
}

#[cfg(test)]
#[path = "task_worker_root_marks_test.rs"]
mod tests;
