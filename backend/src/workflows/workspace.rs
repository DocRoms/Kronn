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

impl Workspace {
    /// Create a new workspace via `git worktree add`.
    /// Branch: `kronn/<workflow_name>/<run_id>`
    pub async fn create(
        repo_path: &Path,
        workflow_name: &str,
        run_id: &str,
        hooks: Option<WorkspaceHooks>,
    ) -> Result<Self> {
        let sanitized_name = workflow_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect::<String>();

        let branch = format!("kronn/{}/{}", sanitized_name, &run_id[..8.min(run_id.len())]);

        // Worktree path: alongside the repo, in a .kronn-worktrees directory
        let worktree_base = repo_path.join(".kronn-worktrees");
        std::fs::create_dir_all(&worktree_base)?;
        let worktree_path = worktree_base.join(format!("{}-{}", sanitized_name, &run_id[..8.min(run_id.len())]));

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
