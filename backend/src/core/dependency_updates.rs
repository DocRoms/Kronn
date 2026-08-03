use crate::core::cmd::async_cmd;
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
                    finalize_manager_update(&mut manager, packages.clone());
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

fn configure_background_command(command: &mut tokio::process::Command, directory: &Path) {
    command
        .current_dir(directory)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("COMPOSER_NO_INTERACTION", "1")
        .kill_on_drop(true);
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
    let mut list = async_cmd("docker");
    list.args(["compose", "ps", "--services", "--status", "running"]);
    configure_background_command(&mut list, &root);
    let listed = capture_command(&mut list, CONTAINER_DISCOVERY_TIMEOUT).await?;
    if !listed.status.success() {
        return Err(CommandFailure::Unavailable);
    }

    let services = prioritize_compose_services(&String::from_utf8_lossy(&listed.stdout));
    for service in services {
        let mut probe = async_cmd("docker");
        probe.args([
            "compose",
            "exec",
            "-T",
            &service,
            "composer",
            "--version",
            "--no-ansi",
        ]);
        configure_background_command(&mut probe, &root);
        let Ok(probed) = capture_command(&mut probe, CONTAINER_DISCOVERY_TIMEOUT).await else {
            continue;
        };
        if !probed.status.success() {
            continue;
        }

        let mut command = async_cmd("docker");
        command.args(["compose", "exec", "-T", &service, "composer"]);
        command.args(args);
        configure_background_command(&mut command, &root);
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
    let mut command = async_cmd("docker");
    command.args(standalone_composer_args(manifest, args));
    configure_background_command(&mut command, &manifest.directory);
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
            let packages = by_manifest
                .entry(manifest.relative_path.clone())
                .or_default();
            let Some(dependencies) = file.get("deps").and_then(Value::as_array) else {
                continue;
            };
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
        }
    }
    Ok(by_manifest)
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

    let mut command = async_cmd("npx");
    command.args([
        "--yes",
        "--package",
        RENOVATE_NODE_PACKAGE,
        "--package",
        RENOVATE_PACKAGE,
        "renovate",
        "--platform=local",
        "--onboarding=false",
        &format!("--enabled-managers={}", enabled.join(",")),
    ]);
    configure_background_command(&mut command, root);
    command
        .env("LOG_LEVEL", "debug")
        .env("LOG_FORMAT", "json")
        .env("RENOVATE_REQUIRE_CONFIG", "optional");
    let output = capture_command(&mut command, FALLBACK_CHECK_TIMEOUT).await?;
    parse_renovate_updates(&String::from_utf8_lossy(&output.stdout), manifests)
        .map_err(|_| CommandFailure::Error)
}

async fn check_manifest(manifest: DetectedManifest) -> DependencyManagerUpdate {
    if manifest.kind == ManagerKind::Gradle {
        return empty_manager(&manifest, DependencyCheckStatus::Unsupported);
    }

    let (program, args): (&str, Vec<&str>) = match manifest.kind {
        ManagerKind::JavaScript if manifest.covers_nested => (
            "npm",
            vec![
                "outdated",
                "--json",
                "--long",
                "--workspaces",
                "--include-workspace-root",
            ],
        ),
        ManagerKind::JavaScript => ("npm", vec!["outdated", "--json", "--long"]),
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

    let mut command = async_cmd(program);
    command.args(&args);
    configure_background_command(&mut command, &manifest.directory);
    let output = match capture_command(&mut command, CHECK_TIMEOUT).await {
        Err(CommandFailure::Unavailable) if manifest.kind == ManagerKind::Composer => {
            match composer_via_compose(&manifest, &args).await {
                Ok(output) => output,
                Err(_) => match composer_via_standalone_container(&manifest, &args).await {
                    Ok(output) => output,
                    Err(error) => return empty_manager(&manifest, error.status()),
                },
            }
        }
        Err(error) => return empty_manager(&manifest, error.status()),
        Ok(output) => output,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed = match manifest.kind {
        ManagerKind::JavaScript => parse_npm_outdated(&stdout),
        ManagerKind::Composer => parse_composer_outdated(&stdout),
        ManagerKind::Cargo => parse_cargo_outdated(&format!("{stdout}\n{stderr}")),
        ManagerKind::Go => parse_go_outdated(&stdout),
        ManagerKind::Bundler => parse_bundler_outdated(&stdout),
        ManagerKind::Gradle => unreachable!("Gradle is handled before command execution"),
        ManagerKind::DotNet => parse_dotnet_outdated(&stdout),
        ManagerKind::Poetry => parse_poetry_outdated(&stdout),
    };
    match parsed {
        Ok(packages) if output.status.success() || !packages.is_empty() => {
            finalize_manager(&manifest, packages)
        }
        Ok(_) => empty_manager(&manifest, DependencyCheckStatus::Error),
        Err(_) if output.status.success() && stdout.trim().is_empty() => {
            finalize_manager(&manifest, Vec::new())
        }
        Err(_) => empty_manager(&manifest, DependencyCheckStatus::Error),
    }
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
    })
}

fn parse_npm_outdated(output: &str) -> Result<Vec<DependencyUpdatePackage>, ()> {
    if output.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(output).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.contains_key("error") {
        return Err(());
    }
    Ok(object
        .iter()
        .filter_map(|(name, entry)| {
            let latest = entry.get("latest")?.as_str()?;
            let current = entry
                .get("current")
                .and_then(Value::as_str)
                .or_else(|| entry.get("wanted").and_then(Value::as_str))?;
            update_package(name, current, latest)
        })
        .collect())
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
            })
            .collect();
        packages.push(DependencyUpdatePackage {
            name: "shlex".into(),
            current: "1.3.0".into(),
            latest: "2.0.1".into(),
            major: true,
        });

        let update = finalize_manager(&manifest, packages);

        assert_eq!(update.major, 1);
        assert_eq!(update.packages.len(), MAX_PACKAGES_RETURNED);
        assert_eq!(update.packages[0].name, "shlex");
        assert!(update.packages[0].major);
    }
}
