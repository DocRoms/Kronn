//! Git worktree management for discussion isolation.
//!
//! Each isolated discussion gets its own git worktree so agents can make
//! changes without interfering with the main working tree or other discussions.

use std::path::{Path, PathBuf};
use super::cmd::sync_cmd;

/// Fix worktree cross-references so they work from the host, not just inside Docker.
///
/// Git worktrees use absolute paths in two places:
/// 1. `<worktree>/.git` file → points to `<repo>/.git/worktrees/<name>`
/// 2. `<repo>/.git/worktrees/<name>/gitdir` → points back to `<worktree>/.git`
///
/// When created inside Docker, these contain container paths (`/host-home/...`).
/// This function rewrites them to use the actual repo path so they work on the host too.
fn fix_worktree_paths(repo_path: &Path, worktree_path: &Path) {
    let wt_name = match worktree_path.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return,
    };

    // Use relative paths so worktrees work both inside Docker and on the host.
    // Worktree is always at <repo>/.kronn/worktrees/<name>, so relative paths are stable.

    // 1. Fix <worktree>/.git — point to ../../.git/worktrees/<name>
    //    Use forward slashes (POSIX) because git always uses forward slashes in gitdir files,
    //    even on Windows (git normalizes internally).
    let dot_git = worktree_path.join(".git");
    if dot_git.exists() {
        let content = format!("gitdir: ../../.git/worktrees/{}", wt_name);
        if let Err(e) = std::fs::write(&dot_git, &content) {
            tracing::warn!("Failed to fix worktree .git file: {}", e);
        }
    }

    // 2. Fix <repo>/.git/worktrees/<name>/gitdir — point back to worktree
    let gitdir_file = repo_path.join(".git").join("worktrees").join(&wt_name).join("gitdir");
    if gitdir_file.exists() {
        let content = format!(".kronn/worktrees/{}/.git\n", wt_name);
        if let Err(e) = std::fs::write(&gitdir_file, &content) {
            tracing::warn!("Failed to fix repo gitdir for worktree: {}", e);
        }
    }
}

/// Information about a created worktree.
#[derive(Debug)]
pub struct WorktreeInfo {
    /// Full path to the worktree directory
    pub path: String,
    /// Branch name (e.g., "kronn/fix-the-bug")
    pub branch: String,
    /// If true, workspace points to the main repo (branch already checked out there)
    pub is_main_repo: bool,
}

/// Check if a branch is checked out in any worktree (including the main repo).
fn branch_checked_out_at(repo_path: &Path, branch: &str) -> Option<PathBuf> {
    let output = sync_cmd("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current_path: Option<String> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.to_string());
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if b == branch {
                return current_path.map(PathBuf::from);
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
    None
}

/// Base directory for worktrees: `.kronn/worktrees/` inside the repo.
fn worktree_base_dir(repo_path: &Path) -> PathBuf {
    repo_path.join(".kronn/worktrees")
}

/// Maximum length of a single slug component (project / discussion).
///
/// Windows MAX_PATH is 260 characters by default. A worktree path looks like:
///   `<repo>\.kronn\worktrees\<project>--<discussion>\…\file`
/// With a typical repo path of ~80 chars and the `.kronn\worktrees\` prefix
/// (~17 chars), capping each slug at 60 chars leaves at least ~100 chars for
/// nested files inside the worktree before hitting the legacy limit.
const MAX_SLUG_LEN: usize = 60;

/// Slugify a string for use in paths and branch names.
///
/// Caps the result at `MAX_SLUG_LEN` so concatenations like
/// `<project>--<discussion>` stay safely below Windows MAX_PATH (260)
/// even before the long-path prefix kicks in.
fn slugify(s: &str) -> String {
    let raw: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    truncate_slug(&raw)
}

/// Truncate a slug to `MAX_SLUG_LEN` chars without splitting on a trailing dash.
fn truncate_slug(s: &str) -> String {
    if s.len() <= MAX_SLUG_LEN {
        return s.to_string();
    }
    // chars() to be unicode-safe (slugs can contain accented letters)
    let truncated: String = s.chars().take(MAX_SLUG_LEN).collect();
    truncated.trim_end_matches('-').to_string()
}

/// Apply the Windows extended-length path prefix `\\?\` so file APIs accept
/// paths longer than 260 chars. No-op on non-Windows. Idempotent.
#[allow(dead_code)]
pub(crate) fn long_path(p: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let s = p.to_string_lossy();
        if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
            return p.to_path_buf();
        }
        // Only meaningful for absolute drive paths (C:\...). UNC paths use
        // a different form: \\?\UNC\server\share\…
        if s.len() >= 3 && s.as_bytes()[1] == b':' {
            return PathBuf::from(format!(r"\\?\{}", s));
        }
        if let Some(rest) = s.strip_prefix(r"\\") {
            // \\server\share → \\?\UNC\server\share
            return PathBuf::from(format!(r"\\?\UNC\{}", rest));
        }
        p.to_path_buf()
    }
    #[cfg(not(target_os = "windows"))]
    {
        p.to_path_buf()
    }
}

/// Create a persistent worktree for a discussion.
///
/// - `repo_path`: the git repo path (resolved via resolve_host_path)
/// - `project_slug`: slugified project name
/// - `discussion_slug`: slugified discussion title or ID
/// - `base_branch`: branch to base the worktree on (e.g., "main")
pub fn create_discussion_worktree(
    repo_path: &Path,
    project_slug: &str,
    discussion_slug: &str,
    base_branch: &str,
) -> Result<WorktreeInfo, String> {
    let project_slug = slugify(project_slug);
    let discussion_slug = slugify(discussion_slug);
    let branch = format!("kronn/{}", discussion_slug);
    let dir_name = format!("{}--{}", project_slug, discussion_slug);
    let worktree_path = worktree_base_dir(repo_path).join(&dir_name);

    // If the branch is already checked out in the main repo, block — user must switch
    // branches before the agent can work. This avoids the agent modifying files under
    // a running dev environment.
    if let Some(existing_path) = branch_checked_out_at(repo_path, &branch) {
        if existing_path == repo_path {
            return Err(format!(
                "Branch {} is currently checked out in the main repo. Please switch to another branch before continuing.",
                branch
            ));
        }
        // Already in a worktree (e.g. .kronn/worktrees/) — reuse it
        tracing::info!(
            "Branch {} already checked out at {}, reusing",
            branch, existing_path.display()
        );
        return Ok(WorktreeInfo {
            path: existing_path.to_string_lossy().to_string(),
            branch,
            is_main_repo: false,
        });
    }

    // Create base directory
    std::fs::create_dir_all(worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    // Ensure .kronn/worktrees/ is gitignored
    if let Some(p) = repo_path.to_str() {
        crate::core::mcp_scanner::ensure_gitignore_public(p, ".kronn/");
    }

    // Mark repo as safe directory (needed in Docker where mount owner differs)
    if crate::core::env::is_docker() {
        let _ = sync_cmd("git")
            .args(["config", "--global", "--add", "safe.directory", &repo_path.to_string_lossy()])
            .output();
        let _ = sync_cmd("git")
            .args(["config", "--global", "--add", "safe.directory", &worktree_path.to_string_lossy()])
            .output();
    }

    // Create the worktree with a new branch based on base_branch
    let output = sync_cmd("git")
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_path)
        .arg(base_branch)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    tracing::info!(
        "Created discussion worktree at {} (branch: {})",
        worktree_path.display(),
        branch
    );

    // Fix gitdir paths so the worktree works from the host too (not just inside Docker)
    fix_worktree_paths(repo_path, &worktree_path);

    // Copy .mcp.json from repo root to worktree (it's gitignored)
    let mcp_src = repo_path.join(".mcp.json");
    if mcp_src.exists() {
        let mcp_dst = worktree_path.join(".mcp.json");
        if let Err(e) = std::fs::copy(&mcp_src, &mcp_dst) {
            tracing::warn!("Failed to copy .mcp.json to worktree: {}", e);
        } else {
            tracing::info!("Copied .mcp.json to worktree");
        }
    }

    // Copy .vibe/config.toml if it exists (for Vibe agent)
    let vibe_src = repo_path.join(".vibe").join("config.toml");
    if vibe_src.exists() {
        let vibe_dir = worktree_path.join(".vibe");
        let _ = std::fs::create_dir_all(&vibe_dir);
        let vibe_dst = vibe_dir.join("config.toml");
        if let Err(e) = std::fs::copy(&vibe_src, &vibe_dst) {
            tracing::warn!("Failed to copy .vibe/config.toml to worktree: {}", e);
        } else {
            tracing::info!("Copied .vibe/config.toml to worktree");
        }
    }

    // Copy .kiro/settings/mcp.json if it exists (for Kiro agent)
    let kiro_src = repo_path.join(".kiro").join("settings").join("mcp.json");
    if kiro_src.exists() {
        let kiro_dir = worktree_path.join(".kiro").join("settings");
        let _ = std::fs::create_dir_all(&kiro_dir);
        let kiro_dst = kiro_dir.join("mcp.json");
        if let Err(e) = std::fs::copy(&kiro_src, &kiro_dst) {
            tracing::warn!("Failed to copy .kiro/settings/mcp.json to worktree: {}", e);
        } else {
            tracing::info!("Copied .kiro/settings/mcp.json to worktree");
        }
    }

    // Copy .gemini/settings.json if it exists (for Gemini CLI agent)
    let gemini_src = repo_path.join(".gemini").join("settings.json");
    if gemini_src.exists() {
        let gemini_dir = worktree_path.join(".gemini");
        let _ = std::fs::create_dir_all(&gemini_dir);
        let gemini_dst = gemini_dir.join("settings.json");
        if let Err(e) = std::fs::copy(&gemini_src, &gemini_dst) {
            tracing::warn!("Failed to copy .gemini/settings.json to worktree: {}", e);
        } else {
            tracing::info!("Copied .gemini/settings.json to worktree");
        }
    }

    Ok(WorktreeInfo {
        path: worktree_path.to_string_lossy().to_string(),
        branch,
        is_main_repo: false,
    })
}

/// Re-attach an existing branch to a new worktree path.
/// Used to migrate worktrees from /data/workspaces/ to .kronn/worktrees/.
pub fn reattach_worktree(
    repo_path: &Path,
    project_slug: &str,
    discussion_slug: &str,
    existing_branch: &str,
) -> Result<WorktreeInfo, String> {
    let project_slug = slugify(project_slug);
    let discussion_slug = slugify(discussion_slug);
    let dir_name = format!("{}--{}", project_slug, discussion_slug);
    let worktree_path = worktree_base_dir(repo_path).join(&dir_name);

    // Block if branch is checked out in the main repo (user is testing)
    if let Some(existing_path) = branch_checked_out_at(repo_path, existing_branch) {
        if existing_path == repo_path {
            return Err(format!(
                "Branch {} is currently checked out in the main repo. Please switch to another branch first.",
                existing_branch
            ));
        }
    }

    // If worktree already exists at new path, just return it
    if worktree_path.exists() {
        return Ok(WorktreeInfo {
            path: worktree_path.to_string_lossy().to_string(),
            branch: existing_branch.to_string(),
            is_main_repo: false,
        });
    }

    std::fs::create_dir_all(worktree_base_dir(repo_path))
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    if let Some(p) = repo_path.to_str() {
        crate::core::mcp_scanner::ensure_gitignore_public(p, ".kronn/");
    }

    if crate::core::env::is_docker() {
        let _ = sync_cmd("git")
            .args(["config", "--global", "--add", "safe.directory", &repo_path.to_string_lossy()])
            .output();
        let _ = sync_cmd("git")
            .args(["config", "--global", "--add", "safe.directory", &worktree_path.to_string_lossy()])
            .output();
    }

    // Prune stale worktree entries first (old /data/workspaces/ refs)
    let _ = sync_cmd("git")
        .args(["worktree", "prune"])
        .current_dir(repo_path)
        .output();

    // Attach existing branch to new worktree path (no -b, branch already exists)
    let output = sync_cmd("git")
        .args(["worktree", "add"])
        .arg(&worktree_path)
        .arg(existing_branch)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree reattach failed: {}", stderr));
    }

    tracing::info!(
        "Re-attached worktree at {} (branch: {})",
        worktree_path.display(),
        existing_branch
    );

    fix_worktree_paths(repo_path, &worktree_path);

    Ok(WorktreeInfo {
        path: worktree_path.to_string_lossy().to_string(),
        branch: existing_branch.to_string(),
        is_main_repo: false,
    })
}

/// Find the branch associated with a worktree path (before removal).
fn find_branch_for_worktree(repo_path: &Path, worktree_path: &str) -> Option<String> {
    let output = sync_cmd("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    // Extract the dir name for matching (git may list relative or absolute paths)
    let wt_dir_name = Path::new(worktree_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found = false;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found = path == worktree_path
                || path.ends_with(&wt_dir_name);
        } else if found && line.starts_with("branch refs/heads/") {
            return Some(line.trim_start_matches("branch refs/heads/").to_string());
        } else if found && line.is_empty() {
            found = false;
        }
    }
    None
}

/// Remove a worktree and optionally delete the branch.
pub fn remove_discussion_worktree(
    repo_path: &Path,
    worktree_path: &str,
    delete_branch: bool,
) -> Result<(), String> {
    // Determine the branch name BEFORE removing the worktree (it won't be listed after)
    let branch_to_delete = if delete_branch {
        find_branch_for_worktree(repo_path, worktree_path)
    } else {
        None
    };

    // Remove the worktree via git (try absolute path, then relative)
    let wt_abs = Path::new(worktree_path);
    let wt_relative = wt_abs.strip_prefix(repo_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let output = sync_cmd("git")
        .args(["worktree", "remove", "--force", worktree_path])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree remove: {}", e))?;

    if !output.status.success() && !wt_relative.is_empty() {
        // Git may know the worktree by relative path (due to relative gitdir refs)
        let _ = sync_cmd("git")
            .args(["worktree", "remove", "--force", &wt_relative])
            .current_dir(repo_path)
            .output();
    }

    // Final fallback: manual cleanup if directory still exists
    if wt_abs.exists() {
        let _ = std::fs::remove_dir_all(wt_abs);
    }

    // Prune stale worktree entries before deleting branch
    let _ = sync_cmd("git")
        .args(["worktree", "prune"])
        .current_dir(repo_path)
        .output();

    if let Some(branch) = branch_to_delete {
        let _ = sync_cmd("git")
            .args(["branch", "-D", &branch])
            .current_dir(repo_path)
            .output();
        tracing::info!("Deleted branch: {}", branch);
    }

    tracing::info!("Removed worktree: {}", worktree_path);
    Ok(())
}

/// List all kronn worktrees for a project.
pub fn list_project_worktrees(repo_path: &Path) -> Vec<WorktreeInfo> {
    let output = match sync_cmd("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.to_string());
            current_branch = None;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        } else if line.is_empty() {
            if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take()) {
                if branch.starts_with("kronn/") {
                    worktrees.push(WorktreeInfo { path, branch, is_main_repo: false });
                }
            }
        }
    }

    // Handle last entry (no trailing empty line)
    if let (Some(path), Some(branch)) = (current_path, current_branch) {
        if branch.starts_with("kronn/") {
            worktrees.push(WorktreeInfo { path, branch, is_main_repo: false });
        }
    }

    worktrees
}

/// Validate that a worktree path still exists on disk.
pub fn validate_worktree(worktree_path: &str) -> bool {
    Path::new(worktree_path).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary git repo for testing.
    fn current_branch(repo_path: &Path) -> Option<String> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(repo_path)
            .output()
            .ok()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }

    fn make_test_repo(name: &str) -> tempfile::TempDir {
        let dir = tempfile::Builder::new().prefix(&format!("kronn-wt-{}", name)).tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Need at least one commit for worktrees to work
        fs::write(dir.path().join("README.md"), "# test").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("My Project"), "my-project");
        assert_eq!(slugify("Fix bug #123"), "fix-bug-123");
        assert_eq!(slugify("  spaces  and---dashes  "), "spaces-and-dashes");
        assert_eq!(slugify("UPPER_case"), "upper-case");
    }

    #[test]
    fn test_slugify_already_slugified() {
        assert_eq!(slugify("my-branch"), "my-branch");
        assert_eq!(slugify("feat-add-thing"), "feat-add-thing");
    }

    #[test]
    fn test_slugify_special_chars() {
        // Slashes, @, ! become dashes, then consecutive dashes collapse
        assert_eq!(slugify("feat/add-@thing!"), "feat-add-thing");
    }

    #[test]
    fn test_slugify_unicode() {
        // Non-alphanumeric unicode chars (accented letters are alphanumeric in Rust)
        // 'é' is alphanumeric → kept as-is; let's verify it doesn't panic
        let result = slugify("café");
        // "café" lowercased is "café", all chars alphanumeric → no dashes → "café"
        assert_eq!(result, "café");

        // Non-alphanumeric unicode punctuation gets replaced
        let result2 = slugify("hello•world");
        assert_eq!(result2, "hello-world");
    }

    #[test]
    fn test_slugify_empty_string() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn test_slugify_caps_at_max_len() {
        // A 200-char input must be capped to MAX_SLUG_LEN (60).
        let long = "a".repeat(200);
        let result = slugify(&long);
        assert!(result.len() <= MAX_SLUG_LEN, "slug must be <= MAX_SLUG_LEN, got {}", result.len());
        assert_eq!(result.len(), MAX_SLUG_LEN);
    }

    #[test]
    fn test_slugify_truncation_does_not_leave_trailing_dash() {
        // 30 chars + dash + 30 chars = 61 chars → truncation lands on the dash boundary
        let s = format!("{}-{}", "a".repeat(30), "b".repeat(30));
        let result = slugify(&s);
        assert!(result.len() <= MAX_SLUG_LEN);
        assert!(!result.ends_with('-'), "truncated slug must not end with a dash, got {:?}", result);
    }

    #[test]
    fn test_truncate_slug_unicode_safe() {
        // Truncation must operate on chars, not bytes, to avoid panicking
        // mid-codepoint with unicode slugs (e.g. lots of "é").
        let s = "é".repeat(200);
        let result = truncate_slug(&s);
        assert!(result.chars().count() <= MAX_SLUG_LEN);
    }

    #[test]
    fn test_long_path_noop_on_unix() {
        // long_path is a no-op on non-Windows. Confirm it returns the same path.
        let p = PathBuf::from("/home/user/project");
        assert_eq!(long_path(&p), p);
    }

    #[test]
    fn test_worktree_base_dir() {
        let repo = PathBuf::from("/home/user/project");
        let base = worktree_base_dir(&repo);
        assert_eq!(base, PathBuf::from("/home/user/project/.kronn/worktrees"));
    }

    #[test]
    fn test_validate_worktree_nonexistent() {
        assert!(!validate_worktree("/nonexistent/path/that/does/not/exist"));
    }

    #[test]
    fn test_list_project_worktrees_no_repo() {
        // Should return empty vec for a non-repo path
        let result = list_project_worktrees(Path::new("/tmp"));
        // May or may not be empty depending on system, but should not panic
        let _ = result;
    }

    // ── Worktree lifecycle tests ─────────────────────────────────────────────

    #[test]
    fn test_create_discussion_worktree_creates_branch_and_dir() {
        let repo = make_test_repo("create");
        let result = create_discussion_worktree(repo.path(), "myproject", "fix-bug", "main");
        assert!(result.is_ok(), "create_discussion_worktree failed: {:?}", result.err());
        let info = result.unwrap();
        assert_eq!(info.branch, "kronn/fix-bug");
        assert!(!info.is_main_repo);
        assert!(Path::new(&info.path).exists(), "Worktree directory should exist");
        assert!(Path::new(&info.path).join(".git").exists(), "Worktree .git file should exist");
    }

    #[test]
    fn test_create_worktree_in_kronn_worktrees_dir() {
        let repo = make_test_repo("basedir");
        let result = create_discussion_worktree(repo.path(), "proj", "feat", "main").unwrap();
        let expected_base = repo.path().join(".kronn/worktrees");
        assert!(result.path.starts_with(&expected_base.to_string_lossy().to_string()));
    }

    #[test]
    fn test_fix_worktree_paths_writes_relative() {
        let repo = make_test_repo("relpath");
        let info = create_discussion_worktree(repo.path(), "proj", "test-rel", "main").unwrap();
        let wt_path = Path::new(&info.path);

        // Verify .git file uses relative path
        let dot_git_content = fs::read_to_string(wt_path.join(".git")).unwrap();
        assert!(
            dot_git_content.contains("../../.git/worktrees/"),
            "Expected relative gitdir, got: {}",
            dot_git_content
        );

        // Verify reverse gitdir uses relative path
        let wt_name = wt_path.file_name().unwrap().to_string_lossy();
        let gitdir_content = fs::read_to_string(
            repo.path().join(".git").join("worktrees").join(wt_name.as_ref()).join("gitdir")
        ).unwrap();
        assert!(
            gitdir_content.contains(".kronn/worktrees/"),
            "Expected relative gitdir back-reference, got: {}",
            gitdir_content
        );
    }

    #[test]
    fn test_remove_worktree_cleans_up() {
        let repo = make_test_repo("remove");
        let info = create_discussion_worktree(repo.path(), "proj", "to-remove", "main").unwrap();
        let wt_path = info.path.clone();
        assert!(Path::new(&wt_path).exists());

        let result = remove_discussion_worktree(repo.path(), &wt_path, false);
        assert!(result.is_ok());
        assert!(!Path::new(&wt_path).exists(), "Worktree directory should be removed");
    }

    #[test]
    fn test_remove_worktree_keeps_branch_when_requested() {
        let repo = make_test_repo("keep-branch");
        let info = create_discussion_worktree(repo.path(), "proj", "keep-me", "main").unwrap();

        remove_discussion_worktree(repo.path(), &info.path, false).unwrap();

        // Branch should still exist
        let output = std::process::Command::new("git")
            .args(["branch", "--list", &info.branch])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(branches.contains("kronn/keep-me"), "Branch should still exist after remove with delete_branch=false");
    }

    #[test]
    fn test_remove_worktree_deletes_branch_when_requested() {
        let repo = make_test_repo("del-branch");
        let info = create_discussion_worktree(repo.path(), "proj", "delete-me", "main").unwrap();

        remove_discussion_worktree(repo.path(), &info.path, true).unwrap();

        let output = std::process::Command::new("git")
            .args(["branch", "--list", &info.branch])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(!branches.contains("kronn/delete-me"), "Branch should be deleted");
    }

    #[test]
    fn test_reattach_worktree_after_remove() {
        let repo = make_test_repo("reattach");
        let info = create_discussion_worktree(repo.path(), "proj", "reattach-test", "main").unwrap();
        let branch = info.branch.clone();

        // Remove worktree but keep branch
        remove_discussion_worktree(repo.path(), &info.path, false).unwrap();
        assert!(!Path::new(&info.path).exists());

        // Re-attach
        let result = reattach_worktree(repo.path(), "proj", "reattach-test", &branch);
        assert!(result.is_ok(), "reattach failed: {:?}", result.err());
        let info2 = result.unwrap();
        assert!(Path::new(&info2.path).exists());
        assert_eq!(info2.branch, branch);
    }

    #[test]
    fn test_create_blocks_when_branch_on_main_repo() {
        let repo = make_test_repo("block");
        // Create branch and check it out in the main repo
        std::process::Command::new("git")
            .args(["checkout", "-b", "kronn/blocked-test"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let result = create_discussion_worktree(repo.path(), "proj", "blocked-test", "main");
        assert!(result.is_err(), "Should fail when branch is checked out in main repo");
        let err = result.unwrap_err();
        assert!(err.contains("checked out"), "Error should mention 'checked out': {}", err);
    }

    #[test]
    fn test_reattach_blocks_when_branch_on_main_repo() {
        let repo = make_test_repo("reattach-block");
        std::process::Command::new("git")
            .args(["checkout", "-b", "kronn/reattach-blocked"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let result = reattach_worktree(repo.path(), "proj", "reattach-blocked", "kronn/reattach-blocked");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("checked out"));
    }

    #[test]
    fn test_current_branch_returns_branch_name() {
        let repo = make_test_repo("curbranch");
        let branch = current_branch(repo.path());
        assert_eq!(branch, Some("main".to_string()));
    }

    #[test]
    fn test_branch_checked_out_at_finds_main() {
        let repo = make_test_repo("checkout-at");
        let result = branch_checked_out_at(repo.path(), "main");
        assert!(result.is_some(), "main should be found as checked out");
        assert_eq!(result.unwrap(), repo.path());
    }

    #[test]
    fn test_branch_checked_out_at_returns_none_for_nonexistent() {
        let repo = make_test_repo("checkout-none");
        let result = branch_checked_out_at(repo.path(), "kronn/does-not-exist");
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_worktree_existing() {
        let repo = make_test_repo("validate");
        let info = create_discussion_worktree(repo.path(), "proj", "val-test", "main").unwrap();
        assert!(validate_worktree(&info.path));
    }
}
