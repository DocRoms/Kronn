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

    for base_path in paths {
        let expanded = shellexpand(base_path);
        let base = resolve_host_path(&expanded);
        tracing::info!("Scanning path: {} (resolved: {})", base_path, base.display());
        if !base.exists() {
            tracing::warn!("Scan path does not exist: {} (resolved: {})", base_path, base.display());
            continue;
        }

        let found = scan_directory(&base, ignore, depth).await?;
        tracing::info!("Found {} repos in {}", found.len(), base.display());
        repos.extend(found);
    }

    // Filter out repos whose host path doesn't exist (ghost paths from symlink resolution)
    repos.retain(|r| {
        let exists = Path::new(&r.path).exists()
            || resolve_host_path(&r.path).exists();
        if !exists {
            tracing::debug!("Filtering non-existent repo path: {}", r.path);
        }
        exists
    });

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

    tracing::info!("Scan complete: {} repositories found", repos.len());
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

/// Map a host-absolute path to the Docker mount point if needed.
/// e.g. /home/priol/Repositories -> /host-home/Repositories
pub fn resolve_host_path(path: &str) -> PathBuf {
    // In Docker: always prefer the /host-home mount over any local path
    if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
        if let Some(relative) = path.strip_prefix(&host_home) {
            let mapped = PathBuf::from(format!("/host-home{}", relative));
            if mapped.exists() {
                tracing::debug!("Mapped host path {} -> {}", path, mapped.display());
                return mapped;
            }
        }
    }
    PathBuf::from(path)
}

/// Detect the AI audit status for a project based on filesystem state.
pub fn detect_audit_status(project_path: &str) -> crate::models::AiAuditStatus {
    use crate::models::AiAuditStatus;

    let path = resolve_host_path(project_path);
    let ai_dir = path.join("ai");
    let index_file = path.join("ai/index.md");

    if !ai_dir.exists() {
        return AiAuditStatus::NoTemplate;
    }

    // ai/ directory exists but index.md doesn't — treat as template installed (partial state)
    if !index_file.exists() {
        return AiAuditStatus::TemplateInstalled;
    }

    let content = match std::fs::read_to_string(&index_file) {
        Ok(c) => c,
        Err(e) => {
            // File exists but can't be read (permission issue) — don't confuse with "no template"
            tracing::warn!("Cannot read ai/index.md at {}: {} — treating as TemplateInstalled", index_file.display(), e);
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

    if content.contains("KRONN:BOOTSTRAPPED") {
        // Validated takes priority over Bootstrapped
        if content.contains("KRONN:VALIDATED") {
            return AiAuditStatus::Validated;
        }
        return AiAuditStatus::Bootstrapped;
    }

    if content.contains("KRONN:VALIDATED") {
        return AiAuditStatus::Validated;
    }

    AiAuditStatus::Audited
}

/// Count remaining <!-- TODO --> markers in ai/ files.
pub fn count_ai_todos(project_path: &str) -> u32 {
    let path = resolve_host_path(project_path);
    let ai_dir = path.join("ai");
    if !ai_dir.is_dir() {
        return 0;
    }

    let mut count = 0u32;
    for entry in WalkDir::new(&ai_dir).max_depth(3).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "md") {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                count += content.matches("<!-- TODO").count() as u32;
            }
        }
    }
    count
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
}
