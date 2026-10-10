//! 0.8.6 (#25) — Git checkpoint commit for Gate steps.
//!
//! `gate_checkpoint_before: Some(true)` on a Gate step instructs the
//! runner to `git add -A && git commit` the run's isolated worktree
//! BEFORE pausing in `WaitingApproval`. A run without its own worktree
//! (shared workspace mode) gets no checkpoint: the sweep would commit
//! the operator's own work on their branch (KT-1042). The resulting SHA
//! is stored in the run's `state` HashMap under `checkpoint:<gate_name>`.
//! The checkpoint captures the POST-implementation state the operator
//! reviews: on "Request Changes" the target step re-runs on top of that
//! committed work, it does not go back to the tree from before the
//! implementation. The resume first verifies the worktree is still
//! exactly at the checkpoint (see [`verify_checkpoint`]).
//!
//! Why a side-file module rather than inlining in gate_step.rs :
//!   1. Gate execution is sync-render-only (no I/O). Keeping the
//!      git-shelling out of that path preserves its testability.
//!   2. The verification lives in `runner::resume_run`, several modules
//!      away from gate_step ; sharing the helpers here keeps both
//!      callsites symmetric.
//!
//! All git invocations go through the local `git_cmd` helper (never
//! `core::cmd::sync_cmd("git")` or raw `std::process::Command` directly):
//! it applies Windows/sandbox cross-platform compliance (cf.
//! `feedback_windows_crossplatform` memory) AND strips every inherited
//! `GIT_*` repo-override env var so `current_dir` is never overruled by
//! the calling process's own environment (KT-493).

use std::path::Path;

/// Prefix on `WorkflowRun.state` keys storing checkpoint SHAs.
/// Format : `checkpoint:<gate_step_name>` → 40-char SHA.
pub const CHECKPOINT_STATE_PREFIX: &str = "checkpoint:";

/// Env vars Git consults to locate the repository/index, independent of
/// `current_dir`/`-C`. A process that inherits any of these (KT-493 : the
/// orchestrated task worktree this module's own unit tests ran inside)
/// silently redirects every git invocation below onto the repo THEY name,
/// not `project_path` — that's how `commit_checkpoint`'s "not a git repo"
/// test landed a real `kronn-checkpoint: pre-pre-merge @ run-abc` commit on
/// the live task branch instead of its throwaway temp dir.
const GIT_ENV_OVERRIDES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// `sync_cmd("git")` scoped to `project_path`, with every inherited
/// repo-override env var stripped. Repository discovery is capped at the
/// requested path's parent so a temporary non-repository nested below the
/// caller cannot fall back to the caller's `.git`. Every git invocation in
/// this module — production and test alike — must go through this, never
/// `sync_cmd("git")` directly.
pub(crate) fn git_cmd(project_path: &Path) -> std::process::Command {
    let mut cmd = crate::core::cmd::git_cmd();
    cmd.current_dir(project_path);
    for var in GIT_ENV_OVERRIDES {
        cmd.env_remove(var);
    }
    if let Ok(canonical_path) = project_path.canonicalize() {
        if let Some(parent) = canonical_path.parent() {
            cmd.env("GIT_CEILING_DIRECTORIES", parent);
        }
    }
    cmd
}

/// Outcome of the checkpoint commit attempt — kept as a typed enum
/// so the runner can decide whether the run continues or bails.
#[derive(Debug)]
pub enum CheckpointOutcome {
    /// SHA captured ; safe to proceed into `WaitingApproval`.
    Committed { sha: String },
    /// Project_path exists but isn't a git repo. Logged + skipped ;
    /// the run continues without a checkpoint (nothing to verify
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
    git_cmd(project_path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// True if `git diff --cached --quiet` exits non-zero, meaning the
/// index has staged changes (user is mid-`git add`, or a previous
/// step staged something we haven't committed yet). Auto-commit
/// here would silently sweep that into the checkpoint. We refuse.
fn has_staged_changes(project_path: &Path) -> bool {
    git_cmd(project_path)
        .args(["diff", "--cached", "--quiet"])
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
/// resume has a stable anchor to verify.
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

    let add = git_cmd(project_path).args(["add", "-A"]).output();
    if let Err(e) = add {
        return CheckpointOutcome::GitCommandFailed {
            stderr: e.to_string(),
        };
    }
    if let Ok(o) = add {
        if !o.status.success() {
            return CheckpointOutcome::GitCommandFailed {
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            };
        }
    }

    let message = format!("kronn-checkpoint: pre-{gate_step_name} @ {run_id}");
    let commit = git_cmd(project_path)
        .args(["commit", "-m", &message, "--allow-empty"])
        .output();
    match commit {
        Ok(o) if !o.status.success() => {
            return CheckpointOutcome::GitCommandFailed {
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            };
        }
        Err(e) => {
            return CheckpointOutcome::GitCommandFailed {
                stderr: e.to_string(),
            };
        }
        Ok(_) => {}
    }

    let head = git_cmd(project_path).args(["rev-parse", "HEAD"]).output();
    match head {
        Ok(o) if o.status.success() => {
            let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            CheckpointOutcome::Committed { sha }
        }
        Ok(o) => CheckpointOutcome::GitCommandFailed {
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => CheckpointOutcome::GitCommandFailed {
            stderr: e.to_string(),
        },
    }
}

/// Takes the Gate checkpoint in the run's own worktree and records its SHA,
/// or returns the notice to show the operator when none was taken.
pub fn checkpoint_gate(
    run_worktree: Option<&str>,
    run_id: &str,
    run_state: &mut std::collections::HashMap<String, String>,
    gate_step_name: &str,
) -> Option<String> {
    let key = format!("{CHECKPOINT_STATE_PREFIX}{gate_step_name}");
    // A failed checkpoint must not leave an earlier cycle's SHA to verify against.
    run_state.remove(&key);
    let Some(worktree) = run_worktree else {
        return Some(SHARED_MODE_NOTICE.to_string());
    };
    match commit_checkpoint(Path::new(worktree), gate_step_name, run_id) {
        CheckpointOutcome::Committed { sha } => {
            run_state.insert(key, sha);
            None
        }
        CheckpointOutcome::NotAGitRepo => Some(format!(
            "Checkpoint not taken: the run's worktree `{worktree}` is not a git repository."
        )),
        CheckpointOutcome::StagedChangesPresent => {
            Some("Checkpoint not taken: the run's worktree has staged changes.".to_string())
        }
        CheckpointOutcome::GitCommandFailed { stderr } => Some(format!(
            "Checkpoint not taken: git failed in the run's worktree: {}",
            stderr.trim()
        )),
    }
}

/// Shown on a Gate whose run works in the operator's checkout.
pub const SHARED_MODE_NOTICE: &str = "Checkpoint not taken: gate_checkpoint_before only applies \
to a run with its own worktree (workspace isolation). In shared mode it would commit the \
operator's checkout.";

/// Checks, before "Request Changes" re-runs its target, that the run's
/// worktree is still exactly the checkpoint: HEAD on its SHA and a clean
/// tree. Nothing is reset; a mismatch means someone else changed it.
pub fn verify_checkpoint(worktree: &Path, sha: &str) -> Result<(), String> {
    // The run is paused between its checkpoint and this check, so any commit
    // past the checkpoint is someone else's.
    let head = git_cmd(worktree)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("git rev-parse spawn failed: {e}"))?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if head != sha {
        return Err(format!(
            "HEAD is `{head}`, no longer the checkpoint `{sha}`: the branch received commits \
             not made by this run"
        ));
    }
    let st = git_cmd(worktree)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| format!("git status spawn failed: {e}"))?;
    if !st.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&st.stderr)
        ));
    }
    if !st.stdout.is_empty() {
        return Err(format!(
            "the worktree has uncommitted changes not from this run (TD-20260709). \
             Dirty entries:\n{}",
            String::from_utf8_lossy(&st.stdout).trim_end()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const INHERITED_CONTEXT_TARGET_ENV: &str = "KRONN_GATE_CHECKPOINT_TEST_TARGET";

    fn tmp_repo() -> PathBuf {
        let tmp =
            std::env::temp_dir().join(format!("kronn-checkpoint-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        // Initialize git + commit-author config so commit doesn't fail
        // in CI sandboxes that have no global git identity.
        git_cmd(&tmp).args(["init", "-q"]).output().unwrap();
        git_cmd(&tmp)
            .args(["config", "user.email", "test@kronn.local"])
            .output()
            .unwrap();
        git_cmd(&tmp)
            .args(["config", "user.name", "Kronn Test"])
            .output()
            .unwrap();
        // Seed with one commit so HEAD exists.
        fs::write(tmp.join("README.md"), "init\n").unwrap();
        git_cmd(&tmp).args(["add", "."]).output().unwrap();
        git_cmd(&tmp)
            .args(["commit", "-q", "-m", "init"])
            .output()
            .unwrap();
        tmp
    }

    /// HEAD sha of `repo`, via a fresh env-scrubbed git call — used to prove
    /// a repo untouched across a `commit_checkpoint` call elsewhere.
    fn git_head(repo: &Path) -> String {
        let out = git_cmd(repo).args(["rev-parse", "HEAD"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `git log --pretty=%s` subjects of `repo`, newest first.
    fn git_log_subjects(repo: &Path) -> Vec<String> {
        let out = git_cmd(repo).args(["log", "--pretty=%s"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn git_status(repo: &Path) -> Vec<u8> {
        let out = git_cmd(repo)
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
    }

    /// Semantic snapshot of the index. Unlike reading `.git/index` directly,
    /// this also works for linked worktrees and ignores harmless stat-cache
    /// refreshes while still detecting any accidental `git add`.
    fn git_index_tree(repo: &Path) -> Vec<u8> {
        let out = git_cmd(repo).args(["write-tree"]).output().unwrap();
        assert!(out.status.success());
        out.stdout
    }

    /// Tracked worktree changes relative to the index. Untracked paths are
    /// represented separately by `git_status` above.
    fn git_worktree_diff(repo: &Path) -> Vec<u8> {
        let out = git_cmd(repo)
            .args(["diff", "--binary", "--no-ext-diff"])
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
    }

    #[test]
    fn commit_checkpoint_in_non_git_dir_returns_not_a_git_repo() {
        let tmp =
            std::env::temp_dir().join(format!("kronn-checkpoint-nogit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let probe = git_cmd(&tmp)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .unwrap();
        assert!(
            !probe.status.success(),
            "temporary non-repo unexpectedly resolved to {} from {}",
            String::from_utf8_lossy(&probe.stdout).trim(),
            tmp.display(),
        );

        let out = commit_checkpoint(&tmp, "pre-merge", "run-abc");
        assert!(
            matches!(out, CheckpointOutcome::NotAGitRepo),
            "expected NotAGitRepo, got {out:?}"
        );

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
        let log = git_cmd(&tmp)
            .args(["log", "-1", "--pretty=%s"])
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
        git_cmd(&tmp).args(["add", "wip.txt"]).output().unwrap();

        let out = commit_checkpoint(&tmp, "pre-merge", "run-1");
        assert!(matches!(out, CheckpointOutcome::StagedChangesPresent));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn verify_refuses_when_the_branch_received_commits_after_the_checkpoint() {
        let tmp = tmp_repo();
        let sha = match commit_checkpoint(&tmp, "g1", "run-1") {
            CheckpointOutcome::Committed { sha } => sha,
            other => panic!("expected Committed, got {other:?}"),
        };
        fs::write(tmp.join("operator.txt"), "operator commit\n").unwrap();
        git_cmd(&tmp).args(["add", "."]).output().unwrap();
        git_cmd(&tmp)
            .args(["commit", "-q", "-m", "operator work after the checkpoint"])
            .output()
            .unwrap();
        let head_before = git_head(&tmp);

        let err = verify_checkpoint(&tmp, &sha).unwrap_err();
        assert!(err.contains("not made by this run"), "{err}");
        assert_eq!(git_head(&tmp), head_before, "the foreign commit stays");
        assert!(tmp.join("operator.txt").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn verify_succeeds_when_head_is_the_clean_checkpoint() {
        let tmp = tmp_repo();
        let sha = match commit_checkpoint(&tmp, "g1", "run-1") {
            CheckpointOutcome::Committed { sha } => sha,
            other => panic!("expected Committed, got {other:?}"),
        };
        verify_checkpoint(&tmp, &sha).expect("the worktree is the checkpoint");
        assert_eq!(git_head(&tmp), sha);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn checkpoint_gate_without_a_worktree_commits_nothing_and_drops_a_stale_sha() {
        let mut run: crate::models::WorkflowRun = serde_json::from_value(serde_json::json!({
            "id": "run-shared", "workflow_id": "wf", "status": "Running",
            "step_results": [], "tokens_used": 0, "started_at": chrono::Utc::now(),
        }))
        .unwrap();
        run.state
            .insert(format!("{CHECKPOINT_STATE_PREFIX}review"), "a".repeat(40));
        let notice = checkpoint_gate(None, &run.id, &mut run.state, "review").expect("a notice");
        assert!(notice.contains("shared mode"), "{notice}");
        assert!(run.state.is_empty(), "{:?}", run.state);
    }

    #[test]
    fn checkpoint_gate_commits_in_the_run_worktree_only() {
        let worktree = tmp_repo();
        fs::write(worktree.join("agent-output.md"), "run output\n").unwrap();
        let mut run: crate::models::WorkflowRun = serde_json::from_value(serde_json::json!({
            "id": "run-isolated", "workflow_id": "wf", "status": "Running",
            "step_results": [], "tokens_used": 0, "started_at": chrono::Utc::now(),
            "workspace_path": worktree.to_string_lossy(),
        }))
        .unwrap();
        assert_eq!(
            checkpoint_gate(
                run.workspace_path.as_deref(),
                &run.id,
                &mut run.state,
                "review"
            ),
            None
        );
        let sha = run
            .state
            .get(&format!("{CHECKPOINT_STATE_PREFIX}review"))
            .unwrap();
        assert_eq!(&git_head(&worktree), sha);
        assert!(
            git_status(&worktree).is_empty(),
            "the run's output was committed"
        );
        let _ = fs::remove_dir_all(&worktree);
    }

    #[test]
    fn verify_refuses_when_tree_has_uncommitted_changes() {
        // TD-20260709 (C): WIP the run didn't create is reported, never
        // touched.
        let tmp = tmp_repo();
        let sha = match commit_checkpoint(&tmp, "g1", "run-1") {
            CheckpointOutcome::Committed { sha } => sha,
            other => panic!("expected Committed, got {other:?}"),
        };
        fs::write(tmp.join("wip.txt"), "human work in progress").unwrap();
        let err = verify_checkpoint(&tmp, &sha).unwrap_err();
        assert!(
            err.contains("uncommitted changes"),
            "must name the refusal reason: {err}"
        );
        assert_eq!(
            fs::read_to_string(tmp.join("wip.txt")).unwrap(),
            "human work in progress",
            "the dirty file must be untouched"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn verify_returns_err_on_bogus_sha() {
        let tmp = tmp_repo();
        let err = verify_checkpoint(&tmp, "deadbeefnotreal").unwrap_err();
        assert!(err.contains("no longer the checkpoint"), "{err}");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// KT-493 regression: reproduce both observed incidents with a simulated
    /// managed-worktree caller: an empty checkpoint must not add a commit to a
    /// clean caller, and a caller's uncommitted files must not be captured.
    #[test]
    fn commit_checkpoint_ignores_inherited_git_dir_and_work_tree() {
        // The parent test starts this same test in a dedicated process with a
        // simulated inherited Git context. Keeping that environment mutation
        // in the child avoids redirecting Git commands from concurrently
        // running tests in this test binary.
        if let Some(target) = crate::core::child_env::var_os(INHERITED_CONTEXT_TARGET_ENV) {
            let target = PathBuf::from(target);
            let out = commit_checkpoint(&target, "pre-merge", "run-abc");
            assert!(
                matches!(out, CheckpointOutcome::Committed { .. }),
                "expected the checkpoint to land in the target repo, got {out:?}"
            );
            return;
        }

        for caller_has_wip in [false, true] {
            let calling_repo = tmp_repo();
            let target = tmp_repo();
            if caller_has_wip {
                fs::write(calling_repo.join("README.md"), "worker modified this\n").unwrap();
                fs::write(calling_repo.join("wip.txt"), "worker's uncommitted work\n").unwrap();
            }
            let head_before = git_head(&calling_repo);
            let index_before = git_index_tree(&calling_repo);
            let worktree_before = git_worktree_diff(&calling_repo);
            let status_before = git_status(&calling_repo);
            let readme_before = fs::read(calling_repo.join("README.md")).unwrap();
            let wip_before = fs::read(calling_repo.join("wip.txt")).ok();

            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("commit_checkpoint_ignores_inherited_git_dir_and_work_tree")
                .arg("--nocapture")
                .env(INHERITED_CONTEXT_TARGET_ENV, &target)
                .env("GIT_DIR", calling_repo.join(".git"))
                .env("GIT_WORK_TREE", &calling_repo)
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "checkpoint child failed (caller_has_wip={caller_has_wip}):\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr),
            );
            assert_eq!(
                git_log_subjects(&target).first().map(String::as_str),
                Some("kronn-checkpoint: pre-pre-merge @ run-abc"),
                "the run-abc checkpoint must exist only in the target repo"
            );
            assert_eq!(
                git_head(&calling_repo),
                head_before,
                "calling repo HEAD must not move (caller_has_wip={caller_has_wip})"
            );
            assert_eq!(
                git_index_tree(&calling_repo),
                index_before,
                "calling repo index must not change (caller_has_wip={caller_has_wip})"
            );
            assert_eq!(
                git_worktree_diff(&calling_repo),
                worktree_before,
                "calling repo tracked worktree diff must not change (caller_has_wip={caller_has_wip})"
            );
            assert_eq!(
                git_status(&calling_repo),
                status_before,
                "calling repo path/status state must not change (caller_has_wip={caller_has_wip})"
            );
            assert!(
                !git_log_subjects(&calling_repo)
                    .iter()
                    .any(|subject| subject.contains("run-abc")),
                "run-abc must never appear in the calling repo"
            );
            assert_eq!(
                fs::read(calling_repo.join("README.md")).unwrap(),
                readme_before
            );
            assert_eq!(fs::read(calling_repo.join("wip.txt")).ok(), wip_before);

            let _ = fs::remove_dir_all(&calling_repo);
            let _ = fs::remove_dir_all(&target);
        }
    }
}
