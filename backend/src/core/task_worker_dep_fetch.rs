//! KT-1649 — fetch what a worker's lock files reference and the host cache lacks.
//!
//! A task worker is offline (`CARGO_NET_OFFLINE`, a sandbox with no outbound
//! connection) and reads the host's dependency caches read-only. A worker that
//! bumped a dependency (KT-1647: `utoipa-swagger-ui` 10.0.1 → 10.0.2) therefore
//! referenced a crate no cache held, could neither compile nor test, and
//! delivered blind until the principal ran `cargo fetch --locked` by hand.
//!
//! Before the lane's first build, Kronn now fills the cache itself, **outside
//! the worker's sandbox, from the lock file, and from nowhere else**. The
//! worker writes the lock file, the manifests and the tool configuration, so
//! none of it is ever handed to the command as it is:
//!
//! * the lock file is the whole input, and it is *parsed*, never scanned.
//!   `Cargo.lock` is TOML: every package source must be crates.io, and a lock
//!   naming a git dependency or an alternate registry is refused as a whole.
//!   `pnpm-lock.yaml` is parsed with a YAML parser (anchors, aliases, merge
//!   keys, folded and flow scalars all resolved) and every entry of `packages`
//!   must be a registry package whose resolution is an `integrity` hash, plus
//!   at most a tarball on `registry.npmjs.org`; a tarball elsewhere, a git, a
//!   directory or any other resolution refuses the whole lock. Nothing is
//!   fetched from a refused lock;
//! * a lock file larger than [`MAX_LOCK_BYTES`] (16 MiB) is refused unread, and
//!   a workspace file larger than [`MAX_WORKSPACE_BYTES`] (1 MiB) as well;
//! * a probe comes first: `cargo fetch --offline` (and its pnpm twin). A cache
//!   that already holds everything never touches the network;
//! * **the command never runs in, or reads, the worktree.** Cargo takes its
//!   configuration (source replacement, registries, proxies) from the working
//!   directory upwards, and a worktree `Cargo.toml` can carry `[patch]`,
//!   `[source]` or `[registries]`; pnpm reads `.npmrc` and `pnpm-workspace.yaml`
//!   from the project. So both run in a private scratch directory holding only
//!   what Kronn generated from the validated lock: for Cargo a lock of the
//!   registry packages and a manifest depending on each of them at its exact
//!   locked version, for pnpm a lock rebuilt from an allow-list (the
//!   `lockfileVersion` and the validated `packages` and `snapshots`: `settings`,
//!   `overrides`, `patchedDependencies` and any other field are dropped, so
//!   what pnpm parses is what Kronn validated), an empty manifest and a
//!   workspace file rebuilt from an allow-list of inert keys (the registry,
//!   proxy, store, cache, hook and certificate settings are never copied). No
//!   `.cargo/` and no `.npmrc` is staged. Neither command runs a build script
//!   or a lifecycle script (`cargo fetch` builds nothing, pnpm gets
//!   `--ignore-scripts`) and the registry and the store are given on the command
//!   line;
//! * **the environment is cleared** (`ProbeCommand::clean_env`) and rebuilt from
//!   an allow-list: `PATH`, the locale, the toolchain homes, the operator's proxy
//!   and CA variables, and the directories Kronn chose. A host
//!   `NPM_CONFIG_REGISTRY`, a `CARGO_REGISTRIES_*` override or a registry token in
//!   the environment reaches neither command;
//! * **the operator's home is not theirs to read.** `HOME`, `USERPROFILE`,
//!   `APPDATA`, `LOCALAPPDATA` and `XDG_CONFIG_HOME` point into the scratch
//!   directory, so `~/.npmrc`, `~/.config/pnpm/rc` and `~/.cargo` are out of reach;
//!   Cargo's `CARGO_HOME` is a staged directory that links only the host's
//!   `registry` (and `git`) — never `credentials.toml` or `config.toml`;
//! * the scratch directory lives under Kronn's data directory (which the worker
//!   cannot read), is removed when the step ends, and anything a killed process
//!   left behind is swept at startup ([`sweep_stale_scratch`]);
//! * every step is journalled on the execution (`dependency_fetch`): the lock,
//!   the registries it was bounded to, the outcome and its duration.
//!
//! Best effort, like the rest of the warm-up: a failed or refused fetch is
//! recorded and the worker still starts, with the cache it has.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::background_priority::Priority;
use crate::core::task_worker_env::{
    ensure_symlink, ProbeCommand, ProbeRunner, SharedCaches, WorkerLane, CARGO_HOME_ENV,
    CARGO_NET_OFFLINE_ENV,
};

/// `task_execution_events.action` of the fetch record.
pub const DEP_FETCH_EVENT_ACTION: &str = "dependency_fetch";

/// The only `source` a `Cargo.lock` package may carry for Kronn to fetch it:
/// crates.io, in its git-index and its sparse form.
pub const CRATES_IO_SOURCES: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// The registry a pnpm fetch is pinned to on the command line.
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

/// The directory, under Kronn's data directory, the scratch projects live in.
pub const SCRATCH_DIR_NAME: &str = "dep-fetch";
const SCRATCH_PREFIX: &str = "kronn-dep-fetch-";

const CARGO_PROBE_TIMEOUT: Duration = Duration::from_secs(120);
const CARGO_FETCH_TIMEOUT: Duration = Duration::from_secs(600);
const PNPM_PROBE_TIMEOUT: Duration = Duration::from_secs(120);
const PNPM_FETCH_TIMEOUT: Duration = Duration::from_secs(600);
/// A lock file larger than this is refused unread: real locks are a few MiB.
pub const MAX_LOCK_BYTES: u64 = 16 * 1024 * 1024;
/// Same cap for `pnpm-workspace.yaml`.
pub const MAX_WORKSPACE_BYTES: u64 = 1024 * 1024;

const FRONTEND_DIR: &str = "frontend";
const PNPM_LOCK: &str = "pnpm-lock.yaml";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";
/// What a staged pnpm project holds instead of the worker's `package.json`.
const STAGED_MANIFEST: &str = "{\"name\":\"kronn-lane-dependency-fetch\",\"private\":true}\n";
/// An empty `.npmrc`, which the user and global npm configuration are pointed at.
const EMPTY_NPMRC: &str = "empty.npmrc";
const STAGED_CARGO_PACKAGE: &str = "kronn-lane-dependency-fetch";
/// The home directory of a fetch command, inside its scratch project.
const SCRATCH_HOME: &str = "home";
/// The `CARGO_HOME` of a cargo fetch, inside its scratch project: it links the
/// host's `registry` (and `git`), and nothing else.
const STAGED_CARGO_HOME: &str = "cargo-home";

/// The only keys of `pnpm-workspace.yaml` copied to the scratch project. Every
/// one is inert data a fetch does not act on (script policy, overrides, catalogs,
/// the platforms to fetch for); everything else is dropped — in particular
/// `packages` (globs that would walk the host), the registries and proxies, the
/// store and cache directories, `pnpmfile`/hooks, certificates and TLS switches.
const WORKSPACE_KEYS: [&str; 9] = [
    "allowBuilds",
    "onlyBuiltDependencies",
    "neverBuiltDependencies",
    "overrides",
    "packageExtensions",
    "catalog",
    "catalogs",
    "ignoredOptionalDependencies",
    "supportedArchitectures",
];

/// What a fetch command inherits from Kronn's environment: nothing else.
/// Credentials and registry settings (`NPM_CONFIG_*`, `CARGO_REGISTRIES_*`,
/// `CARGO_HTTP_*`, tokens) are not on it, and neither is any home directory
/// (`HOME`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `HOMEDRIVE`, `HOMEPATH`,
/// `XDG_*`): a fetch is given the scratch project's own ([`Scratch::home_env`]),
/// so `~/.npmrc`, `~/.config/pnpm/rc` and `~/.cargo` are out of its reach.
const FETCH_ENV_NAMES: [&str; 27] = [
    "PATH",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    // The operator's own network path to the registry.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    // Where the toolchains the commands are shims of live.
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "PNPM_HOME",
    "COREPACK_HOME",
    "VOLTA_HOME",
    // Windows: programs fail to start without these.
    "SystemRoot",
    "windir",
    "ComSpec",
    "PATHEXT",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Cargo,
    Pnpm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FetchOutcome {
    /// The cache already held everything the lock references: no network.
    Cached,
    /// The missing packages were fetched into the host cache.
    Fetched,
    /// The lock references something outside the allow-list (or the step could
    /// not be set up safely): nothing was fetched.
    Refused { reason: String },
    /// The fetch ran and failed.
    Failed { detail: String },
    /// The execution was cancelled during the fetch.
    Cancelled,
}

/// One lock file's fetch step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepFetchStep {
    pub ecosystem: Ecosystem,
    /// The lock file, relative to the worktree.
    pub lockfile: String,
    /// The registries the step was bounded to; the only hosts it could reach.
    pub registries: Vec<String>,
    pub outcome: FetchOutcome,
    pub duration_ms: u64,
}

/// What the launch fetched, one step per lock file that had something to fetch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepFetchRecord {
    pub steps: Vec<DepFetchStep>,
}

impl DepFetchRecord {
    /// Whether a step reached the network and brought something in.
    pub fn fetched_anything(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.outcome == FetchOutcome::Fetched)
    }

    pub fn cancelled(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.outcome == FetchOutcome::Cancelled)
    }
}

/// What a fetch step runs with: where its scratch projects go, the environment
/// the allow-list is taken from, and the (unconfined, networked) runner.
#[derive(Clone, Copy)]
pub struct FetchHost<'a> {
    pub scratch_root: &'a Path,
    /// Kronn's own environment, of which only [`FETCH_ENV_NAMES`] is kept.
    pub env: &'a [(OsString, OsString)],
    pub runner: &'a dyn ProbeRunner,
    pub cancel: &'a CancellationToken,
}

// ─── Lock files ───────────────────────────────────────────────────────────

/// What a `Cargo.lock` asks Cargo to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoLockScan {
    /// Packages with a registry source (path dependencies have none).
    pub registry_packages: usize,
    /// The distinct registries they come from, all allow-listed.
    pub registries: Vec<String>,
    /// `(name, version)` of each registry package, in lock order.
    pub crates: Vec<(String, String)>,
    /// The lock the scratch project gets: the version and the registry
    /// packages, with nothing else the worker's file carried.
    pub lock: String,
}

/// Read a `Cargo.lock`: the registries it references, or why it is out of
/// bounds. Any source other than crates.io — a git dependency, an alternate
/// registry — refuses the whole lock, because `cargo fetch` fetches all of it.
pub fn scan_cargo_lock(text: &str) -> Result<CargoLockScan, String> {
    let table: toml::Table = toml::from_str(text)
        .map_err(|error| format!("Cargo.lock is not valid TOML: {}", first_line(&error)))?;
    let mut scan = CargoLockScan {
        registry_packages: 0,
        registries: Vec::new(),
        crates: Vec::new(),
        lock: String::new(),
    };
    let packages = match table.get("package") {
        None => return Ok(scan),
        Some(toml::Value::Array(packages)) => packages,
        Some(_) => return Err("Cargo.lock `package` is not an array".into()),
    };
    let mut kept = Vec::new();
    for package in packages {
        let Some(source) = package.get("source") else {
            continue;
        };
        let Some(source) = source.as_str() else {
            return Err("a Cargo.lock package has a `source` that is not a string".into());
        };
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or("?");
        if !CRATES_IO_SOURCES.contains(&source) {
            return Err(format!(
                "`{}` comes from `{}`, which is not crates.io: Kronn only fetches \
                 from the lock's crates.io packages",
                shorten(name),
                shorten(source)
            ));
        }
        let version = package
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or("?");
        if !is_crate_name(name) || !is_crate_version(version) {
            return Err(format!(
                "a Cargo.lock package `{}` {} has a name or a version Kronn will not stage",
                shorten(name),
                shorten(version)
            ));
        }
        let mut entry = toml::Table::new();
        for field in ["name", "version", "source"] {
            if let Some(value) = package.get(field) {
                entry.insert(field.into(), value.clone());
            }
        }
        if let Some(checksum) = package.get("checksum") {
            if !checksum.is_str() {
                return Err(format!("`{}` has a checksum that is not a string", name));
            }
            entry.insert("checksum".into(), checksum.clone());
        }
        if let Some(dependencies) = package.get("dependencies") {
            let all_strings = dependencies
                .as_array()
                .is_some_and(|list| list.iter().all(toml::Value::is_str));
            if !all_strings {
                return Err(format!("`{name}` has dependencies that are not strings"));
            }
            entry.insert("dependencies".into(), dependencies.clone());
        }
        kept.push(toml::Value::Table(entry));
        scan.crates.push((name.to_string(), version.to_string()));
        scan.registry_packages += 1;
        if !scan.registries.iter().any(|known| known == source) {
            scan.registries.push(source.to_string());
        }
    }
    scan.registries.sort();
    if scan.registry_packages > 0 {
        let version = match table.get("version") {
            Some(toml::Value::Integer(version)) if *version >= 3 => *version,
            _ => {
                return Err(
                    "Cargo.lock is not a version 3 or later lock file: Kronn does not stage it"
                        .into(),
                )
            }
        };
        let mut lock = toml::Table::new();
        lock.insert("version".into(), toml::Value::Integer(version));
        lock.insert("package".into(), toml::Value::Array(kept));
        scan.lock = toml::to_string(&lock)
            .map_err(|error| format!("cannot write the staged lock: {error}"))?;
    }
    Ok(scan)
}

/// The `Cargo.toml` of the scratch project: no worker setting, just a package
/// depending on every locked crate at its exact version, with default features
/// off so the resolve adds nothing the lock does not already hold.
pub fn staged_cargo_manifest(crates: &[(String, String)]) -> Result<String, String> {
    let mut dependencies = toml::Table::new();
    for (index, (name, version)) in crates.iter().enumerate() {
        let mut dependency = toml::Table::new();
        dependency.insert("package".into(), toml::Value::String(name.clone()));
        dependency.insert("version".into(), toml::Value::String(format!("={version}")));
        dependency.insert("default-features".into(), toml::Value::Boolean(false));
        dependencies.insert(format!("dep{index}"), toml::Value::Table(dependency));
    }
    let mut package = toml::Table::new();
    package.insert("name".into(), STAGED_CARGO_PACKAGE.into());
    package.insert("version".into(), "0.0.0".into());
    package.insert("edition".into(), "2021".into());
    package.insert("publish".into(), toml::Value::Boolean(false));
    let mut lib = toml::Table::new();
    lib.insert("path".into(), "lib.rs".into());
    let mut manifest = toml::Table::new();
    manifest.insert("package".into(), toml::Value::Table(package));
    manifest.insert("lib".into(), toml::Value::Table(lib));
    manifest.insert("dependencies".into(), toml::Value::Table(dependencies));
    toml::to_string(&manifest).map_err(|error| format!("cannot write the staged manifest: {error}"))
}

fn is_crate_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_crate_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 128
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// A `pnpm-lock.yaml` that is in bounds, as Kronn rebuilt it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PnpmLockScan {
    /// The lock the scratch project gets, rebuilt from an allow-list: the
    /// `lockfileVersion`, the validated `packages` and `snapshots`, and nothing
    /// else the worker's file carried (`settings`, `overrides`,
    /// `patchedDependencies`, `pnpmfileChecksum`, `importers`, any other field
    /// of an entry). What the scratch project's pnpm parses is what Kronn
    /// validated, not the worker's text (two YAML parsers may read an exotic
    /// file differently).
    pub lock: String,
    pub packages: usize,
}

/// How a field of a `packages` or `snapshots` entry is validated.
#[derive(Clone, Copy)]
enum FieldShape {
    Flag,
    Text,
    Strings,
    FlagOrStrings,
    /// Names to version ranges (`engines`, `peerDependencies`).
    RangeMap,
    /// Names to a version or the key of another snapshot.
    ReferenceMap,
    /// `peerDependenciesMeta`: names to `{optional: bool}`.
    MetaMap,
}

/// The fields of a `packages` entry kept besides `resolution`. They steer what a
/// fetch downloads (`os`, `cpu` and `libc` filter optional packages) or are
/// inert metadata; any other field (`version`, `name`, `prepare`, …) is dropped.
const PNPM_PACKAGE_FIELDS: [(&str, FieldShape); 9] = [
    ("engines", FieldShape::RangeMap),
    ("cpu", FieldShape::Strings),
    ("os", FieldShape::Strings),
    ("libc", FieldShape::Strings),
    ("deprecated", FieldShape::Text),
    ("hasBin", FieldShape::Flag),
    ("bundledDependencies", FieldShape::FlagOrStrings),
    ("peerDependencies", FieldShape::RangeMap),
    ("peerDependenciesMeta", FieldShape::MetaMap),
];

/// The fields of a `snapshots` entry that are kept: the graph pnpm walks.
const PNPM_SNAPSHOT_FIELDS: [(&str, FieldShape); 4] = [
    ("dependencies", FieldShape::ReferenceMap),
    ("optionalDependencies", FieldShape::ReferenceMap),
    ("transitivePeerDependencies", FieldShape::Strings),
    ("optional", FieldShape::Flag),
];

/// Read a `pnpm-lock.yaml` with a YAML parser, walk every entry of `packages`
/// and `snapshots`, and rebuild the lock from an allow-list. `Ok(None)` when it
/// lists no package. The lock names no resolution of its own: each entry must
/// be a registry package (its key carries no URL or path) whose `resolution` is
/// an `integrity` hash, plus at most a `tarball` on [`NPM_REGISTRY`]'s host. A
/// `tarball` anywhere else, `type`, `repo`, `commit`, `directory`, `path` or any
/// other resolution field refuses the whole lock — however the YAML spells it
/// (anchors and aliases, merge keys, folded or flow scalars all resolve before
/// the walk; a tag or a non-string value is refused). Fields outside the
/// allow-list are stripped, not copied.
pub fn scan_pnpm_lock(text: &str) -> Result<Option<PnpmLockScan>, String> {
    let mut document: serde_yaml::Value = serde_yaml::from_str(text)
        .map_err(|error| format!("pnpm-lock.yaml is not valid YAML: {}", first_line(&error)))?;
    document
        .apply_merge()
        .map_err(|error| format!("pnpm-lock.yaml has a merge key Kronn cannot resolve: {error}"))?;
    // The accessors below see through a tag (`!x {…}` reads as the mapping):
    // a lock has none, so one anywhere is refused rather than reasoned about.
    if contains_tag(&document) {
        return Err("pnpm-lock.yaml carries a YAML tag".into());
    }
    let Some(root) = document.as_mapping() else {
        return Err("pnpm-lock.yaml is not a mapping".into());
    };
    let packages = match root.get("packages") {
        None | Some(serde_yaml::Value::Null) => return Ok(None),
        Some(serde_yaml::Value::Mapping(packages)) => packages,
        Some(_) => return Err("pnpm-lock.yaml `packages` is not a mapping".into()),
    };
    if packages.is_empty() {
        return Ok(None);
    }
    let version = match root.get("lockfileVersion") {
        Some(version @ serde_yaml::Value::String(text)) if is_lock_version(text) => version.clone(),
        Some(version @ serde_yaml::Value::Number(_)) => version.clone(),
        _ => return Err("pnpm-lock.yaml has no usable `lockfileVersion`".into()),
    };
    let mut kept_packages = serde_yaml::Mapping::new();
    for (key, entry) in packages {
        let name = pnpm_key(key)?;
        kept_packages.insert(key.clone(), rebuild_pnpm_package(name, entry)?);
    }
    let mut kept_snapshots = serde_yaml::Mapping::new();
    match root.get("snapshots") {
        None | Some(serde_yaml::Value::Null) => {}
        Some(serde_yaml::Value::Mapping(snapshots)) => {
            for (key, entry) in snapshots {
                let name = pnpm_key(key)?;
                let kept = rebuild_pnpm_entry(name, entry, &PNPM_SNAPSHOT_FIELDS)?;
                kept_snapshots.insert(key.clone(), serde_yaml::Value::Mapping(kept));
            }
        }
        Some(_) => return Err("pnpm-lock.yaml `snapshots` is not a mapping".into()),
    }
    let count = kept_packages.len();
    // The one project is the staged empty manifest's, whatever the worker's said.
    let mut importers = serde_yaml::Mapping::new();
    importers.insert(".".into(), serde_yaml::Value::Mapping(Default::default()));
    let mut lock = serde_yaml::Mapping::new();
    lock.insert("lockfileVersion".into(), version);
    lock.insert("importers".into(), serde_yaml::Value::Mapping(importers));
    lock.insert("packages".into(), serde_yaml::Value::Mapping(kept_packages));
    if !kept_snapshots.is_empty() {
        lock.insert(
            "snapshots".into(),
            serde_yaml::Value::Mapping(kept_snapshots),
        );
    }
    let lock = serde_yaml::to_string(&serde_yaml::Value::Mapping(lock))
        .map_err(|error| format!("cannot write the staged pnpm lock: {error}"))?;
    Ok(Some(PnpmLockScan {
        lock,
        packages: count,
    }))
}

fn is_lock_version(text: &str) -> bool {
    !text.is_empty() && text.len() <= 16 && text.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn contains_tag(value: &serde_yaml::Value) -> bool {
    use serde_yaml::Value;
    match value {
        Value::Tagged(_) => true,
        Value::Sequence(items) => items.iter().any(contains_tag),
        Value::Mapping(map) => map
            .iter()
            .any(|(key, value)| contains_tag(key) || contains_tag(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

/// A `packages` / `snapshots` key: a plain string naming a registry package (it
/// carries no URL or path).
fn pnpm_key(key: &serde_yaml::Value) -> Result<&str, String> {
    let Some(name) = key.as_str() else {
        return Err("a pnpm-lock.yaml `packages` key is not a plain string".into());
    };
    if name.is_empty()
        || name.len() > 512
        || name
            .chars()
            .any(|c| c == ':' || c == '\\' || c.is_whitespace() || c.is_control())
    {
        return Err(format!(
            "pnpm-lock.yaml package `{}` is not a registry package: its key carries a URL or a path",
            shorten(name)
        ));
    }
    Ok(name)
}

/// A `packages` entry rebuilt from its validated `resolution` and the
/// [`PNPM_PACKAGE_FIELDS`] it carries.
fn rebuild_pnpm_package(
    name: &str,
    entry: &serde_yaml::Value,
) -> Result<serde_yaml::Value, String> {
    let resolution = entry
        .as_mapping()
        .and_then(|entry| entry.get("resolution"))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| {
            format!(
                "pnpm-lock.yaml package `{}` has no `resolution` mapping",
                shorten(name)
            )
        })?;
    let mut integrity = false;
    for (field, value) in resolution {
        let (Some(field), Some(value)) = (field.as_str(), value.as_str()) else {
            return Err(format!(
                "pnpm-lock.yaml package `{}` has a resolution that is not plain strings",
                shorten(name)
            ));
        };
        match field {
            "integrity" if is_integrity(value) => integrity = true,
            "tarball" if is_npm_tarball(value) => {}
            _ => {
                return Err(format!(
                    "pnpm-lock.yaml package `{}` carries a `{}` resolution, which is not an \
                     integrity hash from the npm registry: Kronn only fetches packages the \
                     registry resolves",
                    shorten(name),
                    shorten(field)
                ))
            }
        }
    }
    if !integrity {
        return Err(format!(
            "pnpm-lock.yaml package `{}` has no integrity hash",
            shorten(name)
        ));
    }
    let fields = rebuild_pnpm_entry(name, entry, &PNPM_PACKAGE_FIELDS)?;
    // `resolution` first, as pnpm writes it.
    let mut kept = serde_yaml::Mapping::new();
    kept.insert(
        "resolution".into(),
        serde_yaml::Value::Mapping(resolution.clone()),
    );
    kept.extend(fields);
    Ok(serde_yaml::Value::Mapping(kept))
}

/// A mapping of the `fields` of `entry` that are present and well-formed; every
/// other field is dropped. A present field of the wrong shape refuses the lock.
fn rebuild_pnpm_entry(
    name: &str,
    entry: &serde_yaml::Value,
    fields: &[(&str, FieldShape)],
) -> Result<serde_yaml::Mapping, String> {
    let map = match entry {
        serde_yaml::Value::Null => return Ok(serde_yaml::Mapping::new()),
        serde_yaml::Value::Mapping(map) => map,
        _ => {
            return Err(format!(
                "pnpm-lock.yaml entry `{}` is not a mapping",
                shorten(name)
            ))
        }
    };
    let mut kept = serde_yaml::Mapping::new();
    for (field, shape) in fields {
        let Some(value) = map.get(*field) else {
            continue;
        };
        if !in_shape(value, *shape) {
            return Err(format!(
                "pnpm-lock.yaml entry `{}` has a malformed `{field}`",
                shorten(name)
            ));
        }
        kept.insert((*field).into(), value.clone());
    }
    Ok(kept)
}

fn in_shape(value: &serde_yaml::Value, shape: FieldShape) -> bool {
    use serde_yaml::Value;
    let text = |value: &Value| {
        value.as_str().is_some_and(|text| {
            text.len() <= 4096 && !text.chars().any(|c| c.is_control() && c != '\n')
        })
    };
    // A dependency is a version or the key of another snapshot: never a URL, a
    // path or a protocol (`link:`, `file:`, `https:`).
    let reference = |value: &Value| {
        value.as_str().is_some_and(|text| {
            text.len() <= 512
                && !text
                    .chars()
                    .any(|c| c == ':' || c == '\\' || c.is_whitespace() || c.is_control())
        })
    };
    let strings = |value: &Value| {
        value
            .as_sequence()
            .is_some_and(|items| items.len() <= 1024 && items.iter().all(text))
    };
    let map_of = |value: &Value, accepts: &dyn Fn(&Value) -> bool| {
        value.as_mapping().is_some_and(|map| {
            map.len() <= 8192 && map.iter().all(|(key, value)| text(key) && accepts(value))
        })
    };
    match shape {
        FieldShape::Flag => value.is_bool(),
        FieldShape::Text => text(value),
        FieldShape::Strings => strings(value),
        FieldShape::FlagOrStrings => value.is_bool() || strings(value),
        FieldShape::RangeMap => map_of(value, &text),
        FieldShape::ReferenceMap => map_of(value, &reference),
        FieldShape::MetaMap => map_of(value, &|meta| {
            meta.as_mapping().is_some_and(|meta| {
                meta.iter()
                    .all(|(field, flag)| field.as_str() == Some("optional") && flag.is_bool())
            })
        }),
    }
}

fn is_integrity(value: &str) -> bool {
    ["sha1-", "sha256-", "sha384-", "sha512-"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .is_some_and(|hash| {
            !hash.is_empty()
                && hash.len() <= 256
                && hash
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'))
        })
}

/// A tarball URL on the registry's own host. The host is fixed by the prefix, so
/// no userinfo or port trick can move it.
fn is_npm_tarball(value: &str) -> bool {
    value.starts_with(NPM_REGISTRY)
        && value.len() <= 1024
        && !value
            .chars()
            .any(|c| c == '\\' || c.is_whitespace() || c.is_control())
}

/// The `pnpm-workspace.yaml` of the scratch project, rebuilt from the worker's
/// file: only the [`WORKSPACE_KEYS`] survive, and only when their values are
/// inert data (scalars, lists, maps; no tag, no `${…}` expansion). `Ok(None)`
/// when nothing is left to write.
pub fn workspace_allowlist(text: &str) -> Result<Option<String>, String> {
    let mut document: serde_yaml::Value = serde_yaml::from_str(text)
        .map_err(|error| format!("{PNPM_WORKSPACE} is not valid YAML: {}", first_line(&error)))?;
    document.apply_merge().map_err(|error| {
        format!("{PNPM_WORKSPACE} has a merge key Kronn cannot resolve: {error}")
    })?;
    let map = match &document {
        serde_yaml::Value::Null => return Ok(None),
        serde_yaml::Value::Mapping(map) => map,
        _ => return Err(format!("{PNPM_WORKSPACE} is not a mapping")),
    };
    let mut kept = serde_yaml::Mapping::new();
    for key in WORKSPACE_KEYS {
        if let Some(value) = map.get(key) {
            check_inert(value, 0).map_err(|why| format!("{PNPM_WORKSPACE} `{key}`: {why}"))?;
            kept.insert(serde_yaml::Value::String(key.into()), value.clone());
        }
    }
    if kept.is_empty() {
        return Ok(None);
    }
    serde_yaml::to_string(&serde_yaml::Value::Mapping(kept))
        .map(Some)
        .map_err(|error| format!("cannot write the staged workspace file: {error}"))
}

fn check_inert(value: &serde_yaml::Value, depth: usize) -> Result<(), String> {
    use serde_yaml::Value;
    if depth > 8 {
        return Err("nested too deep".into());
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(text) => check_inert_string(text),
        Value::Sequence(items) => items
            .iter()
            .try_for_each(|item| check_inert(item, depth + 1)),
        Value::Mapping(map) => map.iter().try_for_each(|(key, value)| {
            let Some(key) = key.as_str() else {
                return Err("a key is not a plain string".into());
            };
            check_inert_string(key)?;
            check_inert(value, depth + 1)
        }),
        Value::Tagged(_) => Err("a YAML tag".into()),
    }
}

fn check_inert_string(text: &str) -> Result<(), String> {
    if text.len() > 4096 {
        Err("a value is over 4096 bytes".into())
    } else if text.contains("${") {
        Err("an environment expansion".into())
    } else if text.chars().any(|c| c.is_control() && c != '\n') {
        Err("a control character".into())
    } else {
        Ok(())
    }
}

fn first_line(error: &impl std::fmt::Display) -> String {
    error.to_string().lines().next().unwrap_or("").to_string()
}

/// A worker-written string, bounded before it goes into a journal message.
fn shorten(text: &str) -> String {
    let mut shortened: String = text.chars().take(80).collect();
    if text.chars().count() > 80 {
        shortened.push('…');
    }
    shortened
}

fn read_error(name: &str, error: std::io::Error) -> String {
    format!("cannot read {name}: {error}")
}

/// Read a lock-like file: a regular file (a symlink is not followed to a file
/// the worker chose), within `cap` bytes — also while reading, not only by its
/// size when `stat`ed.
fn read_capped(path: &Path, cap: u64) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = std::fs::symlink_metadata(path).map_err(|error| read_error(&name, error))?;
    if !metadata.is_file() {
        return Err(format!("{name} is not a regular file (a symlink?)"));
    }
    if metadata.len() > cap {
        return Err(format!(
            "{name} is {} bytes, over the {cap} a launch reads",
            metadata.len()
        ));
    }
    let file = std::fs::File::open(path).map_err(|error| read_error(&name, error))?;
    let mut text = String::new();
    file.take(cap + 1)
        .read_to_string(&mut text)
        .map_err(|error| read_error(&name, error))?;
    if text.len() as u64 > cap {
        return Err(format!("{name} grew past the {cap} a launch reads"));
    }
    Ok(text)
}

// ─── Environment and scratch ──────────────────────────────────────────────

/// The environment of a fetch command: what [`FETCH_ENV_NAMES`] (and `LC_*`)
/// keeps of `parent`, then `set`. Never an inherited registry setting or token.
pub fn fetch_env(
    parent: &[(OsString, OsString)],
    set: &[(&str, OsString)],
) -> Vec<(String, OsString)> {
    let same = env_name_eq;
    let mut env: Vec<(String, OsString)> = parent
        .iter()
        .filter_map(|(name, value)| {
            let name = name.to_str()?;
            let kept =
                FETCH_ENV_NAMES.iter().any(|known| same(known, name)) || name.starts_with("LC_");
            kept.then(|| (name.to_string(), value.clone()))
        })
        .collect();
    for (name, value) in set {
        env.retain(|(existing, _)| !same(existing, name));
        env.push(((*name).to_string(), value.clone()));
    }
    env
}

fn env_name_eq(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// The toolchain homes a fetch command still needs from the operator's real
/// home, which it is no longer given: `cargo` and `pnpm` are usually shims
/// (rustup, Volta, corepack) that look their toolchain up under `HOME` unless
/// `RUSTUP_HOME`, `VOLTA_HOME` or `COREPACK_HOME` say where. Each is pinned to
/// its default location when `parent` sets none and that directory exists.
/// These hold toolchains and caches, never registry credentials.
fn toolchain_homes(parent: &[(OsString, OsString)]) -> Vec<(&'static str, OsString)> {
    let var = |name: &str| {
        parent
            .iter()
            .find(|(key, _)| key.to_str().is_some_and(|key| env_name_eq(key, name)))
            .map(|(_, value)| PathBuf::from(value))
    };
    let Some(home) = var("HOME").or_else(|| var("USERPROFILE")) else {
        return Vec::new();
    };
    let corepack_cache = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else {
        var("XDG_CACHE_HOME").or_else(|| Some(home.join(".cache")))
    };
    [
        ("RUSTUP_HOME", Some(home.join(".rustup"))),
        ("VOLTA_HOME", Some(home.join(".volta"))),
        (
            "COREPACK_HOME",
            corepack_cache.map(|cache| cache.join("node").join("corepack")),
        ),
    ]
    .into_iter()
    .filter(|(name, _)| var(name).is_none())
    .filter_map(|(name, dir)| {
        dir.filter(|dir| dir.is_dir())
            .map(|dir| (name, dir.into_os_string()))
    })
    .collect()
}

/// Where the scratch projects go: a directory of Kronn's data directory, which
/// the worker's sandbox hides.
pub fn scratch_root() -> anyhow::Result<PathBuf> {
    Ok(crate::core::config::config_dir()?.join(SCRATCH_DIR_NAME))
}

/// Remove the scratch projects an earlier process left (a `Drop` does not run
/// when the process is killed). Call at startup, when no fetch can be running.
/// Returns how many it removed.
pub fn sweep_stale_scratch(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let ours = entry
            .file_name()
            .to_string_lossy()
            .starts_with(SCRATCH_PREFIX);
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if ours && is_dir && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// A scratch project, removed when dropped.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn create(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let dir = root.join(format!("{SCRATCH_PREFIX}{}", Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&dir)?;
        Ok(Self { dir })
    }

    fn write(&self, name: &str, content: &str) -> std::io::Result<()> {
        std::fs::write(self.dir.join(name), content)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Create the home directory of the fetch command and name it in every
    /// variable a tool takes its user configuration from: `HOME` (cargo, pnpm,
    /// npm: `~/.npmrc`, `~/.config/pnpm/rc`), the Windows profile variables and
    /// `XDG_CONFIG_HOME` (pnpm: `$XDG_CONFIG_HOME/pnpm/rc`). All of them point
    /// into the scratch project, which holds nothing but what Kronn wrote.
    fn home_env(&self) -> std::io::Result<Vec<(&'static str, OsString)>> {
        let home = self.path(SCRATCH_HOME);
        let config = home.join(".config");
        let roaming = home.join("AppData").join("Roaming");
        let local = home.join("AppData").join("Local");
        for dir in [&config, &roaming, &local] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(vec![
            ("HOME", home.clone().into_os_string()),
            ("USERPROFILE", home.into_os_string()),
            ("XDG_CONFIG_HOME", config.into_os_string()),
            ("APPDATA", roaming.into_os_string()),
            ("LOCALAPPDATA", local.into_os_string()),
        ])
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ─── Running a step ───────────────────────────────────────────────────────

fn probe(
    program: &str,
    args: Vec<String>,
    cwd: &Path,
    env: Vec<(String, OsString)>,
    timeout: Duration,
) -> ProbeCommand {
    ProbeCommand {
        program: program.into(),
        args,
        cwd: cwd.to_path_buf(),
        env,
        timeout,
        priority: Priority::Normal,
        clean_env: true,
    }
}

/// The probe-then-fetch shape both ecosystems share: `offline` succeeding means
/// the cache is complete; otherwise `online` runs, and its result is the step's.
fn probe_then_fetch(
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
    offline: &ProbeCommand,
    online: &ProbeCommand,
) -> FetchOutcome {
    if cancel.is_cancelled() {
        return FetchOutcome::Cancelled;
    }
    if runner.run_cancellable(offline, cancel).success {
        return FetchOutcome::Cached;
    }
    if cancel.is_cancelled() {
        return FetchOutcome::Cancelled;
    }
    let fetched = runner.run_cancellable(online, cancel);
    if fetched.success {
        FetchOutcome::Fetched
    } else if cancel.is_cancelled() {
        FetchOutcome::Cancelled
    } else {
        FetchOutcome::Failed {
            detail: fetched.detail,
        }
    }
}

fn step(
    ecosystem: Ecosystem,
    lockfile: String,
    registries: Vec<String>,
    started: Instant,
    outcome: FetchOutcome,
) -> DepFetchStep {
    DepFetchStep {
        ecosystem,
        lockfile,
        registries,
        outcome,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// The `CARGO_HOME` of a cargo fetch: a directory of the scratch project that
/// links the host's `registry` (what the step fills) and `git`, and nothing else.
/// The operator's own `credentials.toml` (registry tokens, credential
/// providers) and `config.toml` (source replacement, an authenticated mirror)
/// are never in it, so the networked Cargo cannot read them — the same shape as
/// `WorkerLane::link_cargo_home`, minus the configuration.
fn stage_cargo_home(scratch: &Scratch, host_cargo_home: &Path) -> Result<PathBuf, String> {
    let staged = scratch.path(STAGED_CARGO_HOME);
    let stage_error = |error: std::io::Error| format!("cannot stage the cargo home: {error}");
    std::fs::create_dir(&staged).map_err(stage_error)?;
    // A host that never fetched has no registry directory yet: it is what the
    // fetch is for, so it is created rather than left to the scratch project.
    let registry = host_cargo_home.join("registry");
    std::fs::create_dir_all(&registry).map_err(stage_error)?;
    ensure_symlink(&registry, &staged.join("registry"))?;
    let git = host_cargo_home.join("git");
    if git.is_dir() {
        ensure_symlink(&git, &staged.join("git"))?;
    }
    Ok(staged)
}

/// The crates the lock at `manifest_dir/Cargo.lock` references and `cargo_home`
/// lacks, fetched from crates.io. `None` when there is nothing to fetch (no
/// lock, or no registry package in it).
///
/// Only the lock file of the worktree is read, and what is staged from it goes
/// to a scratch project: Cargo never runs in the worktree, never reads its
/// `Cargo.toml` (`[patch]`, `[source]`, `[registries]`) or its `.cargo/`, and
/// never reads the operator's `CARGO_HOME` configuration or credentials
/// ([`stage_cargo_home`]) nor any file of the operator's home.
pub fn fetch_cargo(
    worktree: &Path,
    manifest_dir: &Path,
    cargo_home: Option<&Path>,
    host: &FetchHost<'_>,
) -> Option<DepFetchStep> {
    let started = Instant::now();
    let lock = manifest_dir.join("Cargo.lock");
    if std::fs::symlink_metadata(&lock).is_err() {
        return None;
    }
    let lockfile = relative(worktree, &lock);
    let refused = |reason: String| {
        step(
            Ecosystem::Cargo,
            lockfile.clone(),
            Vec::new(),
            started,
            FetchOutcome::Refused { reason },
        )
    };
    let scan = match read_capped(&lock, MAX_LOCK_BYTES).and_then(|text| scan_cargo_lock(&text)) {
        Ok(scan) => scan,
        Err(reason) => return Some(refused(reason)),
    };
    if scan.registry_packages == 0 {
        return None;
    }
    let Some(cargo_home) = cargo_home else {
        return Some(refused(
            "the host has no CARGO_HOME to fill: the lane has no shared registry".into(),
        ));
    };
    let staged = staged_cargo_manifest(&scan.crates).and_then(|manifest| {
        let stage_error = |error: std::io::Error| format!("cannot stage the cargo fetch: {error}");
        let scratch = Scratch::create(host.scratch_root).map_err(stage_error)?;
        scratch
            .write("Cargo.toml", &manifest)
            .and_then(|()| scratch.write("Cargo.lock", &scan.lock))
            .and_then(|()| scratch.write("lib.rs", ""))
            .map_err(stage_error)?;
        let mut set = scratch.home_env().map_err(stage_error)?;
        let staged_home = stage_cargo_home(&scratch, cargo_home)?;
        set.push((CARGO_HOME_ENV, staged_home.into_os_string()));
        set.extend(toolchain_homes(host.env));
        Ok((scratch, set))
    });
    let (scratch, set) = match staged {
        Ok(staged) => staged,
        Err(reason) => return Some(refused(reason)),
    };
    let args = |offline: bool| {
        let mut args = vec!["fetch".to_string()];
        if offline {
            args.push("--offline".into());
        }
        args.extend([
            "--manifest-path".into(),
            scratch.path("Cargo.toml").to_string_lossy().into_owned(),
        ]);
        args
    };
    let env = |offline: bool| {
        let mut set = set.clone();
        set.push((
            CARGO_NET_OFFLINE_ENV,
            OsString::from(if offline { "true" } else { "false" }),
        ));
        fetch_env(host.env, &set)
    };
    let outcome = probe_then_fetch(
        host.runner,
        host.cancel,
        &probe(
            "cargo",
            args(true),
            &scratch.dir,
            env(true),
            CARGO_PROBE_TIMEOUT,
        ),
        &probe(
            "cargo",
            args(false),
            &scratch.dir,
            env(false),
            CARGO_FETCH_TIMEOUT,
        ),
    );
    Some(step(
        Ecosystem::Cargo,
        lockfile,
        scan.registries,
        started,
        outcome,
    ))
}

/// The packages `frontend/pnpm-lock.yaml` references and the pnpm `store` lacks,
/// fetched from the npm registry. `None` when there is nothing to fetch.
///
/// pnpm runs in a scratch project built from the validated lock: the worker's
/// `.npmrc`, `package.json` and (but for its inert keys) workspace file are not
/// there, the user and global npm configuration are pointed at an empty file,
/// and `HOME` (with the Windows profile variables and `XDG_CONFIG_HOME`) is a
/// directory of the scratch project, so `~/.npmrc` and pnpm's own `rc` files
/// are the operator's no more.
pub fn fetch_pnpm(
    worktree: &Path,
    frontend: &Path,
    store: Option<&Path>,
    host: &FetchHost<'_>,
) -> Option<DepFetchStep> {
    let started = Instant::now();
    let lock = frontend.join(PNPM_LOCK);
    if std::fs::symlink_metadata(&lock).is_err() {
        return None;
    }
    let lockfile = relative(worktree, &lock);
    let registries = vec![NPM_REGISTRY.to_string()];
    let refused = |reason: String| {
        step(
            Ecosystem::Pnpm,
            lockfile.clone(),
            Vec::new(),
            started,
            FetchOutcome::Refused { reason },
        )
    };
    let scan = match read_capped(&lock, MAX_LOCK_BYTES).and_then(|text| scan_pnpm_lock(&text)) {
        Ok(Some(scan)) => scan,
        Ok(None) => return None,
        Err(reason) => return Some(refused(reason)),
    };
    let Some(store) = store else {
        return Some(refused(
            "the host has no pnpm store to fill: the lane has none to read".into(),
        ));
    };
    let workspace_path = frontend.join(PNPM_WORKSPACE);
    let workspace = if std::fs::symlink_metadata(&workspace_path).is_ok() {
        match read_capped(&workspace_path, MAX_WORKSPACE_BYTES)
            .and_then(|text| workspace_allowlist(&text))
        {
            Ok(workspace) => workspace,
            Err(reason) => return Some(refused(reason)),
        }
    } else {
        None
    };
    let staged = Scratch::create(host.scratch_root).and_then(|scratch| {
        scratch.write(PNPM_LOCK, &scan.lock)?;
        scratch.write("package.json", STAGED_MANIFEST)?;
        scratch.write(EMPTY_NPMRC, "")?;
        if let Some(workspace) = &workspace {
            scratch.write(PNPM_WORKSPACE, workspace)?;
        }
        let set = scratch.home_env()?;
        Ok((scratch, set))
    });
    let (scratch, mut set) = match staged {
        Ok(staged) => staged,
        Err(error) => return Some(refused(format!("cannot stage the pnpm fetch: {error}"))),
    };
    let store_arg = store.to_string_lossy().into_owned();
    let args = |offline: bool| {
        let mut args = vec!["fetch".to_string(), "--ignore-scripts".into()];
        if offline {
            args.push("--offline".into());
        } else {
            args.extend(["--registry".into(), NPM_REGISTRY.into()]);
        }
        args.extend(["--store-dir".into(), store_arg.clone()]);
        args
    };
    let npmrc = scratch.path(EMPTY_NPMRC).into_os_string();
    set.push(("npm_config_userconfig", npmrc.clone()));
    set.push(("npm_config_globalconfig", npmrc));
    set.extend(toolchain_homes(host.env));
    let env = fetch_env(host.env, &set);
    let outcome = probe_then_fetch(
        host.runner,
        host.cancel,
        &probe(
            "pnpm",
            args(true),
            &scratch.dir,
            env.clone(),
            PNPM_PROBE_TIMEOUT,
        ),
        &probe("pnpm", args(false), &scratch.dir, env, PNPM_FETCH_TIMEOUT),
    );
    Some(step(
        Ecosystem::Pnpm,
        lockfile,
        registries,
        started,
        outcome,
    ))
}

fn relative(worktree: &Path, path: &Path) -> String {
    path.strip_prefix(worktree)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Fetch what the lock files of `work_dir` reference and the host caches lack.
/// Never fails: a worktree the lane cannot be derived from has nothing to
/// fetch, and every step says how it ended.
///
/// `runner` must be one that may reach the network — Kronn's own process
/// runner, not the worker's confined one — which is why this is a step of its
/// own before the lane's first build rather than a probe of the smoke check.
/// `scratch_root` is where the staged projects go ([`scratch_root`]).
pub fn fetch_missing_dependencies(
    work_dir: &Path,
    scratch_root: &Path,
    runner: &dyn ProbeRunner,
    cancel: &CancellationToken,
) -> DepFetchRecord {
    let Ok(worktree) = work_dir.canonicalize() else {
        return DepFetchRecord::default();
    };
    let Ok(lane) = WorkerLane::for_worktree(&worktree) else {
        return DepFetchRecord::default();
    };
    let caches = SharedCaches::discover(lane.main_checkout.as_deref());
    let env = crate::core::child_env::vars_os();
    let host = FetchHost {
        scratch_root,
        env: &env,
        runner,
        cancel,
    };
    let mut record = DepFetchRecord::default();
    if let Some(manifest_dir) = [worktree.clone(), worktree.join("backend")]
        .into_iter()
        .find(|dir| dir.join("Cargo.toml").is_file())
    {
        record.steps.extend(fetch_cargo(
            &worktree,
            &manifest_dir,
            caches.cargo_home.as_deref(),
            &host,
        ));
    }
    if !record.cancelled() {
        record.steps.extend(fetch_pnpm(
            &worktree,
            &worktree.join(FRONTEND_DIR),
            caches.pnpm_stores.first().map(PathBuf::as_path),
            &host,
        ));
    }
    record
}

/// Whether `name` is one a fetch command inherits (see [`fetch_env`]).
#[cfg(test)]
fn inherits_into_fetch(name: &str) -> bool {
    fetch_env(&[(OsString::from(name), OsString::from("x"))], &[])
        .iter()
        .any(|(kept, _)| kept == name)
}

#[cfg(test)]
#[path = "task_worker_dep_fetch_test.rs"]
mod task_worker_dep_fetch_test;
