//! Per-run artifacts directory (KT-910), exposed as `{{run.artifacts_dir}}`.
//!
//! Exec steps write there (screenshots, logs, large reports); an Agent step
//! whose prompt names it reads it read-only through the `read_only_repos`
//! policy (`--add-dir`, `Edit` denied, mandatory sandbox). It lives outside
//! every checkout, so it never overlaps the agent's writable directory.
//!
//! Retention: the directory lives as long as its run can still execute. It
//! is removed when the run ends (any terminal status, the same moment its
//! worktree goes), and at startup for a run that ended, no longer exists, or
//! stayed Interrupted beyond `interrupted_worktree_ttl_days`.

use std::path::PathBuf;

use crate::models::RunStatus;

const DIR_NAME: &str = "run-artifacts";

/// Where every run's directory lives.
pub fn root() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(std::env::temp_dir().join(format!("kronn-test-{DIR_NAME}-{}", std::process::id())))
    }
    #[cfg(not(test))]
    {
        crate::core::config::config_dir()
            .ok()
            .map(|dir| dir.join(DIR_NAME))
    }
}

/// A run id names a directory only when it cannot leave the root.
fn safe_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn dir_for(run_id: &str) -> Option<PathBuf> {
    safe_run_id(run_id).then(|| root().map(|root| root.join(run_id)))?
}

/// Creates the run's directory (owner-only) and returns its canonical path.
pub fn ensure(run_id: &str) -> std::io::Result<PathBuf> {
    let dir = dir_for(run_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "run id cannot name an artifacts directory",
        )
    })?;
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    dir.canonicalize()
}

/// Removes the run's directory; a missing one is not an error.
pub fn remove(run_id: &str) {
    let Some(dir) = dir_for(run_id) else {
        return;
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            run_id,
            path = %dir.display(),
            "run artifacts directory not removed: {error}"
        ),
    }
}

/// Whether a run in `status` may still execute and read its directory.
pub fn keeps_directory(
    status: &RunStatus,
    started_at: chrono::DateTime<chrono::Utc>,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    match status {
        RunStatus::Pending
        | RunStatus::Running
        | RunStatus::WaitingApproval
        | RunStatus::WaitingQuota => true,
        RunStatus::Interrupted => {
            now - started_at < chrono::Duration::days(i64::from(interrupted_ttl_days))
        }
        RunStatus::Success
        | RunStatus::Partial
        | RunStatus::Failed
        | RunStatus::Cancelled
        | RunStatus::StoppedByGuard => false,
    }
}

/// Startup sweep: removes the directories no run will read again. Returns
/// how many were removed.
pub async fn sweep(
    db: &crate::db::Database,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    let Some(root) = root() else {
        return 0;
    };
    sweep_root(&root, db, interrupted_ttl_days, now).await
}

async fn sweep_root(
    root: &std::path::Path,
    db: &crate::db::Database,
    interrupted_ttl_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let run_ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| safe_run_id(name))
        .collect();
    let mut removed = 0;
    for run_id in run_ids {
        let lookup = run_id.clone();
        let run = match db
            .with_read_conn(move |conn| crate::db::workflows::get_run(conn, &lookup))
            .await
        {
            Ok(run) => run,
            Err(error) => {
                tracing::warn!(%run_id, "artifacts sweep could not read the run: {error}");
                continue;
            }
        };
        let keep = run.is_some_and(|run| {
            keeps_directory(&run.status, run.started_at, interrupted_ttl_days, now)
        });
        if !keep {
            match std::fs::remove_dir_all(root.join(&run_id)) {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::warn!(%run_id, "run artifacts directory not removed: {error}")
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_run_gets_its_own_owner_only_directory() {
        let first = ensure("run-artifacts-a1").unwrap();
        let second = ensure("run-artifacts-b2").unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        remove("run-artifacts-a1");
        remove("run-artifacts-b2");
        assert!(!first.exists() && !second.exists());
        remove("run-artifacts-a1");
    }

    #[test]
    fn a_run_id_never_escapes_the_root() {
        for id in ["", "..", "../x", "a/b", "a\\b", "é"] {
            assert!(dir_for(id).is_none(), "{id:?}");
            assert!(ensure(id).is_err(), "{id:?}");
        }
    }

    #[test]
    fn the_directory_lives_as_long_as_the_run_can_execute() {
        let now = chrono::Utc::now();
        let recent = now - chrono::Duration::hours(1);
        let old = now - chrono::Duration::days(30);
        assert!(keeps_directory(&RunStatus::WaitingApproval, old, 7, now));
        assert!(keeps_directory(&RunStatus::Running, old, 7, now));
        assert!(keeps_directory(&RunStatus::Interrupted, recent, 7, now));
        assert!(!keeps_directory(&RunStatus::Interrupted, old, 7, now));
        for ended in [
            RunStatus::Success,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Partial,
            RunStatus::StoppedByGuard,
        ] {
            assert!(!keeps_directory(&ended, recent, 7, now));
        }
    }

    #[tokio::test]
    async fn the_sweep_removes_what_no_run_will_read_again() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        for id in ["run-gone", "run-paused", "run-done"] {
            std::fs::create_dir_all(root.path().join(id)).unwrap();
        }
        let workflow: crate::models::Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf-sweep", "name": "Sweep", "project_id": null,
            "trigger": {"type": "Manual"}, "steps": [], "actions": [], "safety": {},
            "workspace_config": null, "concurrency_limit": null, "enabled": true,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let run = |id: &str, status: &str| -> crate::models::WorkflowRun {
            serde_json::from_value(serde_json::json!({
                "id": id, "workflow_id": "wf-sweep", "status": status,
                "trigger_context": null, "step_results": [], "tokens_used": 0,
                "workspace_path": null, "started_at": chrono::Utc::now().to_rfc3339(),
                "finished_at": null, "run_type": "linear", "batch_total": 0,
                "batch_completed": 0, "batch_failed": 0, "batch_no_response": 0,
                "batch_name": null, "parent_run_id": null, "state": {},
                "produced_branches": [], "concurrency_key": null
            }))
            .unwrap()
        };
        let (paused, done) = (
            run("run-paused", "WaitingApproval"),
            run("run-done", "Success"),
        );
        db.with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &workflow)?;
            crate::db::workflows::insert_run(conn, &paused)?;
            crate::db::workflows::insert_run(conn, &done)
        })
        .await
        .unwrap();

        let removed = sweep_root(root.path(), &db, 7, chrono::Utc::now()).await;

        assert_eq!(removed, 2);
        assert!(root.path().join("run-paused").is_dir());
        assert!(!root.path().join("run-gone").exists());
        assert!(!root.path().join("run-done").exists());
    }
}
