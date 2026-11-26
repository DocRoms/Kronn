//! KT-2084 — deleting a discussion never destroys a task worker's checkout or
//! an unmerged branch.

use super::*;
use crate::api::discussions::crud::delete;

fn branch_exists(repo: &std::path::Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .status
    .success()
}

async fn discussion_exists(db: &Database, id: &str) -> bool {
    let id = id.to_string();
    db.with_conn(move |conn| crate::db::discussions::get_discussion(conn, &id))
        .await
        .unwrap()
        .is_some()
}

async fn delete_discussion(
    state: &AppState,
    id: &str,
) -> crate::models::ApiResponse<Option<String>> {
    let Json(response) = delete(State(state.clone()), axum::extract::Path(id.to_string())).await;
    response
}

/// A token the handler would cancel, to prove a refusal cancels nothing.
fn live_token(state: &AppState, key: &str) -> tokio_util::sync::CancellationToken {
    let token = tokio_util::sync::CancellationToken::new();
    state
        .cancel_registry
        .lock()
        .unwrap()
        .insert(key.to_string(), token.clone());
    token
}

fn assert_conflict(response: &crate::models::ApiResponse<Option<String>>, needles: &[&str]) {
    assert!(!response.success, "the delete must be refused");
    assert_eq!(response.error_code.as_deref(), Some("conflict"));
    let error = response.error.clone().unwrap_or_default();
    for needle in needles {
        assert!(error.contains(needle), "{needle:?} missing from: {error}");
    }
}

struct RunningWorker {
    repo: tempfile::TempDir,
    db: Arc<Database>,
    state: AppState,
    task: String,
    parent: String,
    child: String,
    exec_id: String,
    status: TaskExecutionStatus,
    path: String,
    branch: String,
}

/// A CLI worker attached to its execution: its session sits in the room.
async fn running_worker() -> RunningWorker {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (task, parent, child, exec_id) = attached_cli_worker(&db, repo.path()).await;
    worker_fixture(repo, db, task, parent, child, exec_id).await
}

/// A native worker: nothing but the execution row ties it to the room, so
/// deleting the room used to cascade the execution away.
async fn running_native_worker() -> RunningWorker {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (task, parent, _) = seed(&db, repo.path()).await;
    let execution = provision_single_task_execution(
        &db,
        ProvisionInput {
            task_reference: task.clone(),
            parent_discussion_id: parent.clone(),
            worker: native_worker(),
            base_rev: Some("main".into()),
            idempotency_key: Some("kt-2084".into()),
        },
    )
    .await
    .unwrap();
    let child = execution.sub_discussion_id.clone().expect("sub-discussion");
    worker_fixture(repo, db, task, parent, child, execution.id).await
}

async fn worker_fixture(
    repo: tempfile::TempDir,
    db: Arc<Database>,
    task: String,
    parent: String,
    child: String,
    exec_id: String,
) -> RunningWorker {
    let path = managed_worktree_path(&db, &exec_id).await;
    let execution = exec_of(&db, &exec_id).await;
    assert!(!execution.status.is_terminal(), "{:?}", execution.status);
    let state = state_for(&db);
    RunningWorker {
        repo,
        db,
        state,
        task,
        parent,
        child,
        exec_id,
        status: execution.status,
        path,
        branch: execution.child_branch.expect("branch"),
    }
}

impl RunningWorker {
    async fn assert_refused_and_intact(&self, deleted: &str) {
        let token = live_token(&self.state, deleted);
        let response = delete_discussion(&self.state, deleted).await;

        assert_conflict(&response, &[&self.exec_id, &self.task, "cancel"]);
        assert!(
            !token.is_cancelled(),
            "refused before anything is cancelled"
        );
        assert!(discussion_exists(&self.db, deleted).await);
        assert_eq!(exec_of(&self.db, &self.exec_id).await.status, self.status);
        assert!(std::path::Path::new(&self.path).exists(), "checkout intact");
        assert!(
            branch_exists(self.repo.path(), &self.branch),
            "branch intact"
        );
    }

    /// Finished long ago, its agent runs ended, nothing written in its checkout
    /// for two days: what a delete may release.
    async fn finished_long_ago(&self, status: &'static str) {
        force_status(&self.db, &self.exec_id, status).await;
        let child = self.child.clone();
        self.db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE agent_dispatch_jobs SET status = 'Cancelled'
                      WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')",
                    [&child],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        age_checkout(std::path::Path::new(&self.path));
    }

    fn assert_checkout_intact(&self, file: &str, content: &str) {
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&self.path).join(file)).unwrap(),
            content
        );
        assert!(branch_exists(self.repo.path(), &self.branch));
    }

    async fn workspace_row(&self) -> (String, Option<String>) {
        let child = self.child.clone();
        self.db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT state, canonical_path FROM discussion_workspaces
                      WHERE disc_id = ?1 AND ownership = 'managed'",
                    [&child],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .await
            .unwrap()
    }

    /// The room deleted the way the handler did before KT-2084: the execution
    /// row cascades away and its checkout is left as an orphan.
    async fn orphan_the_checkout(&self) {
        let parent = self.parent.clone();
        assert!(self
            .db
            .with_conn(move |conn| crate::db::discussions::delete_discussion(conn, &parent))
            .await
            .unwrap());
    }
}

#[tokio::test]
async fn deleting_the_sub_discussion_of_a_running_execution_is_refused() {
    let fx = running_worker().await;
    fx.assert_refused_and_intact(&fx.child).await;
}

#[tokio::test]
async fn deleting_the_room_of_a_running_execution_is_refused() {
    let fx = running_native_worker().await;
    fx.assert_refused_and_intact(&fx.parent).await;
}

#[tokio::test]
async fn deleting_the_room_of_a_cli_worker_execution_is_refused() {
    let fx = running_worker().await;
    fx.assert_refused_and_intact(&fx.parent).await;
}

#[tokio::test]
async fn deleting_the_room_of_a_cancelled_execution_releases_its_clean_checkout() {
    let fx = running_native_worker().await;
    fx.finished_long_ago("Cancelled").await;

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&fx.db, &fx.parent).await);
    assert!(
        !std::path::Path::new(&fx.path).exists(),
        "checkout released"
    );
    assert!(branch_exists(fx.repo.path(), &fx.branch), "branch kept");
    assert_eq!(
        fx.workspace_row().await,
        ("detached".to_string(), None),
        "workspace row retired"
    );
}

#[tokio::test]
async fn deleting_the_sub_discussion_of_a_done_execution_saves_its_diff_and_keeps_its_branch() {
    let fx = running_worker().await;
    std::fs::write(std::path::Path::new(&fx.path).join("README.md"), "# edited").unwrap();
    fx.finished_long_ago("Done").await;
    worker_session_left(&fx.db).await;
    let _ = std::fs::remove_dir_all(terminal_evidence_dir(&fx.exec_id).unwrap());

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&fx.db, &fx.child).await);
    assert!(
        !std::path::Path::new(&fx.path).exists(),
        "checkout released"
    );
    assert!(branch_exists(fx.repo.path(), &fx.branch), "branch kept");
    let patch = terminal_evidence_dir(&fx.exec_id)
        .unwrap()
        .join(worktree::RELEASED_WORKTREE_PATCH);
    let saved = std::fs::read_to_string(&patch).expect("diff saved as evidence");
    assert!(saved.contains("+# edited"), "{saved}");
    let execution = exec_of(&fx.db, &fx.exec_id).await;
    assert_eq!(execution.status, TaskExecutionStatus::Done);
    assert_eq!(execution.sub_discussion_id, None);
}

/// A finished worker checkout holding an ignored `.env` and an ignored `bindings/` folder.
async fn finished_with_ignored_files() -> RunningWorker {
    let fx = running_native_worker().await;
    ignore_in_repo(fx.repo.path(), ".env");
    ignore_in_repo(fx.repo.path(), "bindings/");
    let checkout = std::path::Path::new(&fx.path);
    std::fs::write(checkout.join(".env"), "TOKEN=1").unwrap();
    std::fs::create_dir_all(checkout.join("backend/bindings")).unwrap();
    std::fs::write(checkout.join("backend/bindings/Task.ts"), "export {};").unwrap();
    fx.finished_long_ago("Cancelled").await;
    let _ = std::fs::remove_dir_all(terminal_evidence_dir(&fx.exec_id).unwrap());
    fx
}

fn assert_ignored_files_kept_as_evidence(fx: &RunningWorker) {
    assert!(
        !std::path::Path::new(&fx.path).exists(),
        "checkout released"
    );
    assert!(branch_exists(fx.repo.path(), &fx.branch), "branch kept");
    let copied = terminal_evidence_dir(&fx.exec_id).unwrap().join("ignored");
    assert_eq!(std::fs::read(copied.join(".env")).unwrap(), b"TOKEN=1");
    assert_eq!(
        std::fs::read(copied.join("backend/bindings/Task.ts")).unwrap(),
        b"export {};"
    );
}

/// KT-2084 — a finished checkout's ignored files (a `.env`, generated bindings) are copied
/// into its evidence, then the delete releases it.
#[tokio::test]
async fn a_finished_checkout_holding_ignored_files_is_released_by_the_delete_with_a_copy() {
    let fx = finished_with_ignored_files().await;

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&fx.db, &fx.parent).await);
    assert_ignored_files_kept_as_evidence(&fx);
}

/// The sweep does the same: the checkout no longer piles up.
#[tokio::test]
async fn a_finished_checkout_holding_ignored_files_is_released_by_the_sweep_with_a_copy() {
    let fx = finished_with_ignored_files().await;

    let mut report = OrchestrationBootReport::default();
    release_terminal_worktrees(&fx.db, std::time::Duration::ZERO, &mut report).await;

    assert_eq!(report.terminal_worktrees_released, 1, "{report:?}");
    assert_ignored_files_kept_as_evidence(&fx);
}

#[tokio::test]
async fn a_finished_checkout_that_cannot_be_released_refuses_the_delete() {
    let fx = running_worker().await;
    std::fs::write(std::path::Path::new(&fx.path).join("README.md"), "# edited").unwrap();
    fx.finished_long_ago("Cancelled").await;
    worker_session_left(&fx.db).await;
    // A file where the evidence directory must be created: the diff cannot be saved.
    let evidence = terminal_evidence_dir(&fx.exec_id).unwrap();
    std::fs::create_dir_all(evidence.parent().unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&evidence);
    std::fs::write(&evidence, b"in the way").unwrap();

    let response = delete_discussion(&fx.state, &fx.parent).await;

    std::fs::remove_file(&evidence).unwrap();
    assert_conflict(
        &response,
        &[&fx.exec_id, "cannot be released", "cannot create"],
    );
    assert!(discussion_exists(&fx.db, &fx.parent).await);
    assert_eq!(
        exec_of(&fx.db, &fx.exec_id).await.status,
        TaskExecutionStatus::Cancelled
    );
    fx.assert_checkout_intact("README.md", "# edited");
}

// ── A finished checkout is released by a delete only once nothing works in it ──

#[tokio::test]
async fn a_checkout_cancelled_moments_ago_is_not_released_by_a_delete() {
    let fx = running_native_worker().await;
    let eid = fx.exec_id.clone();
    fx.db
        .with_conn(move |conn| {
            crate::db::orchestration::cancel_execution_tree(conn, &eid, "test", &backend_actor())
        })
        .await
        .unwrap();
    age_checkout(std::path::Path::new(&fx.path));

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert_conflict(
        &response,
        &[&fx.exec_id, "finished less than", "retry after"],
    );
    assert!(discussion_exists(&fx.db, &fx.parent).await);
    fx.assert_checkout_intact("README.md", "# t");
}

#[tokio::test]
async fn a_checkout_whose_cli_worker_is_still_joined_is_not_released_by_a_delete() {
    let fx = running_worker().await;
    fx.finished_long_ago("Cancelled").await;

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert_conflict(&response, &[&fx.exec_id, "session", "retry"]);
    assert!(discussion_exists(&fx.db, &fx.child).await);
    fx.assert_checkout_intact("README.md", "# t");
}

#[tokio::test]
async fn a_checkout_whose_agent_run_is_still_live_is_not_released_by_a_delete() {
    let fx = running_native_worker().await;
    let dispatch = exec_of(&fx.db, &fx.exec_id)
        .await
        .dispatch_job_id
        .expect("dispatch");
    fx.finished_long_ago("Cancelled").await;
    let token = live_token(&fx.state, &dispatch);

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert_conflict(&response, &[&fx.exec_id, &dispatch, "retry"]);
    assert!(!token.is_cancelled());
    assert!(discussion_exists(&fx.db, &fx.parent).await);
    fx.assert_checkout_intact("README.md", "# t");
}

#[tokio::test]
async fn a_checkout_written_moments_ago_is_not_released_by_a_delete() {
    let fx = running_native_worker().await;
    fx.finished_long_ago("Cancelled").await;
    std::fs::write(std::path::Path::new(&fx.path).join("late.txt"), "late").unwrap();

    let response = delete_discussion(&fx.state, &fx.parent).await;

    // The delete's own window, with the time it ends, not the sweep's setting.
    assert_conflict(
        &response,
        &[
            &fx.exec_id,
            "written less than 5 minutes ago",
            "retry after",
        ],
    );
    assert!(!response
        .error
        .unwrap_or_default()
        .contains("disk_sweep_idle_hours"));
    assert!(discussion_exists(&fx.db, &fx.parent).await);
    fx.assert_checkout_intact("late.txt", "late");
}

/// A second execution of the room, provisioned like the fixture's own.
async fn second_execution(fx: &RunningWorker) -> (String, String) {
    let task = fx
        .db
        .with_conn(|conn| {
            Ok(create_todo_task(conn, "proj-1", "Faire l'autre chose")
                .summary
                .reference)
        })
        .await
        .unwrap();
    let execution = provision_single_task_execution(
        &fx.db,
        ProvisionInput {
            task_reference: task,
            parent_discussion_id: fx.parent.clone(),
            worker: native_worker(),
            base_rev: Some("main".into()),
            idempotency_key: Some("kt-2084-second".into()),
        },
    )
    .await
    .unwrap();
    let path = managed_worktree_path(&fx.db, &execution.id).await;
    (execution.id, path)
}

/// Every execution is judged before any is released: the second one blocked
/// leaves the first one's checkout in place too.
#[tokio::test]
async fn one_blocked_execution_keeps_every_checkout_of_the_room() {
    let fx = running_native_worker().await;
    let (second, second_path) = second_execution(&fx).await;
    fx.finished_long_ago("Cancelled").await;
    let eid = second.clone();
    fx.db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET status = 'Cancelled', finished_at = ?2 WHERE id = ?1",
                rusqlite::params![eid, chrono::Utc::now().to_rfc3339()],
            )?;
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Cancelled'
                  WHERE status IN ('Pending', 'Running')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    age_checkout(std::path::Path::new(&second_path));

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert_conflict(&response, &[&second, "finished less than"]);
    fx.assert_checkout_intact("README.md", "# t");
    assert!(std::path::Path::new(&second_path).exists());
    assert_eq!(fx.workspace_row().await.0, "attached", "nothing released");
}

/// Both executions of the room finished long ago; `alter` then changes what the
/// database says of the second one, with its foreign keys off.
async fn two_finished_executions(fx: &RunningWorker, alter: &'static str) -> (String, String) {
    let (second, second_path) = second_execution(fx).await;
    fx.finished_long_ago("Cancelled").await;
    force_status(&fx.db, &second, "Cancelled").await;
    let (eid, path) = (second.clone(), second_path.clone());
    fx.db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Cancelled'
                  WHERE status IN ('Pending', 'Running')",
                [],
            )?;
            conn.execute_batch("PRAGMA foreign_keys = OFF")?;
            conn.execute(alter, rusqlite::params![eid, path])?;
            conn.execute_batch("PRAGMA foreign_keys = ON")?;
            Ok(())
        })
        .await
        .unwrap();
    age_checkout(std::path::Path::new(&second_path));
    (second, second_path)
}

/// KT-2084 — what is cheap to know is judged for every checkout before any is
/// released: a worker lease on the second keeps the first one too.
#[tokio::test]
async fn a_lease_on_another_checkout_of_the_room_keeps_every_checkout() {
    let fx = running_native_worker().await;
    let (second, second_path) = two_finished_executions(
        &fx,
        "INSERT INTO discussion_workspace_history_leases
           (id, disc_id, session_pk, canonical_path, branch, backup_ref, head_sha,
            acquired_at, expires_at)
         VALUES ('lease-' || ?1, 'elsewhere', 999, ?2, 'kronn/leased', 'refs/x', 'abc',
                 '2020-01-01T00:00:00Z', '2999-01-01T00:00:00Z')",
    )
    .await;

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert_conflict(&response, &[&second, "unreleased worker lease"]);
    fx.assert_checkout_intact("README.md", "# t");
    assert!(std::path::Path::new(&second_path).exists());
    assert_eq!(fx.workspace_row().await.0, "attached", "nothing released");
}

#[tokio::test]
async fn a_checkout_whose_project_is_gone_keeps_every_checkout_of_the_room() {
    let fx = running_native_worker().await;
    let (second, second_path) = two_finished_executions(
        &fx,
        "UPDATE discussion_workspaces SET project_id = 'gone-project'
          WHERE task_execution_id = ?1 AND canonical_path = ?2",
    )
    .await;

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert_conflict(&response, &[&second, "project gone-project is unavailable"]);
    fx.assert_checkout_intact("README.md", "# t");
    assert!(std::path::Path::new(&second_path).exists());
    assert_eq!(fx.workspace_row().await.0, "attached", "nothing released");
}

#[tokio::test]
async fn a_finish_time_that_cannot_be_read_keeps_the_checkout() {
    let fx = running_native_worker().await;
    fx.finished_long_ago("Cancelled").await;
    let eid = fx.exec_id.clone();
    fx.db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE task_executions SET finished_at = 'yesterday-ish' WHERE id = ?1",
                [&eid],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let response = delete_discussion(&fx.state, &fx.parent).await;

    assert_conflict(&response, &[&fx.exec_id, "cannot be read"]);
    assert!(discussion_exists(&fx.db, &fx.parent).await);
    fx.assert_checkout_intact("README.md", "# t");
}

/// Whatever may still come back to a finished execution's checkout keeps it:
/// a paused session, the worker's own session, a resume job, a workflow run.
#[tokio::test]
async fn a_finished_execution_is_active_while_anything_may_come_back_to_its_checkout() {
    struct Ids {
        exec: String,
        child: String,
        path: String,
    }
    type Apply = fn(&rusqlite::Connection, &Ids) -> rusqlite::Result<usize>;
    let fx = running_worker().await;
    fx.finished_long_ago("Cancelled").await;
    worker_session_left(&fx.db).await;
    let ids = Ids {
        exec: fx.exec_id.clone(),
        child: fx.child.clone(),
        path: fx.path.clone(),
    };
    let cases: [(&str, &str, Apply); 5] = [
        ("a paused bound session", "session 101", |conn, _| {
            conn.execute(
                "UPDATE discussion_sessions SET status = 'paused' WHERE id = 101",
                [],
            )
        }),
        (
            "the worker's own session, active, bound by nothing else",
            "session 101",
            |conn, ids| {
                conn.execute(
                    "DELETE FROM task_execution_cli_bindings WHERE task_execution_id = ?1",
                    [&ids.exec],
                )?;
                conn.execute(
                    "UPDATE discussion_sessions SET status = 'active' WHERE id = 101",
                    [],
                )
            },
        ),
        (
            "the worker's own session, paused, bound by nothing else",
            "session 101",
            |conn, ids| {
                conn.execute(
                    "DELETE FROM task_execution_cli_bindings WHERE task_execution_id = ?1",
                    [&ids.exec],
                )?;
                conn.execute(
                    "UPDATE discussion_sessions SET status = 'paused' WHERE id = 101",
                    [],
                )
            },
        ),
        (
            "a pending resume job of its sub-discussion",
            "resume job",
            |conn, ids| {
                conn.execute(
                    "INSERT INTO agent_resume_jobs
                       (id, discussion_id, target_agent_json, kind, status, dedupe_key, reason,
                        scheduled_at, created_at, updated_at)
                     VALUES ('resume-2084', ?1, '{}', 'Wake', 'Pending', 'resume-2084', 'wake',
                             'now', 'now', 'now')",
                    [&ids.child],
                )
            },
        ),
        (
            "an active workflow run on its checkout",
            "workflow run run-2084",
            |conn, ids| {
                conn.execute(
                    "INSERT OR IGNORE INTO workflows (id, name, project_id, trigger_json, steps_json,
                         actions_json, safety_json, workspace_config_json, concurrency_limit,
                         enabled, created_at, updated_at)
                     VALUES ('wf-2084', 'wf', NULL, '{}', '[]', '[]', '{}', NULL, NULL, 1,
                             'now', 'now')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at, status)
                     VALUES ('run-2084', 'wf-2084', ?1, 'now', 'Running')",
                    [&ids.path],
                )
            },
        ),
    ];
    let exec = fx.exec_id.clone();
    let idle = fx
        .db
        .with_conn(move |conn| crate::db::orchestration::finished_execution_activity(conn, &exec))
        .await
        .unwrap();
    assert_eq!(idle, None, "nothing works for it once its worker left");
    let ids = Arc::new(ids);
    let mut missed = Vec::new();
    for (signal, needle, apply) in cases {
        let ids = ids.clone();
        let activity = fx
            .db
            .with_conn(move |conn| {
                let tx = conn.unchecked_transaction()?;
                apply(&tx, &ids)?;
                // Dropped unfinished: each case starts from the idle state.
                crate::db::orchestration::finished_execution_activity(&tx, &ids.exec)
            })
            .await
            .unwrap();
        if !activity
            .as_deref()
            .is_some_and(|reason| reason.contains(needle))
        {
            missed.push(format!("{signal}: {activity:?}"));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

#[tokio::test]
async fn a_held_release_claim_makes_the_sweep_skip_and_the_delete_refuse() {
    let fx = running_native_worker().await;
    fx.finished_long_ago("Cancelled").await;
    let claim = crate::core::checkout_release::ReleaseClaim::take(&fx.exec_id).expect("claim");

    let mut report = OrchestrationBootReport::default();
    release_terminal_worktrees(&fx.db, std::time::Duration::ZERO, &mut report).await;
    assert_eq!(report.terminal_worktrees_released, 0, "{report:?}");
    assert!(
        std::path::Path::new(&fx.path).exists(),
        "the sweep skipped it"
    );

    let response = delete_discussion(&fx.state, &fx.parent).await;
    assert_conflict(&response, &[&fx.exec_id, "another release"]);
    assert!(std::path::Path::new(&fx.path).exists());

    drop(claim);
    let response = delete_discussion(&fx.state, &fx.parent).await;
    assert!(response.success, "{:?}", response.error);
    assert!(!std::path::Path::new(&fx.path).exists());
}

/// An execution launched between the check and the delete must not be
/// cascaded away: the delete re-checks on the connection that deletes.
#[tokio::test]
async fn an_execution_launched_after_the_check_makes_the_delete_refuse() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (task, parent, _) = seed(&db, repo.path()).await;
    let settled = settle_discussion_executions_for_delete(&db, &parent, &Default::default())
        .await
        .unwrap();
    assert_eq!(settled, Ok(Vec::new()));
    let execution = provision_single_task_execution(
        &db,
        ProvisionInput {
            task_reference: task,
            parent_discussion_id: parent.clone(),
            worker: native_worker(),
            base_rev: Some("main".into()),
            idempotency_key: Some("kt-2084-late".into()),
        },
    )
    .await
    .unwrap();

    let outcome =
        crate::api::discussions::crud::delete_discussion_and_worktree_for_tests(&db, &parent).await;

    assert!(
        matches!(&outcome, Ok(crate::api::discussions::crud::DiscussionDelete::Refused(reason))
            if reason.contains(&execution.id) && reason.contains("cancel them first")),
        "{outcome:?}"
    );
    assert!(discussion_exists(&db, &parent).await);
    assert!(!exec_of(&db, &execution.id).await.status.is_terminal());
    assert!(std::path::Path::new(&managed_worktree_path(&db, &execution.id).await).exists());
}

// ── Orphaned task checkouts ──

impl RunningWorker {
    /// The orphaned checkout's dispatches stopped: only the checkout is left.
    async fn stop_orphan_runs(&self) {
        let child = self.child.clone();
        self.db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE agent_dispatch_jobs SET status = 'Cancelled' WHERE discussion_id = ?1",
                    [&child],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    /// The workspace rows still naming the checkout.
    async fn rows_naming_checkout(&self) -> i64 {
        let path = self.path.clone();
        self.db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM discussion_workspaces WHERE canonical_path = ?1",
                    [&path],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap()
    }
}

/// Its sub-discussion's row is the only one left leading to an orphaned
/// checkout: deleting it while the checkout holds work would lose the work.
#[tokio::test]
async fn an_orphaned_dirty_task_checkout_refuses_the_delete_of_its_sub_discussion() {
    let fx = running_native_worker().await;
    fx.orphan_the_checkout().await;
    fx.stop_orphan_runs().await;
    std::fs::write(
        std::path::Path::new(&fx.path).join("README.md"),
        "# worker wip",
    )
    .unwrap();
    let token = live_token(&fx.state, &fx.child);

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert_conflict(
        &response,
        &[
            "an orphaned worker checkout still holds work",
            &fx.path,
            "1 uncommitted",
        ],
    );
    assert!(!token.is_cancelled());
    assert!(discussion_exists(&fx.db, &fx.child).await);
    assert_eq!(fx.rows_naming_checkout().await, 1, "still tracked");
    fx.assert_checkout_intact("README.md", "# worker wip");
}

/// The branch keeps the commits: the checkout goes, the branch stays, and the
/// delete says so (the UI has no action to discard a branch).
#[tokio::test]
async fn an_orphaned_checkout_whose_branch_holds_a_commit_is_released_with_its_branch_kept() {
    let fx = running_native_worker().await;
    fx.orphan_the_checkout().await;
    fx.stop_orphan_runs().await;
    let checkout = std::path::Path::new(&fx.path);
    std::fs::write(checkout.join("work.txt"), "committed").unwrap();
    assert!(git(checkout, &["add", "work.txt"]).status.success());
    assert!(git(checkout, &["commit", "-m", "worker work"])
        .status
        .success());
    let commit = git_rev(checkout, "HEAD");

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert!(response.success, "{:?}", response.error);
    let note = response.data.flatten().unwrap_or_default();
    assert!(
        note.contains(&format!("branch kept: {}", fx.branch)),
        "{note}"
    );
    assert!(!discussion_exists(&fx.db, &fx.child).await);
    assert!(!checkout.exists(), "released");
    assert_eq!(fx.rows_naming_checkout().await, 0);
    assert_eq!(
        git_rev(fx.repo.path(), &format!("refs/heads/{}", fx.branch)),
        commit
    );
}

impl RunningWorker {
    /// An orphaned checkout holding an ignored `.env`, its runs stopped, and the folder its
    /// release copies ignored files into, emptied.
    async fn orphan_holding_an_ignored_file(&self) -> std::path::PathBuf {
        let path = self.path.clone();
        let workspace_id: String = self
            .db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT id FROM discussion_workspaces WHERE canonical_path = ?1",
                    [&path],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        self.orphan_the_checkout().await;
        self.stop_orphan_runs().await;
        ignore_in_repo(self.repo.path(), ".env");
        std::fs::write(std::path::Path::new(&self.path).join(".env"), "TOKEN=1").unwrap();
        let evidence = terminal_evidence_dir(&format!("orphan-{workspace_id}")).unwrap();
        let _ = std::fs::remove_dir_all(&evidence);
        evidence
    }

    fn assert_released_with_a_copy(&self, evidence: &std::path::Path) {
        assert!(!std::path::Path::new(&self.path).exists(), "released");
        assert!(branch_exists(self.repo.path(), &self.branch), "branch kept");
        assert_eq!(
            std::fs::read(evidence.join("ignored/.env")).unwrap(),
            b"TOKEN=1"
        );
    }
}

/// KT-2084 — an orphaned checkout's ignored file (a `.env`) is copied into its evidence,
/// then the delete releases it.
#[tokio::test]
async fn an_orphaned_checkout_holding_an_ignored_file_is_released_by_the_delete_with_a_copy() {
    let fx = running_native_worker().await;
    let evidence = fx.orphan_holding_an_ignored_file().await;

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&fx.db, &fx.child).await);
    fx.assert_released_with_a_copy(&evidence);
    assert_eq!(fx.rows_naming_checkout().await, 0);
    let note = response.data.flatten().unwrap_or_default();
    let copied = evidence.join("ignored");
    assert!(
        note.contains(&format!("ignored files copied to {}", copied.display())),
        "{note}"
    );
}

/// The boot collector releases it the same way, the copy first, and says where it went.
#[tokio::test]
async fn the_boot_collector_releases_an_orphaned_checkout_holding_an_ignored_file_with_a_copy() {
    let fx = running_native_worker().await;
    let evidence = fx.orphan_holding_an_ignored_file().await;

    let report = reconcile_at_boot_with_agents(&fx.state, &[AgentType::ClaudeCode]).await;

    assert_eq!(report.orphan_workspaces_preserved, 0, "{:?}", report.errors);
    fx.assert_released_with_a_copy(&evidence);
    assert_eq!(fx.rows_naming_checkout().await, 0);
    let copied = evidence.join("ignored");
    assert!(
        report
            .lane_target_report
            .iter()
            .any(|line| line.contains(&format!("ignored files copied to {}", copied.display()))),
        "{:?}",
        report.lane_target_report
    );
}

/// `pattern` ignored through the repository's own exclude file, which its
/// worktrees share.
fn ignore_in_repo(repo: &std::path::Path, pattern: &str) {
    use std::io::Write;
    let mut exclude = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(repo.join(".git/info/exclude"))
        .unwrap();
    writeln!(exclude, "{pattern}").unwrap();
}

#[tokio::test]
async fn a_clean_orphaned_checkout_with_nothing_of_its_own_is_released_by_the_delete() {
    let fx = running_native_worker().await;
    fx.orphan_the_checkout().await;
    fx.stop_orphan_runs().await;

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&fx.db, &fx.child).await);
    assert!(!std::path::Path::new(&fx.path).exists(), "released");
    assert_eq!(fx.rows_naming_checkout().await, 0);
}

#[tokio::test]
async fn an_orphaned_checkout_a_run_still_uses_refuses_the_delete() {
    let fx = running_native_worker().await;
    fx.orphan_the_checkout().await;

    let response = delete_discussion(&fx.state, &fx.child).await;

    assert_conflict(&response, &[&fx.path, "agent dispatch", "retry"]);
    assert!(discussion_exists(&fx.db, &fx.child).await);
    assert!(std::path::Path::new(&fx.path).exists());
}

#[tokio::test]
async fn is_task_checkout_names_live_and_orphaned_task_checkouts_only() {
    let fx = running_native_worker().await;
    let is_task = |disc: &str, path: &str| {
        let (disc, path) = (disc.to_string(), path.to_string());
        let db = fx.db.clone();
        async move {
            db.with_conn(move |conn| crate::db::orchestration::is_task_checkout(conn, &disc, &path))
                .await
                .unwrap()
        }
    };
    assert!(is_task(&fx.child, &fx.path).await, "its sub-discussion");
    assert!(is_task("another-room", &fx.path).await, "its path");

    fx.orphan_the_checkout().await;
    assert!(is_task(&fx.child, &fx.path).await, "orphan, by its room");
    assert!(
        is_task(&fx.child, "/elsewhere").await,
        "orphan, by its room"
    );
    assert!(
        is_task("another-room", &fx.path).await,
        "orphan, by its path"
    );

    // A discussion's own worktree, with the rows other writers leave: the legacy
    // backfill (managed, no base) and a CLI declaration (external, with a base).
    let (iso, _) = isolated_discussion(&fx.db, fx.repo.path(), "iso-own", "own").await;
    assert!(!is_task("iso-own", &iso).await);
    let path = iso.clone();
    fx.db
        .with_conn(move |conn| {
            for (id, ownership, base) in [
                ("iso-own:legacy", "managed", None),
                ("iso-own:cli", "external", Some("abc")),
            ] {
                conn.execute(
                    "INSERT INTO discussion_workspaces
                       (id, disc_id, project_id, workspace_path, canonical_path, branch,
                        base_sha, ownership, state, created_at, updated_at)
                     VALUES (?1, 'iso-own', (SELECT project_id FROM discussions WHERE id = 'iso-own'),
                             ?2, NULL, 'kronn/own', ?3, ?4, 'attached', 'now', 'now')",
                    rusqlite::params![id, path, base, ownership],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    assert!(!is_task("iso-own", &iso).await);
}

// ── A discussion's own worktree ──

/// An isolated discussion of `repo` with its own worktree, as `create` makes it.
async fn isolated_discussion(
    db: &Database,
    repo: &std::path::Path,
    id: &str,
    slug: &str,
) -> (String, String) {
    let info = worktree::create_discussion_worktree(repo, "proj", slug, "main").unwrap();
    insert_discussion_with_worktree(db, repo, id, "Isolated", &info.path, &info.branch).await;
    (info.path, info.branch)
}

async fn insert_discussion_with_worktree(
    db: &Database,
    repo: &std::path::Path,
    id: &str,
    mode: &str,
    path: &str,
    branch: &str,
) {
    let project = test_project("proj-iso", &repo.to_string_lossy());
    let mut discussion = plain_discussion(id, "proj-iso");
    discussion.workspace_mode = mode.into();
    let (id, path, branch) = (id.to_string(), path.to_string(), branch.to_string());
    db.with_conn(move |conn| {
        // One project per repository path (UNIQUE): reuse the fixture's own.
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1",
                [&project.path],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_none() {
            crate::db::projects::insert_project(conn, &project)?;
        }
        discussion.project_id = Some(existing.unwrap_or(project.id.clone()));
        crate::db::discussions::insert_discussion(conn, &discussion)?;
        crate::db::discussions::update_discussion_workspace(conn, &id, &path, &branch)?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn deleting_an_isolated_discussion_keeps_a_branch_with_an_unpushed_commit() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, branch) = isolated_discussion(&db, repo.path(), "iso-work", "with-work").await;
    let checkout = std::path::Path::new(&path);
    std::fs::write(checkout.join("work.txt"), "unpushed").unwrap();
    assert!(git(checkout, &["add", "work.txt"]).status.success());
    assert!(git(checkout, &["commit", "-m", "unpushed work"])
        .status
        .success());
    let unpushed = git_rev(checkout, "HEAD");

    let response = delete_discussion(&state, "iso-work").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-work").await);
    assert!(!checkout.exists(), "the checkout goes");
    let note = response.data.clone().flatten().unwrap_or_default();
    assert!(note.contains(&format!("branch kept: {branch}")), "{note}");
    assert_eq!(
        git_rev(repo.path(), &format!("refs/heads/{branch}")),
        unpushed,
        "the branch keeps the only ref to its commit"
    );
}

#[tokio::test]
async fn deleting_an_isolated_discussion_deletes_a_branch_with_no_commit_of_its_own() {
    let repo = init_repo();
    // Copied into the worktree untracked: Kronn's copy, not uncommitted work.
    std::fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, branch) = isolated_discussion(&db, repo.path(), "iso-empty", "no-work").await;
    assert!(std::path::Path::new(&path).join(".mcp.json").is_file());

    let response = delete_discussion(&state, "iso-empty").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-empty").await);
    assert!(!std::path::Path::new(&path).exists());
    assert!(
        !branch_exists(repo.path(), &branch),
        "nothing to lose: deleted"
    );
}

#[tokio::test]
async fn a_delete_the_database_refuses_keeps_the_discussion_worktree() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, branch) = isolated_discussion(&db, repo.path(), "iso-fail", "db-fails").await;
    db.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TEMP TRIGGER kt2084_refuse BEFORE DELETE ON discussions
             WHEN old.id = 'iso-fail' BEGIN SELECT RAISE(ABORT, 'refused by the test'); END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let response = delete_discussion(&state, "iso-fail").await;

    assert!(!response.success);
    assert!(discussion_exists(&db, "iso-fail").await);
    assert!(std::path::Path::new(&path).exists(), "the worktree stays");
    assert!(branch_exists(repo.path(), &branch));
}

#[tokio::test]
async fn a_dirty_isolated_discussion_is_not_deleted() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-dirty", "dirty").await;
    let checkout = std::path::Path::new(&path);
    std::fs::write(checkout.join("README.md"), "# edited").unwrap();
    std::fs::write(checkout.join("notes.md"), "new").unwrap();
    let token = live_token(&state, "iso-dirty");

    let response = delete_discussion(&state, "iso-dirty").await;

    assert_conflict(
        &response,
        &["2 uncommitted", &path, "commit or discard them first"],
    );
    assert!(!token.is_cancelled());
    assert!(discussion_exists(&db, "iso-dirty").await);
    assert_eq!(
        std::fs::read_to_string(checkout.join("notes.md")).unwrap(),
        "new"
    );
}

#[tokio::test]
async fn a_dirty_review_discussion_is_deleted_with_its_checkout() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let review =
        worktree::create_review_worktree(repo.path(), "proj", "rev-dirty", "main", None).unwrap();
    insert_discussion_with_worktree(
        &db,
        repo.path(),
        "rev-dirty",
        crate::db::discussions::REVIEW_WORKSPACE_MODE,
        &review.path,
        "",
    )
    .await;
    std::fs::write(
        std::path::Path::new(&review.path).join("notes.md"),
        "scratch",
    )
    .unwrap();

    let response = delete_discussion(&state, "rev-dirty").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "rev-dirty").await);
    assert!(!std::path::Path::new(&review.path).exists(), "disposable");
}

/// A gitignored file is the human's (a `.env`), Kronn's own copies are not.
#[tokio::test]
async fn an_isolated_discussion_holding_an_ignored_file_is_not_deleted() {
    let repo = init_repo();
    std::fs::write(repo.path().join(".gitignore"), ".env\n.mcp.json\n").unwrap();
    assert!(git(repo.path(), &["add", ".gitignore"]).status.success());
    assert!(git(repo.path(), &["commit", "-m", "ignore"])
        .status
        .success());
    std::fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-env", "env").await;
    let checkout = std::path::Path::new(&path);
    assert!(checkout.join(".mcp.json").is_file(), "Kronn's copy");
    std::fs::write(checkout.join(".env"), "TOKEN=1").unwrap();

    let response = delete_discussion(&state, "iso-env").await;

    // Nothing to commit: the file is to be moved away.
    assert_conflict(
        &response,
        &[
            "1 ignored file(s)",
            ".env",
            &path,
            "move or delete them first",
        ],
    );
    assert!(!response.error.unwrap_or_default().contains("commit"));
    assert_eq!(
        std::fs::read_to_string(checkout.join(".env")).unwrap(),
        "TOKEN=1"
    );

    std::fs::remove_file(checkout.join(".env")).unwrap();
    let response = delete_discussion(&state, "iso-env").await;
    assert!(response.success, "{:?}", response.error);
    assert!(!checkout.exists());
}

/// `patterns` committed to `repo` as its `.gitignore`.
fn commit_gitignore(repo: &std::path::Path, patterns: &str) {
    std::fs::write(repo.join(".gitignore"), patterns).unwrap();
    assert!(git(repo, &["add", ".gitignore"]).status.success());
    assert!(git(repo, &["commit", "-m", "ignore"]).status.success());
}

/// KT-2084 — a build or an install makes these again: an ignored
/// `node_modules/` or `target/` never refuses the delete.
#[tokio::test]
async fn regenerable_build_output_never_refuses_the_delete() {
    let repo = init_repo();
    commit_gitignore(repo.path(), "node_modules/\ntarget/\n");
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-build", "build").await;
    let checkout = std::path::Path::new(&path);
    for file in ["node_modules/pkg/index.js", "target/debug/app"] {
        std::fs::create_dir_all(checkout.join(file).parent().unwrap()).unwrap();
        std::fs::write(checkout.join(file), "built").unwrap();
    }

    let response = delete_discussion(&state, "iso-build").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-build").await);
    assert!(!checkout.exists());
}

/// Removing a link loses nothing: what it points at stays.
#[cfg(unix)] // symlink made through std::os::unix
#[tokio::test]
async fn an_ignored_link_never_refuses_the_delete() {
    let repo = init_repo();
    commit_gitignore(repo.path(), "node_modules\n");
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("kept.txt"), "shared store").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-link", "link").await;
    let checkout = std::path::Path::new(&path);
    std::os::unix::fs::symlink(store.path(), checkout.join("node_modules")).unwrap();

    let response = delete_discussion(&state, "iso-link").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!checkout.exists());
    assert_eq!(
        std::fs::read_to_string(store.path().join("kept.txt")).unwrap(),
        "shared store"
    );
}

/// The usual `node_modules/` rule matches directories only: a link of that name is
/// untracked, and still loses nothing when it goes.
#[cfg(unix)] // symlink made through std::os::unix
#[tokio::test]
async fn an_untracked_link_never_refuses_the_delete() {
    let repo = init_repo();
    commit_gitignore(repo.path(), "node_modules/\n");
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("kept.txt"), "shared store").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-untracked-link", "ulink").await;
    let checkout = std::path::Path::new(&path);
    std::os::unix::fs::symlink(store.path(), checkout.join("node_modules")).unwrap();

    let response = delete_discussion(&state, "iso-untracked-link").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!checkout.exists());
    assert_eq!(
        std::fs::read_to_string(store.path().join("kept.txt")).unwrap(),
        "shared store"
    );
}

/// KT-2084 — a submodule commit no remote branch or tag reaches cannot be proved saved
/// or lost offline: the discussion goes, its worktree stays, and the delete says why.
#[tokio::test]
async fn a_submodule_commit_no_remote_reaches_keeps_the_worktree_of_the_deleted_discussion() {
    let upstream = init_repo();
    let repo = init_repo();
    let file_allowed = ["-c", "protocol.file.allow=always"];
    let add = git(
        repo.path(),
        &[
            &file_allowed[..],
            &[
                "submodule",
                "add",
                "-q",
                &upstream.path().to_string_lossy(),
                "sub",
            ],
        ]
        .concat(),
    );
    assert!(add.status.success(), "{add:?}");
    assert!(git(repo.path(), &["commit", "-qm", "sub"]).status.success());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-sub", "sub-commit").await;
    let checkout = std::path::Path::new(&path);
    let init = git(
        checkout,
        &[&file_allowed[..], &["submodule", "update", "--init", "-q"]].concat(),
    );
    assert!(init.status.success(), "{init:?}");
    let sub = checkout.join("sub");
    std::fs::write(sub.join("README.md"), "local only").unwrap();
    let identity = ["-c", "user.email=t@t.com", "-c", "user.name=T"];
    assert!(git(
        &sub,
        &[&identity[..], &["commit", "-qam", "local"]].concat()
    )
    .status
    .success());
    let commit = git_rev(&sub, "HEAD");
    assert!(git(checkout, &["commit", "-qam", "bump"]).status.success());

    let response = delete_discussion(&state, "iso-sub").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-sub").await);
    let note = response.data.flatten().unwrap_or_default();
    assert!(
        note.contains(&format!(
            "worktree kept at {path}: the submodule sub holds commit {commit}"
        )) && note.contains("no remote branch or tag reaches"),
        "{note}"
    );
    assert!(checkout.exists());
    assert_eq!(git_rev(&sub, "HEAD"), commit, "the commit is intact");
}

/// KT-2084 — a branch another discussion's worktree names is never deleted, even when
/// nothing but that row would keep it.
#[tokio::test]
async fn a_branch_another_discussion_names_is_never_deleted() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, branch) = isolated_discussion(&db, repo.path(), "iso-one", "named").await;
    insert_discussion_with_worktree(
        &db,
        repo.path(),
        "iso-other",
        "Isolated",
        "/gone/elsewhere/other",
        &branch,
    )
    .await;

    let response = delete_discussion(&state, "iso-one").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!std::path::Path::new(&path).exists(), "the checkout goes");
    assert!(branch_exists(repo.path(), &branch), "the branch stays");
    let note = response.data.flatten().unwrap_or_default();
    assert!(note.contains(&format!("branch kept: {branch}")), "{note}");
}

/// KT-2084 — only the discussion's own branch is ever deleted: a worktree switched to
/// another local branch keeps that branch.
#[tokio::test]
async fn a_worktree_switched_to_another_branch_keeps_that_branch() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-switched", "switched").await;
    let checkout = std::path::Path::new(&path);
    assert!(git(repo.path(), &["branch", "feature"]).status.success());
    assert!(git(checkout, &["switch", "-q", "feature"]).status.success());

    let response = delete_discussion(&state, "iso-switched").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-switched").await);
    assert!(
        branch_exists(repo.path(), "feature"),
        "not the discussion's branch"
    );
}

/// KT-2084 — a worktree Kronn does not manage is never removed: the discussion goes, the
/// directory stays, and the delete says so.
#[tokio::test]
async fn a_worktree_kronn_does_not_manage_is_kept_when_its_discussion_is_deleted() {
    let repo = init_repo();
    let outside = tempfile::tempdir().unwrap();
    let path = outside
        .path()
        .join("own-checkout")
        .to_string_lossy()
        .to_string();
    assert!(git(
        repo.path(),
        &["worktree", "add", "-q", "-b", "kronn/own", &path]
    )
    .status
    .success());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    insert_discussion_with_worktree(
        &db,
        repo.path(),
        "iso-own-checkout",
        "Isolated",
        &path,
        "kronn/own",
    )
    .await;

    let response = delete_discussion(&state, "iso-own-checkout").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-own-checkout").await);
    assert!(
        std::path::Path::new(&path).join("README.md").exists(),
        "kept"
    );
    let note = response.data.flatten().unwrap_or_default();
    assert!(note.contains(&format!("worktree kept at {path}")), "{note}");
    assert!(branch_exists(repo.path(), "kronn/own"));
}

/// KT-2084 — a worktree directory git no longer lists (its registration pruned) never blocks
/// the delete: the discussion goes, the directory stays, and the delete says so.
#[tokio::test]
async fn a_worktree_git_no_longer_lists_is_kept_when_its_discussion_is_deleted() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-pruned", "pruned").await;
    let checkout = std::path::Path::new(&path);
    let name = checkout.file_name().unwrap();
    std::fs::remove_dir_all(repo.path().join(".git/worktrees").join(name)).unwrap();
    std::fs::write(checkout.join("notes.md"), "mine").unwrap();

    let response = delete_discussion(&state, "iso-pruned").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!discussion_exists(&db, "iso-pruned").await);
    assert_eq!(
        std::fs::read_to_string(checkout.join("notes.md")).unwrap(),
        "mine"
    );
    let note = response.data.flatten().unwrap_or_default();
    assert!(
        note.contains(&format!(
            "worktree kept at {path}: git no longer lists it as a worktree"
        )),
        "{note}"
    );
}

/// KT-2084 — an ignored agent config in the worktree is Kronn's copy, even once
/// the project's own changed (an MCP sync rewrites it).
#[tokio::test]
async fn a_project_agent_config_rewritten_after_creation_never_refuses_the_delete() {
    let repo = init_repo();
    commit_gitignore(repo.path(), ".mcp.json\n");
    std::fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-mcp", "mcp").await;
    let checkout = std::path::Path::new(&path);
    assert!(checkout.join(".mcp.json").is_file(), "Kronn's copy");
    std::fs::write(repo.path().join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    let response = delete_discussion(&state, "iso-mcp").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!checkout.exists());
}

/// Commits on a detached HEAD live only in the worktree's own reflog, which
/// the removal deletes.
#[tokio::test]
async fn an_isolated_discussion_with_commits_on_a_detached_head_is_not_deleted() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-detached", "detached").await;
    let checkout = std::path::Path::new(&path);
    assert!(git(checkout, &["checkout", "-q", "--detach"])
        .status
        .success());
    std::fs::write(checkout.join("work.txt"), "detached work").unwrap();
    assert!(git(checkout, &["add", "work.txt"]).status.success());
    assert!(git(checkout, &["commit", "-m", "detached work"])
        .status
        .success());
    let commit = git_rev(checkout, "HEAD");
    let token = live_token(&state, "iso-detached");

    let response = delete_discussion(&state, "iso-detached").await;

    assert_conflict(
        &response,
        &[
            &path,
            "commits on a detached HEAD",
            "create a branch or discard them first",
        ],
    );
    assert!(!token.is_cancelled());
    assert!(discussion_exists(&db, "iso-detached").await);
    assert!(checkout.exists());
    assert_eq!(git_rev(checkout, "HEAD"), commit);
}

#[tokio::test]
async fn an_isolated_discussion_detached_on_a_branch_commit_is_deleted() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-detached-ok", "detached-ok").await;
    let checkout = std::path::Path::new(&path);
    assert!(git(checkout, &["checkout", "-q", "--detach"])
        .status
        .success());

    let response = delete_discussion(&state, "iso-detached-ok").await;

    assert!(response.success, "{:?}", response.error);
    assert!(!checkout.exists());
}

/// Same-slug discussions share `kronn/<slug>` and its worktree (KT-1811): the
/// one deleted never takes the other's checkout or branch with it.
#[tokio::test]
async fn deleting_one_of_two_discussions_sharing_a_worktree_keeps_it_for_the_other() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, branch) = isolated_discussion(&db, repo.path(), "iso-twin-a", "twin").await;
    insert_discussion_with_worktree(&db, repo.path(), "iso-twin-b", "Isolated", &path, &branch)
        .await;

    let response = delete_discussion(&state, "iso-twin-a").await;

    assert!(response.success, "{:?}", response.error);
    let kept = response.data.flatten().unwrap_or_default();
    assert!(
        kept.contains("worktree kept at") && kept.contains(&path) && kept.contains("iso-twin-b"),
        "{kept}"
    );
    assert!(!discussion_exists(&db, "iso-twin-a").await);
    assert!(discussion_exists(&db, "iso-twin-b").await);
    assert!(std::path::Path::new(&path).exists(), "the other's checkout");
    assert!(branch_exists(repo.path(), &branch), "the other's branch");
}

/// A project reached through a link: Kronn spells the first discussion's worktree
/// through the link, git (which the same-slug reuse asks) through the real path.
#[cfg(unix)]
fn linked_project(repo: &std::path::Path) -> (tempfile::TempDir, std::path::PathBuf) {
    let links = tempfile::tempdir().unwrap();
    let linked = links.path().join("project");
    std::os::unix::fs::symlink(repo, &linked).unwrap();
    (links, linked)
}

/// KT-2084 — the twin found by its canonical path: the shared worktree, its
/// branch and the other discussion's uncommitted work all stay.
#[cfg(unix)] // reaches the project through a symlink made with std::os::unix
#[tokio::test]
async fn a_twin_spelling_the_shared_worktree_differently_keeps_it_and_its_work() {
    let repo = init_repo();
    let (_links, linked) = linked_project(repo.path());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, branch) = isolated_discussion(&db, &linked, "iso-spelled-a", "spelled").await;
    let twin = worktree::create_discussion_worktree(&linked, "proj", "spelled", "main").unwrap();
    assert_eq!(twin.branch, branch);
    assert_ne!(twin.path, path, "two spellings of one worktree");
    insert_discussion_with_worktree(
        &db,
        &linked,
        "iso-spelled-b",
        "Isolated",
        &twin.path,
        &twin.branch,
    )
    .await;
    let checkout = std::path::Path::new(&path);
    std::fs::write(checkout.join("notes.md"), "the twin's work").unwrap();

    let response = delete_discussion(&state, "iso-spelled-a").await;

    assert!(response.success, "{:?}", response.error);
    let kept = response.data.flatten().unwrap_or_default();
    assert!(kept.contains("iso-spelled-b"), "{kept}");
    assert!(!discussion_exists(&db, "iso-spelled-a").await);
    assert_eq!(
        std::fs::read_to_string(checkout.join("notes.md")).unwrap(),
        "the twin's work"
    );
    assert!(branch_exists(repo.path(), &branch), "the twin's branch");
}

/// A run that names the worktree by its real path still holds it.
#[cfg(unix)] // reaches the project through a symlink made with std::os::unix
#[tokio::test]
async fn a_workflow_run_naming_the_worktree_by_its_real_path_refuses_the_delete() {
    let repo = init_repo();
    let (_links, linked) = linked_project(repo.path());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, &linked, "iso-real-run", "real-run").await;
    let real = std::path::Path::new(&path)
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_ne!(real, path);
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
                 safety_json, workspace_config_json, concurrency_limit, enabled, created_at,
                 updated_at)
             VALUES ('wf-real', 'wf', NULL, '{}', '[]', '[]', '{}', NULL, NULL, 1, 'now', 'now')",
            [],
        )?;
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at, status)
             VALUES ('run-real', 'wf-real', ?1, 'now', 'Running')",
            [&real],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let response = delete_discussion(&state, "iso-real-run").await;

    assert_conflict(&response, &["run-real", "retry"]);
    assert!(discussion_exists(&db, "iso-real-run").await);
    assert!(std::path::Path::new(&path).exists());
}

#[tokio::test]
async fn an_isolated_discussion_whose_worktree_a_workflow_run_uses_is_not_deleted() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (path, _) = isolated_discussion(&db, repo.path(), "iso-run", "run").await;
    let run_path = path.clone();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
                 safety_json, workspace_config_json, concurrency_limit, enabled, created_at,
                 updated_at)
             VALUES ('wf-iso', 'wf', NULL, '{}', '[]', '[]', '{}', NULL, NULL, 1, 'now', 'now')",
            [],
        )?;
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, workspace_path, started_at, status)
             VALUES ('run-iso', 'wf-iso', ?1, 'now', 'Running')",
            [&run_path],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let token = live_token(&state, "iso-run");

    let response = delete_discussion(&state, "iso-run").await;

    assert_conflict(&response, &[&path, "run-iso", "retry"]);
    assert!(!token.is_cancelled());
    assert!(discussion_exists(&db, "iso-run").await);
    assert!(std::path::Path::new(&path).exists());
}

/// Everything that can refuse is known before anything is cancelled: an
/// execution launched after the first checks is caught before the cancels.
#[tokio::test]
async fn an_execution_launched_after_the_checks_is_refused_before_anything_is_cancelled() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);
    let (task, parent, _) = seed(&db, repo.path()).await;
    let room = parent.clone();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO agent_resume_jobs
               (id, discussion_id, target_agent_json, kind, status, dedupe_key, reason,
                scheduled_at, created_at, updated_at)
             VALUES ('resume-room', ?1, '{}', 'Wake', 'Pending', 'resume-room', 'wake',
                     'now', 'now', 'now')",
            [&room],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let token = live_token(&state, &parent);
    let job_token = live_token(&state, "agent-job:resume-room");
    let execution = provision_single_task_execution(
        &db,
        ProvisionInput {
            task_reference: task,
            parent_discussion_id: parent.clone(),
            worker: native_worker(),
            base_rev: Some("main".into()),
            idempotency_key: Some("kt-2084-before-cancel".into()),
        },
    )
    .await
    .unwrap();

    let Json(response) =
        crate::api::discussions::crud::cancel_and_delete_for_tests(&state, &parent).await;

    assert_conflict(&response, &[&execution.id, "cancel them first"]);
    assert!(!token.is_cancelled(), "the room's agent runs on");
    assert!(!job_token.is_cancelled());
    let status: String = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT status FROM agent_resume_jobs WHERE id = 'resume-room'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(status, "Pending");
    assert!(discussion_exists(&db, &parent).await);
}

/// Already deleted (a double click): the client counts it as deleted.
#[tokio::test]
async fn deleting_a_missing_discussion_says_not_found() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = state_for(&db);

    let response = delete_discussion(&state, "never-there").await;

    assert!(!response.success);
    assert_eq!(response.error_code.as_deref(), Some("not_found"));
}

/// A retired managed row (no checkout, no execution) holds nothing: the boot
/// pass has no work and nothing to report for it.
#[tokio::test]
async fn a_retired_orphan_row_is_no_boot_error() {
    let repo = init_repo();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (_, parent, project) = seed(&db, repo.path()).await;
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO discussion_workspaces
               (id, disc_id, project_id, workspace_path, canonical_path, branch, head_sha,
                base_sha, ownership, state, created_at, updated_at)
             VALUES ('retired-2084', ?1, ?2, '/gone/wt', NULL, 'kronn/gone', 'abc', 'abc',
                     'managed', 'detached', 'now', 'now')",
            rusqlite::params![parent, project],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let report = reconcile_at_boot_with_agents(&state_for(&db), &[AgentType::ClaudeCode]).await;

    assert!(
        !report
            .errors
            .iter()
            .any(|error| error.contains("retired-2084")),
        "{:?}",
        report.errors
    );
    assert_eq!(report.orphan_workspaces_preserved, 0);
}
