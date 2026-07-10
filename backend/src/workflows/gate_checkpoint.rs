//! 0.8.6 (#25) — Git checkpoint commit for Gate steps.
//!
//! `gate_checkpoint_before: Some(true)` on a Gate step instructs the
//! runner to `git add -A && git commit` the project working tree
//! BEFORE pausing in `WaitingApproval`. The resulting SHA is stored
//! in the run's `state` HashMap under `checkpoint:<gate_name>` so
//! that — when the operator picks "Request Changes" and the runner
//! resumes via Goto — we can `git reset --hard <sha>` BEFORE
//! re-running the target step. The net effect is **idempotent
//! Goto loops** : the agent re-implements on a clean tree, not on
//! top of its previous cycle's partial output.
//!
//! Why a side-file module rather than inlining in gate_step.rs :
//!   1. Gate execution is sync-render-only (no I/O). Keeping the
//!      git-shelling out of that path preserves its testability.
//!   2. The reset path lives in `runner::resume_run`, several modules
//!      away from gate_step ; sharing the helper here keeps both
//!      callsites symmetric.
//!
//! All git invocations go through `core::cmd::sync_cmd` (no raw
//! `std::process::Command`) for Windows / sandbox cross-platform
//! compliance (cf. `feedback_windows_crossplatform` memory).

use std::path::Path;

/// Prefix on `WorkflowRun.state` keys storing checkpoint SHAs.
/// Format : `checkpoint:<gate_step_name>` → 40-char SHA.
pub const CHECKPOINT_STATE_PREFIX: &str = "checkpoint:";

/// Outcome of the checkpoint commit attempt — kept as a typed enum
/// so the runner can decide whether the run continues or bails.
#[derive(Debug)]
pub enum CheckpointOutcome {
    /// SHA captured ; safe to proceed into `WaitingApproval`.
    Committed { sha: String },
    /// Project_path exists but isn't a git repo. Logged + skipped ;
    /// the run continues without a checkpoint (no reset available
    /// later on Goto, but no error either — this is opt-in feature).
    NotAGitRepo,
    /// Pre-condition failed : the index already has staged changes
    /// the user is in the middle of writing. We refuse to commit
    /// those by accident. The caller surfaces this to the operator.
    StagedChangesPresent,
    /// `git add -A` or `git commit` failed. Carries the stderr
    /// snippet for the operator-facing error.
    GitCommandFailed { stderr: String },
}

/// Probe whether `project_path` is a git working tree. Cheap : a
/// `git rev-parse --is-inside-work-tree` returns "true" in ~5ms.
fn is_git_repo(project_path: &Path) -> bool {
    crate::core::cmd::sync_cmd("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// True if `git diff --cached --quiet` exits non-zero, meaning the
/// index has staged changes (user is mid-`git add`, or a previous
/// step staged something we haven't committed yet). Auto-commit
/// here would silently sweep that into the checkpoint. We refuse.
fn has_staged_changes(project_path: &Path) -> bool {
    crate::core::cmd::sync_cmd("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(project_path)
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(false)
}

/// Create the checkpoint commit. Sequence :
///   1. `git add -A`  (stage every tracked + untracked change)
///   2. `git commit -m "kronn-checkpoint: pre-<gate_name> @ <run_id>"
///        --allow-empty`
///   3. `git rev-parse HEAD`  → return SHA
///
/// `--allow-empty` is intentional : a Gate that fires with NO file
/// changes since the prior step should still get a checkpoint so the
/// reset path has a stable anchor.
pub fn commit_checkpoint(
    project_path: &Path,
    gate_step_name: &str,
    run_id: &str,
) -> CheckpointOutcome {
    if !is_git_repo(project_path) {
        return CheckpointOutcome::NotAGitRepo;
    }
    if has_staged_changes(project_path) {
        return CheckpointOutcome::StagedChangesPresent;
    }

    let add = crate::core::cmd::sync_cmd("git")
        .args(["add", "-A"])
        .current_dir(project_path)
        .output();
    if let Err(e) = add {
        return CheckpointOutcome::GitCommandFailed { stderr: e.to_string() };
    }
    if let Ok(o) = add {
        if !o.status.success() {
            return CheckpointOutcome::GitCommandFailed {
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            };
        }
    }

    let message = format!("kronn-checkpoint: pre-{gate_step_name} @ {run_id}");
    let commit = crate::core::cmd::sync_cmd("git")
        .args(["commit", "-m", &message, "--allow-empty"])
        .current_dir(project_path)
        .output();
    match commit {
        Ok(o) if !o.status.success() => {
            return CheckpointOutcome::GitCommandFailed {
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            };
        }
        Err(e) => {
            return CheckpointOutcome::GitCommandFailed { stderr: e.to_string() };
        }
        Ok(_) => {}
    }

    let head = crate::core::cmd::sync_cmd("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_path)
        .output();
    match head {
        Ok(o) if o.status.success() => {
            let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            CheckpointOutcome::Committed { sha }
        }
        Ok(o) => CheckpointOutcome::GitCommandFailed {
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => CheckpointOutcome::GitCommandFailed { stderr: e.to_string() },
    }
}

/// Reset hard to a previously-captured checkpoint SHA. Used by the
/// runner BEFORE re-firing the Goto target step on a Gate's
/// "Request Changes" decision. Destructive — caller must already
/// have verified the SHA came from the run's own `state` map (no
/// arbitrary-SHA reset).
pub fn reset_to_checkpoint(project_path: &Path, sha: &str) -> Result<(), String> {
    // TD-20260709 (C) — this can run HOURS after the checkpoint, on a tree a
    // human or another run may have touched since. Uncommitted changes are
    // not ours to destroy: refuse, the caller degrades gracefully.
    let st = crate::core::cmd::sync_cmd("git")
        .args(["status", "--porcelain"])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("git status spawn failed: {e}"))?;
    if !st.status.success() {
        return Err(format!(
            "git status failed before reset: {}",
            String::from_utf8_lossy(&st.stderr)
        ));
    }
    if !st.stdout.is_empty() {
        return Err(format!(
            "main tree has uncommitted changes not from this run — refusing `git reset --hard` \
             (TD-20260709). Dirty entries:\n{}",
            String::from_utf8_lossy(&st.stdout).trim_end()
        ));
    }
    let r = crate::core::cmd::sync_cmd("git")
        .args(["reset", "--hard", sha])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("git reset spawn failed: {e}"))?;
    if !r.status.success() {
        return Err(format!(
            "git reset --hard {sha} failed: {}",
            String::from_utf8_lossy(&r.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_repo() -> PathBuf {
        let tmp = std::env::temp_dir().join(format!(
            "kronn-checkpoint-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&tmp).unwrap();
        // Initialize git + commit-author config so commit doesn't fail
        // in CI sandboxes that have no global git identity.
        crate::core::cmd::sync_cmd("git")
            .args(["init", "-q"])
            .current_dir(&tmp)
            .output()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["config", "user.email", "test@kronn.local"])
            .current_dir(&tmp)
            .output()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["config", "user.name", "Kronn Test"])
            .current_dir(&tmp)
            .output()
            .unwrap();
        // Seed with one commit so HEAD exists.
        fs::write(tmp.join("README.md"), "init\n").unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["add", "."])
            .current_dir(&tmp)
            .output()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&tmp)
            .output()
            .unwrap();
        tmp
    }

    #[test]
    fn commit_checkpoint_in_non_git_dir_returns_not_a_git_repo() {
        let tmp = std::env::temp_dir().join(format!(
            "kronn-checkpoint-nogit-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&tmp).unwrap();

        let out = commit_checkpoint(&tmp, "pre-merge", "run-abc");
        assert!(matches!(out, CheckpointOutcome::NotAGitRepo));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn commit_checkpoint_captures_sha_on_clean_repo() {
        let tmp = tmp_repo();
        // Create one new file to checkpoint.
        fs::write(tmp.join("agent-output.md"), "agent wrote this\n").unwrap();

        let out = commit_checkpoint(&tmp, "pre-merge", "run-xyz");
        let sha = match out {
            CheckpointOutcome::Committed { sha } => sha,
            other => panic!("expected Committed, got {other:?}"),
        };
        assert_eq!(sha.len(), 40, "SHA must be 40 hex chars, got {sha}");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));

        // HEAD message should carry our prefix.
        let log = crate::core::cmd::sync_cmd("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(&tmp)
            .output()
            .unwrap();
        let msg = String::from_utf8_lossy(&log.stdout);
        assert!(msg.contains("kronn-checkpoint: pre-pre-merge"));
        assert!(msg.contains("run-xyz"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn commit_checkpoint_allows_empty_when_no_pending_changes() {
        // Gate fires immediately after an Exec step that produced
        // nothing on disk → no agent_output file → empty diff. We
        // still want a stable SHA anchor for the reset path. The
        // helper uses `--allow-empty` for exactly this.
        let tmp = tmp_repo();
        let out = commit_checkpoint(&tmp, "review-gate", "run-1");
        assert!(matches!(out, CheckpointOutcome::Committed { .. }));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn commit_checkpoint_refuses_when_index_has_staged_changes() {
        // User was mid-`git add` when the Kronn run hit the Gate.
        // Auto-committing here would sweep their WIP into the
        // checkpoint commit. We refuse + surface a typed reason.
        let tmp = tmp_repo();
        fs::write(tmp.join("wip.txt"), "human WIP\n").unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["add", "wip.txt"])
            .current_dir(&tmp)
            .output()
            .unwrap();

        let out = commit_checkpoint(&tmp, "pre-merge", "run-1");
        assert!(matches!(out, CheckpointOutcome::StagedChangesPresent));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reset_to_checkpoint_rolls_back_subsequent_commits() {
        let tmp = tmp_repo();
        // Take a checkpoint, then mutate + commit, then reset.
        let sha = match commit_checkpoint(&tmp, "g1", "run-1") {
            CheckpointOutcome::Committed { sha } => sha,
            other => panic!("expected Committed, got {other:?}"),
        };
        fs::write(tmp.join("oops.txt"), "after\n").unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["add", "."])
            .current_dir(&tmp)
            .output()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["commit", "-q", "-m", "after-checkpoint"])
            .current_dir(&tmp)
            .output()
            .unwrap();

        reset_to_checkpoint(&tmp, &sha).expect("reset must succeed");
        // `oops.txt` is gone, HEAD is back at the checkpoint SHA.
        assert!(!tmp.join("oops.txt").exists(), "post-reset file must be removed");
        let head = crate::core::cmd::sync_cmd("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&tmp)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            sha,
            "HEAD must point at the checkpoint sha after reset",
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reset_refuses_when_tree_has_uncommitted_changes() {
        // TD-20260709 (C): a deferred reset must never destroy WIP the run
        // didn't create.
        let tmp = tmp_repo();
        let sha = match commit_checkpoint(&tmp, "g1", "run-1") {
            CheckpointOutcome::Committed { sha } => sha,
            other => panic!("expected Committed, got {other:?}"),
        };
        fs::write(tmp.join("wip.txt"), "human work in progress").unwrap();
        let err = reset_to_checkpoint(&tmp, &sha).unwrap_err();
        assert!(err.contains("uncommitted changes"), "must name the refusal reason: {err}");
        assert_eq!(
            fs::read_to_string(tmp.join("wip.txt")).unwrap(),
            "human work in progress",
            "the dirty file must be untouched"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reset_to_checkpoint_returns_err_on_bogus_sha() {
        let tmp = tmp_repo();
        let err = reset_to_checkpoint(&tmp, "deadbeefnotreal").unwrap_err();
        assert!(err.contains("git reset --hard"));
        let _ = fs::remove_dir_all(&tmp);
    }
}
