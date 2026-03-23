//! Git worktree workspace management for workflow runs.
//!
//! Each workflow run gets an isolated git worktree so changes don't
//! interfere with the main working tree. Lifecycle hooks are executed
//! at each stage.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use tokio::process::Command;

use crate::models::WorkspaceHooks;

/// An active workspace (git worktree) for a workflow run.
pub struct Workspace {
    /// Path to the worktree directory
    pub path: PathBuf,
    /// Branch name created for this worktree
    pub branch: String,
    /// The main repo path (for cleanup)
    repo_path: PathBuf,
    /// Optional lifecycle hooks
    hooks: Option<WorkspaceHooks>,
}

/// Sanitize a workflow name for use in branch names and directory paths.
/// Keeps alphanumeric, dash, and underscore; replaces everything else with `-`.
pub(crate) fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

/// Build a branch name for a workflow run: `kronn/<sanitized_name>/<run_id_prefix>`.
pub(crate) fn build_branch_name(workflow_name: &str, run_id: &str) -> String {
    let sanitized = sanitize_name(workflow_name);
    format!("kronn/{}/{}", sanitized, &run_id[..8.min(run_id.len())])
}

/// Build the worktree directory name: `<sanitized_name>-<run_id_prefix>`.
pub(crate) fn build_worktree_dir_name(workflow_name: &str, run_id: &str) -> String {
    let sanitized = sanitize_name(workflow_name);
    format!("{}-{}", sanitized, &run_id[..8.min(run_id.len())])
}

impl Workspace {
    /// Create a new workspace via `git worktree add`.
    /// Branch: `kronn/<workflow_name>/<run_id>`
    pub async fn create(
        repo_path: &Path,
        workflow_name: &str,
        run_id: &str,
        hooks: Option<WorkspaceHooks>,
    ) -> Result<Self> {
        let _sanitized_name = sanitize_name(workflow_name);

        let branch = build_branch_name(workflow_name, run_id);

        // Worktree path: alongside the repo, in a .kronn-worktrees directory
        let worktree_base = repo_path.join(".kronn-worktrees");
        std::fs::create_dir_all(&worktree_base)?;
        let worktree_path = worktree_base.join(build_worktree_dir_name(workflow_name, run_id));

        // Mark the repo and worktree as safe directories (needed in Docker where
        // the mounted volume owner differs from the container user)
        let _ = Command::new("git")
            .args(["config", "--global", "--add", "safe.directory", &repo_path.to_string_lossy()])
            .output()
            .await;
        let _ = Command::new("git")
            .args(["config", "--global", "--add", "safe.directory", &worktree_path.to_string_lossy()])
            .output()
            .await;

        // Create the worktree with a new branch
        let output = Command::new("git")
            .args(["worktree", "add", "-b", &branch])
            .arg(&worktree_path)
            .current_dir(repo_path)
            .output()
            .await
            .context("Failed to execute git worktree add")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {}", stderr);
        }

        tracing::info!("Created worktree at {} (branch: {})", worktree_path.display(), branch);

        let ws = Self {
            path: worktree_path,
            branch,
            repo_path: repo_path.to_path_buf(),
            hooks,
        };

        // Run after_create hook
        ws.run_hook("after_create").await?;

        Ok(ws)
    }

    /// Run the before_run hook.
    pub async fn before_run(&self) -> Result<()> {
        self.run_hook("before_run").await
    }

    /// Run the after_run hook.
    pub async fn after_run(&self) -> Result<()> {
        self.run_hook("after_run").await
    }

    /// Clean up the workspace: run before_remove hook, then remove the worktree.
    pub async fn cleanup(self) -> Result<()> {
        self.run_hook("before_remove").await?;

        // Remove the worktree
        let output = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.repo_path)
            .output()
            .await
            .context("Failed to execute git worktree remove")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("git worktree remove failed (will try manual cleanup): {}", stderr);
            // Fallback: remove directory manually
            if self.path.exists() {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        // Delete the branch
        let _ = Command::new("git")
            .args(["branch", "-D", &self.branch])
            .current_dir(&self.repo_path)
            .output()
            .await;

        tracing::info!("Cleaned up worktree: {}", self.path.display());
        Ok(())
    }

    /// Execute a lifecycle hook shell command in the workspace directory.
    async fn run_hook(&self, hook_name: &str) -> Result<()> {
        let cmd = match (&self.hooks, hook_name) {
            (Some(h), "after_create") => h.after_create.as_deref(),
            (Some(h), "before_run") => h.before_run.as_deref(),
            (Some(h), "after_run") => h.after_run.as_deref(),
            (Some(h), "before_remove") => h.before_remove.as_deref(),
            _ => None,
        };

        if let Some(cmd) = cmd {
            tracing::info!("Running workspace hook '{}': {}", hook_name, cmd);
            let output = Command::new("sh")
                .args(["-c", cmd])
                .current_dir(&self.path)
                .output()
                .await
                .with_context(|| format!("Failed to run {} hook", hook_name))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("Hook '{}' failed: {}", hook_name, stderr);
                // Hooks are best-effort — log but don't fail the workflow
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── sanitize_name ───────────────────────────────────────────────────

    #[test]
    fn sanitize_name_alphanumeric_unchanged() {
        assert_eq!(sanitize_name("my-workflow_v2"), "my-workflow_v2");
    }

    #[test]
    fn sanitize_name_spaces_replaced() {
        assert_eq!(sanitize_name("my workflow"), "my-workflow");
    }

    #[test]
    fn sanitize_name_special_chars_replaced() {
        assert_eq!(sanitize_name("build & deploy!"), "build---deploy-");
    }

    #[test]
    fn sanitize_name_unicode_alphanumeric_preserved() {
        // Rust's char::is_alphanumeric() includes Unicode letters like é
        assert_eq!(sanitize_name("déploiement"), "déploiement");
    }

    #[test]
    fn sanitize_name_empty() {
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn sanitize_name_all_special() {
        assert_eq!(sanitize_name("!@#$%"), "-----");
    }

    // ─── build_branch_name ───────────────────────────────────────────────

    #[test]
    fn build_branch_name_basic() {
        let branch = build_branch_name("my-workflow", "abcdef12-3456-7890");
        assert_eq!(branch, "kronn/my-workflow/abcdef12");
    }

    #[test]
    fn build_branch_name_short_run_id() {
        let branch = build_branch_name("wf", "abc");
        assert_eq!(branch, "kronn/wf/abc");
    }

    #[test]
    fn build_branch_name_sanitizes_workflow_name() {
        let branch = build_branch_name("My Workflow!", "12345678");
        assert_eq!(branch, "kronn/My-Workflow-/12345678");
    }

    #[test]
    fn build_branch_name_exact_8_char_run_id() {
        let branch = build_branch_name("test", "12345678");
        assert_eq!(branch, "kronn/test/12345678");
    }

    // ─── build_worktree_dir_name ─────────────────────────────────────────

    #[test]
    fn build_worktree_dir_basic() {
        let dir = build_worktree_dir_name("deploy", "abcdef12-rest");
        assert_eq!(dir, "deploy-abcdef12");
    }

    #[test]
    fn build_worktree_dir_sanitizes() {
        let dir = build_worktree_dir_name("build & test", "11223344");
        assert_eq!(dir, "build---test-11223344");
    }

    #[test]
    fn build_worktree_dir_short_run_id() {
        let dir = build_worktree_dir_name("wf", "ab");
        assert_eq!(dir, "wf-ab");
    }

    // ─── Worktree base dir ───────────────────────────────────────────────

    #[test]
    fn worktree_base_is_inside_repo() {
        let repo = PathBuf::from("/home/user/project");
        let base = repo.join(".kronn-worktrees");
        assert_eq!(base.to_str().unwrap(), "/home/user/project/.kronn-worktrees");
    }

    #[test]
    fn worktree_path_combines_base_and_dir() {
        let repo = PathBuf::from("/repos/myapp");
        let base = repo.join(".kronn-worktrees");
        let dir_name = build_worktree_dir_name("audit", "aabbccdd-1234");
        let full = base.join(&dir_name);
        assert_eq!(full.to_str().unwrap(), "/repos/myapp/.kronn-worktrees/audit-aabbccdd");
    }
}
