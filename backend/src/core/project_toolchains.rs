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

/// The roots a project's `rust_roots` setting keeps (`None` keeps every one),
/// and the listed directories that are not a detected root (a typo, or a crate
/// that moved), which the caller reports instead of silently ignoring.
pub fn select_roots(
    roots: Vec<RustRoot>,
    wanted: Option<&[String]>,
) -> (Vec<RustRoot>, Vec<String>) {
    let Some(wanted) = wanted else {
        return (roots, Vec::new());
    };
    let wanted: Vec<String> = wanted
        .iter()
        .map(|entry| crate::models::normalize_lane_root(entry))
        .collect();
    let unknown = wanted
        .iter()
        .filter(|entry| !roots.iter().any(|root| root.label() == **entry))
        .cloned()
        .collect();
    let kept = roots
        .into_iter()
        .filter(|root| wanted.contains(&root.label()))
        .collect();
    (kept, unknown)
}

// ─── KT-1747 — every toolchain of a checkout ──────────────────────────────

/// The package manager of a Node project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeManager {
    Npm,
    Yarn,
    Pnpm,
}

/// A toolchain a repository builds with, found from its manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainKind {
    Node(NodeManager),
    Composer,
    Cargo,
    CocoaPods,
    Xcode,
    Make,
}

impl ToolchainKind {
    /// The short name shown to a worker and in the settings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Node(NodeManager::Npm) => "npm",
            Self::Node(NodeManager::Yarn) => "yarn",
            Self::Node(NodeManager::Pnpm) => "pnpm",
            Self::Composer => "composer",
            Self::Cargo => "cargo",
            Self::CocoaPods => "cocoapods",
            Self::Xcode => "xcode",
            Self::Make => "make",
        }
    }

    /// The directory, next to the manifest, that holds the installed
    /// dependencies: what a lane layers read-only from the main checkout. `None`
    /// for a toolchain that installs nothing there (Cargo's live in `target/`,
    /// which the lane warm-up owns).
    pub fn dependency_dir(self) -> Option<&'static str> {
        match self {
            Self::Node(_) => Some("node_modules"),
            Self::Composer => Some("vendor"),
            Self::CocoaPods => Some("Pods"),
            Self::Cargo | Self::Xcode | Self::Make => None,
        }
    }

    /// The command that fills [`ToolchainKind::dependency_dir`] from the lock
    /// file, run in the toolchain's directory of the MAIN checkout (a lane never
    /// installs: it has no network). `None` where nothing is installed.
    pub fn install_command(self) -> Option<&'static str> {
        match self {
            Self::Node(NodeManager::Npm) => Some("npm ci"),
            Self::Node(NodeManager::Yarn) => Some("yarn install --frozen-lockfile"),
            Self::Node(NodeManager::Pnpm) => Some("pnpm install --frozen-lockfile"),
            Self::Composer => Some("composer install"),
            Self::CocoaPods => Some("pod install"),
            Self::Cargo | Self::Xcode | Self::Make => None,
        }
    }

    /// KT-1751 — the variable that moves this package manager's download cache
    /// out of the user's home, with the name of its folder under the lane's
    /// cache directory. A lane may write only its worktree and its target, so a
    /// manager that caches in `~/.npm`, `~/.cache/yarn` or `~/.composer` would
    /// fail on the first install and could never share a cache with the owner's
    /// real one. `None` for a toolchain with no home cache to move: pnpm's store
    /// is the host's read-only one by design, Cargo's is the lane's `CARGO_HOME`.
    pub fn cache_env(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Node(NodeManager::Npm) => Some(("npm_config_cache", "npm")),
            Self::Node(NodeManager::Yarn) => Some(("YARN_CACHE_FOLDER", "yarn")),
            Self::Composer => Some(("COMPOSER_CACHE_DIR", "composer")),
            Self::Node(NodeManager::Pnpm)
            | Self::Cargo
            | Self::CocoaPods
            | Self::Xcode
            | Self::Make => None,
        }
    }

    /// Order inside one directory; build tools come after package managers.
    fn rank(self) -> u8 {
        match self {
            Self::Node(_) => 0,
            Self::Composer => 1,
            Self::Cargo => 2,
            Self::CocoaPods => 3,
            Self::Xcode => 4,
            Self::Make => 5,
        }
    }
}

/// One toolchain at one directory of a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRoot {
    pub kind: ToolchainKind,
    /// The directory of its manifest, relative to the repository root (empty
    /// for the root itself).
    pub rel_dir: PathBuf,
    /// KT-1747 review — for a package of a Node workspace (npm or yarn `workspaces`,
    /// or `pnpm-workspace.yaml` in an ancestor up to the repository root), the
    /// workspace root, relative to the repository: its dependencies are installed
    /// there, never by the package itself. `None` otherwise.
    pub workspace: Option<PathBuf>,
}

impl PackageRoot {
    /// The relative directory as shown to a worker: `.` for the root.
    pub fn label(&self) -> String {
        if self.rel_dir.as_os_str().is_empty() {
            ".".to_string()
        } else {
            self.rel_dir
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        }
    }

    /// The directory name its dependencies are installed in (see
    /// [`ToolchainKind::dependency_dir`]).
    pub fn dependency_dir(&self) -> Option<&'static str> {
        self.kind.dependency_dir()
    }

    /// Its dependency directory relative to the repository root, `/` separated.
    pub fn dependency_path(&self) -> Option<String> {
        let dir = self.dependency_dir()?;
        let label = self.label();
        Some(if label == "." {
            dir.to_string()
        } else {
            format!("{label}/{dir}")
        })
    }
}

/// A repository with more toolchain roots than this is not described one by one.
const MAX_PROFILE_ROOTS: usize = 40;
/// Never entered when looking for a manifest, besides [`SKIPPED_DIRS`]: the
/// CocoaPods dependency tree and Xcode's build output.
const SKIPPED_DEPENDENCY_DIRS: [&str; 2] = ["Pods", "DerivedData"];
/// Never entered either (KT-1747 review): test fixtures, examples and build
/// output hold manifests that are not toolchains of the project.
const SKIPPED_PROFILE_DIRS: [&str; 4] = ["fixtures", "examples", "dist", "build"];

fn package_json(dir: &Path) -> Option<serde_json::Value> {
    std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
}

/// `dir` and its ancestors up to `repo_root` (included), nearest first.
fn up_to_root<'a>(dir: &'a Path, repo_root: &'a Path) -> impl Iterator<Item = &'a Path> {
    dir.ancestors()
        .take_while(move |ancestor| ancestor.starts_with(repo_root))
}

/// What one directory says about its package manager: the `packageManager`
/// field of its `package.json`, then its lock file.
fn declared_manager(dir: &Path) -> Option<NodeManager> {
    let declared = package_json(dir).and_then(|value| {
        value
            .get("packageManager")
            .and_then(|field| field.as_str())
            .map(|field| field.split('@').next().unwrap_or_default().to_string())
    });
    match declared.as_deref() {
        Some("pnpm") => return Some(NodeManager::Pnpm),
        Some("yarn") => return Some(NodeManager::Yarn),
        Some("npm") => return Some(NodeManager::Npm),
        _ => {}
    }
    lock_file_manager(dir)
}

/// The manager a directory's own lock file names, if it has one.
fn lock_file_manager(dir: &Path) -> Option<NodeManager> {
    if dir.join("pnpm-lock.yaml").is_file() {
        Some(NodeManager::Pnpm)
    } else if dir.join("yarn.lock").is_file() {
        Some(NodeManager::Yarn)
    } else if dir.join("package-lock.json").is_file() {
        Some(NodeManager::Npm)
    } else {
        None
    }
}

/// The package manager a `package.json` directory uses: what the nearest
/// directory from it up to the repository root declares (a workspace keeps its
/// lock and `packageManager` at its root), else npm.
fn node_manager(dir: &Path, repo_root: &Path) -> NodeManager {
    up_to_root(dir, repo_root)
        .find_map(declared_manager)
        .unwrap_or(NodeManager::Npm)
}

/// KT-1747 review — the root of the Node workspace `dir` is a package of,
/// relative to the repository: the nearest strict ancestor up to the
/// repository root with `pnpm-workspace.yaml` or a `workspaces` field. A
/// package with its own lock file is installed on its own, never hoisted.
fn node_workspace_of(dir: &Path, repo_root: &Path) -> Option<PathBuf> {
    if lock_file_manager(dir).is_some() {
        return None;
    }
    up_to_root(dir, repo_root)
        .skip(1)
        .find(|ancestor| {
            ancestor.join("pnpm-workspace.yaml").is_file()
                || package_json(ancestor).is_some_and(|value| value.get("workspaces").is_some())
        })
        .map(|ancestor| {
            ancestor
                .strip_prefix(repo_root)
                .unwrap_or(ancestor)
                .to_path_buf()
        })
}

fn has_make_file(dir: &Path) -> bool {
    ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .any(|name| dir.join(name).is_file())
}

fn has_xcode_project(entries: &[PathBuf]) -> bool {
    entries.iter().any(|path| {
        path.extension()
            .is_some_and(|ext| ext == "xcworkspace" || ext == "xcodeproj")
    })
}

/// Every toolchain of the checkout at `repo_root`, at any depth up to
/// [`MAX_DEPTH`]: Node (npm, yarn or pnpm), Composer, Cargo, CocoaPods, Xcode
/// and Make. Found from manifests and lock files only, so a repository of a
/// shape Kronn has never seen is described as well as its own. Dependency trees,
/// hidden directories and Xcode bundles are never entered.
///
/// Ordered by directory, package managers before build tools.
pub fn detect_toolchains(repo_root: &Path) -> Vec<PackageRoot> {
    let mut found: Vec<PackageRoot> = rust_roots(repo_root)
        .into_iter()
        .map(|root| PackageRoot {
            kind: ToolchainKind::Cargo,
            rel_dir: root.rel_dir,
            workspace: None,
        })
        .collect();
    collect_toolchains(repo_root, repo_root, 0, &mut found);
    found.sort_by(|a, b| {
        let key = |root: &PackageRoot| {
            (
                root.kind == ToolchainKind::Make,
                root.rel_dir.clone(),
                root.kind.rank(),
            )
        };
        key(a).cmp(&key(b))
    });
    found.truncate(MAX_PROFILE_ROOTS);
    found
}

fn collect_toolchains(repo_root: &Path, dir: &Path, depth: usize, found: &mut Vec<PackageRoot>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    let rel_dir = dir.strip_prefix(repo_root).unwrap_or(dir).to_path_buf();
    let mut push = |kind: ToolchainKind| {
        let workspace = match kind {
            ToolchainKind::Node(_) => node_workspace_of(dir, repo_root),
            _ => None,
        };
        found.push(PackageRoot {
            kind,
            rel_dir: rel_dir.clone(),
            workspace,
        });
    };
    if dir.join("package.json").is_file() {
        push(ToolchainKind::Node(node_manager(dir, repo_root)));
    }
    if dir.join("composer.json").is_file() {
        push(ToolchainKind::Composer);
    }
    if dir.join("Podfile").is_file() {
        push(ToolchainKind::CocoaPods);
    }
    if has_xcode_project(&entries) {
        push(ToolchainKind::Xcode);
    }
    if has_make_file(dir) {
        push(ToolchainKind::Make);
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let mut children: Vec<PathBuf> = entries
        .into_iter()
        // A real directory: a link is never followed (KT-1747 review).
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir()))
        .filter(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            !name.starts_with('.')
                && !SKIPPED_DIRS.contains(&name.as_str())
                && !SKIPPED_DEPENDENCY_DIRS.contains(&name.as_str())
                && !SKIPPED_PROFILE_DIRS.contains(&name.as_str())
                && !name.ends_with(".xcodeproj")
                && !name.ends_with(".xcworkspace")
        })
        .collect();
    children.sort();
    for child in children {
        collect_toolchains(repo_root, &child, depth + 1, found);
    }
}

/// The dependency directories (relative to the repository, `/` separated) a lane
/// layers read-only from the main checkout: the detected ones, or exactly the
/// ones the project names (`Some`, possibly empty to layer nothing). A named
/// entry that is not a directory inside the repository (absolute, `..`) is
/// returned in the second list and never used.
pub fn dependency_dirs(
    profile: &[PackageRoot],
    wanted: Option<&[String]>,
) -> (Vec<String>, Vec<String>) {
    let Some(wanted) = wanted else {
        let mut detected: Vec<String> = profile
            .iter()
            .filter_map(PackageRoot::dependency_path)
            .collect();
        detected.sort();
        detected.dedup();
        return (detected, Vec::new());
    };
    let mut chosen: Vec<String> = Vec::new();
    let mut rejected = Vec::new();
    for entry in wanted {
        let normalized = crate::models::normalize_lane_root(entry);
        let inside = !normalized.is_empty()
            && normalized != "."
            && !normalized.contains('\0')
            && Path::new(&normalized)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)));
        if !inside {
            rejected.push(entry.clone());
        } else if !chosen.contains(&normalized) {
            chosen.push(normalized);
        }
    }
    chosen.sort();
    (chosen, rejected)
}

/// What a launch tells the owner when the main checkout has not installed the
/// dependency directory `path` (relative to the repository): the directory and
/// the command that installs it, in the toolchain's own directory. The command
/// comes from the detected toolchain that owns `path`; a directory the project
/// named itself gets the generic wording.
/// KT-1751 — the cache variables of a project's package managers, once per
/// variable, in the profile's order (see [`ToolchainKind::cache_env`]).
pub fn cache_env(profile: &[PackageRoot]) -> Vec<(&'static str, &'static str)> {
    let mut found: Vec<(&'static str, &'static str)> = Vec::new();
    for root in profile {
        if let Some(entry) = root.kind.cache_env() {
            if !found.contains(&entry) {
                found.push(entry);
            }
        }
    }
    found
}

pub fn missing_dependency_notice(profile: &[PackageRoot], path: &str) -> String {
    let owner = profile
        .iter()
        .find(|root| root.dependency_path().as_deref() == Some(path));
    // A workspace package is installed by its workspace root.
    let owner = match owner.and_then(|root| root.workspace.as_ref()) {
        Some(workspace) => profile
            .iter()
            .find(|root| &root.rel_dir == workspace && matches!(root.kind, ToolchainKind::Node(_)))
            .or(owner),
        None => owner,
    };
    match owner.and_then(|root| root.kind.install_command().map(|cmd| (cmd, root.label()))) {
        Some((command, dir)) => {
            let place = if dir == "." {
                "the repository root".to_string()
            } else {
                format!("`{dir}`")
            };
            format!(
                "`{path}` is not installed in the main checkout, so this lane has no \
                 dependencies there: run `{command}` in {place} of the main checkout \
                 (a lane never installs)"
            )
        }
        None => format!(
            "`{path}` is not installed in the main checkout, so this lane has no dependencies \
             there: install the project's dependencies in the main checkout (a lane never installs)"
        ),
    }
}

/// One line per toolchain for the "Project conventions" section of a worker's
/// brief: what the project builds with and where it keeps its dependencies.
/// Empty when nothing was detected.
pub fn conventions_lines(profile: &[PackageRoot]) -> Vec<String> {
    conventions_lines_in(profile, false)
}

/// [`conventions_lines`] in the room's language (`fr`: French).
pub fn conventions_lines_in(profile: &[PackageRoot], fr: bool) -> Vec<String> {
    let (at, deps) = if fr {
        ("à", "dépendances dans")
    } else {
        ("at", "dependencies in")
    };
    profile
        .iter()
        .map(|root| match root.dependency_path() {
            Some(path) => format!(
                "{} {at} `{}` ({deps} `{path}/`)",
                root.kind.name(),
                root.label()
            ),
            None => format!("{} {at} `{}`", root.kind.name(), root.label()),
        })
        .collect()
}

/// The "Project conventions" section of a worker's brief (KT-1747), composed
/// from the toolchains detected in the worktree: what the project builds with and
/// where it keeps its dependencies, so a worker uses the project's own tools
/// instead of assuming Kronn's. Empty when nothing was detected.
pub fn conventions_section(profile: &[PackageRoot], fr: bool) -> String {
    conventions_section_from(&conventions_lines_in(profile, fr), fr)
}

/// [`conventions_section`] from the lines already composed (toolchains, the
/// project's own agent instructions, its commit format).
pub fn conventions_section_from(lines: &[String], fr: bool) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (heading, intro) = if fr {
        (
            "## Conventions du projet",
            "Détectées depuis les manifestes du dépôt ; utilise les outils du projet, pas ceux d'un autre :",
        )
    } else {
        (
            "## Project conventions",
            "Detected from the repository's manifests; use the project's own tools, not another project's:",
        )
    };
    let list: String = lines.iter().map(|line| format!("- {line}\n")).collect();
    format!("{heading}\n{intro}\n{list}\n")
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

#[cfg(test)]
#[path = "project_toolchains_detect_test.rs"]
mod detect_tests;
