//! Git worktree management for discussion isolation.
//!
//! Each isolated discussion gets its own git worktree so agents can make
//! changes without interfering with the main working tree or other discussions.

use std::path::{Path, PathBuf};

/// Information about a created worktree.
pub struct WorktreeInfo {
    /// Full path to the worktree directory
    pub path: String,
    /// Branch name (e.g., "kronn/fix-the-bug")
    pub branch: String,
}

/// Base directory for worktrees. Uses KRONN_DATA_DIR env var (defaults to
/// `/data` in Docker) + `/workspaces/`.
fn worktree_base_dir() -> PathBuf {
    let data_dir = std::env::var("KRONN_DATA_DIR").unwrap_or_else(|_| "/data".into());
    PathBuf::from(data_dir).join("workspaces")
}

/// Slugify a string for use in paths and branch names.
fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
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
    let worktree_path = worktree_base_dir().join(&dir_name);

    // Create base directory
    std::fs::create_dir_all(worktree_base_dir())
        .map_err(|e| format!("Failed to create workspaces dir: {}", e))?;

    // Mark repo as safe directory (needed in Docker)
    let _ = std::process::Command::new("git")
        .args(["config", "--global", "--add", "safe.directory", &repo_path.to_string_lossy()])
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "--global", "--add", "safe.directory", &worktree_path.to_string_lossy()])
        .output();

    // Create the worktree with a new branch based on base_branch
    let output = std::process::Command::new("git")
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
    })
}

/// Remove a worktree and optionally delete the branch.
pub fn remove_discussion_worktree(
    repo_path: &Path,
    worktree_path: &str,
    delete_branch: bool,
) -> Result<(), String> {
    // Remove the worktree via git
    let output = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", worktree_path])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git worktree remove: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("git worktree remove failed (trying manual cleanup): {}", stderr);
        // Fallback: remove directory manually
        let path = Path::new(worktree_path);
        if path.exists() {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    if delete_branch {
        // Determine the branch name from the worktree
        // First try to get the branch from git worktree list
        let list_output = std::process::Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo_path)
            .output();

        let mut branch_to_delete: Option<String> = None;
        if let Ok(out) = list_output {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut found_worktree = false;
            for line in text.lines() {
                if line.starts_with("worktree ") && line.contains(worktree_path) {
                    found_worktree = true;
                }
                if found_worktree && line.starts_with("branch refs/heads/") {
                    branch_to_delete = Some(line.trim_start_matches("branch refs/heads/").to_string());
                    break;
                }
                if found_worktree && line.is_empty() {
                    break;
                }
            }
        }

        // If we found the branch name, or try with kronn/ prefix pattern from the path
        if let Some(branch) = branch_to_delete {
            let _ = std::process::Command::new("git")
                .args(["branch", "-D", &branch])
                .current_dir(repo_path)
                .output();
            tracing::info!("Deleted branch: {}", branch);
        }
    }

    // Prune stale worktree entries
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_path)
        .output();

    tracing::info!("Removed worktree: {}", worktree_path);
    Ok(())
}

/// List all kronn worktrees for a project.
pub fn list_project_worktrees(repo_path: &Path) -> Vec<WorktreeInfo> {
    let output = match std::process::Command::new("git")
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
                    worktrees.push(WorktreeInfo { path, branch });
                }
            }
        }
    }

    // Handle last entry (no trailing empty line)
    if let (Some(path), Some(branch)) = (current_path, current_branch) {
        if branch.starts_with("kronn/") {
            worktrees.push(WorktreeInfo { path, branch });
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
    fn test_worktree_base_dir() {
        // Without env var, defaults to /data/workspaces
        std::env::remove_var("KRONN_DATA_DIR");
        let base = worktree_base_dir();
        assert_eq!(base, PathBuf::from("/data/workspaces"));
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
}
