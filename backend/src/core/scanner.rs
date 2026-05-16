use std::collections::HashSet;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::models::{AiConfigType, DetectedRepo};
use super::cmd::async_cmd;

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
pub async fn scan_paths(
    paths: &[String],
    ignore: &[String],
) -> Result<Vec<DetectedRepo>> {
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
        host_os = %std::env::var("KRONN_HOST_OS").unwrap_or_else(|_| "<unset>".into()),
        host_home = %std::env::var("KRONN_HOST_HOME").unwrap_or_else(|_| "<unset>".into()),
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

    // Deduplicate repos (handles macOS symlinks like /Users -> /private/var/Users).
    // Strategy: use a composite key of (repo name, git remote URL) to detect duplicates.
    // This works even inside Docker where host paths can't be canonicalized.
    // Fallback: if no remote URL, use the repo name + canonicalized container path.
    {
        let mut seen = HashSet::new();
        repos.retain(|r| {
            let key = if let Some(ref url) = r.remote_url {
                // Same name + same remote URL = same repo found via different paths
                // (different names with same URL = intentional separate clones, keep both)
                format!("{}:{}", r.name, url)
            } else {
                // No remote: try canonical path of the container-mapped path
                let container_path = resolve_host_path(&r.path);
                let canon = std::fs::canonicalize(&container_path)
                    .unwrap_or(container_path);
                format!("path:{}", canon.display())
            };
            if seen.contains(&key) {
                tracing::debug!("Filtering duplicate repo: {} (key: {})", r.path, key);
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

    let walker = WalkDir::new(base)
        .max_depth(max_depth)
        .follow_links(false)
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
    let hidden = path.components().any(|c| {
        c.as_os_str().to_string_lossy().starts_with('.')
    });

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
    let remainder = s.strip_prefix(r"\\wsl.localhost\")
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
                return async_cmd("wsl.exe")
                    .args(["-e", "bash", "-lc", &git_cmd])
                    .output().await
                    .context("Failed to run git via wsl.exe");
            }
        }
        async_cmd("git")
            .args(args).current_dir(path)
            .output().await
            .context("Failed to run git")
    }
    #[cfg(not(target_os = "windows"))]
    {
        async_cmd("git")
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
fn restore_host_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(relative) = s.strip_prefix("/host-home") {
        if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
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

/// Every plausible host-home prefix the caller's path could start with.
/// Order matters: we try the raw KRONN_HOST_HOME first, then the macOS
/// firmlink alternatives. On Linux / WSL only the first entry is used.
///
/// Exposed as `pub(crate)` so the scanner tests can sanity-check the
/// alias expansion logic in isolation.
pub(crate) fn host_home_aliases() -> Vec<String> {
    let Ok(host_home) = std::env::var("KRONN_HOST_HOME") else {
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
        if canonical != host_home { aliases.push(canonical); }
        let legacy = format!("/private/var/Users/{}", rest);
        if legacy != host_home { aliases.push(legacy); }
    }
    aliases
}

/// Detect or default the project's documentation directory.
///
/// Preference order : `docs/` (post-0.7.1 convention, modern plural) →
/// `doc/` (legacy singular, some Ruby/Rails projects) → `ai/` (Kronn 0.7.0
/// and earlier). Discriminated by the entry-point file:
///   - `docs/AGENTS.md` (post-pivot)
///   - `doc/AGENTS.md` (post-pivot, singular projects)
///   - `ai/index.md` (legacy)
///
/// When NONE exists, returns `docs/` so fresh bootstraps install the new
/// convention. Caller is responsible for creating the directory if it
/// doesn't exist (some flows only need to know "where to write" not "is
/// it real yet").
///
/// Path-agnostic detect-and-adapt strategy : every site that hardcoded
/// `ai/` should call this instead. Existing projects keep working,
/// migration becomes a `git mv` away, and new projects ship the new
/// convention without ceremony.
pub fn detect_docs_dir(project_path: &Path) -> PathBuf {
    // Prefer the dir that has the canonical entry file — most reliable
    // signal that an audit was actually completed there.
    if project_path.join("docs/AGENTS.md").is_file() {
        return project_path.join("docs");
    }
    if project_path.join("doc/AGENTS.md").is_file() {
        return project_path.join("doc");
    }
    if project_path.join("ai/index.md").is_file() {
        return project_path.join("ai");
    }
    // No entry file — fall back to whichever folder physically exists
    // (e.g. half-finished bootstrap, audit started but never sealed).
    // Without this fallback, `ensure_redirectors`-style helpers that
    // check `dir.is_dir()` no-op on legacy projects whose template was
    // copied but where `ai/index.md` was never written.
    if project_path.join("docs").is_dir() {
        return project_path.join("docs");
    }
    if project_path.join("doc").is_dir() {
        return project_path.join("doc");
    }
    if project_path.join("ai").is_dir() {
        return project_path.join("ai");
    }
    // Default for fresh projects.
    project_path.join("docs")
}

/// File path of the docs directory's entry point. `AGENTS.md` post-pivot,
/// legacy `index.md` for projects that haven't migrated yet.
pub fn detect_docs_entry(project_path: &Path) -> PathBuf {
    let dir = detect_docs_dir(project_path);
    if dir.file_name().and_then(|n| n.to_str()) == Some("ai") {
        dir.join("index.md")
    } else {
        dir.join("AGENTS.md")
    }
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
    if project_path.join("docs/AGENTS.md").is_file() {
        return false;
    }
    if project_path.join("doc/AGENTS.md").is_file() {
        return false;
    }
    project_path.join("ai/index.md").is_file()
}

/// Detect the AI audit status for a project based on filesystem state.
pub fn detect_audit_status(project_path: &str) -> crate::models::AiAuditStatus {
    use crate::models::AiAuditStatus;

    let path = resolve_host_path(project_path);

    // Audit status detection accepts both the new (`docs/AGENTS.md`) and
    // legacy (`ai/index.md`) conventions during the migration window.
    // A directory in EITHER shape with NO entry file = TemplateInstalled
    // (partial state, audit was started but never finished).
    let any_docs_dir_exists = path.join("docs").is_dir()
        || path.join("doc").is_dir()
        || path.join("ai").is_dir();
    if !any_docs_dir_exists {
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
            tracing::warn!("Cannot read {} at {}: {} — treating as TemplateInstalled",
                index_file.file_name().and_then(|n| n.to_str()).unwrap_or("entry"),
                index_file.display(), e);
            return AiAuditStatus::TemplateInstalled;
        }
    };

    if content.contains("KRONN:BOOTSTRAP:START") || content.contains("KRONN:BOOTSTRAP:END") {
        return AiAuditStatus::TemplateInstalled;
    }
    // Check for unfilled placeholders like {{PROJECT_NAME}}, but ignore instructional
    // text that mentions {{...}} as an example (e.g., "If you see an unfilled {{...}}")
    if regex_lite::Regex::new(r"\{\{[A-Z_]+\}\}").ok()
        .map(|re| re.is_match(&content)).unwrap_or(false) {
        return AiAuditStatus::TemplateInstalled;
    }

    // 0.8.4 — canonical source of truth: `docs/.kronn.json`. Survives `git
    // clone`, lives outside the agent-read path (no token cost), and can't
    // be inferred by accident from the user's own `docs/AGENTS.md`.
    //
    // We still honour the legacy `KRONN:VALIDATED` / `KRONN:BOOTSTRAPPED`
    // HTML markers and `docs/checksums.json` so projects audited before
    // this release keep their badge — but we no longer fall through to
    // `Audited` based on filesystem heuristics alone (the old bug:
    // any project with a pre-existing `docs/AGENTS.md` was tagged green).
    if let Some(state) = crate::core::kronn_state::read(&path) {
        if state.validated_at.is_some() {
            return AiAuditStatus::Validated;
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
    // is there but no Kronn audit has been recorded.
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
        "node_modules", "vendor", "target", ".git", "dist", "build",
        ".next", ".kronn", ".kronn-worktrees", ".venv", "__pycache__",
    ];
    // Allowlist of text-source extensions. Anything else (.png, .lock,
    // .so, .map…) is skipped without a read, keeping the scan fast.
    static TEXT_EXTS: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs",
        "php", "py", "go", "java", "kt", "rb", "cs",
        "c", "h", "cpp", "hpp", "cc",
        "html", "twig", "vue", "svelte",
        "yml", "yaml", "toml", "json", "ini", "env",
        "sh", "bash", "zsh", "fish",
        "scss", "sass", "css", "less",
        "md", "mdx", "txt",
    ];
    // The marker grammar:
    //   KRONN-<KIND>(<id>):<space><note...>
    // where:
    //   <KIND> ∈ {ASSUMED, MOCKED, TODO}
    //   <id>   = kebab-case identifier ([A-Za-z0-9_-]+)
    //   <note> = anything until end of line
    static MARKER_RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"KRONN-(ASSUMED|MOCKED|TODO)\(([A-Za-z0-9_.-]+)\):\s*(.*)"
        ).expect("static regex must compile")
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
        let ext_ok = path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| TEXT_EXTS.contains(&ext))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else { continue; };
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
    for entry in WalkDir::new(&docs_dir).max_depth(3).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "md") {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                count += content.matches("<!-- TODO").count() as u32;
            }
        }
    }
    count
}

/// Count tech-debt entries in a project's documentation, deduplicated
/// by ID. Sources:
///   - `.md` file names directly under `docs/tech-debt/` (id = stem,
///     e.g. `TD-20260314-openapi-coverage`).
///   - Table rows in `docs/inconsistencies-tech-debt.md` that start
///     with `| TD-` (id = the first `TD-...` token in the row).
///
/// An entry counted from both sources counts once. This matches user
/// expectation: the badge "<N> TD" should equal the unique tech-debt
/// items the user can actually open, not 2×N when both the index row
/// and the detail file exist (the common case for well-documented
/// projects). Path-agnostic like `count_ai_todos`.
pub fn count_tech_debt(project_path: &str) -> u32 {
    let path = resolve_host_path(project_path);
    let docs_dir = detect_docs_dir(&path);
    if !docs_dir.is_dir() {
        return 0;
    }

    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Detail files under tech-debt/
    let td_dir = docs_dir.join("tech-debt");
    if td_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&td_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                // Only count top-level .md files; a README/TEMPLATE.md
                // in the same folder is excluded so it doesn't inflate
                // the badge.
                if p.is_file()
                    && p.extension().is_some_and(|ext| ext == "md")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !matches!(n, "README.md" | "TEMPLATE.md" | "_template.md"))
                {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        ids.insert(stem.to_string());
                    }
                }
            }
        }
    }

    // Index file table rows
    let index_path = docs_dir.join("inconsistencies-tech-debt.md");
    if let Ok(content) = std::fs::read_to_string(&index_path) {
        for line in content.lines() {
            let trimmed = line.trim_start();
            // Markdown table rows starting with `| TD-`. We extract the
            // ID (everything up to the next space, pipe, or end) so a
            // row that mirrors an existing detail file is deduped.
            if let Some(rest) = trimmed.strip_prefix('|') {
                let cell = rest.trim_start();
                if cell.starts_with("TD-") {
                    let id: String = cell
                        .chars()
                        .take_while(|c| !c.is_whitespace() && *c != '|')
                        .collect();
                    if !id.is_empty() {
                        ids.insert(id);
                    }
                }
            }
        }
    }

    ids.len() as u32
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
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
}

/// Check if a path looks like a WSL UNC path (for timeout and routing decisions).
/// Works on all platforms (pure string check).
pub fn is_wsl_unc_path_str(path: &str) -> bool {
    path.starts_with(r"\\wsl.localhost\") || path.starts_with(r"\\wsl$\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ─── shellexpand ────────────────────────────────────────────────────────

    #[test]
    #[serial]
    fn shellexpand_tilde() {
        let prev = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/home/testuser");
        assert_eq!(shellexpand("~/repos"), "/home/testuser/repos");
        if let Some(p) = prev { std::env::set_var("HOME", p); }
    }

    #[test]
    fn shellexpand_no_tilde() {
        assert_eq!(shellexpand("/absolute/path"), "/absolute/path");
    }

    #[test]
    #[serial]
    fn shellexpand_windows_backslash() {
        let prev = std::env::var("HOME").ok();
        std::env::set_var("HOME", r"C:\Users\testuser");
        assert_eq!(shellexpand(r"~\repos"), r"C:\Users\testuser\repos");
        if let Some(p) = prev { std::env::set_var("HOME", p); }
    }

    // ─── WSL UNC path detection ─────────────────────────────────────────────

    #[test]
    fn wsl_unc_detection_wsl_localhost() {
        assert!(is_wsl_unc_path_str(r"\\wsl.localhost\Ubuntu\home\user\repos"));
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
    fn restore_host_path_no_host_home() {
        std::env::remove_var("KRONN_HOST_HOME");
        let path = Path::new("/some/local/path");
        assert_eq!(restore_host_path(path), "/some/local/path");
    }

    // ─── resolve_host_path ──────────────────────────────────────────────────

    #[test]
    fn resolve_host_path_passthrough_without_env() {
        std::env::remove_var("KRONN_HOST_HOME");
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
    fn resolve_host_path_refuses_traversal_with_host_home() {
        // Even if KRONN_HOST_HOME is set, a `..` in the path must not be
        // mapped into /host-home — that would let a caller pivot outside
        // the mount root. We return the original PathBuf so downstream
        // operations fail rather than silently succeeding on the wrong target.
        std::env::set_var("KRONN_HOST_HOME", "/home/user");
        let result = resolve_host_path("/home/user/../etc/passwd");
        assert_eq!(result, PathBuf::from("/home/user/../etc/passwd"));
        std::env::remove_var("KRONN_HOST_HOME");
    }

    // ─── host_home_aliases (macOS APFS firmlinks) ──────────────────────────

    #[test]
    #[serial]
    fn host_home_aliases_empty_when_env_unset() {
        std::env::remove_var("KRONN_HOST_HOME");
        assert!(host_home_aliases().is_empty());
    }

    #[test]
    #[serial]
    fn host_home_aliases_empty_when_env_blank() {
        std::env::set_var("KRONN_HOST_HOME", "");
        assert!(host_home_aliases().is_empty());
        std::env::remove_var("KRONN_HOST_HOME");
    }

    #[test]
    #[serial]
    fn host_home_aliases_linux_wsl_single_entry() {
        // Non-macOS homedirs don't match the `/Users/` special-case, so we
        // return just the raw env value. Keeps the WSL/Linux hot path
        // identical to the pre-fix behavior.
        std::env::set_var("KRONN_HOST_HOME", "/home/john");
        assert_eq!(host_home_aliases(), vec!["/home/john".to_string()]);
        std::env::remove_var("KRONN_HOST_HOME");
    }

    #[test]
    #[serial]
    fn host_home_aliases_macos_includes_firmlink_variants() {
        // `/Users/xxx` paths get the two APFS variants added so a
        // canonicalized path (e.g. `/System/Volumes/Data/Users/xxx/Code`)
        // still maps cleanly to `/host-home/Code` at scan time.
        std::env::set_var("KRONN_HOST_HOME", "/Users/john");
        let aliases = host_home_aliases();
        assert_eq!(aliases.len(), 3);
        assert_eq!(aliases[0], "/Users/john");
        assert!(aliases.contains(&"/System/Volumes/Data/Users/john".to_string()));
        assert!(aliases.contains(&"/private/var/Users/john".to_string()));
        std::env::remove_var("KRONN_HOST_HOME");
    }

    // ─── resolve_host_path with APFS firmlink canonicalized paths ──────────

    #[test]
    #[serial]
    fn resolve_host_path_unmapped_when_no_alias_matches() {
        // A path entirely outside the configured HOST_HOME is returned
        // unchanged — `exists()` will then fail downstream and the caller
        // (scanner) will filter the entry.
        std::env::set_var("KRONN_HOST_HOME", "/Users/john");
        let result = resolve_host_path("/etc/hosts");
        assert_eq!(result, PathBuf::from("/etc/hosts"));
        std::env::remove_var("KRONN_HOST_HOME");
    }

    #[test]
    #[serial]
    fn resolve_host_path_no_mapping_when_env_missing() {
        // If KRONN_HOST_HOME isn't set we can't map anything — path
        // returned as-is. Prevents crashes on native (non-Docker) runs.
        std::env::remove_var("KRONN_HOST_HOME");
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
        assert_eq!(detect_docs_entry(tmp.path()), tmp.path().join("docs/AGENTS.md"));
    }

    #[test]
    fn detect_docs_dir_falls_back_to_singular_doc_when_only_that_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("doc")).unwrap();
        std::fs::write(tmp.path().join("doc/AGENTS.md"), "# x").unwrap();

        let dir = detect_docs_dir(tmp.path());
        assert_eq!(dir, tmp.path().join("doc"));
        assert_eq!(detect_docs_entry(tmp.path()), tmp.path().join("doc/AGENTS.md"));
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
        assert_eq!(detect_docs_entry(tmp.path()), tmp.path().join("ai/index.md"));
    }

    #[test]
    fn detect_docs_dir_defaults_to_modern_docs_for_fresh_project() {
        // No docs/, no doc/, no ai/ — fresh bootstrap target is docs/
        // (the new convention).
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = detect_docs_dir(tmp.path());
        assert_eq!(dir, tmp.path().join("docs"));
        assert_eq!(detect_docs_entry(tmp.path()), tmp.path().join("docs/AGENTS.md"));
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
        ).unwrap();

        let markers = scan_kronn_markers(tmp.path());
        assert_eq!(markers.len(), 3, "expected 3 markers, got {markers:?}");
        let kinds: Vec<&str> = markers.iter().map(|m| m.kind.as_str()).collect();
        assert!(kinds.contains(&"ASSUMED"));
        assert!(kinds.contains(&"MOCKED"));
        assert!(kinds.contains(&"TODO"));
        // decision_id preserved verbatim.
        assert!(markers.iter().any(|m| m.decision_id == "brand-context-impl"));
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
        ).unwrap();
        // Marker in src/ — must be found.
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("src/app.js"),
            "// KRONN-MOCKED(y): real marker\n",
        ).unwrap();

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
        ).unwrap();
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
        ).unwrap();
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
