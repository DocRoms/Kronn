//! Boot reclamation of the worktrees `Interrupted` runs leave behind.
//!
//! An interrupted run keeps its worktree so it can be resumed. Once nobody has
//! resumed it for the configured lifetime, boot gives the checkout back — but
//! only when that cannot lose work: a dirty or detached checkout is kept, and
//! commits no known base holds stay on a branch recorded on the run.

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::models::ProducedBranch;
use crate::workflows::workspace::{self, InterruptedCheckout};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReclaimReport {
    pub reclaimed: usize,
    /// Reclaimed checkouts whose branch was kept for its commits.
    pub preserved_branches: usize,
    pub kept_dirty: usize,
    /// Detached HEAD, path outside `.kronn/worktrees`, git or database error.
    pub kept_other: usize,
}

/// A reclaimed run keeps its `workspace_path`, so a later resume is refused
/// with "worktree no longer exists" instead of running in the main checkout.
pub async fn reclaim_stale_interrupted_worktrees(
    db: &Database,
    ttl_days: u32,
    now: DateTime<Utc>,
) -> ReclaimReport {
    let mut report = ReclaimReport::default();
    if ttl_days == 0 {
        return report;
    }
    let cutoff = now - chrono::Duration::days(i64::from(ttl_days));
    let candidates = match db
        .with_conn(move |conn| {
            crate::db::workflows::stale_interrupted_workspace_candidates(conn, cutoff)
        })
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!("Interrupted workflow worktree scan failed: {error}");
            return report;
        }
    };

    for candidate in candidates {
        let repo = crate::core::scanner::resolve_host_path(&candidate.project_path);
        let path = crate::core::scanner::resolve_host_path(&candidate.workspace_path);
        let managed_root = repo.join(".kronn").join("worktrees");
        if !path.starts_with(&managed_root) || path == managed_root {
            tracing::warn!(
                run_id = %candidate.run_id,
                path = %path.display(),
                "refusing to reclaim an Interrupted run's path outside .kronn/worktrees"
            );
            report.kept_other += 1;
            continue;
        }
        if !path.exists() {
            continue;
        }

        let (branch, head_sha, preserve) =
            match workspace::inspect_interrupted_checkout(&path).await {
                Ok(InterruptedCheckout::Removable {
                    branch,
                    head_sha,
                    preserve,
                }) => (branch, head_sha, preserve),
                Ok(InterruptedCheckout::Dirty { entries }) => {
                    tracing::warn!(
                        run_id = %candidate.run_id,
                        path = %path.display(),
                        entries,
                        "kept the worktree of a stale Interrupted run: it holds uncommitted work"
                    );
                    report.kept_dirty += 1;
                    continue;
                }
                Ok(InterruptedCheckout::Detached) => {
                    tracing::warn!(
                        run_id = %candidate.run_id,
                        path = %path.display(),
                        "kept the worktree of a stale Interrupted run: its HEAD is on no branch"
                    );
                    report.kept_other += 1;
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %candidate.run_id,
                        path = %path.display(),
                        "kept the worktree of a stale Interrupted run: {error}"
                    );
                    report.kept_other += 1;
                    continue;
                }
            };

        // Recorded before the checkout goes: the branch is then the only trace.
        if let Some(preserved) = &preserve {
            let produced = ProducedBranch {
                branch_name: preserved.branch_name.clone(),
                head_sha: preserved.head_sha.clone(),
                ahead: preserved.ahead,
                pushed_upstream: preserved.pushed_upstream,
            };
            let run_id = candidate.run_id.clone();
            let workspace_path = candidate.workspace_path.clone();
            let recorded = db
                .with_conn(move |conn| {
                    crate::db::workflows::record_reclaimed_interrupted_branch(
                        conn,
                        &run_id,
                        &workspace_path,
                        &produced,
                    )
                })
                .await;
            match recorded {
                Ok(true) => {}
                Ok(false) => {
                    report.kept_other += 1;
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %candidate.run_id,
                        "kept a stale Interrupted worktree: its branch could not be recorded: {error}"
                    );
                    report.kept_other += 1;
                    continue;
                }
            }
        }

        if let Err(error) = workspace::remove_clean_checkout(&repo, &path).await {
            tracing::warn!(
                run_id = %candidate.run_id,
                path = %path.display(),
                "stale Interrupted worktree not reclaimed: {error}"
            );
            report.kept_other += 1;
            continue;
        }
        report.reclaimed += 1;
        if preserve.is_some() {
            report.preserved_branches += 1;
        } else if branch
            == workspace::build_branch_name(&candidate.workflow_name, &candidate.run_id)
        {
            // Only the run's own branch, and only at the commit found integrated.
            if let Err(error) = workspace::delete_branch_at(&repo, &branch, &head_sha).await {
                tracing::warn!(run_id = %candidate.run_id, "{error}");
            }
        }
        tracing::info!(
            run_id = %candidate.run_id,
            path = %path.display(),
            ttl_days,
            branch_preserved = preserve.is_some(),
            "reclaimed the worktree of an Interrupted run nobody resumed"
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RunStatus, WorkflowRun, WorkflowSafety, WorkflowTrigger};
    use crate::workflows::workspace::Workspace;
    use std::path::{Path, PathBuf};

    const TTL_DAYS: u32 = 7;
    const WORKFLOW: &str = "reclaim";

    async fn git(cwd: &Path, args: &[&str]) -> String {
        let out = crate::core::cmd::async_cmd("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    async fn branch_exists(repo: &Path, branch: &str) -> bool {
        crate::core::cmd::async_cmd("git")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(repo)
            .status()
            .await
            .unwrap()
            .success()
    }

    struct Fixture {
        db: Database,
        _dir: tempfile::TempDir,
        repo: PathBuf,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).await;
        git(&repo, &["config", "user.email", "test@kronn.local"]).await;
        git(&repo, &["config", "user.name", "test"]).await;
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        git(&repo, &["add", "."]).await;
        git(&repo, &["commit", "-q", "-m", "init"]).await;

        let db = Database::open_in_memory().unwrap();
        let now = Utc::now();
        let project: crate::models::Project = serde_json::from_value(serde_json::json!({
            "id": "proj-reclaim", "name": "Reclaim",
            "path": repo.to_string_lossy(),
            "repo_url": null, "token_override": null,
            "ai_config": {"detected": false, "configs": []},
            "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(),
        }))
        .unwrap();
        let workflow = crate::models::Workflow {
            pinned: false,
            id: "wf-reclaim".into(),
            name: WORKFLOW.into(),
            project_id: Some(project.id.clone()),
            trigger: WorkflowTrigger::Manual,
            steps: vec![],
            actions: vec![],
            safety: WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            concurrency_key: None,
            guards: None,
            artifacts: Default::default(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        db.with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &project)?;
            crate::db::workflows::insert_workflow(conn, &workflow)
        })
        .await
        .unwrap();
        Fixture {
            db,
            _dir: dir,
            repo,
        }
    }

    fn run_row(run_id: &str, status: RunStatus, age_days: i64, path: &Path) -> WorkflowRun {
        let finished = Utc::now() - chrono::Duration::days(age_days);
        WorkflowRun {
            id: run_id.into(),
            workflow_id: "wf-reclaim".into(),
            status,
            trigger_context: None,
            step_results: vec![],
            tokens_used: 0,
            workspace_path: Some(path.to_string_lossy().to_string()),
            started_at: finished - chrono::Duration::hours(1),
            finished_at: Some(finished),
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_no_response: 0,
            batch_name: None,
            parent_run_id: None,
            state: Default::default(),
            produced_branches: vec![],
            parent_workflow_id: None,
            parent_workflow_name: None,
            parent_run_started_at: None,
            concurrency_key: None,
            triggered_by_run_id: None,
        }
    }

    async fn insert(fixture: &Fixture, run: WorkflowRun) {
        fixture
            .db
            .with_conn(move |conn| crate::db::workflows::insert_run(conn, &run))
            .await
            .unwrap();
    }

    /// A run interrupted `age_days` ago, its worktree left on disk as a crash leaves it.
    async fn interrupted(fixture: &Fixture, run_id: &str, age_days: i64) -> Workspace {
        let ws = Workspace::create(&fixture.repo, WORKFLOW, run_id, None, None)
            .await
            .unwrap();
        insert(
            fixture,
            run_row(run_id, RunStatus::Interrupted, age_days, &ws.path),
        )
        .await;
        ws
    }

    async fn stored_run(fixture: &Fixture, run_id: &str) -> WorkflowRun {
        let run_id = run_id.to_string();
        fixture
            .db
            .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id))
            .await
            .unwrap()
            .unwrap()
    }

    async fn reclaim(fixture: &Fixture, ttl_days: u32) -> ReclaimReport {
        reclaim_stale_interrupted_worktrees(&fixture.db, ttl_days, Utc::now()).await
    }

    #[tokio::test]
    async fn an_interrupted_worktree_older_than_its_lifetime_is_reclaimed_at_boot() {
        let fixture = fixture().await;
        let ws = interrupted(&fixture, "aaaa1111-old", 10).await;

        let report = reclaim(&fixture, TTL_DAYS).await;

        assert_eq!(
            report,
            ReclaimReport {
                reclaimed: 1,
                ..Default::default()
            }
        );
        assert!(!ws.path.exists(), "the checkout is given back");
        assert!(
            !git(&fixture.repo, &["worktree", "list", "--porcelain"])
                .await
                .contains(&*ws.path.to_string_lossy()),
            "git no longer lists it"
        );
        assert!(
            !branch_exists(&fixture.repo, &ws.branch).await,
            "a branch with nothing of its own goes with it"
        );
        let run = stored_run(&fixture, "aaaa1111-old").await;
        assert_eq!(run.status, RunStatus::Interrupted, "the run record stays");
        assert_eq!(
            run.workspace_path.as_deref(),
            Some(&*ws.path.to_string_lossy()),
            "kept so a resume is refused instead of running in the main checkout"
        );
        assert!(run.produced_branches.is_empty());
    }

    #[tokio::test]
    async fn a_recent_interrupted_run_keeps_its_worktree_resumable() {
        let fixture = fixture().await;
        let recent = interrupted(&fixture, "bbbb2222-recent", 2).await;
        let old = interrupted(&fixture, "cccc3333-old", 30).await;

        let report = reclaim(&fixture, TTL_DAYS).await;

        assert_eq!(report.reclaimed, 1, "{report:?}");
        assert!(!old.path.exists());
        assert!(
            recent.path.exists(),
            "within its lifetime it stays resumable"
        );
        assert!(branch_exists(&fixture.repo, &recent.branch).await);
        assert_eq!(
            reclaim(&fixture, TTL_DAYS).await,
            ReclaimReport::default(),
            "the next boot finds nothing left to do"
        );
    }

    #[tokio::test]
    async fn a_dirty_interrupted_worktree_is_never_reclaimed() {
        let fixture = fixture().await;
        let untracked = interrupted(&fixture, "dddd4444-new-file", 30).await;
        std::fs::write(untracked.path.join("notes.md"), "draft\n").unwrap();
        let modified = interrupted(&fixture, "eeee5555-edited", 30).await;
        std::fs::write(modified.path.join("README.md"), "edited\n").unwrap();

        let report = reclaim(&fixture, TTL_DAYS).await;

        assert_eq!(
            report,
            ReclaimReport {
                kept_dirty: 2,
                ..Default::default()
            }
        );
        assert!(untracked.path.join("notes.md").exists());
        assert_eq!(
            std::fs::read_to_string(modified.path.join("README.md")).unwrap(),
            "edited\n"
        );
    }

    #[tokio::test]
    async fn unintegrated_commits_survive_on_a_preserved_branch() {
        let fixture = fixture().await;
        let ws = interrupted(&fixture, "ffff6666-commits", 30).await;
        std::fs::write(ws.path.join("feature.rs"), "fn main() {}\n").unwrap();
        git(&ws.path, &["add", "."]).await;
        git(&ws.path, &["commit", "-q", "-m", "feat: work in progress"]).await;
        let head = git(&ws.path, &["rev-parse", "HEAD"]).await;

        let report = reclaim(&fixture, TTL_DAYS).await;

        assert_eq!(
            report,
            ReclaimReport {
                reclaimed: 1,
                preserved_branches: 1,
                ..Default::default()
            }
        );
        assert!(!ws.path.exists());
        assert_eq!(
            git(&fixture.repo, &["rev-parse", &ws.branch]).await,
            head,
            "the branch still carries the commit"
        );
        let run = stored_run(&fixture, "ffff6666-commits").await;
        assert_eq!(run.produced_branches.len(), 1, "surfaced on the run");
        assert_eq!(run.produced_branches[0].branch_name, ws.branch);
        assert_eq!(run.produced_branches[0].head_sha, head);
        assert_eq!(run.produced_branches[0].ahead, 1);
    }

    #[tokio::test]
    async fn a_detached_interrupted_worktree_is_kept() {
        let fixture = fixture().await;
        let ws = interrupted(&fixture, "abab7777-detached", 30).await;
        git(&ws.path, &["checkout", "-q", "--detach"]).await;

        let report = reclaim(&fixture, TTL_DAYS).await;

        assert_eq!(report.kept_other, 1, "{report:?}");
        assert!(ws.path.exists());
    }

    #[tokio::test]
    async fn a_zero_lifetime_never_reclaims() {
        let fixture = fixture().await;
        let ws = interrupted(&fixture, "cdcd8888-ancient", 365).await;

        assert_eq!(reclaim(&fixture, 0).await, ReclaimReport::default());
        assert!(ws.path.exists());
    }

    #[tokio::test]
    async fn a_worktree_a_recent_run_still_shares_is_kept() {
        let fixture = fixture().await;
        // The parent was resumed and interrupted again; its sub-workflow child,
        // interrupted long ago, still points at the inherited worktree.
        let parent = interrupted(&fixture, "efef9999-parent", 1).await;
        let mut child = run_row("fefe0000-child", RunStatus::Interrupted, 30, &parent.path);
        child.parent_run_id = Some("efef9999-parent".into());
        child.run_type = "subworkflow".into();
        insert(&fixture, child).await;

        assert_eq!(reclaim(&fixture, TTL_DAYS).await, ReclaimReport::default());
        assert!(parent.path.exists(), "the parent can still be resumed");
    }
}
