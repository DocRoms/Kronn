//! KT-1742 — the settings a lane is warmed with: what is detected, what the
//! owner set for every project, what the owner set for one project.
//!
//! Resolution is one fold, project over global over detected, field by field:
//!
//! | setting | detected default |
//! |---|---|
//! | `compiler_env` | on macOS, `CFLAGS_<triple>=-save-temps=obj` when a Rust root of the project lists `cc` in its `Cargo.lock`; nothing otherwise |
//! | `seed_alignment` | on |
//! | `first_build_command` | none: the detected cargo warm-up of every Rust root |
//! | `first_build_timeout_min` | `KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS` when set on the host, else 60 (the variable only fills an unset config) |
//! | `rust_roots` (a project's own only) | every Rust root found from the manifests |
//! | `dependency_dirs` (a project's own only) | the dependency directory of every toolchain root found from the manifests (`node_modules`, `vendor`, `Pods`) |
//! | `validation_executables` (a project's own only) | none: Quick Exec's built-in list (KT-1751) |
//!
//! Every consumer of the lane's environment (the warm-up, the worker's launch,
//! its sandbox) asks [`effective_for`], so a build the worker runs later sees the
//! very environment the seed and the first build were made with. The owner's two
//! sources reach this module through [`rebuild`] (the whole snapshot from the
//! project table: both boots, an import, a reset), [`set_global`] (on every
//! config save) and [`set_project`] (on every save of a project's workspace
//! settings): a process-wide snapshot, like the other Settings >
//! Workers switches, so no caller has to carry a config around.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use crate::core::task_worker_env::compiler_temp_env_for;
use crate::models::{LaneWarmConfig, DEFAULT_LANE_FIRST_BUILD_TIMEOUT_MIN};

/// A lane's settings once every layer has been folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveLaneSettings {
    /// Build the project's own seed with [`Self::compiler_env`] before cloning it.
    pub seed_alignment: bool,
    /// The C compiler environment of the lane's builds and of the seed's.
    pub compiler_env: Vec<(String, String)>,
    /// A command that replaces the detected first build.
    pub first_build_command: Option<String>,
    /// The bound in force when no env var fills it: the configured one, else 60 min.
    pub first_build_timeout: Duration,
    /// Whether a project or the global setting gave `first_build_timeout`. When
    /// not, `KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS` (if set) fills it.
    pub first_build_timeout_configured: bool,
    /// The Rust roots to warm (directories relative to the repository), or
    /// `None` for every root found from the manifests. A project's own setting.
    pub rust_roots: Option<Vec<String>>,
    /// KT-1747 — the dependency directories to layer (relative to the
    /// repository), or `None` for the ones detected from the manifests. `Some`
    /// of an empty list layers nothing. A project's own setting.
    pub dependency_dirs: Option<Vec<String>>,
    /// KT-1751 — the executables the project's validations may run beyond
    /// Kronn's own allowlist (bare names, repository-relative paths). The
    /// human's decision for this project: empty unless the project lists some.
    pub validation_executables: Vec<String>,
}

impl EffectiveLaneSettings {
    /// The first-build bound the owner set (project, else global), if any.
    pub fn configured_first_build_timeout(&self) -> Option<Duration> {
        self.first_build_timeout_configured
            .then_some(self.first_build_timeout)
    }
}

/// What detection says about a project, computed only when a setting needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detected {
    pub compiler_env: Vec<(String, String)>,
}

impl Detected {
    /// The macOS clang default, only for a project that builds C through `cc`.
    /// `macos` and `builds_c` are parameters so the rule is testable anywhere.
    pub fn for_host(macos: bool, builds_c_through_cc: bool) -> Self {
        let env = if macos && builds_c_through_cc {
            compiler_temp_env_for(true)
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.to_string_lossy().into_owned()))
                .collect()
        } else {
            Vec::new()
        };
        Self { compiler_env: env }
    }
}

/// Fold the layers: the project's field when it has one, else the global one,
/// else the detected default. `detected` is only asked for when no layer sets
/// `compiler_env`, so a project that declares its own never pays for detection.
pub fn resolve(
    global: &LaneWarmConfig,
    project: Option<&LaneWarmConfig>,
    detected: impl FnOnce() -> Detected,
) -> EffectiveLaneSettings {
    let empty = LaneWarmConfig::default();
    let project = project.unwrap_or(&empty);
    let compiler_env = project
        .compiler_env_pairs()
        .or_else(|| global.compiler_env_pairs())
        .unwrap_or_else(|| detected().compiler_env);
    let configured_minutes = project
        .first_build_timeout_min
        .or(global.first_build_timeout_min);
    let minutes = configured_minutes
        .unwrap_or(DEFAULT_LANE_FIRST_BUILD_TIMEOUT_MIN)
        .max(1);
    EffectiveLaneSettings {
        seed_alignment: project
            .seed_alignment
            .or(global.seed_alignment)
            .unwrap_or(true),
        compiler_env,
        first_build_command: project
            .first_build_command
            .clone()
            .or_else(|| global.first_build_command.clone())
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty()),
        first_build_timeout: Duration::from_secs(u64::from(minutes) * 60),
        first_build_timeout_configured: configured_minutes.is_some(),
        rust_roots: project.rust_roots.clone().filter(|roots| !roots.is_empty()),
        dependency_dirs: project.dependency_dirs.clone(),
        // The global entry never adds one: what is trusted for one project
        // would widen every other.
        validation_executables: project.validation_executables.clone().unwrap_or_default(),
    }
}

// ─── The process-wide snapshot ────────────────────────────────────────────

struct Snapshot {
    global: LaneWarmConfig,
    /// Per project, by the project's main-checkout path.
    projects: BTreeMap<PathBuf, LaneWarmConfig>,
    /// KT-1751 third review — every registered project's checkout, with a lane
    /// entry or not: who owns a path is decided among all of them.
    registered: BTreeSet<PathBuf>,
    /// KT-1751 fifth review — false until a rebuild from the full project list
    /// succeeds: while false, `registered` may be partial (only what was saved
    /// since), so no path outside the managed layout is resolved to a project.
    /// A test build starts armed: its tests publish what they read.
    armed: bool,
}

static SNAPSHOT: std::sync::LazyLock<RwLock<Snapshot>> = std::sync::LazyLock::new(|| {
    RwLock::new(Snapshot {
        global: LaneWarmConfig {
            seed_alignment: None,
            compiler_env: None,
            first_build_command: None,
            first_build_timeout_min: None,
            rust_roots: None,
            dependency_dirs: None,
            validation_executables: None,
        },
        projects: BTreeMap::new(),
        registered: BTreeSet::new(),
        armed: crate::core::test_mode::on(),
    })
});

fn key_of(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    // KT-1751 fourth review — a folder that is gone (a deleted or moved
    // project): its deepest existing ancestor is canonicalised, so the key is
    // the one stored while it existed, even through a link (`/var` and
    // `/private/var` on macOS).
    let mut names = Vec::new();
    let mut dir = path;
    while let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
        names.push(name);
        if let Ok(real) = parent.canonicalize() {
            return names.iter().rev().fold(real, |key, name| key.join(name));
        }
        dir = parent;
    }
    path.to_path_buf()
}

/// Both spellings of a project's path: its key, and the path as given.
fn spellings_of(path: &Path) -> [PathBuf; 2] {
    [key_of(path), path.to_path_buf()]
}

/// KT-1751 fourth review — the whole snapshot, rebuilt from the global entry
/// and the FULL project list: every project registered, each one's lane entry
/// published, nothing left of a project that is not in the list. Called
/// wherever the project table is first read or replaced (both boots, an
/// import, a reset), through `crate::core::task_worker_lane_settings::arm_lane_settings`.
pub fn rebuild(global: &LaneWarmConfig, projects: &[crate::models::Project]) {
    let mut fresh = Snapshot {
        global: global.clone(),
        projects: BTreeMap::new(),
        registered: BTreeSet::new(),
        armed: true,
    };
    for project in projects {
        let key = key_of(Path::new(&project.path));
        fresh.registered.insert(key.clone());
        let entry = project
            .workspace
            .as_ref()
            .map(|workspace| &workspace.lane_warm)
            .filter(|config| !config.is_empty());
        if let Some(config) = entry {
            fresh.projects.insert(key, config.clone());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    if !test_guard::owns_snapshot() {
        return;
    }
    *SNAPSHOT.write().unwrap_or_else(|e| e.into_inner()) = fresh;
}

/// KT-1751 fifth review — the project listing failed: no project's list stays
/// in force, and nothing is looked up outside the managed layout until a
/// rebuild succeeds.
pub fn disarm(global: &LaneWarmConfig) {
    let empty = Snapshot {
        global: global.clone(),
        projects: BTreeMap::new(),
        registered: BTreeSet::new(),
        armed: false,
    };
    #[cfg(any(test, feature = "test-support"))]
    if !test_guard::owns_snapshot() {
        return;
    }
    *SNAPSHOT.write().unwrap_or_else(|e| e.into_inner()) = empty;
}

/// Publish Settings > Workers' lane entry to this process.
pub fn set_global(config: &LaneWarmConfig) {
    SNAPSHOT.write().unwrap_or_else(|e| e.into_inner()).global = config.clone();
}

/// Publish one project's own lane entry (`None` or an empty one removes it).
/// The project stays registered either way: it still owns its tree.
pub fn set_project(project_path: &Path, config: Option<&LaneWarmConfig>) {
    let key = key_of(project_path);
    let mut snapshot = SNAPSHOT.write().unwrap_or_else(|e| e.into_inner());
    snapshot.registered.insert(key.clone());
    match config.filter(|config| !config.is_empty()) {
        Some(config) => {
            snapshot.projects.insert(key, config.clone());
        }
        None => {
            snapshot.projects.remove(&key);
        }
    }
}

/// KT-1751 third review — record a newly registered project's checkout, so it
/// owns its tree before it has any lane entry. A new project has no lane entry
/// yet: one left at its path (by a project gone without being forgotten) is
/// dropped, never inherited.
///
/// KT-1751 fifth review — only a leftover is dropped: an entry whose path no
/// registered project holds. A second project at the path of a live one (no
/// duplicate check on create, a trailing `/` on add-folder) leaves the live
/// project's list in force, as its stored workspace still says.
pub fn register_project(project_path: &Path) {
    let mut snapshot = SNAPSHOT.write().unwrap_or_else(|e| e.into_inner());
    let spellings = spellings_of(project_path);
    let live = spellings
        .iter()
        .any(|spelling| snapshot.registered.contains(spelling));
    if !live {
        for spelling in &spellings {
            snapshot.projects.remove(spelling);
        }
    }
    snapshot.registered.insert(key_of(project_path));
}

/// A deleted project: its lane entry and its registration both go, under
/// either spelling of its path.
pub fn forget_project(project_path: &Path) {
    let mut snapshot = SNAPSHOT.write().unwrap_or_else(|e| e.into_inner());
    for spelling in spellings_of(project_path) {
        snapshot.projects.remove(&spelling);
        snapshot.registered.remove(&spelling);
    }
}

/// A project whose checkout moved (a remap, a re-clone): its lane entry and
/// its registration follow it to the new path, found under either spelling of
/// the old one.
pub fn rekey_project(old_path: &Path, new_path: &Path) {
    let new = key_of(new_path);
    let mut snapshot = SNAPSHOT.write().unwrap_or_else(|e| e.into_inner());
    let mut moved = None;
    for spelling in spellings_of(old_path) {
        moved = snapshot.projects.remove(&spelling).or(moved);
        snapshot.registered.remove(&spelling);
    }
    if let Some(config) = moved {
        snapshot.projects.insert(new.clone(), config);
    }
    snapshot.registered.insert(new);
}

/// The settings of the lane of `worktree`, whose project lives at
/// `main_checkout` (a worktree that is not a managed one is its own project).
pub fn effective_for(worktree: &Path, main_checkout: Option<&Path>) -> EffectiveLaneSettings {
    let (global, project) = {
        let snapshot = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner());
        (
            snapshot.global.clone(),
            snapshot
                .projects
                .get(&key_of(main_checkout.unwrap_or(worktree)))
                .cloned(),
        )
    };
    resolve(&global, project.as_ref(), || {
        Detected::for_host(
            cfg!(target_os = "macos"),
            crate::core::project_toolchains::builds_c_through_cc(worktree),
        )
    })
}

/// KT-1751 — the executables the project that owns `worktree` lists for its
/// validations. A managed worktree (`<repo>/.kronn/worktrees/<name>`) belongs
/// to `<repo>`; a tree anywhere else (a configured worktree root, an autoCode
/// root, a sub-directory) belongs to the registered project whose own worktree
/// list names it. A tree of no registered project gets nothing.
///
/// KT-1751 fifth review — until the snapshot is armed (a rebuild from the full
/// project list succeeded), only the managed layout is resolved: a registry
/// filled by saves alone may miss a nested project, and would widen.
pub fn validation_executables_for(worktree: &Path) -> Vec<String> {
    let worktree = key_of(worktree);
    let managed = crate::core::task_worker_env::WorkerLane::for_worktree(&worktree)
        .ok()
        .and_then(|lane| lane.main_checkout);
    let armed = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner()).armed;
    let main_checkout = match managed {
        Some(checkout) => Some(checkout),
        None if armed => project_listing(&worktree),
        None => return Vec::new(),
    };
    effective_for(&worktree, main_checkout.as_deref()).validation_executables
}

/// The registered project that owns `path`: the one whose checkout, or one of
/// whose worktrees (as its own `git worktree list` says), is the most specific
/// tree containing `path`. Its list is what runs there, empty when it has none.
///
/// KT-1751 third review — the owner is chosen among EVERY registered project,
/// with a lane entry or not: a project registered inside another one (a
/// monorepo sub-folder, with or without a repository of its own) owns its own
/// files and never gets the outer project's list. At equal depth a project's
/// own checkout beats a tree it only shares through a common repository; a tie
/// that is still left (two projects of one repository, a worktree of it)
/// resolves to no project. A repository nested between the owning tree and
/// `path` owns `path` instead. Every doubt narrows what runs, never widens it.
fn project_listing(path: &Path) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = {
        let snapshot = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner());
        snapshot
            .registered
            .iter()
            .chain(snapshot.projects.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let rank = |project: &PathBuf, tree: &PathBuf| (tree.components().count(), tree == project);
    let mut owners: Vec<((usize, bool), PathBuf, PathBuf)> = candidates
        .into_iter()
        .filter_map(|project| {
            let mut trees = vec![project.clone()];
            trees.extend(
                crate::core::worktree::registered_worktree_paths(&project)
                    .iter()
                    .map(|tree| key_of(tree)),
            );
            let tree = trees
                .into_iter()
                .filter(|tree| path.starts_with(tree))
                .max_by_key(|tree| rank(&project, tree))?;
            Some((rank(&project, &tree), project, tree))
        })
        .collect();
    owners.sort_by_key(|(rank, _, _)| std::cmp::Reverse(*rank));
    let (best, project, tree) = owners.first()?.clone();
    if owners.get(1).is_some_and(|(next, _, _)| *next == best) {
        return None;
    }
    let nested = path
        .ancestors()
        .take_while(|dir| *dir != tree.as_path())
        .any(|dir| dir.join(".git").exists());
    (!nested).then_some(project)
}

/// KT-1751 — every project's listed executables, deduplicated: what the intake
/// of a validation (a campaign, a swap, a launch that carries no project) is
/// checked against. The authoritative check is the run's own, per project; this
/// one only keeps a name no project lists from being accepted at all.
pub fn all_validation_executables() -> Vec<String> {
    let snapshot = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<String> = Vec::new();
    for config in snapshot.projects.values() {
        for entry in config.validation_executables.iter().flatten() {
            if !all.contains(entry) {
                all.push(entry.clone());
            }
        }
    }
    all
}

/// The lane's compiler environment as variables a command can carry. A name is
/// interned: an environment is `&'static str`-named across the lane code, and
/// the set of names an owner declares is tiny and fixed.
pub fn compiler_env_for(
    worktree: &Path,
    main_checkout: Option<&Path>,
) -> Vec<(&'static str, OsString)> {
    effective_for(worktree, main_checkout)
        .compiler_env
        .into_iter()
        .map(|(name, value)| (intern(&name), OsString::from(value)))
        .collect()
}

fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// KT-1751 fourth review — the snapshot is process-wide, and `cargo test` runs
/// tests as threads of one process: a test that rebuilds it would wipe what a
/// concurrent test just published. A test that publishes then reads holds
/// `shared()`; a test that rebuilds holds `exclusive()`, and a rebuild only
/// takes effect on the thread that holds it (an import or a test elsewhere
/// that rebuilds as a side effect leaves the others' entries alone).
/// A tokio lock (held across awaits by async tests, as clippy requires); a sync
/// test takes it with the `blocking_` forms, an async one with `.await`.
/// KT-1751 fourth review — load the lane snapshot from the database: every
/// project registered (with a lane entry or not) and each one's lane entry
/// published, beside the global entry. Both boots call it before the first
/// launch; an import and a reset call it once the project table is replaced.
pub async fn arm_lane_settings(
    database: &std::sync::Arc<crate::db::Database>,
    global: &crate::models::LaneWarmConfig,
) -> anyhow::Result<()> {
    let listed = database
        .with_read_conn(crate::db::projects::list_projects)
        .await;
    #[cfg(any(test, feature = "test-support"))]
    let listed = listed.and_then(|projects| {
        if test_guard::listing_fails() {
            anyhow::bail!("the project table could not be read (test)")
        }
        Ok(projects)
    });
    // KT-1751 fifth review — a listing that fails leaves no snapshot in force:
    // emptied and unarmed, so neither a stale list nor a registry later filled
    // by saves alone applies outside the managed layout.
    let projects = match listed {
        Ok(projects) => projects,
        Err(error) => {
            disarm(global);
            return Err(error);
        }
    };
    rebuild(global, &projects);
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_guard {
    use std::cell::Cell;
    use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    static GUARD: RwLock<()> = RwLock::const_new(());

    thread_local! {
        static OWNER: Cell<bool> = const { Cell::new(false) };
    }

    pub fn shared() -> RwLockReadGuard<'static, ()> {
        GUARD.blocking_read()
    }

    pub async fn shared_async() -> RwLockReadGuard<'static, ()> {
        GUARD.read().await
    }

    pub struct Exclusive {
        _guard: RwLockWriteGuard<'static, ()>,
    }

    impl Drop for Exclusive {
        fn drop(&mut self) {
            OWNER.with(|owner| owner.set(false));
            // A test that disarmed (even one that panicked) hands the others
            // an armed snapshot, as the test build starts.
            super::SNAPSHOT
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .armed = true;
        }
    }

    fn own(guard: RwLockWriteGuard<'static, ()>) -> Exclusive {
        OWNER.with(|owner| owner.set(true));
        Exclusive { _guard: guard }
    }

    pub fn exclusive() -> Exclusive {
        own(GUARD.blocking_write())
    }

    pub async fn exclusive_async() -> Exclusive {
        own(GUARD.write().await)
    }

    pub fn owns_snapshot() -> bool {
        OWNER.with(Cell::get)
    }

    thread_local! {
        static LISTING_FAILS: Cell<bool> = const { Cell::new(false) };
    }

    /// While held, `crate::core::task_worker_lane_settings::arm_lane_settings` on this thread fails as if the
    /// project table could not be read (a `#[tokio::test]` runs on one thread).
    pub struct FailingListing;

    impl Drop for FailingListing {
        fn drop(&mut self) {
            LISTING_FAILS.with(|fails| fails.set(false));
        }
    }

    pub fn fail_listing() -> FailingListing {
        LISTING_FAILS.with(|fails| fails.set(true));
        FailingListing
    }

    pub fn listing_fails() -> bool {
        LISTING_FAILS.with(Cell::get)
    }
}

#[cfg(test)]
#[path = "task_worker_lane_settings_test.rs"]
mod tests;

#[cfg(any(test, feature = "test-support"))]
#[path = "task_worker_lane_settings_registry_test.rs"]
pub mod registry_tests;

#[cfg(test)]
#[path = "task_worker_lane_settings_deps_test.rs"]
mod deps_tests;

#[cfg(test)]
#[path = "task_worker_lane_settings_exec_test.rs"]
mod exec_tests;
