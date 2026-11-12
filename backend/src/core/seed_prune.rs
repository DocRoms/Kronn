//! KT-1691 — keep the seed build dir to its latest build.
//!
//! Every worker lane starts as a copy-on-write clone of the main checkout's
//! `target/debug` (KT-1250, see `task_worker_lane_warm`). Cargo never removes an
//! artifact it no longer needs: on 2026-11-11 the seed held 174 GB, 91 GB of it
//! stale test binaries and objects of builds months old, and every lane cloned
//! all of it, then rewrote part of it (which is real disk). So the seed is pruned
//! *before* a lane clones it.
//!
//! What "stale" means, decided from Cargo's own bookkeeping and never from a
//! file's age alone (an unchanged dependency keeps its months-old mtime and is
//! exactly what a lane wants):
//!
//! * a **unit** is a `.fingerprint/<package>-<hash>` directory; its **kind** is
//!   the artifact it describes (`lib-serde`, `bin-kronn`, `test-lib-kronn`...) and
//!   its `<kind>.json` is the fingerprint Cargo itself compares: `rustc`,
//!   `features`, `profile`, `target`, `path`, `rustflags`, `compile_kind`...
//!   Units of one package that differ by any of these are **distinct valid
//!   variants** (`hashbrown` with `["raw"]` and with its default features, a
//!   dependency built for the host and for the target) and one build needs
//!   several of them: a package is never reduced to its newest unit. Only units
//!   with the *same identity* (package, kind, features, profile, target, path,
//!   config, rustflags, compile kind) are one unit built several times, and the
//!   one written last is the current one. The older ones are leftovers (a
//!   dependency changed underneath them) and go with every file of the same hash
//!   (`deps/*-<hash>.*`, `build/<package>-<hash>/`);
//! * a unit built by a **different `rustc`** than the units written last is stale
//!   whatever its age (a toolchain upgrade): Cargo rebuilds it anyway. When the
//!   last writes disagree on the compiler, nothing is judged by it;
//! * a unit whose fingerprint cannot be read or parsed, or whose write time
//!   cannot be read, is never judged: it stays. So does a unit that carries no
//!   `target` and no `path` (the run of a build script, zeroed by Cargo): it
//!   cannot be told apart from the run of another version of the package;
//! * a unit of a package the main checkout's `Cargo.lock` no longer lists (a
//!   removed dependency) is stale whatever its age, when the caller gives the
//!   locked package names.
//!
//! Pruning cannot ping-pong: what it removes is what a build of the same
//! configuration has already superseded, never a variant the next build needs.
//!
//! `incremental/` is left alone: its directories are keyed by crate name, where
//! a library and its tests share a name and are both current.
//!
//! Never while a build uses the dir: Cargo's own build lock is taken exclusively,
//! and a held lock makes the prune a no-op that says so.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::core::disk_usage::{measure_bounded, MeasureBudget};

const FINGERPRINT_DIR: &str = ".fingerprint";
const DEPS_DIR: &str = "deps";
const BUILD_DIR: &str = "build";
/// Cargo's advisory lock on a profile directory, held for a build's duration.
pub const CARGO_BUILD_LOCK: &str = ".cargo-lock";
/// The compiler of the latest build is the one every unit written within this
/// of the newest write agrees on.
pub const SAME_BUILD_WINDOW: Duration = Duration::from_secs(3600);
/// A hash in Cargo's file names: 16 lowercase hex digits.
const HASH_LEN: usize = 16;

/// What a prune removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneReport {
    /// Stale units (fingerprint directories) removed.
    pub removed_units: usize,
    /// Files and directories removed, fingerprints included.
    pub removed_entries: usize,
    /// Apparent bytes of what was removed.
    pub removed_bytes: u64,
}

/// What a prune attempt did.
#[derive(Debug, PartialEq, Eq)]
pub enum PruneOutcome {
    Pruned(PruneReport),
    /// Nothing was touched, and why (a build holds the lock, no `debug/`...).
    Skipped(String),
}

/// The `name = "..."` of every `[[package]]` in the given `Cargo.lock` texts,
/// with `-` and `_` unified, or `None` when no text lists a package (an unread
/// lock must never make every unit look removed).
pub fn locked_package_names<'a>(
    lock_texts: impl IntoIterator<Item = &'a str>,
) -> Option<HashSet<String>> {
    let mut names = HashSet::new();
    for text in lock_texts {
        let mut in_package = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_package = line == "[[package]]";
            } else if in_package {
                if let Some(rest) = line.strip_prefix("name = \"") {
                    if let Some(name) = rest.strip_suffix('"') {
                        names.insert(unify(name));
                    }
                }
            }
        }
    }
    (!names.is_empty()).then_some(names)
}

fn unify(name: &str) -> String {
    name.replace('-', "_")
}

/// `(package, hash)` of a `<package>-<16 hex>` name.
fn split_unit_name(name: &str) -> Option<(&str, &str)> {
    let (package, hash) = name.rsplit_once('-')?;
    (!package.is_empty() && is_hash(hash)).then_some((package, hash))
}

fn is_hash(text: &str) -> bool {
    text.len() == HASH_LEN
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The hash a `deps/` entry belongs to: `libserde-<hash>.rlib`,
/// `kronn-<hash>`, `kronn-<hash>.2x.rcgu.o`, `kronn-<hash>.dSYM`...
fn hash_of_deps_entry(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    (0..name.len().saturating_sub(HASH_LEN))
        .rev()
        .filter(|&dash| bytes[dash] == b'-')
        .find_map(|dash| {
            let hash = name.get(dash + 1..dash + 1 + HASH_LEN)?;
            let end = dash + 1 + HASH_LEN;
            (is_hash(hash) && (end == name.len() || bytes[end] == b'.')).then_some(hash)
        })
}

struct Unit {
    package: String,
    kind: String,
    dir_name: String,
    hash: String,
    written: SystemTime,
    fingerprint: Fingerprint,
}

/// What of a unit's `<kind>.json` the prune reads.
struct Fingerprint {
    /// Cargo's hash of the compiler that built it.
    rustc: String,
    /// The configuration of the unit; `None` when it carries no `target` and no
    /// `path` (the run of a build script) and so identifies nothing.
    identity: Option<String>,
}

/// Read `<kind>.json`; `None` when it cannot be read or is not a fingerprint.
fn read_fingerprint(dir: &Path, kind: &str) -> Option<Fingerprint> {
    let text = fs::read_to_string(dir.join(format!("{kind}.json"))).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let field = |name: &str| json.get(name).filter(|value| !value.is_null());
    let rustc = field("rustc")?.to_string();
    let features = field("features")?;
    let profile = field("profile")?;
    let target = field("target")?;
    let path = field("path")?;
    let zero = |value: &serde_json::Value| value.as_u64() == Some(0);
    // `config`, `rustflags` and `compile_kind` are absent from older Cargos.
    let identity = (!(zero(target) && zero(path))).then(|| {
        serde_json::json!([
            features,
            profile,
            target,
            path,
            field("config"),
            field("rustflags"),
            field("compile_kind"),
        ])
        .to_string()
    });
    Some(Fingerprint { rustc, identity })
}

/// What a unit describes: the extension-less file Cargo keeps in its
/// fingerprint directory (`lib-serde`, `bin-kronn`, `test-lib-kronn`,
/// `run-build-script-build-script-build`).
fn unit_kind(dir: &Path) -> String {
    let mut kinds: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| {
            !name.contains('.') && !name.starts_with("dep-") && !name.starts_with("output-")
        })
        .collect();
    kinds.sort();
    kinds.into_iter().next().unwrap_or_default()
}

/// The newest write of a unit: its directory and the files in it (a rebuild
/// rewrites the fingerprint; an unchanged unit keeps its old one).
fn unit_written(dir: &Path) -> Option<SystemTime> {
    let modified = |path: &Path| fs::symlink_metadata(path).ok()?.modified().ok();
    let mut newest = modified(dir)?;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        if let Some(time) = modified(&entry.path()) {
            newest = newest.max(time);
        }
    }
    Some(newest)
}

fn bytes_of(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            measure_bounded(
                path,
                &[],
                MeasureBudget::within(2_000_000, Duration::from_secs(120)),
            )
            .bytes
        }
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

fn remove_entry(path: &Path, report: &mut PruneReport) -> io::Result<()> {
    let bytes = bytes_of(path);
    let removed = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
    };
    removed?;
    report.removed_entries += 1;
    report.removed_bytes += bytes;
    Ok(())
}

/// The compiler of the latest build: the one the units written within
/// [`SAME_BUILD_WINDOW`] of the newest write all agree on, `None` when they do
/// not (two toolchains in use: nothing is stale by its compiler).
fn latest_rustc(units: &[Unit]) -> Option<&str> {
    let newest = units.iter().map(|unit| unit.written).max()?;
    let mut latest = units
        .iter()
        .filter(|unit| {
            newest
                .duration_since(unit.written)
                .is_ok_and(|behind| behind <= SAME_BUILD_WINDOW)
        })
        .map(|unit| unit.fingerprint.rustc.as_str());
    let first = latest.next()?;
    latest.all(|rustc| rustc == first).then_some(first)
}

/// The hashes of the stale units of `debug_dir`, the hashes that stay, and the
/// fingerprint directories to remove.
fn stale_units(
    debug_dir: &Path,
    locked: Option<&HashSet<String>>,
) -> (HashSet<String>, HashSet<String>, Vec<(String, PathBuf)>) {
    let fingerprints = debug_dir.join(FINGERPRINT_DIR);
    let mut units: Vec<Unit> = Vec::new();
    let mut stale_hashes = HashSet::new();
    let mut kept_hashes = HashSet::new();
    let mut stale_dirs = Vec::new();
    for entry in fs::read_dir(&fingerprints).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((package, hash)) = split_unit_name(&name) else {
            continue;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // A unit whose write time cannot be read is never judged: it stays.
        let Some(written) = unit_written(&path) else {
            kept_hashes.insert(hash.to_string());
            continue;
        };
        if locked.is_some_and(|names| !names.contains(&unify(package))) {
            stale_hashes.insert(hash.to_string());
            stale_dirs.push((hash.to_string(), path));
            continue;
        }
        // Nor one whose fingerprint cannot be read or parsed.
        let kind = unit_kind(&path);
        let Some(fingerprint) = read_fingerprint(&path, &kind) else {
            kept_hashes.insert(hash.to_string());
            continue;
        };
        units.push(Unit {
            package: package.to_string(),
            kind,
            dir_name: name.clone(),
            hash: hash.to_string(),
            written,
            fingerprint,
        });
    }

    let latest = latest_rustc(&units).map(str::to_string);
    let mut groups: HashMap<(&str, &str, &str), Vec<&Unit>> = HashMap::new();
    for unit in &units {
        if latest
            .as_deref()
            .is_some_and(|rustc| rustc != unit.fingerprint.rustc)
        {
            stale_hashes.insert(unit.hash.clone());
            stale_dirs.push((unit.hash.clone(), fingerprints.join(&unit.dir_name)));
        } else if let Some(identity) = &unit.fingerprint.identity {
            groups
                .entry((&unit.package, &unit.kind, identity))
                .or_default()
                .push(unit);
        } else {
            kept_hashes.insert(unit.hash.clone());
        }
    }
    for same_identity in groups.into_values() {
        let newest = same_identity.iter().map(|unit| unit.written).max();
        for unit in same_identity {
            // A tie on the write time is not a verdict: both stay.
            if Some(unit.written) == newest {
                kept_hashes.insert(unit.hash.clone());
            } else {
                stale_hashes.insert(unit.hash.clone());
                stale_dirs.push((unit.hash.clone(), fingerprints.join(&unit.dir_name)));
            }
        }
    }
    (stale_hashes, kept_hashes, stale_dirs)
}

/// Remove the stale artifacts of `debug_dir` (`<target>/debug`). The caller
/// holds Cargo's build lock exclusively; this never takes it.
pub fn prune_locked_debug_dir(
    debug_dir: &Path,
    locked_packages: Option<&HashSet<String>>,
) -> PruneReport {
    let mut report = PruneReport::default();
    let (stale, kept, stale_dirs) = stale_units(debug_dir, locked_packages);
    // A hash is stale only when no live unit carries it.
    let stale: HashSet<&String> = stale.difference(&kept).collect();
    for (hash, dir) in &stale_dirs {
        if !stale.contains(hash) {
            continue;
        }
        if remove_entry(dir, &mut report).is_ok() {
            report.removed_units += 1;
        }
    }
    for sub in [DEPS_DIR, BUILD_DIR] {
        for entry in fs::read_dir(debug_dir.join(sub))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let hash = if sub == DEPS_DIR {
                hash_of_deps_entry(&name).map(str::to_string)
            } else {
                split_unit_name(&name).map(|(_, hash)| hash.to_string())
            };
            if hash.is_some_and(|hash| stale.contains(&hash)) {
                let _ = remove_entry(&entry.path(), &mut report);
            }
        }
    }
    report
}

/// Take Cargo's build lock on `debug_dir` exclusively, prune what is stale, and
/// hand the lock back **shared** (the clone that follows holds it so a starting
/// build waits instead of changing what is being cloned).
///
/// `Err` when the lock cannot be had: a build is writing the directory, or it
/// has no Cargo lock (so it is not a Cargo profile directory). The directory is
/// then untouched.
pub fn prune_and_lock_shared(
    debug_dir: &Path,
    locked_packages: Option<&HashSet<String>>,
) -> Result<(fs::File, PruneReport), String> {
    let lock = fs::File::open(debug_dir.join(CARGO_BUILD_LOCK))
        .map_err(|error| format!("no Cargo build lock ({error})"))?;
    FileExt::try_lock_exclusive(&lock).map_err(|_| "a build is writing it".to_string())?;
    let report = prune_locked_debug_dir(debug_dir, locked_packages);
    // flock converts the lock held on this descriptor.
    FileExt::try_lock_shared(&lock)
        .map_err(|_| "a build took it while it was being pruned".to_string())?;
    Ok((lock, report))
}

/// [`prune_and_lock_shared`] without keeping the lock: the prune on its own,
/// for a caller that does not clone afterwards.
pub fn prune_seed(debug_dir: &Path, locked_packages: Option<&HashSet<String>>) -> PruneOutcome {
    match prune_and_lock_shared(debug_dir, locked_packages) {
        Ok((lock, report)) => {
            let _ = FileExt::unlock(&lock);
            PruneOutcome::Pruned(report)
        }
        Err(reason) => PruneOutcome::Skipped(reason),
    }
}

#[cfg(test)]
#[path = "seed_prune_test.rs"]
mod seed_prune_test;
