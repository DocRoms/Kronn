//! KT-1438 — a task worktree gets the project's generated, gitignored assets.
//!
//! A fresh worktree holds the tracked tree and nothing else, so a project whose
//! code needs a generated directory that git ignores (a compiled `public/build`,
//! a vector-image set) cannot run its own tests there: the worktree of a PHP
//! site answered 500 on every page because `public/images` was missing, and its
//! validation went red on a correct candidate.
//!
//! A project declares **lane seeds**: repository-relative paths, copied from its
//! main checkout into each task worktree at provisioning (an APFS clone or a
//! reflink where the file system has one, else a bounded copy) and refreshed
//! when their source changed, like the warm Cargo target of
//! [`crate::core::task_worker_lane_warm`].
//!
//! * The source is the **main checkout** only: no worker's sandbox can write it
//!   (see `seed_candidates` in the warm module for why a lane is never a source).
//! * A seed never enters a delivery: a path with a tracked file under it, or one
//!   whose copy would leave anything but ignored files in `git status`, is
//!   refused and removed.
//! * What Kronn seeded is remembered in the worktree's own git directory (never
//!   in the tree), so a refresh only replaces a copy Kronn made.
//!
//! Everything here is best-effort: [`seed_worktree`] never raises; each path
//! gets an outcome saying what happened, and an unseedable worktree just
//! starts without it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::core::task_worker_lane_warm::CloneTree;

/// `task_execution_events.action` of the seeding record.
pub const LANE_SEEDS_EVENT_ACTION: &str = "lane_seeds";

/// How many seeds one project may declare.
pub const MAX_LANE_SEEDS: usize = 20;
const MAX_SEED_PATH_BYTES: usize = 512;
/// A copy (the fallback when the file system cannot clone) over this size is
/// refused: it would cost the disk it is meant to spare.
pub const COPY_FALLBACK_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
/// A seed tree with more entries than this is not a generated-assets directory.
const MAX_SEED_ENTRIES: u64 = 400_000;
/// Remembered inside the worktree's git directory.
const MARKER_FILE: &str = "kronn-lane-seeds.json";

// ─── Declaring ────────────────────────────────────────────────────────────

/// A declared seed as a normalized repository-relative path with `/`
/// separators, or why it cannot be one. Refuses absolute paths, `..`, the
/// repository root, a `.git` component and Kronn's own `.kronn` directory.
pub fn normalize_seed_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("a lane seed path is empty".into());
    }
    if trimmed.len() > MAX_SEED_PATH_BYTES {
        return Err(format!(
            "`{}…` is longer than {MAX_SEED_PATH_BYTES} bytes",
            trimmed.chars().take(40).collect::<String>()
        ));
    }
    if trimmed.contains('\\') || trimmed.contains('\0') {
        return Err(format!(
            "`{trimmed}`: use `/` separators and plain characters"
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_string_lossy().into_owned();
                if name == ".git" || name == ".kronn" {
                    return Err(format!("`{trimmed}`: `{name}` is not a seedable path"));
                }
                parts.push(name);
            }
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "`{trimmed}` must be a path inside the repository (no leading `/`, no `..`)"
                ))
            }
        }
    }
    if parts.is_empty() {
        return Err(format!(
            "`{trimmed}` is the repository root, not a seedable path"
        ));
    }
    Ok(parts.join("/"))
}

/// Normalize a declared list: each entry valid, none repeated or nested in
/// another (the outer one already carries it), at most [`MAX_LANE_SEEDS`].
pub fn normalize_seed_list(raw: &[String]) -> Result<Vec<String>, String> {
    let mut seeds: Vec<String> = Vec::new();
    for entry in raw {
        let path = normalize_seed_path(entry)?;
        if !seeds.contains(&path) {
            seeds.push(path);
        }
    }
    if seeds.len() > MAX_LANE_SEEDS {
        return Err(format!(
            "a project may declare at most {MAX_LANE_SEEDS} lane seeds"
        ));
    }
    for outer in &seeds {
        if let Some(inner) = seeds
            .iter()
            .find(|inner| *inner != outer && inner.starts_with(&format!("{outer}/")))
        {
            return Err(format!(
                "`{inner}` is already inside `{outer}`: declare one of them"
            ));
        }
    }
    Ok(seeds)
}

// ─── Which seeds apply ────────────────────────────────────────────────────

/// Where the seeds a project is seeded with come from. The `[lane_seeds]` table
/// of `kronn/project.toml` is not a source: it reaches the repository through a
/// pull request, not through the human gate, so it is only *proposed* in the
/// settings and takes effect once a human adopts it there (which declares it in
/// Kronn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LaneSeedsSource {
    /// A human declared them in Kronn (they may have cleared them).
    Project,
    /// None declared in Kronn.
    None,
}

/// A declared seed that is, or holds, something that looks like a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct LaneSeedSecretFinding {
    /// The declared seed.
    pub path: String,
    /// What looks secret, relative to the repository: the seed itself when its
    /// own path does, else the files and directories found under it.
    pub files: Vec<String>,
    /// More were found than `files` lists.
    pub truncated: bool,
    /// Some of these files are not confirmed by a human: the seed is not copied
    /// into task worktrees until they are.
    pub held: bool,
}

/// The secret-looking files a human has seen and accepted, by declared seed
/// (repository-relative paths). A file that is not listed here holds its seed.
pub type ConfirmedSecrets = BTreeMap<String, Vec<String>>;

/// How many secret-looking paths one finding lists.
const MAX_SECRET_FILES_LISTED: usize = 10;
/// How many secret-looking files one seed may hold and still be confirmable: a
/// human cannot review more, and the confirmation is stored.
pub const MAX_CONFIRMABLE_SECRET_FILES: usize = 200;

/// Every secret-looking file `seed` is or holds in `repo`, sorted. A seed copies
/// files to where a worker reads them, and a gitignored `.env` or key is a
/// gitignored path too. The seed's own path is judged by
/// [`crate::acp::secret_files::is_secret_path`]; the tree under it is walked
/// (bounded by [`MAX_SEED_ENTRIES`], links not followed) and each entry's name is
/// judged, a symlink by the same path rule. Fails closed: a tree that cannot be
/// read in full, or is too large to, or holds more than
/// [`MAX_CONFIRMABLE_SECRET_FILES`], is an error, never "nothing found".
pub fn scan_secret_files(repo: &Path, seed: &str) -> Result<Vec<String>, String> {
    use crate::acp::secret_files::{is_secret_file, is_secret_path};
    let root = repo.join(seed);
    if is_secret_path(&root) {
        return Ok(vec![seed.to_string()]);
    }
    let meta = match std::fs::symlink_metadata(&root) {
        Ok(meta) => meta,
        // Nothing there to copy yet.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read `{seed}`: {error}")),
    };
    if !meta.is_dir() {
        return Ok(Vec::new());
    }
    let (mut files, mut entries) = (Vec::new(), 0u64);
    let mut stack = vec![(root, seed.to_string())];
    while let Some((dir, rel)) = stack.pop() {
        let children =
            std::fs::read_dir(&dir).map_err(|error| format!("cannot list `{rel}`: {error}"))?;
        for child in children {
            let child = child.map_err(|error| format!("cannot list `{rel}`: {error}"))?;
            entries += 1;
            if entries > MAX_SEED_ENTRIES {
                return Err(format!(
                    "`{seed}` has more than {MAX_SEED_ENTRIES} entries: it cannot be checked \
                     for secrets, and is not a generated-assets directory"
                ));
            }
            let path = child.path();
            let child_rel = format!("{rel}/{}", child.file_name().to_string_lossy());
            let kind = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot read `{child_rel}`: {error}"))?;
            let secret = if kind.file_type().is_symlink() {
                is_secret_path(&path)
            } else {
                is_secret_file(Path::new(&child_rel))
            };
            if secret {
                if files.len() >= MAX_CONFIRMABLE_SECRET_FILES {
                    return Err(format!(
                        "`{seed}` holds more than {MAX_CONFIRMABLE_SECRET_FILES} secret-looking \
                         files: it cannot be reviewed, and is not a generated-assets directory"
                    ));
                }
                files.push(child_rel.clone());
            }
            if kind.is_dir() {
                stack.push((path, child_rel));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The files of `found` that `confirmed` (for the same seed) does not cover.
pub fn unconfirmed_secret_files(found: &[String], confirmed: Option<&Vec<String>>) -> Vec<String> {
    found
        .iter()
        .filter(|file| !confirmed.is_some_and(|known| known.contains(file)))
        .cloned()
        .collect()
}

/// Which of `seeds` (in `repo`) look like a secret file or hold one, and whether
/// each is held, i.e. has a file `confirmed` does not cover. See
/// [`scan_secret_files`] for how it is judged and why it fails closed.
pub fn secret_findings_against(
    repo: &Path,
    seeds: &[String],
    confirmed: &ConfirmedSecrets,
) -> Result<Vec<LaneSeedSecretFinding>, String> {
    let mut findings = Vec::new();
    for seed in seeds {
        let files = scan_secret_files(repo, seed)?;
        if files.is_empty() {
            continue;
        }
        let held = !unconfirmed_secret_files(&files, confirmed.get(seed)).is_empty();
        findings.push(LaneSeedSecretFinding::new(seed, files, held));
    }
    Ok(findings)
}

impl LaneSeedSecretFinding {
    /// A finding listing the first of `files` (all of them secret-looking).
    pub fn new(seed: &str, files: Vec<String>, held: bool) -> Self {
        let truncated = files.len() > MAX_SECRET_FILES_LISTED;
        Self {
            path: seed.to_string(),
            files: files.into_iter().take(MAX_SECRET_FILES_LISTED).collect(),
            truncated,
            held,
        }
    }
}

/// [`secret_findings_against`] with nothing confirmed.
pub fn secret_findings(
    repo: &Path,
    seeds: &[String],
) -> Result<Vec<LaneSeedSecretFinding>, String> {
    secret_findings_against(repo, seeds, &ConfirmedSecrets::new())
}

/// How many scans [`cached_secret_files`] keeps.
const SCAN_CACHE_LIMIT: usize = 64;

type ScanCache = std::sync::Mutex<std::collections::HashMap<PathBuf, (String, Vec<String>)>>;

/// [`scan_secret_files`] of a seed whose source fingerprint is `fingerprint`,
/// remembered by source path: every task worktree of a project is seeded from the
/// same tree, which is only walked again once its fingerprint (entries, sizes,
/// names, modification and change times) moved.
fn cached_secret_files(repo: &Path, seed: &str, fingerprint: &str) -> Result<Vec<String>, String> {
    static CACHE: std::sync::OnceLock<ScanCache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = repo.join(seed);
    if let Ok(known) = cache.lock() {
        if let Some((seen, files)) = known.get(&key) {
            if seen == fingerprint {
                return Ok(files.clone());
            }
        }
    }
    let files = scan_secret_files(repo, seed)?;
    if let Ok(mut known) = cache.lock() {
        if known.len() >= SCAN_CACHE_LIMIT {
            known.clear();
        }
        known.insert(key, (fingerprint.to_string(), files.clone()));
    }
    Ok(files)
}

// ─── Outcomes ─────────────────────────────────────────────────────────────

/// How a seed was put in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedMethod {
    /// Copy-on-write clone (APFS `clonefile(2)`, a reflink elsewhere).
    Clone,
    /// Plain copy, the file system having no clone.
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SeedStatus {
    /// Put in place; `refreshed` when an older copy of Kronn's was replaced.
    Seeded {
        method: SeedMethod,
        refreshed: bool,
        ms: u64,
        /// Symlinks of the source left out of the copy because they point
        /// outside the seed (named relative to it).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dropped_links: Vec<String>,
    },
    /// Already there, and its source has not changed since.
    UpToDate,
    Skipped {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSeedOutcome {
    pub path: String,
    #[serde(flatten)]
    pub status: SeedStatus,
}

impl LaneSeedOutcome {
    fn skipped(path: &str, reason: impl Into<String>) -> Self {
        Self {
            path: path.to_string(),
            status: SeedStatus::Skipped {
                reason: reason.into(),
            },
        }
    }
}

/// What seeding one worktree did, journalled on the execution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSeedsRecord {
    pub seeds: Vec<LaneSeedOutcome>,
}

impl LaneSeedsRecord {
    pub fn skipped(&self) -> impl Iterator<Item = (&str, &str)> {
        self.seeds.iter().filter_map(|seed| match &seed.status {
            SeedStatus::Skipped { reason } => Some((seed.path.as_str(), reason.as_str())),
            _ => None,
        })
    }
}

// ─── Git ──────────────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    crate::core::cmd::project_git_cmd()
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|error| format!("git {} failed to start: {error}", args.join(" ")))
}

/// `:(literal)path`: a seed path is a path, never a glob.
fn literal(rel: &str) -> String {
    format!(":(literal){rel}")
}

/// Whether anything under `rel` in `dir` is tracked.
fn has_tracked_files(dir: &Path, rel: &str) -> Result<bool, String> {
    let output = git(dir, &["ls-files", "-z", "--", &literal(rel)])?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(!output.stdout.is_empty())
}

/// What `git status` reports under `rel` in `dir`: empty when every file there
/// is ignored (or tracked and unchanged).
fn status_under(dir: &Path, rel: &str) -> Result<String, String> {
    let output = git(
        dir,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            &literal(rel),
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The worktree's own git directory (`<repo>/.git/worktrees/<name>`), where
/// Kronn remembers what it seeded: outside the tree, gone with the worktree.
fn worktree_git_dir(worktree: &Path) -> Result<PathBuf, String> {
    let output = git(worktree, &["rev-parse", "--absolute-git-dir"])?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

// ─── Source fingerprint ───────────────────────────────────────────────────

/// FNV-1a: stable across runs and Rust versions, which the persisted marker needs.
fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A cheap fingerprint of a tree and its size in bytes: entries, bytes, the
/// newest modification and change times, and an order-independent hash of every
/// entry's relative name, kind and size. Equal fingerprints mean the seed need
/// not be redone. The change time and the hash catch what the modification time
/// alone cannot: a same-size rewrite whose mtime was preserved (`cp -p`, `rsync`).
fn fingerprint(root: &Path) -> Result<(String, u64), String> {
    let mut stack = vec![(root.to_path_buf(), String::new())];
    let (mut entries, mut bytes) = (0u64, 0u64);
    let (mut newest_mtime, mut newest_ctime) = (0u128, 0u128);
    let mut names = 0u64;
    while let Some((path, relative)) = stack.pop() {
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        entries += 1;
        if entries > MAX_SEED_ENTRIES {
            return Err(format!(
                "more than {MAX_SEED_ENTRIES} entries: not a generated-assets directory"
            ));
        }
        if let Ok(modified) = meta.modified() {
            if let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) {
                newest_mtime = newest_mtime.max(since.as_nanos());
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let ctime = i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec());
            newest_ctime = newest_ctime.max(ctime.max(0) as u128);
        }
        let kind: u8 = if meta.is_dir() {
            b'd'
        } else if meta.file_type().is_symlink() {
            b'l'
        } else {
            b'f'
        };
        let size = if meta.is_dir() { 0 } else { meta.len() };
        let mut entry_hash = fnv1a(0xcbf2_9ce4_8422_2325, relative.as_bytes());
        entry_hash = fnv1a(entry_hash, &[0, kind]);
        entry_hash = fnv1a(entry_hash, &size.to_le_bytes());
        names = names.wrapping_add(entry_hash);
        if meta.is_dir() {
            let children = std::fs::read_dir(&path)
                .map_err(|error| format!("cannot list {}: {error}", path.display()))?;
            for child in children {
                let child =
                    child.map_err(|error| format!("cannot list {}: {error}", path.display()))?;
                let name = child.file_name().to_string_lossy().into_owned();
                stack.push((child.path(), format!("{relative}/{name}")));
            }
        } else {
            bytes += meta.len();
        }
    }
    Ok((
        format!("{entries}:{bytes}:{newest_mtime}:{newest_ctime}:{names:x}"),
        bytes,
    ))
}

// ─── Placing a copy ───────────────────────────────────────────────────────

/// Recursive copy for a file system that cannot clone. Symlinks are copied as
/// symlinks (on Unix; elsewhere they are left out).
fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(std::fs::read_link(source)?, destination);
        }
        #[cfg(not(unix))]
        {
            return Ok(());
        }
    }
    if meta.is_dir() {
        std::fs::create_dir(destination)?;
        for child in std::fs::read_dir(source)? {
            let child = child?;
            copy_tree(&child.path(), &destination.join(child.file_name()))?;
        }
        std::fs::set_permissions(destination, meta.permissions())?;
        return Ok(());
    }
    std::fs::copy(source, destination).map(|_| ())
}

/// Remove `path` whatever it is, without following a symlink.
fn remove_any(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// An existing ancestor of `rel` under `root` that is a symlink: the copy would
/// land (or the removal would reach) outside the tree.
fn symlinked_component(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for part in rel.split('/') {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return Some(current),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Longest list of dropped link names kept in a record.
const MAX_DROPPED_NAMES: usize = 10;

/// Whether a symlink living `depth` directories below the seed root, pointing at
/// `target`, stays inside the seed. Only a relative target made of leading `..`
/// (no more than `depth`) followed by plain names qualifies: a `..` after a name
/// would be resolved through whatever link that name is, which a lexical check
/// cannot follow, so it is refused too.
fn link_stays_inside(target: &Path, depth: usize) -> bool {
    let (mut ups, mut named) = (0usize, false);
    for component in target.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if !named => ups += 1,
            Component::Normal(_) => named = true,
            _ => return false,
        }
    }
    ups <= depth
}

/// Remove, from the fresh copy at `staging`, every symlink that points out of
/// the seed (absolute, or climbing past its root): copied as-is, it would hand
/// the worker a window onto the host. Returns their names relative to the seed.
/// A seed may be a single file: it is looked at (never listed) and kept. A seed
/// that is itself a link is an error: its target is not part of the copy.
fn drop_escaping_links(staging: &Path) -> io::Result<Vec<String>> {
    let root = std::fs::symlink_metadata(staging)?;
    if root.file_type().is_symlink() {
        return Err(io::Error::other("the seed itself is a symlink"));
    }
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut dropped = Vec::new();
    let mut stack = vec![(staging.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        for child in std::fs::read_dir(&dir)? {
            let path = child?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                stack.push((path, depth + 1));
            } else if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path)?;
                if !link_stays_inside(&target, depth) {
                    std::fs::remove_file(&path)?;
                    dropped.push(
                        path.strip_prefix(staging)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
    }
    dropped.sort();
    Ok(dropped)
}

/// A fresh, unguessable name directly under the worktree root, for a copy being
/// built or an older one being set aside.
fn transient_name(kind: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!(".kronn-{kind}-{}", &id[..16])
}

const STAGING_KIND: &str = "seeding";
const TRASH_KIND: &str = "seeded-old";

/// Take `rel` out of the worktree: set aside by a rooted rename (a parent
/// swapped for a link is not followed), then deleted from its one-component name.
fn discard(worktree: &Path, rel: &str) -> io::Result<()> {
    let aside = transient_name(TRASH_KIND);
    match crate::core::rooted_io::rename(worktree, Path::new(rel), Path::new(&aside)) {
        Ok(()) => remove_any(&worktree.join(aside)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Leftovers of a seeding interrupted by a crash.
fn sweep_transients(worktree: &Path) {
    let Ok(children) = std::fs::read_dir(worktree) else {
        return;
    };
    for child in children.flatten() {
        let name = child.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!(".kronn-{STAGING_KIND}-"))
            || name.starts_with(&format!(".kronn-{TRASH_KIND}-"))
        {
            let _ = remove_any(&child.path());
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SeedMarker {
    /// Source fingerprint of each seed Kronn put in this worktree.
    #[serde(default)]
    seeds: BTreeMap<String, String>,
}

fn read_marker(git_dir: &Path) -> SeedMarker {
    std::fs::read(git_dir.join(MARKER_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_marker(git_dir: &Path, marker: &SeedMarker) -> io::Result<()> {
    let staging = git_dir.join(format!("{MARKER_FILE}.tmp"));
    std::fs::write(
        &staging,
        serde_json::to_vec(marker).map_err(io::Error::other)?,
    )?;
    std::fs::rename(staging, git_dir.join(MARKER_FILE))
}

// ─── Seeding ──────────────────────────────────────────────────────────────

/// Seed `worktree` from `main_checkout` with every declared path. Idempotent:
/// a copy whose source did not change is left alone, one whose source changed
/// is replaced, and a path the worktree holds that Kronn did not put there is
/// never touched.
pub fn seed_worktree(
    main_checkout: &Path,
    worktree: &Path,
    seeds: &[String],
    confirmed: &ConfirmedSecrets,
    clone: CloneTree<'_>,
) -> LaneSeedsRecord {
    if seeds.is_empty() {
        return LaneSeedsRecord::default();
    }
    let git_dir = match worktree_git_dir(worktree) {
        Ok(dir) => dir,
        Err(reason) => {
            return LaneSeedsRecord {
                seeds: seeds
                    .iter()
                    .map(|path| LaneSeedOutcome::skipped(path, reason.clone()))
                    .collect(),
            }
        }
    };
    // A copy under construction (or one left by a crash) sits at the worktree
    // root under a transient name: have git ignore that name before anything is
    // created there, so a worker's `git add -A` can never pick it up.
    for kind in [STAGING_KIND, TRASH_KIND] {
        if let Err(reason) =
            crate::core::worktree::ensure_local_git_exclude(worktree, &format!("/.kronn-{kind}-*"))
        {
            let reason = format!("its temporary copy cannot be kept out of git: {reason}");
            return LaneSeedsRecord {
                seeds: seeds
                    .iter()
                    .map(|path| LaneSeedOutcome::skipped(path, reason.clone()))
                    .collect(),
            };
        }
    }
    sweep_transients(worktree);
    let mut marker = read_marker(&git_dir);
    let mut outcomes = Vec::with_capacity(seeds.len());
    for declared in seeds {
        let outcome = match normalize_seed_path(declared) {
            Ok(rel) => {
                let outcome = seed_one(
                    main_checkout,
                    worktree,
                    &rel,
                    confirmed.get(&rel),
                    &mut marker,
                    clone,
                );
                if let SeedStatus::Skipped { .. } = outcome.status {
                    marker.seeds.remove(&rel);
                }
                outcome
            }
            Err(reason) => LaneSeedOutcome::skipped(declared, reason),
        };
        outcomes.push(outcome);
    }
    if let Err(error) = write_marker(&git_dir, &marker) {
        tracing::warn!(worktree = %worktree.display(), "lane seed marker not written: {error}");
    }
    LaneSeedsRecord { seeds: outcomes }
}

/// Why a seed holding unconfirmed secret-looking files is not copied.
fn held_reason(unconfirmed: &[String]) -> String {
    const NAMED: usize = 5;
    let mut names = unconfirmed
        .iter()
        .take(NAMED)
        .map(|file| format!("`{file}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if unconfirmed.len() > NAMED {
        names.push_str(&format!(" and {} more", unconfirmed.len() - NAMED));
    }
    format!(
        "held: it holds secret-looking files a human has not confirmed ({names}); confirm \
         them in the project's lane seeds settings"
    )
}

fn seed_one(
    main_checkout: &Path,
    worktree: &Path,
    rel: &str,
    confirmed: Option<&Vec<String>>,
    marker: &mut SeedMarker,
    clone: CloneTree<'_>,
) -> LaneSeedOutcome {
    let skip = |reason: String| LaneSeedOutcome::skipped(rel, reason);
    let source = main_checkout.join(rel);
    match std::fs::symlink_metadata(&source) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return skip("the source in the main checkout is a symlink".into())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return skip("the main checkout does not have it (generate it there first)".into())
        }
        Err(error) => return skip(format!("the source cannot be read: {error}")),
    }
    // A symlinked parent could point the source outside the checkout.
    match (source.canonicalize(), main_checkout.canonicalize()) {
        (Ok(real), Ok(root)) if real.starts_with(&root) => {}
        _ => return skip("the source resolves outside the main checkout".into()),
    }
    if let Some(link) = symlinked_component(worktree, rel) {
        return skip(format!(
            "{} in the worktree is a symlink",
            link.strip_prefix(worktree).unwrap_or(&link).display()
        ));
    }
    match has_tracked_files(worktree, rel) {
        Ok(false) => {}
        Ok(true) => return skip("tracked files live under it: it is not generated".into()),
        Err(reason) => return skip(reason),
    }
    match status_under(main_checkout, rel) {
        Ok(status) if status.is_empty() => {}
        Ok(_) => return skip("it is not gitignored in the main checkout".into()),
        Err(reason) => return skip(reason),
    }
    let (current, bytes) = match fingerprint(&source) {
        Ok(found) => found,
        Err(reason) => return skip(reason),
    };
    // The human accepted the secret-looking files the seed held when they
    // confirmed it: a `.env` or a key that appeared since is not covered, and the
    // seed stays out of every worktree until they have seen it.
    match cached_secret_files(main_checkout, rel, &current) {
        Ok(found) => {
            let unconfirmed = unconfirmed_secret_files(&found, confirmed);
            if !unconfirmed.is_empty() {
                // A copy Kronn made earlier would go stale beside the new file:
                // take it out, so that a confirmation reseeds from scratch.
                if marker.seeds.contains_key(rel) {
                    let _ = discard(worktree, rel);
                }
                return skip(held_reason(&unconfirmed));
            }
        }
        Err(reason) => return skip(format!("it cannot be checked for secrets: {reason}")),
    }

    let destination = worktree.join(rel);
    let refreshing = match std::fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return skip(format!("the worktree path cannot be read: {error}")),
        Ok(_) => match marker.seeds.get(rel) {
            Some(known) if *known == current => {
                // Up to date, but the worktree is the worker's to write: its
                // `.gitignore` may have stopped ignoring the copy since, and a
                // copy `git status` shows could enter a delivery.
                return match status_under(worktree, rel) {
                    Ok(status) if status.is_empty() => LaneSeedOutcome {
                        path: rel.to_string(),
                        status: SeedStatus::UpToDate,
                    },
                    Ok(_) => {
                        let _ = discard(worktree, rel);
                        skip("it is no longer gitignored in the worktree, so it would enter a delivery".into())
                    }
                    Err(reason) => skip(reason),
                };
            }
            Some(_) => true,
            None => {
                return skip("the worktree already has it and Kronn did not put it there".into())
            }
        },
    };

    // The copy is built under a fresh name at the worktree root (a directory
    // Kronn chose), then published into place by a rooted rename.
    let staging_name = transient_name(STAGING_KIND);
    let staging = worktree.join(&staging_name);
    let started = Instant::now();
    let method = match clone(&source, &staging) {
        Ok(()) => SeedMethod::Clone,
        Err(clone_error) => {
            let _ = remove_any(&staging);
            if bytes > COPY_FALLBACK_LIMIT_BYTES {
                return skip(format!(
                    "the file system cannot clone it ({clone_error}) and a copy of {} MiB is over \
                     the {} MiB limit",
                    bytes / (1024 * 1024),
                    COPY_FALLBACK_LIMIT_BYTES / (1024 * 1024)
                ));
            }
            if let Err(error) = copy_tree(&source, &staging) {
                let _ = remove_any(&staging);
                return skip(format!("the copy failed: {error}"));
            }
            SeedMethod::Copy
        }
    };
    let mut dropped_links = match drop_escaping_links(&staging) {
        Ok(dropped) => dropped,
        Err(error) => {
            let _ = remove_any(&staging);
            return skip(format!("the copy's links cannot be checked: {error}"));
        }
    };
    dropped_links.truncate(MAX_DROPPED_NAMES);

    let aside = transient_name(TRASH_KIND);
    if refreshing {
        if let Err(error) =
            crate::core::rooted_io::rename(worktree, Path::new(rel), Path::new(&aside))
        {
            let _ = remove_any(&staging);
            return skip(format!("the older copy cannot be replaced: {error}"));
        }
    }
    if let Err(error) =
        crate::core::rooted_io::rename(worktree, Path::new(&staging_name), Path::new(rel))
    {
        let _ = remove_any(&staging);
        if refreshing {
            let _ = crate::core::rooted_io::rename(worktree, Path::new(&aside), Path::new(rel));
        }
        return skip(format!("the copy cannot be put in place: {error}"));
    }
    if refreshing {
        let _ = remove_any(&worktree.join(&aside));
    }

    // Whatever the worktree's own ignore rules say: a seed that would show up
    // in `git status` could enter a delivery, so it is taken back out.
    match status_under(worktree, rel) {
        Ok(status) if status.is_empty() => {}
        Ok(_) => {
            let _ = discard(worktree, rel);
            return skip(
                "it is not gitignored in the worktree, so it would enter a delivery".into(),
            );
        }
        Err(reason) => {
            let _ = discard(worktree, rel);
            return skip(reason);
        }
    }
    marker.seeds.insert(rel.to_string(), current);
    LaneSeedOutcome {
        path: rel.to_string(),
        status: SeedStatus::Seeded {
            method,
            refreshed: refreshing,
            ms: started.elapsed().as_millis() as u64,
            dropped_links,
        },
    }
}

// ─── Suggesting ───────────────────────────────────────────────────────────

/// A gitignored path the project's tracked code mentions: a likely seed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct LaneSeedCandidate {
    /// Repository-relative, `/` separated, no trailing slash.
    pub path: String,
    /// Size on disk; `None` when the directory is too large to measure.
    pub size_bytes: Option<u64>,
    /// Why it is suggested.
    pub reason: String,
}

/// Directories nobody wants cloned into a worktree: dependency trees and
/// caches the toolchain rebuilds on its own.
const NEVER_SUGGESTED: &[&str] = &[
    "node_modules",
    "vendor",
    "target",
    ".git",
    ".kronn",
    ".idea",
    ".vscode",
    ".cache",
    ".next",
    ".nuxt",
    ".gradle",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    "coverage",
    "var",
    "tmp",
    "log",
    "logs",
];
const MAX_SUGGESTIONS: usize = 30;

fn tracked_code_mentions(repo: &Path, needle: &str, skip_prefix: &str) -> bool {
    let exclude = format!(":(exclude,literal){skip_prefix}");
    // The `.gitignore` that ignores a directory names it: that is no use of it.
    let ignore_files = ":(exclude,glob)**/.gitignore";
    git(
        repo,
        &[
            "grep",
            "-q",
            "-I",
            "-F",
            "-e",
            needle,
            "--",
            ".",
            &exclude,
            ignore_files,
        ],
    )
    .map(|output| output.status.success())
    .unwrap_or(false)
}

/// Gitignored directories of `repo` that tracked code references, for a human
/// to pick from. A directory is suggested when a tracked file mentions it by
/// its repository path, or by its last component(s) as a path segment
/// (`images/`, `public/images/`): a template names `@web/images/…`, not
/// `application/public/images`.
pub fn suggest_seeds(repo: &Path) -> Result<Vec<LaneSeedCandidate>, String> {
    let output = git(
        repo,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let listing = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut found = Vec::new();
    for entry in listing.split('\0').filter(|entry| !entry.is_empty()) {
        // Only directories: `--directory` collapses a fully ignored directory
        // to `dir/`; an ignored single file is not a generated-assets tree.
        let Some(dir) = entry.strip_suffix('/') else {
            continue;
        };
        let Ok(rel) = normalize_seed_path(dir) else {
            continue;
        };
        if rel
            .split('/')
            .any(|part| NEVER_SUGGESTED.contains(&part) || part.starts_with(".kronn"))
        {
            continue;
        }
        let parts: Vec<&str> = rel.split('/').collect();
        let mentions = (0..parts.len())
            .rev()
            .map(|start| format!("{}/", parts[start..].join("/")))
            .find(|needle| tracked_code_mentions(repo, needle, &rel));
        let Some(needle) = mentions else {
            continue;
        };
        let size_bytes = fingerprint(&repo.join(&rel)).ok().map(|(_, bytes)| bytes);
        found.push(LaneSeedCandidate {
            reason: format!("gitignored; tracked code mentions `{needle}`"),
            path: rel,
            size_bytes,
        });
        if found.len() >= MAX_SUGGESTIONS {
            break;
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

#[cfg(test)]
#[path = "task_worker_lane_seeds_test.rs"]
mod task_worker_lane_seeds_test;
