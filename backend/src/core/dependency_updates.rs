use crate::core::child_env::{self, ChildRoute};
use crate::core::cmd::async_cmd;
use crate::core::dependency_lockfiles::{composer_locked_versions, JsLockfile};
use crate::models::{
    DependencyCheckStatus, DependencyManagerUpdate, DependencyUpdatePackage,
    DependencyUpdateSummary,
};
use chrono::Utc;
use futures::{stream, StreamExt};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const CHECK_TIMEOUT: Duration = Duration::from_secs(25);
const FALLBACK_CHECK_TIMEOUT: Duration = Duration::from_secs(120);
const CONTAINER_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(4);
const COMPOSER_CONTAINER_TIMEOUT: Duration = Duration::from_secs(45);
const COMPOSER_CONTAINER_IMAGE: &str = "composer:2";
const RENOVATE_NODE_PACKAGE: &str = "node@24.11.0";
const RENOVATE_PACKAGE: &str = "renovate@43.280.3";
const MAX_MANIFESTS: usize = 10;
const MAX_SCAN_DIRS: usize = 300;
const MAX_PACKAGES_RETURNED: usize = 12;
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone)]
pub struct CachedDependencyUpdates {
    pub fingerprint: u64,
    pub inserted_at: Instant,
    pub summary: DependencyUpdateSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ManagerKind {
    JavaScript,
    Composer,
    Cargo,
    Go,
    Bundler,
    Gradle,
    DotNet,
    Poetry,
}

impl ManagerKind {
    #[cfg(test)]
    const ALL: [Self; 8] = [
        Self::JavaScript,
        Self::Composer,
        Self::Cargo,
        Self::Go,
        Self::Bundler,
        Self::Gradle,
        Self::DotNet,
        Self::Poetry,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::JavaScript => "JS / TS",
            Self::Composer => "Composer",
            Self::Cargo => "Cargo",
            Self::Go => "Go Modules",
            Self::Bundler => "Bundler",
            Self::Gradle => "Gradle",
            Self::DotNet => "NuGet",
            Self::Poetry => "Poetry",
        }
    }
}

/// Every manager label the watch can carry, for the tests that must cover them all.
#[cfg(test)]
pub(crate) fn manager_labels() -> Vec<&'static str> {
    ManagerKind::ALL.iter().map(|kind| kind.label()).collect()
}

#[derive(Debug, Clone)]
struct DetectedManifest {
    kind: ManagerKind,
    project_root: PathBuf,
    directory: PathBuf,
    relative_path: String,
    covers_nested: bool,
}

pub fn cache_key(root: &Path) -> String {
    root.to_string_lossy().to_string()
}

pub fn manifest_fingerprint(root: &Path) -> u64 {
    let manifests = detect_manifests(root);
    let mut relevant_files = Vec::new();
    for manifest in &manifests {
        relevant_files.push(root.join(&manifest.relative_path));
        let lockfiles: &[&str] = match manifest.kind {
            ManagerKind::JavaScript => &[
                "package-lock.json",
                "npm-shrinkwrap.json",
                "pnpm-lock.yaml",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            ],
            ManagerKind::Composer => &["composer.lock"],
            ManagerKind::Cargo => &["Cargo.lock"],
            ManagerKind::Go => &["go.sum"],
            ManagerKind::Bundler => &["Gemfile.lock"],
            ManagerKind::Gradle => &[
                "gradle.lockfile",
                "gradle/libs.versions.toml",
                "gradle/wrapper/gradle-wrapper.properties",
            ],
            ManagerKind::DotNet => &["packages.lock.json"],
            ManagerKind::Poetry => &["poetry.lock"],
        };
        relevant_files.extend(
            lockfiles
                .iter()
                .map(|lockfile| manifest.directory.join(lockfile)),
        );
    }
    relevant_files.sort();
    relevant_files.dedup();

    let mut hash = 0xcbf29ce484222325u64;
    for path in relevant_files {
        if !path.is_file() {
            continue;
        }
        let relative_path = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
        for byte in relative_path.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        if let Ok(metadata) = std::fs::metadata(path) {
            hash ^= metadata.len();
            if let Ok(modified) = metadata.modified() {
                if let Ok(duration) = modified.duration_since(SystemTime::UNIX_EPOCH) {
                    hash ^= duration.as_secs();
                    hash ^= u64::from(duration.subsec_nanos());
                }
            }
        }
    }
    hash
}

pub async fn inspect_dependency_updates(root: &Path) -> DependencyUpdateSummary {
    let manifests = detect_manifests(root);
    let checked: Vec<_> = stream::iter(manifests.clone())
        .map(|manifest| async move {
            let kind = manifest.kind;
            (kind, check_manifest(manifest).await)
        })
        .buffer_unordered(3)
        .collect()
        .await;
    let incomplete_kinds: HashSet<_> = checked
        .iter()
        .filter(|(_, manager)| !dependency_check_complete(&manager.status))
        .map(|(kind, _)| *kind)
        .collect();
    let fallback = if incomplete_kinds.is_empty() {
        HashMap::new()
    } else {
        renovate_dependency_updates(root, &manifests, &incomplete_kinds)
            .await
            .unwrap_or_default()
    };
    let mut managers: Vec<_> = checked
        .into_iter()
        .map(|(_, mut manager)| {
            if !dependency_check_complete(&manager.status) {
                if let Some(packages) = fallback.get(&manager.manifest) {
                    let mut packages = packages.clone();
                    if let Some(detected) = manifests.iter().find(|m| {
                        m.relative_path == manager.manifest && m.kind.label() == manager.manager
                    }) {
                        apply_fallback_locks(detected, &mut packages);
                    }
                    finalize_manager_update(&mut manager, packages);
                }
            }
            manager
        })
        .collect();
    managers.sort_by(|left, right| {
        left.manifest
            .cmp(&right.manifest)
            .then_with(|| left.manager.cmp(&right.manager))
    });
    let total_outdated = managers.iter().map(|manager| manager.outdated).sum();
    let total_major = managers.iter().map(|manager| manager.major).sum();
    DependencyUpdateSummary {
        managers,
        total_outdated,
        total_major,
        checked_at: Utc::now(),
        cached: false,
        monitoring_interval_days: Some(7),
        next_check_at: None,
    }
}

fn dependency_check_complete(status: &DependencyCheckStatus) -> bool {
    matches!(
        status,
        DependencyCheckStatus::UpToDate | DependencyCheckStatus::UpdatesAvailable
    )
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".kronn"
            | ".cache"
            | ".next"
            | ".nuxt"
            | ".output"
            | ".venv"
            | "node_modules"
            | "vendor"
            | "vendors"
            | "target"
            | "dist"
            | "build"
            | "coverage"
            | "cache"
            | "tmp"
            | "temp"
    )
}

fn classify_manifest(name: &str, directory: &Path) -> Option<ManagerKind> {
    match name {
        "package.json" => Some(ManagerKind::JavaScript),
        "composer.json" => Some(ManagerKind::Composer),
        "Cargo.toml" => Some(ManagerKind::Cargo),
        "go.mod" => Some(ManagerKind::Go),
        "Gemfile" => Some(ManagerKind::Bundler),
        "settings.gradle" | "settings.gradle.kts" => Some(ManagerKind::Gradle),
        "build.gradle" | "build.gradle.kts"
            if !directory.join("settings.gradle").is_file()
                && !directory.join("settings.gradle.kts").is_file() =>
        {
            Some(ManagerKind::Gradle)
        }
        "pyproject.toml" if directory.join("poetry.lock").is_file() => Some(ManagerKind::Poetry),
        _ if name.ends_with(".csproj") || name.ends_with(".sln") => Some(ManagerKind::DotNet),
        _ => None,
    }
}

fn manifest_covers_nested(kind: ManagerKind, path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    match kind {
        ManagerKind::JavaScript => serde_json::from_str::<Value>(&content)
            .ok()
            .and_then(|value| value.get("workspaces").cloned())
            .is_some(),
        ManagerKind::Cargo => content.lines().any(|line| line.trim() == "[workspace]"),
        ManagerKind::Gradle => path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "settings.gradle" | "settings.gradle.kts")),
        _ => false,
    }
}

fn detect_manifests(root: &Path) -> Vec<DetectedManifest> {
    fn visit(
        root: &Path,
        directory: &Path,
        depth: usize,
        visited_dirs: &mut usize,
        found: &mut Vec<DetectedManifest>,
    ) {
        if depth > 3 || *visited_dirs >= MAX_SCAN_DIRS || found.len() >= MAX_MANIFESTS {
            return;
        }
        *visited_dirs += 1;
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name().to_ascii_lowercase());
        for entry in &entries {
            if found.len() >= MAX_MANIFESTS {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(kind) = classify_manifest(&name, directory) else {
                continue;
            };
            let entry_path = entry.path();
            let relative_path = entry_path
                .strip_prefix(root)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .replace('\\', "/");
            found.push(DetectedManifest {
                kind,
                project_root: root.to_path_buf(),
                directory: directory.to_path_buf(),
                relative_path,
                covers_nested: manifest_covers_nested(kind, &entry_path),
            });
        }
        for entry in entries {
            if found.len() >= MAX_MANIFESTS {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            if file_type.is_dir() && !is_skipped_dir(&name) {
                visit(root, &entry.path(), depth + 1, visited_dirs, found);
            }
        }
    }

    let mut manifests = Vec::new();
    let mut visited_dirs = 0;
    visit(root, root, 0, &mut visited_dirs, &mut manifests);

    // A root workspace manifest already covers its nested members for the
    // package managers that understand workspaces. Avoid duplicate network
    // checks and misleading double counts.
    let root_workspace_kinds: HashSet<_> = manifests
        .iter()
        .filter(|manifest| manifest.directory == root && manifest.covers_nested)
        .map(|manifest| manifest.kind)
        .collect();
    manifests.retain(|manifest| {
        manifest.directory == root || !root_workspace_kinds.contains(&manifest.kind)
    });

    // A solution covers the .csproj files below it.
    if manifests
        .iter()
        .any(|manifest| manifest.relative_path.ends_with(".sln"))
    {
        manifests.retain(|manifest| {
            manifest.kind != ManagerKind::DotNet || manifest.relative_path.ends_with(".sln")
        });
    }
    manifests.truncate(MAX_MANIFESTS);
    manifests
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandFailure {
    Unavailable,
    Error,
    TimedOut,
}

impl CommandFailure {
    fn status(self) -> DependencyCheckStatus {
        match self {
            Self::Unavailable => DependencyCheckStatus::Unavailable,
            Self::Error => DependencyCheckStatus::Error,
            Self::TimedOut => DependencyCheckStatus::TimedOut,
        }
    }
}

async fn capture_command(
    command: &mut tokio::process::Command,
    timeout: Duration,
) -> Result<std::process::Output, CommandFailure> {
    match tokio::time::timeout(timeout, command.output()).await {
        Err(_) => Err(CommandFailure::TimedOut),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(CommandFailure::Unavailable)
        }
        Ok(Err(_)) => Err(CommandFailure::Error),
        Ok(Ok(output)) => Ok(output),
    }
}

fn compose_root(manifest: &DetectedManifest) -> Option<PathBuf> {
    manifest
        .directory
        .ancestors()
        .take_while(|directory| directory.starts_with(&manifest.project_root))
        .find(|directory| {
            [
                "compose.yml",
                "compose.yaml",
                "docker-compose.yml",
                "docker-compose.yaml",
            ]
            .iter()
            .any(|name| directory.join(name).is_file())
        })
        .map(Path::to_path_buf)
}

fn is_safe_compose_service(service: &str) -> bool {
    !service.is_empty()
        && service
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn prioritize_compose_services(output: &str) -> Vec<String> {
    let mut services: Vec<_> = output
        .lines()
        .map(str::trim)
        .filter(|service| is_safe_compose_service(service))
        .map(str::to_string)
        .collect();
    services.sort_by_key(|service| {
        let normalized = service.to_ascii_lowercase();
        if normalized == "php" {
            0
        } else if normalized.contains("php") {
            1
        } else if normalized == "app" || normalized == "backend" {
            2
        } else {
            3
        }
    });
    services.truncate(8);
    services
}

/// A dependency-check process: the environment built for `route` (the
/// repository's config runs inside package managers), non-interactive
/// settings, then `extra`; sealed last.
fn background_command(
    program: &str,
    directory: &Path,
    route: ChildRoute,
    extra: &[(&str, &str)],
) -> tokio::process::Command {
    let mut command = async_cmd(program, route);
    command
        .current_dir(directory)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("COMPOSER_NO_INTERACTION", "1")
        .envs(extra.iter().copied())
        .kill_on_drop(true);
    child_env::seal(command.as_std_mut(), route, &[]);
    command
}

/// Composer is frequently project-local inside a Docker Compose PHP service
/// rather than installed on the host (especially on macOS). When the direct
/// binary is absent, inspect only already-running services and execute the same
/// read-only command in the first container that exposes Composer.
async fn composer_via_compose(
    manifest: &DetectedManifest,
    args: &[&str],
) -> Result<std::process::Output, CommandFailure> {
    let root = compose_root(manifest).ok_or(CommandFailure::Unavailable)?;
    let mut list = background_command("docker", &root, ChildRoute::Docker, &[]);
    list.args(["compose", "ps", "--services", "--status", "running"]);
    let listed = capture_command(&mut list, CONTAINER_DISCOVERY_TIMEOUT).await?;
    if !listed.status.success() {
        return Err(CommandFailure::Unavailable);
    }

    let services = prioritize_compose_services(&String::from_utf8_lossy(&listed.stdout));
    for service in services {
        let mut probe = background_command("docker", &root, ChildRoute::Docker, &[]);
        probe.args([
            "compose",
            "exec",
            "-T",
            &service,
            "composer",
            "--version",
            "--no-ansi",
        ]);
        let Ok(probed) = capture_command(&mut probe, CONTAINER_DISCOVERY_TIMEOUT).await else {
            continue;
        };
        if !probed.status.success() {
            continue;
        }

        let mut command = background_command("docker", &root, ChildRoute::Docker, &[]);
        command.args(["compose", "exec", "-T", &service, "composer"]);
        command.args(args);
        return capture_command(&mut command, CHECK_TIMEOUT).await;
    }
    Err(CommandFailure::Unavailable)
}

fn standalone_composer_args(manifest: &DetectedManifest, args: &[&str]) -> Vec<String> {
    let mut docker_args = vec![
        "run".into(),
        "--rm".into(),
        "--pull=missing".into(),
        "--volume".into(),
        format!("{}:/app:ro", manifest.directory.to_string_lossy()),
        "--workdir".into(),
        "/app".into(),
        COMPOSER_CONTAINER_IMAGE.into(),
    ];
    docker_args.extend(args.iter().map(|arg| (*arg).to_string()));
    docker_args
}

/// Last-resort Composer runtime for projects whose stack is stopped or not yet
/// built. The official image sees only the manifest directory, read-only, and
/// does not start any application service or dependency.
async fn composer_via_standalone_container(
    manifest: &DetectedManifest,
    args: &[&str],
) -> Result<std::process::Output, CommandFailure> {
    let mut command = background_command("docker", &manifest.directory, ChildRoute::Docker, &[]);
    command.args(standalone_composer_args(manifest, args));
    capture_command(&mut command, COMPOSER_CONTAINER_TIMEOUT).await
}

fn renovate_manager_names(kind: ManagerKind) -> &'static [&'static str] {
    match kind {
        ManagerKind::JavaScript => &["npm"],
        ManagerKind::Composer => &["composer"],
        ManagerKind::Cargo => &["cargo"],
        ManagerKind::Go => &["gomod"],
        ManagerKind::Bundler => &["bundler"],
        ManagerKind::Gradle => &["gradle", "gradle-wrapper"],
        ManagerKind::DotNet => &["nuget"],
        ManagerKind::Poetry => &["poetry"],
    }
}

fn renovate_manager_kind(name: &str) -> Option<ManagerKind> {
    match name {
        "npm" => Some(ManagerKind::JavaScript),
        "composer" => Some(ManagerKind::Composer),
        "cargo" => Some(ManagerKind::Cargo),
        "gomod" => Some(ManagerKind::Go),
        "bundler" => Some(ManagerKind::Bundler),
        "gradle" | "gradle-wrapper" => Some(ManagerKind::Gradle),
        "nuget" => Some(ManagerKind::DotNet),
        "poetry" => Some(ManagerKind::Poetry),
        _ => None,
    }
}

fn fallback_manifest<'a>(
    kind: ManagerKind,
    package_file: &str,
    manifests: &'a [DetectedManifest],
) -> Option<&'a DetectedManifest> {
    if let Some(exact) = manifests
        .iter()
        .find(|manifest| manifest.kind == kind && manifest.relative_path == package_file)
    {
        return Some(exact);
    }

    let mut covering: Vec<_> = manifests
        .iter()
        .filter(|manifest| manifest.kind == kind && manifest.covers_nested)
        .filter(|manifest| {
            let directory = manifest
                .directory
                .strip_prefix(&manifest.project_root)
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .replace('\\', "/");
            directory.is_empty()
                || package_file == directory
                || package_file.starts_with(&format!("{directory}/"))
        })
        .collect();
    covering.sort_by_key(|manifest| std::cmp::Reverse(manifest.directory.as_os_str().len()));
    if let Some(manifest) = covering.first() {
        return Some(manifest);
    }

    let mut matching = manifests.iter().filter(|manifest| manifest.kind == kind);
    let only = matching.next()?;
    matching.next().is_none().then_some(only)
}

fn parse_renovate_updates(
    output: &str,
    manifests: &[DetectedManifest],
) -> Result<HashMap<String, Vec<DependencyUpdatePackage>>, ()> {
    let event = output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value.get("msg").and_then(Value::as_str) == Some("packageFiles with updates"))
        .ok_or(())?;
    let config = event.get("config").and_then(Value::as_object).ok_or(())?;
    let mut by_manifest: HashMap<String, Vec<DependencyUpdatePackage>> = HashMap::new();

    for (manager_name, files) in config {
        let Some(kind) = renovate_manager_kind(manager_name) else {
            continue;
        };
        let Some(files) = files.as_array() else {
            continue;
        };
        for file in files {
            let package_file = file
                .get("packageFile")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(manifest) = fallback_manifest(kind, package_file, manifests) else {
                continue;
            };
            let Some(dependencies) = file.get("deps").and_then(Value::as_array) else {
                continue;
            };
            let mut packages = Vec::new();
            let mut resolved = false;
            for dependency in dependencies {
                let Some(name) = dependency
                    .get("depName")
                    .or_else(|| dependency.get("packageName"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(current) = dependency
                    .get("currentVersion")
                    .or_else(|| dependency.get("lockedVersion"))
                    .or_else(|| dependency.get("currentValue"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                // A skipped dependency, a failed lookup or a dependency that
                // was never looked up (no `updates` array) is not a verdict.
                // Warnings alone do not discard resolved updates: only a
                // warned dependency with nothing resolved is unknown.
                let updates = dependency.get("updates").and_then(Value::as_array);
                let warned = dependency
                    .get("warnings")
                    .and_then(Value::as_array)
                    .is_some_and(|warnings| !warnings.is_empty());
                if dependency.get("skipReason").is_some()
                    || updates.is_none_or(|updates| warned && updates.is_empty())
                {
                    continue;
                }
                resolved = true;
                let Some(latest) = dependency
                    .get("updates")
                    .and_then(Value::as_array)
                    .and_then(|updates| updates.last())
                    .and_then(|update| update.get("newVersion").or_else(|| update.get("newValue")))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if let Some(package) = update_package(name, current, latest) {
                    packages.push(package);
                }
            }
            // A package file Renovate listed without resolving one dependency
            // says nothing: it must not read "up to date" (KT-1279).
            if resolved {
                by_manifest
                    .entry(manifest.relative_path.clone())
                    .or_default()
                    .extend(packages);
            }
        }
    }
    Ok(by_manifest)
}

/// The local Renovate run: npx fetches and runs it inside the repository.
fn renovate_command(root: &Path, enabled_managers: &[&str]) -> tokio::process::Command {
    let mut command = background_command(
        "npx",
        root,
        ChildRoute::DependencyCheck,
        &[
            ("LOG_LEVEL", "debug"),
            ("LOG_FORMAT", "json"),
            ("RENOVATE_REQUIRE_CONFIG", "optional"),
        ],
    );
    command.args([
        "--yes",
        "--package",
        RENOVATE_NODE_PACKAGE,
        "--package",
        RENOVATE_PACKAGE,
        "renovate",
        "--platform=local",
        "--onboarding=false",
        &format!("--enabled-managers={}", enabled_managers.join(",")),
    ]);
    command
}

async fn renovate_dependency_updates(
    root: &Path,
    manifests: &[DetectedManifest],
    kinds: &HashSet<ManagerKind>,
) -> Result<HashMap<String, Vec<DependencyUpdatePackage>>, CommandFailure> {
    let mut enabled: Vec<_> = kinds
        .iter()
        .flat_map(|kind| renovate_manager_names(*kind))
        .copied()
        .collect();
    enabled.sort_unstable();
    enabled.dedup();

    let mut command = renovate_command(root, &enabled);
    let output = capture_command(&mut command, FALLBACK_CHECK_TIMEOUT).await?;
    parse_renovate_updates(&String::from_utf8_lossy(&output.stdout), manifests)
        .map_err(|_| CommandFailure::Error)
}

async fn check_manifest(manifest: DetectedManifest) -> DependencyManagerUpdate {
    if manifest.kind == ManagerKind::Gradle {
        return unchecked_manager(
            &manifest,
            DependencyCheckStatus::Unsupported,
            "Gradle has no automated outdated check",
        );
    }

    let (program, args): (&str, Vec<&str>) = match manifest.kind {
        ManagerKind::JavaScript => (
            "npm",
            npm_outdated_args(manifest.covers_nested, js_dependencies_installed(&manifest)),
        ),
        ManagerKind::Composer => (
            "composer",
            vec![
                "--no-plugins",
                "--no-scripts",
                "outdated",
                "--direct",
                "--format=json",
                "--locked",
                "--no-interaction",
            ],
        ),
        ManagerKind::Cargo => (
            "cargo",
            vec!["update", "--dry-run", "--verbose", "--color", "never"],
        ),
        ManagerKind::Go => ("go", vec!["list", "-m", "-u", "-json", "all"]),
        ManagerKind::Bundler => ("bundle", vec!["outdated", "--parseable", "--only-explicit"]),
        ManagerKind::Gradle => unreachable!("Gradle is handled before command selection"),
        ManagerKind::DotNet => (
            "dotnet",
            vec![
                "list",
                "package",
                "--outdated",
                "--format",
                "json",
                "--output-version",
                "1",
                "--no-restore",
            ],
        ),
        ManagerKind::Poetry => (
            "poetry",
            vec!["show", "--outdated", "--top-level", "--no-ansi"],
        ),
    };

    let mut command = background_command(
        program,
        &manifest.directory,
        ChildRoute::DependencyCheck,
        &[],
    );
    command.args(&args);
    let output = match capture_command(&mut command, CHECK_TIMEOUT).await {
        Err(CommandFailure::Unavailable) if manifest.kind == ManagerKind::Composer => {
            match composer_via_compose(&manifest, &args).await {
                Ok(output) => output,
                Err(_) => match composer_via_standalone_container(&manifest, &args).await {
                    Ok(output) => output,
                    Err(error) => {
                        let docker = match error {
                            CommandFailure::Unavailable => "Docker is not available",
                            CommandFailure::TimedOut => "the standalone container timed out",
                            CommandFailure::Error => "the standalone container could not be run",
                        };
                        return unchecked_manager(
                            &manifest,
                            error.status(),
                            format!(
                                "`composer` is not on the host PATH, no running Docker Compose \
                                 service exposes it, and {docker} for the \
                                 `{COMPOSER_CONTAINER_IMAGE}` fallback"
                            ),
                        );
                    }
                },
            }
        }
        Err(error) => {
            return unchecked_manager(&manifest, error.status(), failure_reason(program, error))
        }
        Ok(output) => output,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lockfile_hits = 0;
    let parsed = match manifest.kind {
        ManagerKind::JavaScript => {
            let lock = (!js_dependencies_installed(&manifest) || npm_rows_lack_current(&stdout))
                .then(|| js_lockfile(&manifest))
                .flatten();
            let declared = js_declared_ranges(&manifest.directory.join("package.json"));
            parse_npm_outdated_locked(&stdout, lock.as_ref(), &declared).map(|(packages, hits)| {
                lockfile_hits = hits;
                packages
            })
        }
        ManagerKind::Composer => parse_composer_outdated(&stdout).map(|mut packages| {
            // `outdated --locked` compares composer.lock with the registry.
            for package in &mut packages {
                package.lockfile = Some("composer.lock".into());
            }
            packages
        }),
        ManagerKind::Cargo => parse_cargo_outdated(&format!("{stdout}\n{stderr}")),
        ManagerKind::Go => parse_go_outdated(&stdout),
        ManagerKind::Bundler => parse_bundler_outdated(&stdout),
        ManagerKind::Gradle => unreachable!("Gradle is handled before command execution"),
        ManagerKind::DotNet => parse_dotnet_outdated(&stdout),
        ManagerKind::Poetry => parse_poetry_outdated(&stdout),
    };
    match parsed {
        Ok(packages) if output.status.success() || !packages.is_empty() => {
            settle_checked(&manifest, packages, lockfile_hits > 0)
        }
        Ok(_) => unchecked_manager(
            &manifest,
            DependencyCheckStatus::Error,
            run_failure_reason(program, &output, &stdout, &stderr),
        ),
        Err(_) if output.status.success() && stdout.trim().is_empty() => {
            settle_checked(&manifest, Vec::new(), false)
        }
        Err(_) => unchecked_manager(
            &manifest,
            DependencyCheckStatus::Error,
            if output.status.success() {
                format!(
                    "could not parse the output of `{program}`: {}",
                    output_excerpt(&stdout, &stderr).unwrap_or_default()
                )
            } else {
                run_failure_reason(program, &output, &stdout, &stderr)
            },
        ),
    }
}

/// `npm outdated` reads installed versions from `node_modules`. Without it
/// the workspace flags make npm 11 print `{}` and exit 0 without querying the
/// registry (KT-1279), so they are only passed to an installed tree.
fn npm_outdated_args(covers_nested: bool, installed: bool) -> Vec<&'static str> {
    let mut args = vec!["outdated", "--json", "--long"];
    if covers_nested && installed {
        args.extend(["--workspaces", "--include-workspace-root"]);
    }
    args
}

/// Whether `node_modules` holds a real install: an installer's state file or
/// a package entry. A directory with only `.cache` or `.bin` is not one.
fn node_modules_holds_packages(node_modules: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(node_modules) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        !name.starts_with('.')
            || matches!(
                name.as_ref(),
                ".package-lock.json" | ".yarn-state.yml" | ".modules.yaml"
            )
    })
}

/// Whether npm will find installed packages for this manifest: its own
/// `node_modules`, or, for a workspace member whose dependencies are hoisted,
/// the install of an ancestor (inside the project) that declares `workspaces`:
/// npm run from a member uses that root. An ancestor without `workspaces` is
/// not used by npm, so its install says nothing about this manifest.
fn js_dependencies_installed(manifest: &DetectedManifest) -> bool {
    if node_modules_holds_packages(&manifest.directory.join("node_modules")) {
        return true;
    }
    manifest
        .directory
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&manifest.project_root))
        .any(|dir| {
            manifest_covers_nested(ManagerKind::JavaScript, &dir.join("package.json"))
                && node_modules_holds_packages(&dir.join("node_modules"))
        })
}

/// Whether a `package.json` has anything to install: dependencies of its own,
/// or workspace members that have some.
fn js_declares_dependencies(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return true;
    };
    value.get("workspaces").is_some()
        || [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ]
        .iter()
        .any(|key| {
            value
                .get(key)
                .and_then(Value::as_object)
                .is_some_and(|dependencies| !dependencies.is_empty())
        })
}

/// A tool answered and printed `packages`. An empty answer is "up to date"
/// only when the tool provably inspected something: a JS manifest with
/// dependencies and no `node_modules` has nothing npm could compare, so it
/// stays incomplete and the Renovate fallback (which reads the lockfile)
/// decides. A lockfile that resolved at least one package is an inspection.
fn settle_checked(
    manifest: &DetectedManifest,
    packages: Vec<DependencyUpdatePackage>,
    lockfile_inspected: bool,
) -> DependencyManagerUpdate {
    let nothing_inspected = manifest.kind == ManagerKind::JavaScript
        && packages.is_empty()
        && !lockfile_inspected
        && !js_dependencies_installed(manifest)
        && js_declares_dependencies(&manifest.directory.join("package.json"));
    if nothing_inspected {
        return unchecked_manager(
            manifest,
            DependencyCheckStatus::NotInstalled,
            "no node_modules and no readable lockfile (package-lock.json, yarn.lock, \
             pnpm-lock.yaml) resolves the declared packages: install them or commit a lockfile",
        );
    }
    finalize_manager(manifest, packages)
}

fn empty_manager(
    manifest: &DetectedManifest,
    status: DependencyCheckStatus,
) -> DependencyManagerUpdate {
    DependencyManagerUpdate {
        manager: manifest.kind.label().into(),
        manifest: manifest.relative_path.clone(),
        status,
        outdated: 0,
        major: 0,
        packages: Vec::new(),
        reason: None,
    }
}

/// A manifest the check did not settle, with why (KT-1325): a status that is
/// not a verdict is never left unexplained.
fn unchecked_manager(
    manifest: &DetectedManifest,
    status: DependencyCheckStatus,
    reason: impl AsRef<str>,
) -> DependencyManagerUpdate {
    let mut update = empty_manager(manifest, status);
    update.reason = Some(bounded_reason(reason.as_ref()));
    update
}

const MAX_REASON_CHARS: usize = 300;

fn bounded_reason(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_REASON_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(MAX_REASON_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The first lines a failed tool wrote: stderr, else stdout.
fn output_excerpt(stdout: &str, stderr: &str) -> Option<String> {
    [stderr, stdout].iter().find_map(|text| {
        let lines: Vec<_> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();
        (!lines.is_empty()).then(|| lines.join(" | "))
    })
}

/// Why a tool that ran did not produce a verdict: the error it printed (npm
/// writes `{"error":{"summary":…}}` on stdout), else its first output lines,
/// else its exit status.
fn run_failure_reason(
    program: &str,
    output: &std::process::Output,
    stdout: &str,
    stderr: &str,
) -> String {
    let printed_error = serde_json::from_str::<Value>(stdout)
        .ok()
        .and_then(|value| {
            let error = value.get("error")?;
            ["summary", "message", "detail"]
                .iter()
                .find_map(|key| error.get(key).and_then(Value::as_str))
                .map(str::to_string)
        });
    let detail = printed_error.or_else(|| output_excerpt(stdout, stderr));
    match (detail, output.status.code()) {
        (Some(detail), Some(code)) => format!("`{program}` failed (exit {code}): {detail}"),
        (Some(detail), None) => format!("`{program}` failed: {detail}"),
        (None, Some(code)) => format!("`{program}` exited with status {code} and no output"),
        (None, None) => format!("`{program}` was terminated and printed nothing"),
    }
}

fn failure_reason(program: &str, failure: CommandFailure) -> String {
    match failure {
        CommandFailure::Unavailable => format!("`{program}` was not found on this machine"),
        CommandFailure::TimedOut => format!(
            "`{program}` did not answer within {} s",
            CHECK_TIMEOUT.as_secs()
        ),
        CommandFailure::Error => format!("`{program}` could not be run"),
    }
}

fn finalize_manager(
    manifest: &DetectedManifest,
    packages: Vec<DependencyUpdatePackage>,
) -> DependencyManagerUpdate {
    let mut update = DependencyManagerUpdate {
        manager: manifest.kind.label().into(),
        manifest: manifest.relative_path.clone(),
        status: DependencyCheckStatus::UpToDate,
        outdated: 0,
        major: 0,
        packages: Vec::new(),
        reason: None,
    };
    finalize_manager_update(&mut update, packages);
    update
}

fn finalize_manager_update(
    update: &mut DependencyManagerUpdate,
    mut packages: Vec<DependencyUpdatePackage>,
) {
    // The UI only receives a bounded preview. Keep every major update ahead of
    // compatible updates so a red major counter always has a visible package
    // explaining it, even in projects with hundreds of outdated crates.
    packages.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| a.name.cmp(&b.name)));
    packages.dedup_by(|a, b| a.name == b.name);
    let outdated = packages.len() as u32;
    let major = packages.iter().filter(|package| package.major).count() as u32;
    packages.truncate(MAX_PACKAGES_RETURNED);
    update.status = if outdated == 0 {
        DependencyCheckStatus::UpToDate
    } else {
        DependencyCheckStatus::UpdatesAvailable
    };
    update.outdated = outdated;
    update.major = major;
    update.packages = packages;
    update.reason = None;
}

fn version_major(version: &str) -> Option<u64> {
    let digits: String = version
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn update_package(name: &str, current: &str, latest: &str) -> Option<DependencyUpdatePackage> {
    if name.is_empty() || latest.is_empty() || current == latest {
        return None;
    }
    Some(DependencyUpdatePackage {
        name: name.to_string(),
        current: current.to_string(),
        latest: latest.to_string(),
        major: match (version_major(current), version_major(latest)) {
            (Some(current), Some(latest)) => latest > current,
            _ => false,
        },
        declared_only: false,
        lockfile: None,
    })
}

fn parse_npm_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    parse_npm_outdated_locked(output, None, &HashMap::new()).map(|(packages, _)| packages)
}

/// Parses `npm outdated --json`. A row npm printed without an installed
/// version is resolved from `lock` when it holds the package (KT-1325); the
/// second value counts those resolutions, because a package the lockfile
/// proves up to date prints no row yet proves something was inspected.
fn parse_npm_outdated_locked(
    output: &str,
    lock: Option<&JsLockfile>,
    declared: &HashMap<String, String>,
) -> Result<(Vec<DependencyUpdatePackage>, usize), ()> {
    if output.trim().is_empty() {
        return Ok((Vec::new(), 0));
    }
    let value: Value = serde_json::from_str(output).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.contains_key("error") {
        return Err(());
    }
    let mut resolved = 0;
    let packages = object
        .iter()
        .filter_map(|(name, entry)| {
            let latest = entry.get("latest")?.as_str()?;
            match entry.get("current").and_then(Value::as_str) {
                Some(current) => update_package(name, current, latest),
                None => {
                    let range = declared.get(name).map(String::as_str);
                    if let Some((lock, locked)) =
                        lock.and_then(|lock| Some((lock, lock.version(name, range)?)))
                    {
                        resolved += 1;
                        let mut package = update_package(name, &locked, latest)?;
                        package.lockfile = Some(lock.file.to_string());
                        return Some(package);
                    }
                    // No installed version: `wanted` is only the highest
                    // registry version the declared range allows, neither the
                    // installed version nor the range floor. It still proves
                    // `latest` is out of range, but it must never be shown as
                    // what is in place.
                    let wanted = entry.get("wanted").and_then(Value::as_str)?;
                    let mut package = update_package(name, wanted, latest)?;
                    package.current = String::new();
                    package.declared_only = true;
                    Some(package)
                }
            }
        })
        .collect();
    Ok((packages, resolved))
}

/// Whether `npm outdated` printed a row without an installed version.
fn npm_rows_lack_current(output: &str) -> bool {
    serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .is_some_and(|rows| {
            rows.values()
                .any(|row| row.get("current").and_then(Value::as_str).is_none())
        })
}

/// The ranges `package.json` declares, by package name: what picks the entry
/// of a Yarn lockfile that holds several versions of one package.
fn js_declared_ranges(path: &Path) -> HashMap<String, String> {
    let mut ranges = HashMap::new();
    let Some(value) = std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
    else {
        return ranges;
    };
    for key in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if let Some(dependencies) = value.get(key).and_then(Value::as_object) {
            for (name, range) in dependencies {
                if let Some(range) = range.as_str() {
                    ranges
                        .entry(name.clone())
                        .or_insert_with(|| range.to_string());
                }
            }
        }
    }
    ranges
}

/// The lockfile that resolves this manifest's packages: its own directory's,
/// else a workspace root's above it inside the project (the install npm would
/// use from a member, as in [`js_dependencies_installed`]).
fn js_lockfile(manifest: &DetectedManifest) -> Option<JsLockfile> {
    manifest
        .directory
        .ancestors()
        .take_while(|dir| dir.starts_with(&manifest.project_root))
        .enumerate()
        .filter(|(depth, dir)| {
            *depth == 0
                || manifest_covers_nested(ManagerKind::JavaScript, &dir.join("package.json"))
                || dir.join("pnpm-workspace.yaml").is_file()
        })
        .find_map(|(_, dir)| JsLockfile::load(dir, &manifest.directory))
}

/// Replaces `current` with the version a lockfile resolves, for the Renovate
/// fallback: its `currentVersion` can be a range, not what is in place.
fn apply_fallback_locks(manifest: &DetectedManifest, packages: &mut Vec<DependencyUpdatePackage>) {
    match manifest.kind {
        ManagerKind::JavaScript => {
            let Some(lock) = js_lockfile(manifest) else {
                return;
            };
            let declared = js_declared_ranges(&manifest.directory.join("package.json"));
            for package in packages.iter_mut() {
                if let Some(version) = lock.version(
                    &package.name,
                    declared.get(&package.name).map(String::as_str),
                ) {
                    relock(package, version, lock.file);
                }
            }
        }
        ManagerKind::Composer => {
            let locked = composer_locked_versions(&manifest.directory);
            for package in packages.iter_mut() {
                if let Some(version) = locked.get(&package.name) {
                    relock(package, version.clone(), "composer.lock");
                }
            }
        }
        _ => {}
    }
    packages.retain(|package| package.current != package.latest);
}

fn relock(package: &mut DependencyUpdatePackage, version: String, file: &str) {
    package.major = match (version_major(&version), version_major(&package.latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    };
    package.current = version;
    package.declared_only = false;
    package.lockfile = Some(file.to_string());
}

fn parse_composer_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    let value: Value = serde_json::from_str(output).map_err(|_| ())?;
    let packages = value
        .get("installed")
        .or_else(|| value.get("locked"))
        .and_then(Value::as_array)
        .ok_or(())?;
    Ok(packages
        .iter()
        .filter_map(|entry| {
            update_package(
                entry.get("name")?.as_str()?,
                entry
                    .get("version")
                    .or_else(|| entry.get("pretty_version"))?
                    .as_str()?,
                entry.get("latest")?.as_str()?,
            )
        })
        .collect())
}

fn parse_cargo_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    let mut packages = Vec::new();
    for line in output.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("Updating ") else {
            continue;
        };
        if rest.starts_with("crates.io index") || !rest.contains(" -> ") {
            continue;
        }
        let Some((before, latest)) = rest.rsplit_once(" -> ") else {
            continue;
        };
        let mut parts = before.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let current = parts.next().unwrap_or_default();
        if let Some(package) = update_package(name, current, latest.trim()) {
            packages.push(package);
        }
    }
    Ok(packages)
}

fn parse_go_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    let mut packages = Vec::new();
    let stream = serde_json::Deserializer::from_str(output).into_iter::<Value>();
    for value in stream {
        let value = value.map_err(|_| ())?;
        if value.get("Main").and_then(Value::as_bool).unwrap_or(false)
            || value
                .get("Indirect")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        let Some(update) = value.get("Update") else {
            continue;
        };
        if let Some(package) = update_package(
            value
                .get("Path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .get("Version")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            update
                .get("Version")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ) {
            packages.push(package);
        }
    }
    Ok(packages)
}

fn parse_bundler_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    let mut packages = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(newest_index) = line.find("newest ") else {
            continue;
        };
        let Some(installed_index) = line.find("installed ") else {
            continue;
        };
        let name = line
            .trim_start_matches('*')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        let latest = line[newest_index + 7..]
            .split([',', ')'])
            .next()
            .unwrap_or_default()
            .trim();
        let current = line[installed_index + 10..]
            .split([',', ')'])
            .next()
            .unwrap_or_default()
            .trim();
        if let Some(package) = update_package(name, current, latest) {
            packages.push(package);
        }
    }
    Ok(packages)
}

fn collect_dotnet_packages(value: &Value, packages: &mut Vec<DependencyUpdatePackage>) {
    match value {
        Value::Object(object) => {
            let name = object
                .get("id")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str);
            let current = object
                .get("resolvedVersion")
                .or_else(|| object.get("resolved"))
                .and_then(Value::as_str);
            let latest = object
                .get("latestVersion")
                .or_else(|| object.get("latest"))
                .and_then(Value::as_str);
            if let (Some(name), Some(current), Some(latest)) = (name, current, latest) {
                if let Some(package) = update_package(name, current, latest) {
                    packages.push(package);
                }
            }
            for child in object.values() {
                collect_dotnet_packages(child, packages);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_dotnet_packages(child, packages);
            }
        }
        _ => {}
    }
}

fn parse_dotnet_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    let value: Value = serde_json::from_str(output).map_err(|_| ())?;
    let mut packages = Vec::new();
    collect_dotnet_packages(&value, &mut packages);
    Ok(packages)
}

fn parse_poetry_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    let mut packages = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(current), Some(latest)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if let Some(package) = update_package(name, current, latest) {
            packages.push(package);
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package manager runs the repository's config: it gets the
    /// dependency-check environment, never the backend's (KT-1006).
    #[cfg(unix)]
    #[test]
    fn a_dependency_check_runs_without_the_backend_environment() {
        use crate::core::child_env::probe;
        probe::plant_real_sentinel();
        let bin = tempfile::TempDir::new().unwrap();
        let project = tempfile::TempDir::new().unwrap();
        std::fs::write(project.path().join("package.json"), "{}").unwrap();
        let out = probe::env_dumping_program(bin.path(), "npm");
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let manifest = DetectedManifest {
            kind: ManagerKind::JavaScript,
            project_root: project.path().to_path_buf(),
            directory: project.path().to_path_buf(),
            relative_path: "package.json".into(),
            covers_nested: false,
        };
        let mut parent = probe::parent_with_secrets(&path, "/home/u");
        parent.push(("GOPROXY".into(), "https://proxy.example".into()));
        let borrowed: Vec<(&str, &str)> = parent
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        crate::core::child_env::with_parent_env(&borrowed, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(check_manifest(manifest))
        });
        let recorded = probe::read_dump(&out);
        probe::assert_dump_without_secrets(&recorded, &[]);
        assert_eq!(recorded.get("CI").map(String::as_str), Some("1"));
        assert_eq!(
            recorded.get("GOPROXY").map(String::as_str),
            Some("https://proxy.example"),
            "a package manager keeps its registry settings"
        );
    }

    #[test]
    fn renovate_and_docker_helpers_build_their_environment() {
        use crate::core::child_env::probe;
        let dir = tempfile::TempDir::new().unwrap();
        let (renovate, docker) = probe::with_secret_parent("/usr/bin", "/home/u", || {
            (
                renovate_command(dir.path(), &["npm"]),
                background_command("docker", dir.path(), ChildRoute::Docker, &[]),
            )
        });
        probe::assert_built_without_secrets(renovate.as_std(), "/usr/bin", &[]);
        probe::assert_built_without_secrets(docker.as_std(), "/usr/bin", &[]);
        let renovate_env = probe::env_of(renovate.as_std());
        assert_eq!(
            renovate_env
                .get("RENOVATE_REQUIRE_CONFIG")
                .map(String::as_str),
            Some("optional")
        );
    }

    #[test]
    fn detects_root_workspace_once_and_nested_composer_projects() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("packages/ui")).unwrap();
        std::fs::write(temp.path().join("packages/ui/package.json"), "{}").unwrap();
        std::fs::create_dir_all(temp.path().join("application")).unwrap();
        std::fs::write(temp.path().join("application/composer.json"), "{}").unwrap();

        let manifests = detect_manifests(temp.path());
        let paths: Vec<_> = manifests
            .iter()
            .map(|manifest| manifest.relative_path.as_str())
            .collect();

        assert!(paths.contains(&"package.json"));
        assert!(!paths.contains(&"packages/ui/package.json"));
        assert!(paths.contains(&"application/composer.json"));
    }

    #[test]
    fn detects_gradle_settings_as_the_android_workspace_manifest() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("settings.gradle"), "include ':app'\n").unwrap();
        std::fs::write(temp.path().join("build.gradle"), "plugins {}\n").unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(temp.path().join("app/build.gradle"), "plugins {}\n").unwrap();

        let manifests = detect_manifests(temp.path());
        let gradle: Vec<_> = manifests
            .iter()
            .filter(|manifest| manifest.kind == ManagerKind::Gradle)
            .collect();

        assert_eq!(gradle.len(), 1);
        assert_eq!(gradle[0].relative_path, "settings.gradle");
        assert!(gradle[0].covers_nested);
    }

    #[test]
    fn generic_fallback_maps_nested_gradle_and_bundler_updates_to_manifests() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("settings.gradle"), "include ':app'\n").unwrap();
        std::fs::write(temp.path().join("Gemfile"), "gem 'fastlane'\n").unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(temp.path().join("app/build.gradle"), "dependencies {}\n").unwrap();
        let manifests = detect_manifests(temp.path());
        let output = r#"{"config":{"gradle":[{"packageFile":"app/build.gradle","deps":[{"depName":"androidx.core:core","currentVersion":"1.9.0","updates":[{"newVersion":"1.12.0"},{"newVersion":"2.0.0"}]}]}],"gradle-wrapper":[{"packageFile":"gradle/wrapper/gradle-wrapper.properties","deps":[{"depName":"gradle","currentValue":"7.3.3","updates":[{"newVersion":"8.0.0"}]}]}],"bundler":[{"packageFile":"Gemfile","deps":[{"depName":"fastlane","lockedVersion":"2.182.0","updates":[{"newVersion":"2.237.0"}]}]}]},"msg":"packageFiles with updates"}"#;

        let parsed = parse_renovate_updates(output, &manifests).unwrap();

        assert_eq!(parsed["settings.gradle"].len(), 2);
        assert_eq!(parsed["Gemfile"].len(), 1);
        assert!(parsed["settings.gradle"]
            .iter()
            .all(|package| package.major));
        assert!(!parsed["Gemfile"][0].major);
    }

    #[test]
    fn keeps_nested_independent_manifests_without_a_workspace() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();
        std::fs::create_dir_all(temp.path().join("standalone")).unwrap();
        std::fs::write(temp.path().join("standalone/package.json"), "{}").unwrap();

        let manifests = detect_manifests(temp.path());
        let paths: Vec<_> = manifests
            .iter()
            .map(|manifest| manifest.relative_path.as_str())
            .collect();

        assert!(paths.contains(&"package.json"));
        assert!(paths.contains(&"standalone/package.json"));
    }

    #[test]
    fn composer_fallback_finds_project_compose_root_and_prioritizes_php() {
        let temp = tempfile::TempDir::new().unwrap();
        let application = temp.path().join("application");
        std::fs::create_dir_all(&application).unwrap();
        std::fs::write(temp.path().join("docker-compose.yml"), "services: {}\n").unwrap();
        let manifest = DetectedManifest {
            kind: ManagerKind::Composer,
            project_root: temp.path().to_path_buf(),
            directory: application,
            relative_path: "application/composer.json".into(),
            covers_nested: false,
        };

        assert_eq!(compose_root(&manifest), Some(temp.path().to_path_buf()));
        assert_eq!(
            prioritize_compose_services("redis\nbackend\nphp-worker\nphp\nbad service\n"),
            vec!["php", "php-worker", "backend", "redis"]
        );
        assert_eq!(
            CommandFailure::Unavailable.status(),
            DependencyCheckStatus::Unavailable
        );

        let args = standalone_composer_args(
            &manifest,
            &["outdated", "--format=json", "--locked", "--no-interaction"],
        );
        assert_eq!(
            args,
            vec![
                "run".to_string(),
                "--rm".to_string(),
                "--pull=missing".to_string(),
                "--volume".to_string(),
                format!("{}:/app:ro", manifest.directory.to_string_lossy()),
                "--workdir".to_string(),
                "/app".to_string(),
                "composer:2".to_string(),
                "outdated".to_string(),
                "--format=json".to_string(),
                "--locked".to_string(),
                "--no-interaction".to_string(),
            ]
        );
    }

    #[test]
    fn npm_parser_counts_major_and_compatible_updates() {
        let packages = parse_npm_outdated(
            r#"{
              "react": {"current":"18.3.1","wanted":"18.3.1","latest":"19.1.0"},
              "vite": {"current":"6.0.0","wanted":"6.1.0","latest":"6.1.0"},
              "fixed": {"current":"1.0.0","wanted":"1.0.0","latest":"1.0.0"}
            }"#,
        )
        .unwrap();
        assert_eq!(packages.len(), 2);
        assert!(
            packages
                .iter()
                .find(|package| package.name == "react")
                .unwrap()
                .major
        );
        assert!(
            !packages
                .iter()
                .find(|package| package.name == "vite")
                .unwrap()
                .major
        );
    }

    #[test]
    fn manifest_fingerprint_changes_with_lockfile() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();
        std::fs::write(temp.path().join("package-lock.json"), "{}").unwrap();
        let before = manifest_fingerprint(temp.path());

        std::fs::write(temp.path().join("package-lock.json"), r#"{"changed":true}"#).unwrap();

        assert_ne!(before, manifest_fingerprint(temp.path()));
    }

    #[test]
    fn composer_and_go_parsers_keep_direct_updates() {
        let composer = parse_composer_outdated(
            r#"{"installed":[
              {"name":"symfony/console","version":"v6.4.1","latest":"v7.0.0"},
              {"name":"psr/log","version":"3.0.0","latest":"3.0.0"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(composer.len(), 1);
        assert!(composer[0].major);

        let go = parse_go_outdated(
            r#"{"Path":"example.test/app","Main":true}
{"Path":"golang.org/x/text","Version":"v0.10.0","Update":{"Path":"golang.org/x/text","Version":"v0.11.0"}}
{"Path":"example.test/transitive","Version":"v1.0.0","Indirect":true,"Update":{"Version":"v2.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(go.len(), 1);
        assert_eq!(go[0].name, "golang.org/x/text");
    }

    #[test]
    fn cargo_bundler_and_dotnet_parsers_are_bounded_to_real_rows() {
        let cargo = parse_cargo_outdated(
            "Updating crates.io index\nUpdating serde v1.0.1 -> v1.0.2\nUpdating axum v0.8.0 -> v1.0.0",
        )
        .unwrap();
        assert_eq!(cargo.len(), 2);
        assert_eq!(cargo.iter().filter(|package| package.major).count(), 1);

        let bundler =
            parse_bundler_outdated("rack (newest 3.1.0, installed 2.2.0, requested ~> 2.0)\n")
                .unwrap();
        assert_eq!(bundler.len(), 1);
        assert!(bundler[0].major);

        let dotnet = parse_dotnet_outdated(
            r#"{"projects":[{"frameworks":[{"topLevelPackages":[
              {"id":"Serilog","resolvedVersion":"3.1.0","latestVersion":"4.0.0"}
            ]}]}]}"#,
        )
        .unwrap();
        assert_eq!(dotnet.len(), 1);
        assert!(dotnet[0].major);
    }

    #[test]
    fn bounded_manager_preview_keeps_major_updates_visible() {
        let temp = tempfile::TempDir::new().unwrap();
        let manifest = DetectedManifest {
            kind: ManagerKind::Cargo,
            project_root: temp.path().to_path_buf(),
            directory: temp.path().to_path_buf(),
            relative_path: "Cargo.toml".into(),
            covers_nested: false,
        };
        let mut packages: Vec<_> = (0..MAX_PACKAGES_RETURNED + 3)
            .map(|index| DependencyUpdatePackage {
                name: format!("compatible-{index:02}"),
                current: "1.0.0".into(),
                latest: "1.1.0".into(),
                major: false,
                declared_only: false,
                lockfile: None,
            })
            .collect();
        packages.push(DependencyUpdatePackage {
            name: "shlex".into(),
            current: "1.3.0".into(),
            latest: "2.0.1".into(),
            major: true,
            declared_only: false,
            lockfile: None,
        });

        let update = finalize_manager(&manifest, packages);

        assert_eq!(update.major, 1);
        assert_eq!(update.packages.len(), MAX_PACKAGES_RETURNED);
        assert_eq!(update.packages[0].name, "shlex");
        assert!(update.packages[0].major);
    }

    /// A fake `npm` that records its argv and prints `stdout`; returns the
    /// manager row `check_manifest` built for `directory`.
    #[cfg(unix)]
    fn check_with_fake_npm(
        directory: &Path,
        covers_nested: bool,
        stdout: &str,
    ) -> (DependencyManagerUpdate, Vec<String>) {
        check_with_fake_tool(
            ManagerKind::JavaScript,
            "npm",
            directory,
            covers_nested,
            (stdout, "", 0),
        )
    }

    /// A fake `program` that records its argv, prints `stdout` / `stderr` and
    /// exits with `code`; `PATH` holds nothing else the check could reach.
    #[cfg(unix)]
    fn check_with_fake_tool(
        kind: ManagerKind,
        program: &str,
        directory: &Path,
        covers_nested: bool,
        (stdout, stderr, code): (&str, &str, i32),
    ) -> (DependencyManagerUpdate, Vec<String>) {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::TempDir::new().unwrap();
        let argv = bin.path().join(format!("{program}.argv"));
        let script = bin.path().join(program);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat <<'JSON'\n{}\nJSON\ncat >&2 <<'ERR'\n{}\nERR\nexit {}\n",
                argv.display(),
                stdout,
                stderr,
                code
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Pay the first-exec scan outside the deadline under test.
        std::process::Command::new(&script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let manifest = DetectedManifest {
            kind,
            project_root: directory.to_path_buf(),
            directory: directory.to_path_buf(),
            relative_path: match kind {
                ManagerKind::Composer => "composer.json".into(),
                _ => "package.json".into(),
            },
            covers_nested,
        };
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let update = crate::core::child_env::with_parent_env(
            &[("PATH", path.as_str()), ("HOME", "/home/u")],
            || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(check_manifest(manifest))
            },
        );
        let recorded = std::fs::read_to_string(&argv)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        (update, recorded)
    }

    fn js_project(workspaces: bool, installed: bool) -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().unwrap();
        let workspaces = if workspaces {
            r#""workspaces":["packages/*"],"#
        } else {
            ""
        };
        std::fs::write(
            temp.path().join("package.json"),
            format!(r#"{{{workspaces}"dependencies":{{"joi":"^17.0.0"}}}}"#),
        )
        .unwrap();
        if installed {
            std::fs::create_dir_all(temp.path().join("node_modules/joi")).unwrap();
        }
        temp
    }

    const WORKSPACE_FLAGS: [&str; 2] = ["--workspaces", "--include-workspace-root"];

    #[test]
    fn npm_workspace_flags_are_only_passed_to_an_installed_tree() {
        assert_eq!(
            npm_outdated_args(true, true),
            vec![
                "outdated",
                "--json",
                "--long",
                "--workspaces",
                "--include-workspace-root"
            ]
        );
        assert_eq!(
            npm_outdated_args(true, false),
            vec!["outdated", "--json", "--long"]
        );
        assert_eq!(
            npm_outdated_args(false, true),
            vec!["outdated", "--json", "--long"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_with_node_modules_runs_npm_with_the_workspace_flags() {
        let project = js_project(true, true);
        let (update, argv) = check_with_fake_npm(project.path(), true, "{}");
        assert_eq!(
            argv,
            ["outdated", "--json", "--long"]
                .iter()
                .chain(&WORKSPACE_FLAGS)
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(update.status, DependencyCheckStatus::UpToDate);
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_without_node_modules_drops_the_flags_and_never_reads_up_to_date() {
        let project = js_project(true, false);
        let (update, argv) = check_with_fake_npm(project.path(), true, "{}");
        assert_eq!(argv, vec!["outdated", "--json", "--long"]);
        assert_eq!(update.status, DependencyCheckStatus::NotInstalled);
        assert!(!dependency_check_complete(&update.status));
    }

    #[cfg(unix)]
    #[test]
    fn a_plain_manifest_without_node_modules_is_not_installed_not_up_to_date() {
        let project = js_project(false, false);
        for empty in ["{}", ""] {
            let (update, argv) = check_with_fake_npm(project.path(), false, empty);
            assert_eq!(argv, vec!["outdated", "--json", "--long"]);
            assert_eq!(
                update.status,
                DependencyCheckStatus::NotInstalled,
                "{empty:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn outdated_rows_without_node_modules_still_report_updates() {
        let project = js_project(true, false);
        let (update, _) = check_with_fake_npm(
            project.path(),
            true,
            r#"{"joi":{"wanted":"17.13.0","latest":"18.0.0"}}"#,
        );
        assert_eq!(update.status, DependencyCheckStatus::UpdatesAvailable);
        assert_eq!(update.major, 1);
    }

    #[cfg(unix)]
    #[test]
    fn rows_without_an_installed_version_are_declared_only_with_no_current() {
        let project = js_project(false, false);
        let (update, _) = check_with_fake_npm(
            project.path(),
            false,
            r#"{"joi":{"wanted":"17.13.0","latest":"18.0.0"},"qs":{"current":"6.14.0","wanted":"6.14.2","latest":"6.16.0"}}"#,
        );
        let joi = update.packages.iter().find(|p| p.name == "joi").unwrap();
        assert!(joi.declared_only);
        assert_eq!(joi.current, "");
        assert!(joi.major);
        let qs = update.packages.iter().find(|p| p.name == "qs").unwrap();
        assert!(!qs.declared_only);
        assert_eq!(qs.current, "6.14.0");
    }

    #[cfg(unix)]
    #[test]
    fn a_node_modules_holding_only_dot_entries_is_not_an_install() {
        let project = js_project(true, false);
        std::fs::create_dir_all(project.path().join("node_modules/.cache")).unwrap();
        std::fs::create_dir_all(project.path().join("node_modules/.bin")).unwrap();
        let (update, argv) = check_with_fake_npm(project.path(), true, "{}");
        assert_eq!(argv, vec!["outdated", "--json", "--long"]);
        assert_eq!(update.status, DependencyCheckStatus::NotInstalled);
    }

    #[cfg(unix)]
    #[test]
    fn an_installer_state_file_or_a_package_dir_is_an_install() {
        for entry in [".package-lock.json", ".yarn-state.yml", ".modules.yaml"] {
            let project = js_project(true, false);
            std::fs::create_dir_all(project.path().join("node_modules")).unwrap();
            std::fs::write(project.path().join("node_modules").join(entry), "").unwrap();
            let (_, argv) = check_with_fake_npm(project.path(), true, "{}");
            assert!(argv.contains(&"--workspaces".to_string()), "{entry}");
        }
        let project = js_project(true, false);
        std::fs::create_dir_all(project.path().join("node_modules/@scope")).unwrap();
        let (_, argv) = check_with_fake_npm(project.path(), true, "{}");
        assert!(argv.contains(&"--workspaces".to_string()));
    }

    #[test]
    fn a_hoisted_member_defers_to_the_workspace_roots_install() {
        let root = tempfile::TempDir::new().unwrap();
        let member = root.path().join("packages/a");
        std::fs::create_dir_all(member.join("node_modules/.bin")).unwrap();
        std::fs::write(
            member.join("package.json"),
            r#"{"dependencies":{"joi":"^17.0.0"}}"#,
        )
        .unwrap();
        let manifest = DetectedManifest {
            kind: ManagerKind::JavaScript,
            project_root: root.path().to_path_buf(),
            directory: member.clone(),
            relative_path: "packages/a/package.json".into(),
            covers_nested: false,
        };

        // A root without `workspaces` is not used by npm from the member.
        std::fs::write(root.path().join("package.json"), r#"{"name":"r"}"#).unwrap();
        std::fs::create_dir_all(root.path().join("node_modules/joi")).unwrap();
        assert!(!js_dependencies_installed(&manifest));

        // A workspace root with an install is.
        std::fs::write(
            root.path().join("package.json"),
            r#"{"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        assert!(js_dependencies_installed(&manifest));

        // A workspace root with an empty install is not.
        std::fs::remove_dir_all(root.path().join("node_modules")).unwrap();
        std::fs::create_dir_all(root.path().join("node_modules/.cache")).unwrap();
        assert!(!js_dependencies_installed(&manifest));
    }

    #[test]
    fn renovate_warnings_do_not_drop_resolved_updates() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();
        let manifests = detect_manifests(temp.path());
        let output = r#"{"config":{"npm":[{"packageFile":"package.json","deps":[{"depName":"joi","currentValue":"17.0.0","warnings":[{"message":"w"}],"updates":[{"newVersion":"18.0.0"}]}]}]},"msg":"packageFiles with updates"}"#;
        let parsed = parse_renovate_updates(output, &manifests).unwrap();
        assert_eq!(parsed["package.json"].len(), 1);
        assert_eq!(parsed["package.json"][0].latest, "18.0.0");
    }

    #[cfg(unix)]
    #[test]
    fn a_manifest_with_nothing_to_install_is_up_to_date_without_node_modules() {
        let project = tempfile::TempDir::new().unwrap();
        std::fs::write(project.path().join("package.json"), r#"{"name":"x"}"#).unwrap();
        let (update, _) = check_with_fake_npm(project.path(), false, "{}");
        assert_eq!(update.status, DependencyCheckStatus::UpToDate);
    }

    #[test]
    fn a_renovate_package_file_without_resolved_dependencies_is_not_a_verdict() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();
        let manifests = detect_manifests(temp.path());

        let listed_only = r#"{"config":{"npm":[{"packageFile":"package.json","deps":[]}]},"msg":"packageFiles with updates"}"#;
        assert!(parse_renovate_updates(listed_only, &manifests)
            .unwrap()
            .is_empty());

        let no_deps_key = r#"{"config":{"npm":[{"packageFile":"package.json"}]},"msg":"packageFiles with updates"}"#;
        assert!(parse_renovate_updates(no_deps_key, &manifests)
            .unwrap()
            .is_empty());

        let skipped = r#"{"config":{"npm":[{"packageFile":"package.json","deps":[{"depName":"a","currentValue":"1.0.0","skipReason":"invalid-version","updates":[]},{"depName":"b","currentValue":"1.0.0","warnings":[{"message":"lookup failed"}],"updates":[]},{"depName":"c","currentValue":"1.0.0"}]}]},"msg":"packageFiles with updates"}"#;
        assert!(parse_renovate_updates(skipped, &manifests)
            .unwrap()
            .is_empty());

        let resolved = r#"{"config":{"npm":[{"packageFile":"package.json","deps":[{"depName":"a","currentValue":"1.0.0","updates":[]}]}]},"msg":"packageFiles with updates"}"#;
        let parsed = parse_renovate_updates(resolved, &manifests).unwrap();
        assert_eq!(parsed["package.json"].len(), 0);
        assert!(parsed.contains_key("package.json"));
    }

    #[test]
    fn an_unresolved_renovate_file_leaves_the_manager_incomplete() {
        // The merge only finalizes a manager Renovate has an entry for.
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();
        let manifests = detect_manifests(temp.path());
        let output = r#"{"config":{"npm":[{"packageFile":"package.json","deps":[]}]},"msg":"packageFiles with updates"}"#;
        let fallback = parse_renovate_updates(output, &manifests).unwrap();
        let mut manager = empty_manager(&manifests[0], DependencyCheckStatus::NotInstalled);
        if let Some(packages) = fallback.get(&manager.manifest) {
            finalize_manager_update(&mut manager, packages.clone());
        }
        assert_eq!(manager.status, DependencyCheckStatus::NotInstalled);
    }

    /// KT-1325 — what `npm outdated` prints for a checkout with no
    /// `node_modules`: no `current`, only the range's `wanted` and `latest`.
    #[cfg(unix)]
    const NPM_ROWS_WITHOUT_CURRENT: &str = r#"{
        "lodash":{"wanted":"3.10.1","latest":"4.17.21"},
        "lazysizes":{"wanted":"2.0.7","latest":"5.3.2"},
        "gulp-rev":{"wanted":"8.1.1","latest":"11.0.0"}}"#;

    #[cfg(unix)]
    fn lockfile_project(file: &str, content: &str) -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"lodash":"^3.10.1","lazysizes":"^2.0.0","gulp-rev":"^8.1.0"}}"#,
        )
        .unwrap();
        std::fs::write(temp.path().join(file), content).unwrap();
        temp
    }

    #[cfg(unix)]
    #[test]
    fn without_node_modules_the_current_version_comes_from_each_lockfile_format() {
        let lockfiles: [(&str, &str); 7] = [
            (
                "package-lock.json",
                r#"{"lockfileVersion":3,"packages":{"":{},"node_modules/lodash":{"version":"3.10.1"},"node_modules/lazysizes":{"version":"2.0.7"},"node_modules/gulp-rev":{"version":"8.1.1"}}}"#,
            ),
            (
                "package-lock.json",
                r#"{"lockfileVersion":2,"packages":{"node_modules/lodash":{"version":"3.10.1"},"node_modules/lazysizes":{"version":"2.0.7"},"node_modules/gulp-rev":{"version":"8.1.1"}},"dependencies":{}}"#,
            ),
            (
                "package-lock.json",
                r#"{"lockfileVersion":1,"dependencies":{"lodash":{"version":"3.10.1"},"lazysizes":{"version":"2.0.7"},"gulp-rev":{"version":"8.1.1"}}}"#,
            ),
            (
                "yarn.lock",
                "# yarn lockfile v1\n\nlodash@^3.10.1:\n  version \"3.10.1\"\n\nlazysizes@^2.0.0:\n  version \"2.0.7\"\n\ngulp-rev@^8.1.0:\n  version \"8.1.1\"\n",
            ),
            (
                "yarn.lock",
                "__metadata:\n  version: 8\n\n\"lodash@npm:^3.10.1\":\n  version: 3.10.1\n\n\"lazysizes@npm:^2.0.0\":\n  version: 2.0.7\n\n\"gulp-rev@npm:^8.1.0\":\n  version: 8.1.1\n",
            ),
            (
                "pnpm-lock.yaml",
                "lockfileVersion: '9.0'\n\nimporters:\n  .:\n    dependencies:\n      lodash:\n        specifier: ^3.10.1\n        version: 3.10.1\n      lazysizes:\n        specifier: ^2.0.0\n        version: 2.0.7\n      gulp-rev:\n        specifier: ^8.1.0\n        version: 8.1.1\n",
            ),
            (
                "pnpm-lock.yaml",
                "lockfileVersion: 5.4\n\nspecifiers:\n  lodash: ^3.10.1\n\ndependencies:\n  lodash: 3.10.1\n  lazysizes: 2.0.7\n  gulp-rev: 8.1.1\n",
            ),
        ];
        for (file, content) in lockfiles {
            let project = lockfile_project(file, content);
            let (update, _) = check_with_fake_npm(project.path(), false, NPM_ROWS_WITHOUT_CURRENT);
            assert_eq!(
                update.status,
                DependencyCheckStatus::UpdatesAvailable,
                "{file}"
            );
            assert_eq!(update.reason, None);
            for (name, current) in [
                ("lodash", "3.10.1"),
                ("lazysizes", "2.0.7"),
                ("gulp-rev", "8.1.1"),
            ] {
                let package = update.packages.iter().find(|p| p.name == name).unwrap();
                assert_eq!(package.current, current, "{file} {name}");
                assert!(!package.declared_only, "{file} {name}");
                assert_eq!(package.lockfile.as_deref(), Some(file), "{file} {name}");
                assert!(package.major, "{file} {name}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_package_without_a_lock_entry_stays_declared_only() {
        let project = lockfile_project(
            "package-lock.json",
            r#"{"lockfileVersion":3,"packages":{"node_modules/lodash":{"version":"3.10.1"}}}"#,
        );
        let (update, _) = check_with_fake_npm(project.path(), false, NPM_ROWS_WITHOUT_CURRENT);
        let lazysizes = update
            .packages
            .iter()
            .find(|p| p.name == "lazysizes")
            .unwrap();
        assert!(lazysizes.declared_only);
        assert_eq!(lazysizes.current, "");
        assert_eq!(lazysizes.lockfile, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_lockfile_at_the_latest_version_reads_up_to_date_not_uninstalled() {
        let project = lockfile_project(
            "package-lock.json",
            r#"{"lockfileVersion":3,"packages":{"node_modules/lodash":{"version":"4.17.21"}}}"#,
        );
        let (update, _) = check_with_fake_npm(
            project.path(),
            false,
            r#"{"lodash":{"wanted":"4.17.21","latest":"4.17.21"}}"#,
        );
        assert_eq!(update.status, DependencyCheckStatus::UpToDate);
        assert_eq!(update.reason, None);
    }

    #[cfg(unix)]
    #[test]
    fn an_installed_tree_keeps_npms_current_and_no_lockfile_source() {
        let project = lockfile_project(
            "package-lock.json",
            r#"{"lockfileVersion":3,"packages":{"node_modules/lodash":{"version":"3.10.0"}}}"#,
        );
        std::fs::create_dir_all(project.path().join("node_modules/lodash")).unwrap();
        let (update, _) = check_with_fake_npm(
            project.path(),
            false,
            r#"{"lodash":{"current":"3.10.1","wanted":"3.10.1","latest":"4.17.21"}}"#,
        );
        assert_eq!(update.packages[0].current, "3.10.1");
        assert_eq!(update.packages[0].lockfile, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_not_installed_manifest_without_a_lockfile_says_why() {
        let project = js_project(false, false);
        let (update, _) = check_with_fake_npm(project.path(), false, "{}");
        assert_eq!(update.status, DependencyCheckStatus::NotInstalled);
        let reason = update.reason.unwrap();
        assert!(reason.contains("no node_modules"), "{reason}");
    }

    #[cfg(unix)]
    #[test]
    fn a_composer_failure_carries_the_reason_composer_printed() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("composer.json"), "{}").unwrap();
        let (update, argv) = check_with_fake_tool(
            ManagerKind::Composer,
            "composer",
            temp.path(),
            false,
            (
                "",
                "No composer.lock file present. Run composer update to generate one.",
                1,
            ),
        );
        assert!(argv.contains(&"--locked".to_string()));
        assert_eq!(update.status, DependencyCheckStatus::Error);
        let reason = update.reason.expect("an Error carries its reason");
        assert!(reason.contains("`composer` failed (exit 1)"), "{reason}");
        assert!(reason.contains("No composer.lock file present"), "{reason}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unparseable_composer_answer_says_it_could_not_be_parsed() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("composer.json"), "{}").unwrap();
        let (update, _) = check_with_fake_tool(
            ManagerKind::Composer,
            "composer",
            temp.path(),
            false,
            ("Deprecated: PHP 8.4 notice", "", 0),
        );
        assert_eq!(update.status, DependencyCheckStatus::Error);
        let reason = update.reason.unwrap();
        assert!(
            reason.contains("could not parse the output of `composer`"),
            "{reason}"
        );
        assert!(reason.contains("Deprecated: PHP 8.4 notice"), "{reason}");
    }

    #[cfg(unix)]
    #[test]
    fn composer_rows_name_composer_lock_as_their_source() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("composer.json"), "{}").unwrap();
        let (update, _) = check_with_fake_tool(
            ManagerKind::Composer,
            "composer",
            temp.path(),
            false,
            (
                r#"{"installed":[{"name":"symfony/console","version":"v6.4.1","latest":"v7.1.0"}]}"#,
                "",
                0,
            ),
        );
        assert_eq!(update.status, DependencyCheckStatus::UpdatesAvailable);
        assert_eq!(
            update.packages[0].lockfile.as_deref(),
            Some("composer.lock")
        );
        assert_eq!(update.reason, None);
    }

    #[cfg(unix)]
    #[test]
    fn an_npm_error_document_surfaces_its_summary() {
        let project = js_project(false, true);
        let (update, _) = check_with_fake_tool(
            ManagerKind::JavaScript,
            "npm",
            project.path(),
            false,
            (
                r#"{"error":{"code":"ENOTFOUND","summary":"request to https://registry.npmjs.org failed"}}"#,
                "",
                1,
            ),
        );
        assert_eq!(update.status, DependencyCheckStatus::Error);
        let reason = update.reason.unwrap();
        assert!(
            reason.contains("request to https://registry.npmjs.org failed"),
            "{reason}"
        );
    }

    #[test]
    fn every_unsettled_status_has_a_reason() {
        let manifest = DetectedManifest {
            kind: ManagerKind::Composer,
            project_root: PathBuf::from("/p"),
            directory: PathBuf::from("/p"),
            relative_path: "composer.json".into(),
            covers_nested: false,
        };
        for failure in [
            CommandFailure::Unavailable,
            CommandFailure::Error,
            CommandFailure::TimedOut,
        ] {
            let update = unchecked_manager(
                &manifest,
                failure.status(),
                failure_reason("composer", failure),
            );
            let reason = update.reason.unwrap();
            assert!(reason.contains("composer"), "{reason}");
        }
        let long = unchecked_manager(&manifest, DependencyCheckStatus::Error, "x ".repeat(500));
        assert!(long.reason.unwrap().chars().count() <= MAX_REASON_CHARS);
        // A verdict from the fallback clears the reason of the failed tool.
        let mut failed = unchecked_manager(&manifest, DependencyCheckStatus::Error, "boom");
        finalize_manager_update(&mut failed, Vec::new());
        assert_eq!(failed.reason, None);
    }

    #[test]
    fn renovate_rows_are_re_read_from_the_lockfile() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"lodash":"^3.10.1"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("yarn.lock"),
            "lodash@^3.10.1:\n  version \"3.10.1\"\n",
        )
        .unwrap();
        let manifests = detect_manifests(temp.path());
        // Renovate reported the range as the current value.
        let mut packages = vec![update_package("lodash", "^3.10.1", "4.17.21").unwrap()];
        apply_fallback_locks(&manifests[0], &mut packages);
        assert_eq!(packages[0].current, "3.10.1");
        assert_eq!(packages[0].lockfile.as_deref(), Some("yarn.lock"));
        assert!(packages[0].major);
    }

    #[test]
    fn composer_lock_resolves_renovate_composer_rows() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("composer.json"), "{}").unwrap();
        std::fs::write(
            temp.path().join("composer.lock"),
            r#"{"packages":[{"name":"symfony/console","version":"6.4.1"}],"packages-dev":[]}"#,
        )
        .unwrap();
        let manifests = detect_manifests(temp.path());
        let mut packages = vec![update_package("symfony/console", "^6.0", "7.1.0").unwrap()];
        apply_fallback_locks(&manifests[0], &mut packages);
        assert_eq!(packages[0].current, "6.4.1");
        assert_eq!(packages[0].lockfile.as_deref(), Some("composer.lock"));
    }
}
