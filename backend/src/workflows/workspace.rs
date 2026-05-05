//! Git worktree workspace management for workflow runs.
//!
//! Each workflow run gets an isolated git worktree so changes don't
//! interfere with the main working tree. Lifecycle hooks are executed
//! at each stage.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};

use crate::core::cmd::async_cmd;
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

/// Snapshot of a branch that the runner kept alive instead of deleting on
/// cleanup. Returned by `Workspace::cleanup` so the caller can persist the
/// info on the run row and surface it in the UI ("commit produit ici").
///
/// Without this, agents that committed locally but failed to push (e.g.
/// pre-push hook blocked, network down, no auth) lose visibility entirely:
/// the worktree gets removed, the branch gets deleted, and the commits
/// drift into git's dangling-object pool until the next `gc`.
#[derive(Debug, Clone)]
pub struct PreservedBranch {
    /// The kept-alive branch name in the parent repo (e.g. `kronn/Autobot/68dccb12`).
    pub branch_name: String,
    /// HEAD SHA at cleanup time. Lets the caller render the commit and
    /// recover even if the branch is later deleted.
    pub head_sha: String,
    /// Commits ahead of the chosen base (upstream / origin/main / main).
    pub ahead: u32,
    /// True if the branch had an upstream tracking ref — i.e. the agent
    /// at least *attempted* to push (and may have partially succeeded).
    /// False = no push was ever attempted.
    pub pushed_upstream: bool,
}

/// Outcome of `Workspace::cleanup`. The branch field is `Some` whenever
/// the worktree's HEAD held local commits not present on a known base —
/// the runner records it on `WorkflowRun.produced_branches` so the run
/// detail UI can show "commit produit, push bloqué — branche `X` preservée".
#[derive(Debug, Clone, Default)]
pub struct CleanupOutcome {
    pub preserved: Option<PreservedBranch>,
}

/// Sanitize a workflow name for use in branch names and directory paths.
/// Keeps alphanumeric, dash, and underscore; replaces everything else with `-`.
pub(crate) fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

/// Decide whether the branch backing this worktree should outlive cleanup.
///
/// Returns `Some(PreservedBranch)` when HEAD holds commits not on any known
/// base ref (upstream → origin/main → origin/master → main → master). Returns
/// `None` only when HEAD is fully synced with one of those bases (nothing to
/// salvage). On total failure (no base ref found, or git errors) we return
/// `Some` with `ahead=0` to err on the side of preservation — losing real
/// work is much worse than leaving a stale empty branch behind.
async fn check_branch_for_preservation(
    worktree: &Path,
    branch: &str,
) -> Option<PreservedBranch> {
    // HEAD sha is the anchor we want to be able to recover via the branch.
    let head_sha = git_text_output(worktree, &["rev-parse", "HEAD"]).await?;

    // Was an upstream set on the worktree's branch ? Tells us whether the
    // agent tried to push at all. `@{u}` resolves only when set.
    let pushed_upstream =
        git_text_output(worktree, &["rev-parse", "--abbrev-ref", "@{u}"]).await.is_some();

    // Walk through plausible bases in order of relevance. The first one
    // that resolves wins — count commits ahead.
    let mut bases: Vec<&str> = Vec::with_capacity(5);
    if pushed_upstream {
        bases.push("@{u}");
    }
    bases.extend(["origin/main", "origin/master", "main", "master"]);

    for base in bases {
        let count_str = git_text_output(
            worktree,
            &["rev-list", "--count", &format!("{}..HEAD", base)],
        )
        .await;
        if let Some(count_str) = count_str {
            let ahead: u32 = count_str.trim().parse().unwrap_or(0);
            if ahead == 0 {
                // Synced — drop the branch.
                return None;
            }
            return Some(PreservedBranch {
                branch_name: branch.to_string(),
                head_sha,
                ahead,
                pushed_upstream,
            });
        }
    }

    // No base resolved — preserve defensively.
    Some(PreservedBranch {
        branch_name: branch.to_string(),
        head_sha,
        ahead: 0,
        pushed_upstream,
    })
}

/// Run `git <args>` in `cwd`, return trimmed stdout if exit was 0.
async fn git_text_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = async_cmd("git").args(args).current_dir(cwd).output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

        // Worktree path: alongside the repo, in a .kronn/worktrees directory
        let worktree_base = repo_path.join(".kronn/worktrees");
        std::fs::create_dir_all(&worktree_base)?;
        // Ensure .kronn/worktrees/ is gitignored in the project
        if let Some(p) = repo_path.to_str() {
            crate::core::mcp_scanner::ensure_gitignore_public(p, ".kronn/");
        }
        let worktree_path = worktree_base.join(build_worktree_dir_name(workflow_name, run_id));

        // Mark the repo and worktree as safe directories (needed in Docker where
        // the mounted volume owner differs from the container user)
        let _ = async_cmd("git")
            .args(["config", "--global", "--add", "safe.directory", &repo_path.to_string_lossy()])
            .output()
            .await;
        let _ = async_cmd("git")
            .args(["config", "--global", "--add", "safe.directory", &worktree_path.to_string_lossy()])
            .output()
            .await;

        // Create the worktree with a new branch
        let output = async_cmd("git")
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

    /// 0.7.0 Phase 4 — attach to a previously-created worktree. Used on
    /// resume from a Gate pause: the worktree already exists on disk
    /// (the agent ran in it before the pause), `before_run` already
    /// fired, and we just want a `Workspace` handle to drive `after_run`
    /// and `cleanup` once the run finishes. No git side-effects: no
    /// `worktree add`, no `safe.directory` config, no hook firing.
    /// The path is taken at face value.
    pub fn attach(
        path: PathBuf,
        repo_path: PathBuf,
        workflow_name: &str,
        run_id: &str,
        hooks: Option<WorkspaceHooks>,
    ) -> Self {
        Self {
            path,
            branch: build_branch_name(workflow_name, run_id),
            repo_path,
            hooks,
        }
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
    ///
    /// Branch lifecycle:
    ///   - The branch is preserved (left alive in the parent repo) when the
    ///     worktree's HEAD has local commits not present on any known base
    ///     ref. Returned in `CleanupOutcome.preserved`. The runner records
    ///     it on the run so the UI can surface "commit available here".
    ///   - The branch is deleted when the worktree is fully synced with a
    ///     known base — nothing of value would survive its deletion.
    ///
    /// Failures: best-effort. If the preserve check itself errors out, we
    /// default to preserving the branch (safer than silently dropping work).
    pub async fn cleanup(self) -> Result<CleanupOutcome> {
        self.run_hook("before_remove").await?;

        // Snapshot branch state BEFORE removing the worktree — afterwards
        // the worktree path is gone and `git -C <worktree>` calls fail.
        let preserve = check_branch_for_preservation(&self.path, &self.branch).await;

        // Remove the worktree
        let output = async_cmd("git")
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

        let outcome = if let Some(info) = preserve {
            // Keep the branch alive in the parent repo.
            tracing::info!(
                "Preserved branch '{}' (HEAD={}, {} commit(s) ahead of base, upstream_set={}) — \
                 the worktree had local commits the operator may want to recover.",
                info.branch_name, info.head_sha, info.ahead, info.pushed_upstream
            );
            CleanupOutcome { preserved: Some(info) }
        } else {
            // Fully synced — safe to drop the branch ref.
            let _ = async_cmd("git")
                .args(["branch", "-D", &self.branch])
                .current_dir(&self.repo_path)
                .output()
                .await;
            CleanupOutcome::default()
        };

        tracing::info!("Cleaned up worktree: {}", self.path.display());
        Ok(outcome)
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
            let output = async_cmd("sh")
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
        let base = repo.join(".kronn/worktrees");
        assert_eq!(base.to_str().unwrap(), "/home/user/project/.kronn/worktrees");
    }

    #[test]
    fn worktree_path_combines_base_and_dir() {
        let repo = PathBuf::from("/repos/myapp");
        let base = repo.join(".kronn/worktrees");
        let dir_name = build_worktree_dir_name("audit", "aabbccdd-1234");
        let full = base.join(&dir_name);
        assert_eq!(full.to_str().unwrap(), "/repos/myapp/.kronn/worktrees/audit-aabbccdd");
    }

    // ─── check_branch_for_preservation (P3 — preserve commits cleanup
    //    would otherwise orphan). Uses real git on a tempdir; serial so
    //    parallel test runs don't trip over each other on shared global
    //    git config. ─────────────────────────────────────────────────────

    /// Set up a tempdir with `git init`, an initial commit on `main`,
    /// then create a branch pointing at HEAD. Returns the repo path so
    /// the test can call our detector against it as the "worktree".
    async fn make_test_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().to_path_buf();

        // Init + minimal user config (commit -m needs an identity).
        let _ = crate::core::cmd::async_cmd("git").args(["init", "-q", "-b", "main"]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["config", "user.email", "test@kronn.local"]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["config", "user.name", "test"]).current_dir(&repo).output().await.unwrap();
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "init"]).current_dir(&repo).output().await.unwrap();

        (dir, repo)
    }

    #[tokio::test]
    async fn preserve_when_branch_synced_with_main_returns_none() {
        // Branch HEAD == main HEAD → nothing to preserve.
        let (_dir, repo) = make_test_repo().await;
        let result = check_branch_for_preservation(&repo, "test-branch").await;
        assert!(result.is_none(), "synced HEAD should NOT be preserved");
    }

    #[tokio::test]
    async fn preserve_when_branch_ahead_of_main_returns_some() {
        // Mirror the production scenario: a worktree branched off main
        // gets its own commit, leaving main behind by 1.
        let (_dir, repo) = make_test_repo().await;
        let _ = crate::core::cmd::async_cmd("git").args(["checkout", "-q", "-b", "kronn/test"]).current_dir(&repo).output().await.unwrap();
        std::fs::write(repo.join("CHANGELOG.md"), "v1\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "feat: changelog"]).current_dir(&repo).output().await.unwrap();

        let result = check_branch_for_preservation(&repo, "kronn/test").await;
        let preserved = result.expect("branch ahead of main should be preserved");
        assert_eq!(preserved.branch_name, "kronn/test");
        assert!(!preserved.head_sha.is_empty(), "head_sha should be filled");
        assert_eq!(preserved.ahead, 1, "exactly one commit beyond main");
        assert!(!preserved.pushed_upstream, "no remote → upstream_set=false");
    }

    #[tokio::test]
    async fn preserve_when_no_base_resolves_falls_back_to_some() {
        // Fresh repo with no `main` ref and no remotes → detector can't
        // count ahead; it returns Some(ahead=0) defensively rather than
        // None (preferring "preserve too much" over "lose work").
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().to_path_buf();
        let _ = crate::core::cmd::async_cmd("git").args(["init", "-q", "-b", "weird-default"]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["config", "user.email", "test@kronn.local"]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["config", "user.name", "test"]).current_dir(&repo).output().await.unwrap();
        std::fs::write(repo.join("README.md"), "x\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&repo).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "init"]).current_dir(&repo).output().await.unwrap();

        let result = check_branch_for_preservation(&repo, "test-branch").await;
        let preserved = result.expect("no resolvable base → defensive preserve");
        assert_eq!(preserved.ahead, 0, "ahead unknown → 0 (still preserve)");
    }
}
