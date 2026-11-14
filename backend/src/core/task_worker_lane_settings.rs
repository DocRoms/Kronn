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
//! | `first_build_timeout_min` | 60 |
//! | `rust_roots` (a project's own only) | every Rust root found from the manifests |
//!
//! Every consumer of the lane's environment (the warm-up, the worker's launch,
//! its sandbox) asks [`effective_for`], so a build the worker runs later sees the
//! very environment the seed and the first build were made with. The owner's two
//! sources reach this module through [`set_global`] (on every config load and
//! save) and [`set_project`] (at boot, and on every save of a project's
//! workspace settings): a process-wide snapshot, like the other Settings >
//! Workers switches, so no caller has to carry a config around.

use std::collections::{BTreeMap, HashSet};
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
    pub first_build_timeout: Duration,
    /// The Rust roots to warm (directories relative to the repository), or
    /// `None` for every root found from the manifests. A project's own setting.
    pub rust_roots: Option<Vec<String>>,
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
    let minutes = project
        .first_build_timeout_min
        .or(global.first_build_timeout_min)
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
        rust_roots: project.rust_roots.clone().filter(|roots| !roots.is_empty()),
    }
}

// ─── The process-wide snapshot ────────────────────────────────────────────

struct Snapshot {
    global: LaneWarmConfig,
    /// Per project, by the project's main-checkout path.
    projects: BTreeMap<PathBuf, LaneWarmConfig>,
}

static SNAPSHOT: RwLock<Snapshot> = RwLock::new(Snapshot {
    global: LaneWarmConfig {
        seed_alignment: None,
        compiler_env: None,
        first_build_command: None,
        first_build_timeout_min: None,
        rust_roots: None,
    },
    projects: BTreeMap::new(),
});

fn key_of(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Publish Settings > Workers' lane entry to this process.
pub fn set_global(config: &LaneWarmConfig) {
    SNAPSHOT.write().unwrap_or_else(|e| e.into_inner()).global = config.clone();
}

/// Publish one project's own lane entry (`None` or an empty one removes it).
pub fn set_project(project_path: &Path, config: Option<&LaneWarmConfig>) {
    let key = key_of(project_path);
    let mut snapshot = SNAPSHOT.write().unwrap_or_else(|e| e.into_inner());
    match config.filter(|config| !config.is_empty()) {
        Some(config) => {
            snapshot.projects.insert(key, config.clone());
        }
        None => {
            snapshot.projects.remove(&key);
        }
    }
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

#[cfg(test)]
#[path = "task_worker_lane_settings_test.rs"]
mod tests;
