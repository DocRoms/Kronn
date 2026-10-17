use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::models::{AiConfigType, DetectedRepo, ProjectWriteAccess, ProjectWriteAccessStatus};

/// AI config file patterns to look for in repositories
const AI_CONFIG_PATTERNS: &[(&str, AiConfigType)] = &[
    ("CLAUDE.md", AiConfigType::ClaudeMd),
    (".claude", AiConfigType::ClauseDir),
    (".ai", AiConfigType::AiDir),
    (".cursorrules", AiConfigType::CursorRules),
    (".continue", AiConfigType::ContinueDev),
    (".mcp.json", AiConfigType::McpJson),
];

/// Default depth when scanning for git repos
const DEFAULT_SCAN_DEPTH: usize = 4;

/// Scan a list of paths for git repositories
pub async fn scan_paths(paths: &[String], ignore: &[String]) -> Result<Vec<DetectedRepo>> {
    scan_paths_with_depth(paths, ignore, DEFAULT_SCAN_DEPTH).await
}

/// Scan a list of paths for git repositories with configurable depth (2–10)
pub async fn scan_paths_with_depth(
    paths: &[String],
    ignore: &[String],
    depth: usize,
) -> Result<Vec<DetectedRepo>> {
    let depth = depth.clamp(2, 10);
    let mut repos = Vec::new();

    // One-shot env dump at the start of a scan. Mirrors the agent-detect
    // dump — together they make "why is this macOS install broken"
    // questions answerable from a single `make logs | grep kronn::` sweep.
    tracing::info!(target: "kronn::scanner",
        host_os = %crate::core::child_env::var("KRONN_HOST_OS").unwrap_or_else(|_| "<unset>".into()),
        host_home = %crate::core::child_env::var("KRONN_HOST_HOME").unwrap_or_else(|_| "<unset>".into()),
        host_home_aliases = ?host_home_aliases(),
        depth = depth,
        paths = ?paths,
        "starting scan",
    );

    for base_path in paths {
        let expanded = shellexpand(base_path);
        let base = resolve_host_path(&expanded);
        tracing::info!(target: "kronn::scanner",
            "scanning '{}' (expanded: '{}', resolved: '{}')", base_path, expanded, base.display());
        if !base.exists() {
            tracing::warn!(target: "kronn::scanner",
                "scan path does not exist: {} (resolved: {}) — skipping", base_path, base.display());
            continue;
        }

        let found = scan_directory(&base, ignore, depth).await?;
        tracing::info!(target: "kronn::scanner",
            "found {} repos in {}", found.len(), base.display());
        repos.extend(found);
    }

    // Filter out repos whose host path doesn't exist (ghost paths from symlink resolution)
    let before_ghost = repos.len();
    repos.retain(|r| {
        let exists = Path::new(&r.path).exists()
            || resolve_host_path(&r.path).exists();
        if !exists {
            tracing::debug!(target: "kronn::scanner",
                "ghost-path filter: dropping {} (neither raw path nor resolve_host_path exists)", r.path);
        }
        exists
    });
    if before_ghost != repos.len() {
        tracing::info!(target: "kronn::scanner",
            "ghost-path filter: dropped {} repos ({} -> {})",
            before_ghost - repos.len(), before_ghost, repos.len());
    }

    // Deduplicate canonical working-copy paths after host/container mapping.
    // Names and remotes do not distinguish independent clones or local-only repos.
    {
        let mut seen = HashSet::new();
        repos.retain(|r| {
            let container_path = resolve_host_path(&r.path);
            let key = std::fs::canonicalize(&container_path)
                .unwrap_or(container_path)
                .display()
                .to_string();
            if seen.contains(&key) {
                tracing::debug!(target: "kronn::scanner",
                    "same working copy reached twice: {} (resolved: {})", r.path, key);
                false
            } else {
                seen.insert(key);
                true
            }
        });
    }

    tracing::info!(target: "kronn::scanner",
        "scan complete: {} repositories", repos.len());
    Ok(repos)
}

/// Recursively scan a directory for git repos
async fn scan_directory(
    base: &Path,
    ignore: &[String],
    max_depth: usize,
) -> Result<Vec<DetectedRepo>> {
    let mut repos = Vec::new();

    // Follow symlinks for repositories outside the scan root. Canonical-path
    // tracking prevents repeated traversal and cycles.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let walker = WalkDir::new(base)
        .max_depth(max_depth)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            // Case-insensitive comparison: APFS (macOS) and NTFS (Windows) are
            // case-insensitive by default, so "node_modules" must also match "Node_Modules".
            let name_lower = name.to_ascii_lowercase();
            !ignore.iter().any(|i| name_lower == i.to_ascii_lowercase())
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("Walkdir error (skipping): {}", e);
                continue;
            }
        };

        let path = entry.path();

        // One directory, one visit, whatever route led here. Two symlinks into
        // the same tree would otherwise report the same repository twice.
        if path.is_dir() {
            let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if !visited.insert(key) {
                continue;
            }
        }

        // Check if this directory contains a .git folder
        if path.is_dir() && path.join(".git").exists() {
            match detect_repo(path).await {
                Ok(repo) => repos.push(repo),
                Err(e) => tracing::warn!("Error scanning {}: {}", path.display(), e),
            }
        }
    }

    Ok(repos)
}

/// Analyze a single git repository
async fn detect_repo(path: &Path) -> Result<DetectedRepo> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Read git remote
    let remote_url = read_git_remote(path).await.ok();

    // Read current branch
    let branch = read_git_branch(path)
        .await
        .unwrap_or_else(|_| "main".to_string());

    // Detect AI configs
    let ai_configs = detect_ai_configs(path).await;

    // Convert container path back to host path for storage
    let path_str = restore_host_path(path);
    // Hidden if any parent directory starts with "."
    let hidden = path
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'));

    Ok(DetectedRepo {
        path: path_str,
        name,
        remote_url,
        branch,
        ai_configs,
        has_project: false,
        hidden,
    })
}

/// Detect AI configuration files/directories in a repo
async fn detect_ai_configs(path: &Path) -> Vec<AiConfigType> {
    let mut found = Vec::new();

    for (pattern, config_type) in AI_CONFIG_PATTERNS {
        let check_path = path.join(pattern);
        if check_path.exists() {
            found.push(config_type.clone());
        }
    }

    found
}

/// Check if a path is a WSL UNC path (\\wsl.localhost\... or \\wsl$\...)
#[cfg(target_os = "windows")]
fn is_wsl_unc_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with(r"\\wsl.localhost\") || s.starts_with(r"\\wsl$\")
}

/// Convert a WSL UNC path to a Linux path for use inside WSL.
/// e.g. \\wsl.localhost\Ubuntu\home\user\repo → /home/user/repo
#[cfg(target_os = "windows")]
fn unc_to_wsl_linux_path(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    // Strip \\wsl.localhost\<distro>\ or \\wsl$\<distro>\
    let remainder = s
        .strip_prefix(r"\\wsl.localhost\")
        .or_else(|| s.strip_prefix(r"\\wsl$\"))?;
    // Skip distro name (first path component) — handle both backslash and forward slash
    let sep_idx = remainder.find('\\').or_else(|| remainder.find('/'))?;
    let linux_part = &remainder[sep_idx..];
    Some(linux_part.replace('\\', "/"))
}

/// Run a git command, handling both local and WSL UNC paths on Windows.
/// On Windows with WSL UNC paths, runs git via wsl.exe to avoid git.exe UNC issues.
async fn run_git_command(path: &Path, args: &[&str]) -> Result<std::process::Output> {
    #[cfg(target_os = "windows")]
    {
        if is_wsl_unc_path(path) {
            // Run git inside WSL for WSL filesystem paths
            if let Some(linux_path) = unc_to_wsl_linux_path(path) {
                let git_cmd = format!("git -C '{}' {}", linux_path, args.join(" "));
                // git runs the repository's hooks: git's own environment.
                let mut command =
                    super::cmd::async_cmd("wsl.exe", crate::core::child_env::ChildRoute::Git);
                return command
                    .args(["-e", "bash", "-lc", &git_cmd])
                    .output()
                    .await
                    .context("Failed to run git via wsl.exe");
            }
        }
        crate::core::cmd::async_git_cmd()
            .args(args)
            .current_dir(path)
            .output()
            .await
            .context("Failed to run git")
    }
    #[cfg(not(target_os = "windows"))]
    {
        crate::core::cmd::async_git_cmd()
            .args(args)
            .current_dir(path)
            .output()
            .await
            .context("Failed to run git")
    }
}

/// Read the git remote origin URL
async fn read_git_remote(path: &Path) -> Result<String> {
    let output = run_git_command(path, &["remote", "get-url", "origin"]).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Read the current git branch
async fn read_git_branch(path: &Path) -> Result<String> {
    let output = run_git_command(path, &["branch", "--show-current"]).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Convert a container mount path back to the original host path.
/// e.g. /host-home/Repositories/foo -> /home/priol/Repositories/foo
pub(crate) fn restore_host_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(relative) = s.strip_prefix("/host-home") {
        if let Ok(host_home) = crate::core::child_env::var("KRONN_HOST_HOME") {
            return format!("{}{}", host_home, relative);
        }
    }
    s.to_string()
}

/// Returns true if a path string contains a `..` component anywhere.
/// We reject these defensively before mapping to /host-home so a request
/// cannot escape the mount root via traversal (`/host-home/../etc/passwd`).
/// Works on both Unix and Windows path separators.
pub fn contains_parent_dir(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Map a host-absolute path to the Docker mount point if needed.
/// e.g. /home/priol/Repositories -> /host-home/Repositories
///
/// SECURITY: paths containing `..` components are returned as-is without
/// mapping. The caller still gets a `PathBuf`, but downstream filesystem
/// calls will fail to resolve outside the mount root, and we never silently
/// translate a traversal attempt into a `/host-home` access.
pub fn resolve_host_path(path: &str) -> PathBuf {
    if contains_parent_dir(path) {
        tracing::warn!(target: "kronn::scanner",
            "resolve_host_path({}): refused — path contains '..'", path);
        return PathBuf::from(path);
    }

    // In Docker: try every plausible alias of HOST_HOME. On macOS APFS,
    // `/Users/x` is a firmlink to `/System/Volumes/Data/Users/x` and some
    // paths (especially canonicalized ones) arrive in the second form —
    // we need to accept both so the scan doesn't drop them.
    let aliases = host_home_aliases();
    if aliases.is_empty() {
        tracing::debug!(target: "kronn::scanner",
            "resolve_host_path({}): KRONN_HOST_HOME unset — returning path as-is", path);
        return PathBuf::from(path);
    }

    for alias in &aliases {
        if let Some(relative) = path.strip_prefix(alias) {
            let mapped = PathBuf::from(format!("/host-home{}", relative));
            if mapped.exists() {
                tracing::debug!(target: "kronn::scanner",
                    "resolve_host_path({}): mapped via alias '{}' -> {}",
                    path, alias, mapped.display());
                return mapped;
            } else {
                tracing::debug!(target: "kronn::scanner",
                    "resolve_host_path({}): strip matched alias '{}' but target {} does not exist",
                    path, alias, mapped.display());
            }
        }
    }

    tracing::debug!(target: "kronn::scanner",
        "resolve_host_path({}): no alias matched (tried {:?}) — returning path as-is",
        path, aliases);
    PathBuf::from(path)
}

/// Describe whether Kronn can write project-local runtime files.
///
/// Docker deliberately exposes the full host home read-only and overlays only
/// configured repository roots read-write. Existence therefore does not imply
/// writability: a sibling repo can be scanned successfully but reject every
/// `.mcp.json` update. Keep that security boundary and make it observable.
pub fn diagnose_project_write_access(path: &str) -> ProjectWriteAccess {
    let resolved = resolve_host_path(path);
    if !resolved.exists() {
        return ProjectWriteAccess {
            status: ProjectWriteAccessStatus::Missing,
            reason: Some("path_missing".into()),
            writable_roots: writable_repo_roots(),
        };
    }

    let roots = writable_repo_roots();
    let in_docker = crate::core::child_env::var("KRONN_IN_DOCKER").as_deref() == Ok("1");
    if in_docker && !roots.is_empty() {
        let writable = roots.iter().any(|root| {
            Path::new(path).starts_with(root) || resolved.starts_with(resolve_host_path(root))
        });
        return ProjectWriteAccess {
            status: if writable {
                ProjectWriteAccessStatus::Writable
            } else {
                ProjectWriteAccessStatus::ReadOnly
            },
            reason: (!writable).then(|| "outside_rw_perimeter".into()),
            writable_roots: roots,
        };
    }

    // Native mode has no Docker mount boundary. Avoid a mutation probe on a
    // read path; the filesystem read-only bit is the best side-effect-free
    // signal and "Unknown" remains available for metadata failures.
    match std::fs::metadata(&resolved) {
        Ok(metadata) => ProjectWriteAccess {
            status: if metadata.permissions().readonly() {
                ProjectWriteAccessStatus::ReadOnly
            } else {
                ProjectWriteAccessStatus::Writable
            },
            reason: metadata
                .permissions()
                .readonly()
                .then(|| "filesystem_read_only".into()),
            writable_roots: roots,
        },
        Err(_) => ProjectWriteAccess {
            status: ProjectWriteAccessStatus::Unknown,
            reason: Some("permission_unknown".into()),
            writable_roots: roots,
        },
    }
}

/// Host paths mounted read-write into the backend. The primary root is always
/// passed by docker-compose; optional roots use the same colon-separated value
/// already consumed by Makefile's generated override.
pub fn writable_repo_roots() -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(root) = crate::core::child_env::var("KRONN_REPOS_DIR") {
        let root = root.trim();
        if !root.is_empty() {
            roots.push(root.trim_end_matches('/').to_string());
        }
    }
    if let Ok(extra) = crate::core::child_env::var("KRONN_EXTRA_REPOS") {
        for root in extra
            .split(':')
            .map(str::trim)
            .filter(|root| !root.is_empty())
        {
            let root = root.trim_end_matches('/').to_string();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    roots
}

/// Every plausible host-home prefix the caller's path could start with.
/// Order matters: we try the raw KRONN_HOST_HOME first, then the macOS
/// firmlink alternatives. On Linux / WSL only the first entry is used.
///
/// Exposed as `pub(crate)` so the scanner tests can sanity-check the
/// alias expansion logic in isolation.
pub(crate) fn host_home_aliases() -> Vec<String> {
    let Ok(host_home) = crate::core::child_env::var("KRONN_HOST_HOME") else {
        return Vec::new();
    };
    if host_home.is_empty() {
        return Vec::new();
    }
    let mut aliases = vec![host_home.clone()];

    // macOS APFS: `/Users/<name>` ↔ `/System/Volumes/Data/Users/<name>`
    // (firmlink on modern APFS) ↔ `/private/var/Users/<name>` (legacy
    // layout still seen in older homedirs / some VMs). Include all forms.
    if let Some(rest) = host_home.strip_prefix("/Users/") {
        let canonical = format!("/System/Volumes/Data/Users/{}", rest);
        if canonical != host_home {
            aliases.push(canonical);
        }
        let legacy = format!("/private/var/Users/{}", rest);
        if legacy != host_home {
            aliases.push(legacy);
        }
    }
    aliases
}

/// The project's documentation directory (KT-263).
///
/// Resolved by `core::docs_location`: the project's override, then the global
/// default from the settings, then the historical order — `docs/AGENTS.md`,
/// `doc/AGENTS.md`, `ai/index.md`, then whichever of those folders exists, and
/// `docs/` for a fresh project. Callers create the folder when they need it.
pub fn detect_docs_dir(project_path: &Path) -> PathBuf {
    crate::core::docs_location::docs_dir(project_path)
}

/// File path of the docs directory's entry point. `AGENTS.md` post-pivot,
/// legacy `index.md` for projects that haven't migrated yet.
pub fn detect_docs_entry(project_path: &Path) -> PathBuf {
    let dir = detect_docs_dir(project_path);
    // An audit run in an `ai/` folder writes `AGENTS.md` like any other root.
    if dir.file_name().and_then(|n| n.to_str()) == Some("ai") && !dir.join("AGENTS.md").is_file() {
        dir.join("index.md")
    } else {
        dir.join("AGENTS.md")
    }
}

const BOOTSTRAP_START: &str = "KRONN:BOOTSTRAP:START";
const BOOTSTRAP_END: &str = "KRONN:BOOTSTRAP:END";

/// A marker comment on its own line, outside fenced code.
fn is_bootstrap_marker(line: &str, marker: &str, other: &str) -> bool {
    let line = line.trim();
    line.starts_with("<!--")
        && line.ends_with("-->")
        && line.contains(marker)
        && !line.contains(other)
}

/// `content` without the first-run bootstrap block older Kronn versions wrote
/// at the top of the entry file ("FIRST-RUN TASK — … You must analyze the
/// repository and fill in all files"), or `None` when there is no complete
/// START…END block. An unmatched START is left alone: cutting to the end of
/// the file would delete what a person wrote below it.
pub fn without_legacy_bootstrap_block(content: &str) -> Option<String> {
    // Segments keep their own terminator (`\n` or `\r\n`), so everything outside
    // the block is written back byte for byte.
    let segments: Vec<&str> = content.split_inclusive('\n').collect();
    let mut in_fence = false;
    let mut start = None;
    let mut end = None;
    for (index, segment) in segments.iter().enumerate() {
        let line = segment.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if start.is_none() {
            if is_bootstrap_marker(line, BOOTSTRAP_START, BOOTSTRAP_END) {
                start = Some(index);
            }
        } else if is_bootstrap_marker(line, BOOTSTRAP_END, BOOTSTRAP_START) {
            end = Some(index);
            break;
        }
    }
    let (start, end) = (start?, end?);
    // A block at the very top leaves its padding behind: drop the blank lines
    // around it, and only those. A block further down leaves its neighbours as is.
    let at_top = segments[..start].iter().all(|s| s.trim().is_empty());
    let mut resume = end + 1;
    if at_top {
        while resume < segments.len() && segments[resume].trim().is_empty() {
            resume += 1;
        }
    }
    let head = if at_top { &[][..] } else { &segments[..start] };
    Some(head.iter().chain(&segments[resume..]).copied().collect())
}

/// Removes the legacy bootstrap block from the project's docs entry file, so
/// no agent that reads it is handed a "fill in all files" task, audited or not.
/// `Ok(true)` when the file was rewritten. A symlinked path is refused. The
/// rewrite is atomic (temp sibling + rename), keeps the file's permissions and
/// is abandoned if the file changed since it was read.
pub fn retire_legacy_bootstrap_block(project_path: &Path) -> Result<bool, String> {
    let entry = detect_docs_entry(project_path);
    let Ok(observed) = std::fs::read(&entry) else {
        return Ok(false);
    };
    let Ok(content) = std::str::from_utf8(&observed) else {
        return Ok(false);
    };
    let Some(cleaned) = without_legacy_bootstrap_block(content) else {
        return Ok(false);
    };
    crate::core::fs_guard::assert_contained_no_symlink(project_path, &entry)?;
    crate::core::mcp_scanner::atomic_write_if_unchanged_preserving_permissions(
        &entry, &cleaned, &observed,
    )
    .map_err(|e| format!("Cannot rewrite {}: {e}", entry.display()))?;
    Ok(true)
}

/// True when the project still has the legacy `ai/index.md` layout AND
/// no migrated `docs/AGENTS.md` (or `doc/AGENTS.md`) has appeared yet.
///
/// Drives the "Migrer vers docs/" banner on `ProjectCard` — once the
/// operator triggers `migrate_docs`, the helper flips back to false on
/// next list refresh and the banner disappears. We deliberately do NOT
/// treat a symlinked `ai/` (created post-migration for retro-compat) as
/// "needs migration" because the docs/AGENTS.md check short-circuits.
pub fn needs_docs_migration(project_path: &Path) -> bool {
    // A configured folder is the operator's choice: never offer to move it.
    let resolved = crate::core::docs_location::resolution(project_path);
    resolved.source == crate::core::docs_location::DocsDirSource::Detected
        && resolved.dir == project_path.join("ai")
        && project_path.join("ai/index.md").is_file()
}

/// True when `content` still has an unfilled template placeholder like
/// `{{PROJECT_NAME}}` — anchored to all-caps + underscore so it doesn't
/// false-positive on Twig (`{{ user.name }}`), GitHub Actions (`${{ secrets.FOO }}`),
/// or instructional text that mentions `{{...}}` as an example.
pub(crate) fn has_unfilled_placeholder(content: &str) -> bool {
    regex_lite::Regex::new(r"\{\{[A-Z_]+\}\}")
        .ok()
        .map(|re| re.is_match(content))
        .unwrap_or(false)
}

/// Detect the AI audit status for a project based on filesystem state.
pub fn detect_audit_status(project_path: &str) -> crate::models::AiAuditStatus {
    detect_audit_status_with_runs(project_path, false)
}

/// Same as `detect_audit_status`, plus the instance's own record: a filled docs
/// entry with no evidence on this branch counts as Audited when this Kronn
/// instance completed a Full audit of the project (`completed_full_run`).
/// Never Validated: validation needs evidence the docs on this branch carry.
pub fn detect_audit_status_with_runs(
    project_path: &str,
    completed_full_run: bool,
) -> crate::models::AiAuditStatus {
    use crate::models::AiAuditStatus;

    let path = resolve_host_path(project_path);

    // Audit status detection accepts both the new (`docs/AGENTS.md`) and
    // legacy (`ai/index.md`) conventions during the migration window.
    // A directory in EITHER shape with NO entry file = TemplateInstalled
    // (partial state, audit was started but never finished).
    // Unconfigured, the resolved folder exists iff one of docs/doc/ai does.
    if !detect_docs_dir(&path).is_dir() {
        return AiAuditStatus::NoTemplate;
    }
    let index_file = detect_docs_entry(&path);
    if !index_file.exists() {
        return AiAuditStatus::TemplateInstalled;
    }

    let content = match std::fs::read_to_string(&index_file) {
        Ok(c) => c,
        Err(e) => {
            // File exists but can't be read (permission issue) — don't confuse with "no template"
            tracing::warn!(
                "Cannot read {} at {}: {} — treating as TemplateInstalled",
                index_file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("entry"),
                index_file.display(),
                e
            );
            return AiAuditStatus::TemplateInstalled;
        }
    };

    if content.contains("KRONN:BOOTSTRAP:START") || content.contains("KRONN:BOOTSTRAP:END") {
        return AiAuditStatus::TemplateInstalled;
    }
    if has_unfilled_placeholder(&content) {
        return AiAuditStatus::TemplateInstalled;
    }

    // Canonical source of truth: `docs/.kronn.json`. Legacy projects (markers,
    // `docs/checksums.json`) are read by the fallbacks below without writing:
    // the state file is backfilled only by the write paths (audit, validation).
    if let Some(state) = crate::core::kronn_state::read(&path) {
        if state.validated_at.is_some() {
            return AiAuditStatus::Validated;
        }
        // A real audit (or an explicit human attestation) performed after
        // bootstrap advances the project to Audited. Legacy backfill also
        // seeds an audit entry, so it must not outrank its more precise
        // Bootstrapped marker below.
        if state.audits.iter().any(|entry| {
            entry.provenance != crate::core::kronn_state::AuditProvenance::LegacyEvidence
        }) {
            return AiAuditStatus::Audited;
        }
        if state.bootstrapped_at.is_some() {
            return AiAuditStatus::Bootstrapped;
        }
        if state.has_any_audit() {
            return AiAuditStatus::Audited;
        }
        // File present but empty (e.g. partial init) — fall through to
        // legacy checks rather than asserting Audited.
    }

    // Legacy fallbacks for projects audited before `.kronn.json` existed.
    // Order matters: Validated wins over Bootstrapped, which wins over
    // Audited-from-checksums.
    let has_validated_marker = content.contains("KRONN:VALIDATED");
    let has_bootstrapped_marker = content.contains("KRONN:BOOTSTRAPPED");

    if has_bootstrapped_marker {
        if has_validated_marker {
            return AiAuditStatus::Validated;
        }
        return AiAuditStatus::Bootstrapped;
    }
    if has_validated_marker {
        return AiAuditStatus::Validated;
    }

    // `docs/checksums.json` is written by every full/partial audit and
    // predates `.kronn.json`. Its presence is hard evidence that Kronn
    // actually ran an audit on this project — distinct from "docs/
    // happens to exist because the user wrote their own AGENTS.md".
    if crate::core::checksums::read_checksums_file(&path).is_some() {
        return AiAuditStatus::Audited;
    }

    // No marker, no checksums, no state file — `docs/AGENTS.md` exists
    // but Kronn never touched it. Treat as "template-ish": the docs dir
    // is there but no Kronn audit has been recorded on this branch.
    if completed_full_run {
        return AiAuditStatus::Audited;
    }
    AiAuditStatus::TemplateInstalled
}

/// One `KRONN-(ASSUMED|MOCKED|TODO)(<id>): <why>` marker found in
/// source code by `scan_kronn_markers`. 0.8.3 — populates the
/// `code_locations` field on `agent_decisions` rows so the
/// Decision-log page can deep-link to the actual line.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct KronnMarker {
    /// File path, relative to the scan root.
    pub path: String,
    pub line: usize,
    /// `ASSUMED` | `MOCKED` | `TODO`.
    pub kind: String,
    /// The kebab-case identifier inside the parentheses.
    pub decision_id: String,
    /// Everything after the `: ` colon, trimmed. May be empty if the
    /// agent forgot the rationale.
    pub note: String,
}

/// Scan a directory tree for `KRONN-(ASSUMED|MOCKED|TODO)(<id>): <why>`
/// markers. Designed for the Feasibility-Gated Implementation pattern:
/// after the `implement` step finishes, this surfaces every freedom
/// the agent took (or every block it left), tied back to the manifest
/// via `decision_id`.
///
/// The walk skips common heavy dirs (`node_modules`, `vendor`,
/// `target`, `.git`, `dist`, `build`) and binary files (via extension
/// allowlist — only ASCII-text source extensions are scanned). Max
/// depth is bounded to keep the scan under ~1s on a 100kLOC repo.
pub fn scan_kronn_markers(root: &std::path::Path) -> Vec<KronnMarker> {
    static SKIP_DIRS: &[&str] = &[
        "node_modules",
        "vendor",
        "target",
        ".git",
        "dist",
        "build",
        ".next",
        ".kronn",
        ".kronn-worktrees",
        ".venv",
        "__pycache__",
    ];
    // Allowlist of text-source extensions. Anything else (.png, .lock,
    // .so, .map…) is skipped without a read, keeping the scan fast.
    static TEXT_EXTS: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "php", "py", "go", "java", "kt", "rb", "cs",
        "c", "h", "cpp", "hpp", "cc", "html", "twig", "vue", "svelte", "yml", "yaml", "toml",
        "json", "ini", "env", "sh", "bash", "zsh", "fish", "scss", "sass", "css", "less", "md",
        "mdx", "txt",
    ];
    // The marker grammar:
    //   KRONN-<KIND>(<id>):<space><note...>
    // where:
    //   <KIND> ∈ {ASSUMED, MOCKED, TODO}
    //   <id>   = kebab-case identifier ([A-Za-z0-9_-]+)
    //   <note> = anything until end of line
    static MARKER_RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"KRONN-(ASSUMED|MOCKED|TODO)\(([A-Za-z0-9_.-]+)\):\s*(.*)")
            .expect("static regex must compile")
    });
    let re = &*MARKER_RE;

    let mut out = Vec::new();
    let walker = WalkDir::new(root)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| {
            // Skip heavy dirs anywhere in the tree.
            e.file_name()
                .to_str()
                .map(|name| !SKIP_DIRS.contains(&name))
                .unwrap_or(true)
        });
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| TEXT_EXTS.contains(&ext))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        // Cheap pre-filter so we don't run regex on every line of
        // every source file — the marker is rare.
        if !content.contains("KRONN-") {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        for (idx, line) in content.lines().enumerate() {
            if let Some(captures) = re.captures(line) {
                out.push(KronnMarker {
                    path: rel.to_string_lossy().to_string(),
                    line: idx + 1,
                    kind: captures[1].to_string(),
                    decision_id: captures[2].to_string(),
                    note: captures[3].trim().to_string(),
                });
            }
        }
    }
    out
}

/// Count remaining `<!-- TODO -->` markers under the project's docs
/// folder. Path-agnostic — picks `docs/` (post-pivot) or `ai/`
/// (legacy) via `detect_docs_dir`, so projects on either layout get
/// the same count without the caller having to know.
pub fn count_ai_todos(project_path: &str) -> u32 {
    let path = resolve_host_path(project_path);
    let docs_dir = detect_docs_dir(&path);
    if !docs_dir.is_dir() {
        return 0;
    }

    let mut count = 0u32;
    for entry in WalkDir::new(&docs_dir)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "md") {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                count += content.matches("<!-- TODO").count() as u32;
            }
        }
    }
    count
}

/// Count the active tech-debt entries of a project, deduplicated by id:
///   - sheets directly under `docs/tech-debt/` that are active debt
///     (`TdSheet::is_active`: rejected and accepted sheets are settled);
///     `_`-prefixed reports and scaffolding are never sheets;
///   - rows of `docs/inconsistencies-tech-debt.md` starting with `| TD-`
///     whose id has no sheet (a sheet, when present, decides).
///
/// The health score's severity tally applies the same sheet rule.
pub fn count_tech_debt(project_path: &str) -> u32 {
    let path = resolve_host_path(project_path);
    let docs_dir = detect_docs_dir(&path);
    if !docs_dir.is_dir() {
        return 0;
    }

    let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut with_sheet: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Ok(entries) = std::fs::read_dir(docs_dir.join("tech-debt")) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id) = crate::core::td_index::counted_sheet_stem(name) else {
                continue;
            };
            with_sheet.insert(id.to_string());
            // An unreadable sheet still counts: the file is there.
            let is_active = std::fs::read_to_string(&p).map_or(true, |content| {
                crate::core::td_index::content_is_active(id, &content)
            });
            if is_active {
                active.insert(id.to_string());
            }
        }
    }

    if let Ok(content) = std::fs::read_to_string(docs_dir.join("inconsistencies-tech-debt.md")) {
        for line in content.lines() {
            let Some(rest) = line.trim_start().strip_prefix('|') else {
                continue;
            };
            let cell = rest.trim_start();
            if !cell.starts_with("TD-") {
                continue;
            }
            let id: String = cell
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '|')
                .collect();
            if !with_sheet.contains(&id) {
                active.insert(id);
            }
        }
    }

    active.len() as u32
}

/// Expand ~ in paths
fn shellexpand(path: &str) -> String {
    // Handle both Unix (~/) and Windows (~\) tilde expansion
    if path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs_home() {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

fn dirs_home() -> Option<String> {
    crate::core::child_env::var("HOME")
        .or_else(|_| crate::core::child_env::var("USERPROFILE"))
        .ok()
}

/// Check if a path looks like a WSL UNC path (for timeout and routing decisions).
/// Works on all platforms (pure string check).
pub fn is_wsl_unc_path_str(path: &str) -> bool {
    path.starts_with(r"\\wsl.localhost\") || path.starts_with(r"\\wsl$\")
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn an_ai_folder_audited_in_place_is_entered_by_its_agents_md() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("ai")).unwrap();
        assert_eq!(detect_docs_entry(root), root.join("ai/index.md"));
        std::fs::write(root.join("ai/AGENTS.md"), "# Project\n").unwrap();
        assert_eq!(detect_docs_entry(root), root.join("ai/AGENTS.md"));
    }
    use serial_test::serial;

    // ─── shellexpand ────────────────────────────────────────────────────────

    #[test]
    #[serial]
    fn shellexpand_tilde() {
        let prev = crate::core::child_env::var("HOME").ok();
        crate::core::child_env::set_var("HOME", "/home/testuser");
        assert_eq!(shellexpand("~/repos"), "/home/testuser/repos");
        if let Some(p) = prev {
            crate::core::child_env::set_var("HOME", p);
        }
    }

    #[test]
    fn shellexpand_no_tilde() {
        assert_eq!(shellexpand("/absolute/path"), "/absolute/path");
    }

    #[test]
    #[serial]
    fn shellexpand_windows_backslash() {
        let prev = crate::core::child_env::var("HOME").ok();
        crate::core::child_env::set_var("HOME", r"C:\Users\testuser");
        assert_eq!(shellexpand(r"~\repos"), r"C:\Users\testuser\repos");
        if let Some(p) = prev {
            crate::core::child_env::set_var("HOME", p);
        }
    }

    // ─── WSL UNC path detection ─────────────────────────────────────────────

    #[test]
    fn wsl_unc_detection_wsl_localhost() {
        assert!(is_wsl_unc_path_str(
            r"\\wsl.localhost\Ubuntu\home\user\repos"
        ));
    }

    #[test]
    fn wsl_unc_detection_wsl_dollar() {
        assert!(is_wsl_unc_path_str(r"\\wsl$\Ubuntu\home\user\repos"));
    }

    #[test]
    fn wsl_unc_detection_local_path() {
        assert!(!is_wsl_unc_path_str(r"C:\Users\user\repos"));
        assert!(!is_wsl_unc_path_str("/home/user/repos"));
    }

    // ─── WSL UNC to Linux path conversion (Windows only) ────────────────────

    // These functions are only compiled on Windows, but we can test the logic
    // by extracting the conversion algorithm into a platform-independent test.

    #[test]
    fn unc_to_linux_path_conversion_logic() {
        // Simulate what unc_to_wsl_linux_path does (platform-independent logic test)
        let unc = r"\\wsl.localhost\Ubuntu\home\user\repos";
        let remainder = unc.strip_prefix(r"\\wsl.localhost\").unwrap();
        assert_eq!(remainder, r"Ubuntu\home\user\repos");

        let linux_part = &remainder[remainder.find('\\').unwrap()..];
        let linux_path = linux_part.replace('\\', "/");
        assert_eq!(linux_path, "/home/user/repos");
    }

    #[test]
    fn unc_wsl_dollar_to_linux_path_conversion_logic() {
        let unc = r"\\wsl$\Ubuntu\home\user\repos";
        let remainder = unc.strip_prefix(r"\\wsl$\").unwrap();
        let linux_part = &remainder[remainder.find('\\').unwrap()..];
        let linux_path = linux_part.replace('\\', "/");
        assert_eq!(linux_path, "/home/user/repos");
    }

    // ─── restore_host_path ──────────────────────────────────────────────────

    #[test]
    #[serial]
    fn restore_host_path_no_host_home() {
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
        let path = Path::new("/some/local/path");
        assert_eq!(restore_host_path(path), "/some/local/path");
    }

    // ─── resolve_host_path ──────────────────────────────────────────────────

    #[test]
    #[serial]
    fn resolve_host_path_passthrough_without_env() {
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
        let result = resolve_host_path("/home/user/repos");
        assert_eq!(result, PathBuf::from("/home/user/repos"));
    }

    // ─── path traversal rejection ───────────────────────────────────────────

    #[test]
    fn contains_parent_dir_detects_dotdot() {
        assert!(contains_parent_dir("/home/user/../etc/passwd"));
        assert!(contains_parent_dir("../../etc/passwd"));
        assert!(contains_parent_dir("/a/b/../c"));
    }

    #[test]
    fn contains_parent_dir_allows_clean_paths() {
        assert!(!contains_parent_dir("/home/user/repos"));
        assert!(!contains_parent_dir("/home/user/repos/.kronn"));
        assert!(!contains_parent_dir("relative/path"));
        // A literal '..' inside a filename component is fine — only path
        // separators around it count.
        assert!(!contains_parent_dir("/home/user/file..bak"));
    }

    #[test]
    #[serial]
    fn resolve_host_path_refuses_traversal_with_host_home() {
        // Even if KRONN_HOST_HOME is set, a `..` in the path must not be
        // mapped into /host-home — that would let a caller pivot outside
        // the mount root. We return the original PathBuf so downstream
        // operations fail rather than silently succeeding on the wrong target.
        // #[serial]: this env mutation raced every non-neighbouring #[serial]
        // test reading KRONN_HOST_HOME (the codex_global_sync flaky).
        crate::core::child_env::set_var("KRONN_HOST_HOME", "/home/user");
        let result = resolve_host_path("/home/user/../etc/passwd");
        assert_eq!(result, PathBuf::from("/home/user/../etc/passwd"));
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
    }

    // ─── host_home_aliases (macOS APFS firmlinks) ──────────────────────────

    #[test]
    #[serial]
    fn host_home_aliases_empty_when_env_unset() {
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
        assert!(host_home_aliases().is_empty());
    }

    #[test]
    #[serial]
    fn host_home_aliases_empty_when_env_blank() {
        crate::core::child_env::set_var("KRONN_HOST_HOME", "");
        assert!(host_home_aliases().is_empty());
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
    }

    #[test]
    #[serial]
    fn host_home_aliases_linux_wsl_single_entry() {
        // Non-macOS homedirs don't match the `/Users/` special-case, so we
        // return just the raw env value. Keeps the WSL/Linux hot path
        // identical to the pre-fix behavior.
        crate::core::child_env::set_var("KRONN_HOST_HOME", "/home/john");
        assert_eq!(host_home_aliases(), vec!["/home/john".to_string()]);
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
    }

    #[test]
    #[serial]
    fn host_home_aliases_macos_includes_firmlink_variants() {
        // `/Users/xxx` paths get the two APFS variants added so a
        // canonicalized path (e.g. `/System/Volumes/Data/Users/xxx/Code`)
        // still maps cleanly to `/host-home/Code` at scan time.
        crate::core::child_env::set_var("KRONN_HOST_HOME", "/Users/john");
        let aliases = host_home_aliases();
        assert_eq!(aliases.len(), 3);
        assert_eq!(aliases[0], "/Users/john");
        assert!(aliases.contains(&"/System/Volumes/Data/Users/john".to_string()));
        assert!(aliases.contains(&"/private/var/Users/john".to_string()));
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
    }

    // ─── resolve_host_path with APFS firmlink canonicalized paths ──────────

    #[test]
    #[serial]
    fn resolve_host_path_unmapped_when_no_alias_matches() {
        // A path entirely outside the configured HOST_HOME is returned
        // unchanged — `exists()` will then fail downstream and the caller
        // (scanner) will filter the entry.
        crate::core::child_env::set_var("KRONN_HOST_HOME", "/Users/john");
        let result = resolve_host_path("/etc/hosts");
        assert_eq!(result, PathBuf::from("/etc/hosts"));
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
    }

    #[test]
    #[serial]
    fn resolve_host_path_no_mapping_when_env_missing() {
        // If KRONN_HOST_HOME isn't set we can't map anything — path
        // returned as-is. Prevents crashes on native (non-Docker) runs.
        crate::core::child_env::remove_var("KRONN_HOST_HOME");
        let result = resolve_host_path("/Users/john/Code/proj");
        assert_eq!(result, PathBuf::from("/Users/john/Code/proj"));
    }

    // ─── detect_docs_dir / detect_docs_entry (0.7.1 convention pivot) ──────

    #[test]
    fn detect_docs_dir_prefers_modern_plural_when_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/AGENTS.md"), "# x").unwrap();
        // Also add a legacy ai/ — the modern path wins.
        std::fs::create_dir_all(tmp.path().join("ai")).unwrap();
        std::fs::write(tmp.path().join("ai/index.md"), "# legacy").unwrap();

        let dir = detect_docs_dir(tmp.path());
        assert_eq!(dir, tmp.path().join("docs"));
        assert_eq!(
            detect_docs_entry(tmp.path()),
            tmp.path().join("docs/AGENTS.md")
        );
    }

    #[test]
    fn detect_docs_dir_falls_back_to_singular_doc_when_only_that_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("doc")).unwrap();
        std::fs::write(tmp.path().join("doc/AGENTS.md"), "# x").unwrap();

        let dir = detect_docs_dir(tmp.path());
        assert_eq!(dir, tmp.path().join("doc"));
        assert_eq!(
            detect_docs_entry(tmp.path()),
            tmp.path().join("doc/AGENTS.md")
        );
    }

    #[test]
    fn detect_docs_dir_falls_back_to_legacy_ai_when_only_that_present() {
        // Existing Kronn-managed project (pre-0.7.1) — must still resolve
        // to ai/ so runtime doesn't break before the migration runs.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("ai")).unwrap();
        std::fs::write(tmp.path().join("ai/index.md"), "# legacy").unwrap();

        let dir = detect_docs_dir(tmp.path());
        assert_eq!(dir, tmp.path().join("ai"));
        // Entry file is the legacy index.md, not AGENTS.md.
        assert_eq!(
            detect_docs_entry(tmp.path()),
            tmp.path().join("ai/index.md")
        );
    }

    #[test]
    fn detect_docs_dir_defaults_to_modern_docs_for_fresh_project() {
        // No docs/, no doc/, no ai/ — fresh bootstrap target is docs/
        // (the new convention).
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = detect_docs_dir(tmp.path());
        assert_eq!(dir, tmp.path().join("docs"));
        assert_eq!(
            detect_docs_entry(tmp.path()),
            tmp.path().join("docs/AGENTS.md")
        );
    }

    #[test]
    fn detect_docs_dir_ignores_directories_without_entry_file() {
        // A `docs/` that exists but contains NO `AGENTS.md` is not a Kronn
        // docs dir — could be an existing human docs/ folder. Falls
        // through to legacy `ai/` if that has the entry, else default.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/some-other-doc.md"), "# human").unwrap();
        std::fs::create_dir_all(tmp.path().join("ai")).unwrap();
        std::fs::write(tmp.path().join("ai/index.md"), "# legacy").unwrap();

        let dir = detect_docs_dir(tmp.path());
        // Falls back to ai/ because docs/AGENTS.md doesn't exist yet.
        // Migration (T9) will populate docs/AGENTS.md and the next call
        // will return docs/.
        assert_eq!(dir, tmp.path().join("ai"));
    }

    #[test]
    fn detect_docs_dir_falls_back_to_existing_dir_without_entry_file() {
        // Half-finished bootstrap: `ai/` exists but `ai/index.md` was
        // never written. Without the existence-fallback, detect_docs_dir
        // would return the default `docs/` (which doesn't exist) and
        // helpers like `ensure_redirectors` no-op silently.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("ai")).unwrap();
        // No ai/index.md, no docs/, no doc/.
        assert_eq!(detect_docs_dir(tmp.path()), tmp.path().join("ai"));
    }

    #[test]
    fn detect_docs_dir_prefers_docs_when_both_are_empty() {
        // Both folders exist with no entry files. Picks `docs/` first
        // (post-pivot is the modern convention).
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::create_dir_all(tmp.path().join("ai")).unwrap();
        assert_eq!(detect_docs_dir(tmp.path()), tmp.path().join("docs"));
    }

    // ─── KT-263: configured AI doc folder ───────────────────────────────────

    #[test]
    fn a_project_keeping_its_doc_in_dot_ai_is_detected_and_audited() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join(".ai")).unwrap();
        std::fs::create_dir_all(root.join("ai")).unwrap();
        std::fs::write(root.join("ai/index.md"), "# legacy\n").unwrap();
        std::fs::write(root.join(".ai/AGENTS.md"), "# Project\n\n<!-- TODO x -->\n").unwrap();
        let path = root.to_string_lossy().into_owned();

        // Unconfigured: the legacy layout wins and `.ai/` is invisible.
        assert_eq!(detect_docs_dir(&root), root.join("ai"));
        assert!(needs_docs_migration(&root));

        crate::core::docs_location::set_project_override(&path, Some(".ai"));
        let dir = detect_docs_dir(&root);
        let entry = detect_docs_entry(&root);
        let migration = needs_docs_migration(&root);
        let todos = count_ai_todos(&path);
        let before_audit = detect_audit_status(&path);
        let recorded = crate::core::kronn_state::record_audit(&root, "full");
        let after_audit = detect_audit_status(&path);
        crate::core::docs_location::set_project_override(&path, None);

        assert_eq!(dir, root.join(".ai"));
        assert_eq!(entry, root.join(".ai/AGENTS.md"));
        assert!(
            !migration,
            "a configured folder is never offered a migration"
        );
        assert_eq!(todos, 1);
        assert_eq!(
            before_audit,
            crate::models::AiAuditStatus::TemplateInstalled
        );
        recorded.unwrap();
        assert!(root.join(".ai/.kronn.json").is_file());
        assert!(!root.join("ai/.kronn.json").exists());
        assert_eq!(after_audit, crate::models::AiAuditStatus::Audited);
    }

    #[test]
    fn a_configured_folder_that_is_missing_reads_as_no_template() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# filled\n").unwrap();
        let path = root.to_string_lossy().into_owned();
        crate::core::docs_location::set_project_override(&path, Some("context"));
        let status = detect_audit_status(&path);
        crate::core::docs_location::set_project_override(&path, None);
        assert_eq!(status, crate::models::AiAuditStatus::NoTemplate);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_override_reads_nothing_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(outside.path().join("AGENTS.md"), "# outside\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join(".ai")).unwrap();
        let path = root.to_string_lossy().into_owned();
        crate::core::docs_location::set_project_override(&path, Some(".ai"));
        let status = detect_audit_status(&path);
        let write = crate::core::kronn_state::record_audit(&root, "full");
        crate::core::docs_location::set_project_override(&path, None);
        assert_eq!(status, crate::models::AiAuditStatus::NoTemplate);
        assert!(write.unwrap_err().contains("symlink"));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    // ─── scan_kronn_markers (0.8.3 Feasibility-Gated Implementation) ─────────

    #[test]
    fn scan_kronn_markers_finds_all_three_variants() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
        std::fs::write(
            tmp.path().join("src/Service/BrandContext.php"),
            "<?php\n\
             // KRONN-ASSUMED(brand-context-impl): EventListener over CompilerPass\n\
             final class BrandContext {}\n\
             // KRONN-MOCKED(adobe-dtm-an): env var KRONN_ADOBE_DTM_AN_URL_PROD\n\
             public function getDtm(): string { return ''; }\n\
             // KRONN-TODO(adobe-visitor-an): waiting on Data team\n",
        )
        .unwrap();

        let markers = scan_kronn_markers(tmp.path());
        assert_eq!(markers.len(), 3, "expected 3 markers, got {markers:?}");
        let kinds: Vec<&str> = markers.iter().map(|m| m.kind.as_str()).collect();
        assert!(kinds.contains(&"ASSUMED"));
        assert!(kinds.contains(&"MOCKED"));
        assert!(kinds.contains(&"TODO"));
        // decision_id preserved verbatim.
        assert!(markers
            .iter()
            .any(|m| m.decision_id == "brand-context-impl"));
        assert!(markers.iter().any(|m| m.decision_id == "adobe-dtm-an"));
        assert!(markers.iter().any(|m| m.decision_id == "adobe-visitor-an"));
    }

    #[test]
    fn scan_kronn_markers_skips_heavy_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Marker INSIDE node_modules — must be ignored.
        std::fs::create_dir_all(tmp.path().join("node_modules/foo")).unwrap();
        std::fs::write(
            tmp.path().join("node_modules/foo/index.js"),
            "// KRONN-ASSUMED(x): should not be found\n",
        )
        .unwrap();
        // Marker in src/ — must be found.
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("src/app.js"),
            "// KRONN-MOCKED(y): real marker\n",
        )
        .unwrap();

        let markers = scan_kronn_markers(tmp.path());
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].decision_id, "y");
    }

    #[test]
    fn scan_kronn_markers_skips_binary_extensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A `.png` file with the marker string in it (pretend binary)
        // — must not be read.
        std::fs::write(
            tmp.path().join("logo.png"),
            "KRONN-TODO(should-not-match): binary file\n",
        )
        .unwrap();
        let markers = scan_kronn_markers(tmp.path());
        assert!(markers.is_empty(), "binary file must be skipped");
    }

    #[test]
    fn scan_kronn_markers_captures_line_number_and_note() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("a.rs"),
            "// line 1\n\
             fn main() {}\n\
             // KRONN-ASSUMED(decision-id): the rationale text here\n",
        )
        .unwrap();
        let markers = scan_kronn_markers(tmp.path());
        assert_eq!(markers.len(), 1);
        let m = &markers[0];
        assert_eq!(m.line, 3);
        assert_eq!(m.decision_id, "decision-id");
        assert_eq!(m.note, "the rationale text here");
    }

    #[test]
    fn scan_kronn_markers_empty_when_no_markers() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn main() {}\n").unwrap();
        assert!(scan_kronn_markers(tmp.path()).is_empty());
    }
}
