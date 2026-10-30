//! KT-1134 — every state an audit run can be left in offers an action, and
//! nothing reads "running" once it has stopped: after a Kronn restart, a
//! cancel, or a lease nobody could settle. A cancel removes only its own
//! run's validation, and no validation agent writes docs/ while an audit runs.
use axum::extract::{Path, State};

use crate::AppState;

pub(super) fn state() -> AppState {
    AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

pub(super) async fn seed_project(state: &AppState, path: &std::path::Path) {
    let path = path.to_string_lossy().into_owned();
    state
        .db
        .with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

/// A run row started `seconds_ago`, so runs order the way they were launched.
fn insert_run(conn: &rusqlite::Connection, id: &str, seconds_ago: i64) -> anyhow::Result<()> {
    crate::db::audit_runs::insert_running(
        conn,
        id,
        "p1",
        "Full",
        "ClaudeCode",
        chrono::Utc::now() - chrono::Duration::seconds(seconds_ago),
    )
}

fn insert_validation(
    conn: &rusqlite::Connection,
    disc_id: &str,
    run_id: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
         VALUES (?1, 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
        [disc_id],
    )?;
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
         VALUES (?1, ?2, 'User', 'Validate the audit.', datetime('now'), 0)",
        rusqlite::params![format!("m-{disc_id}"), disc_id],
    )?;
    if let Some(run_id) = run_id {
        crate::db::audit_runs::set_validation_discussion(conn, run_id, disc_id)?;
    }
    Ok(())
}

async fn discussion_exists(state: &AppState, id: &str) -> bool {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| Ok(crate::db::discussions::get_discussion(conn, &id)?.is_some()))
        .await
        .unwrap()
}

async fn timeline(state: &AppState) -> super::AuditTimelineData {
    super::audit_timeline(State(state.clone()), Path("p1".to_string()))
        .await
        .0
        .data
        .expect("timeline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_mid_run_leaves_a_resumable_run_and_no_live_progress() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    state
        .db
        .with_conn(|conn| {
            insert_run(conn, "r1", 60)?;
            for step in 1..=3 {
                crate::db::audit_runs::insert_audit_step_start(
                    conn,
                    "r1",
                    step,
                    "docs/x.md",
                    chrono::Utc::now(),
                )?;
                crate::db::audit_runs::finalize_audit_step(
                    conn,
                    "r1",
                    step,
                    chrono::Utc::now(),
                    1,
                    &crate::db::audit_runs::StepTokens::default(),
                    None,
                    true,
                    None,
                    false,
                )?;
            }
            crate::db::audit_runs::update_last_completed_step(conn, "r1", 3)?;
            // Step 4 was running when the process died.
            crate::db::audit_runs::insert_audit_step_start(
                conn,
                "r1",
                4,
                "docs/y.md",
                chrono::Utc::now(),
            )?;
            // What `Database::open` runs at boot.
            crate::db::audit_runs::reconcile_stale_runs(conn, 0)?;
            Ok(())
        })
        .await
        .unwrap();

    let progress = super::audit_status(State(state.clone()), Path("p1".to_string()), None, None)
        .await
        .0;
    assert!(
        progress.data.unwrap().is_none(),
        "no live progress after a restart"
    );
    let resumable = super::audit_latest_resumable(State(state.clone()), Path("p1".to_string()))
        .await
        .0
        .data
        .unwrap()
        .expect("the interrupted run is resumable");
    let resumable = serde_json::to_value(resumable).unwrap();
    assert_eq!(resumable["status"], "Interrupted");
    let total = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len();
    assert_eq!(
        resumable["steps_to_redo"],
        serde_json::json!((4..=total as u32).collect::<Vec<_>>())
    );
    let data = timeline(&state).await;
    assert_eq!(data.runs[0].status, "Interrupted");
    assert!(!data.lease_orphaned);
    assert!(
        state.audit_tracker.lock().unwrap().try_acquire_lease("p1"),
        "a launch is accepted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_deletes_only_its_own_runs_validation_and_leaves_the_project_launchable() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    state
        .db
        .with_conn(|conn| {
            insert_run(conn, "r-old", 120)?;
            crate::db::audit_runs::update_last_completed_step(conn, "r-old", 3)?;
            crate::db::audit_runs::mark_interrupted(conn, "r-old", "test")?;
            insert_validation(conn, "d-old-partial", Some("r-old"))?;
            insert_validation(conn, "d-unlinked", None)?;
            insert_run(conn, "r-live", 10)?;
            insert_validation(conn, "d-live", Some("r-live"))?;
            Ok(())
        })
        .await
        .unwrap();
    // The live worker acknowledges the cancel the way the pipeline does.
    let worker = {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                let requested = state.audit_tracker.lock().unwrap().cancelled.contains("p1");
                if requested {
                    state
                        .db
                        .with_conn(|conn| crate::db::audit_runs::mark_cancelled(conn, "r-live"))
                        .await
                        .unwrap();
                    state.audit_tracker.lock().unwrap().cancelled.remove("p1");
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
    };

    let cancelled = crate::api::audit::full::cancel_audit(State(state.clone()), Path("p1".into()))
        .await
        .0;
    worker.await.unwrap();
    assert!(cancelled.success, "{:?}", cancelled.error);
    assert!(!discussion_exists(&state, "d-live").await);
    assert!(
        discussion_exists(&state, "d-old-partial").await,
        "an earlier run's validation stays"
    );
    assert!(discussion_exists(&state, "d-unlinked").await);

    // Cancelled: not resumable, nothing running, a fresh launch is accepted.
    let resumable = super::audit_latest_resumable(State(state.clone()), Path("p1".to_string()))
        .await
        .0;
    assert!(resumable.data.unwrap().is_none());
    let data = timeline(&state).await;
    assert_eq!(data.runs[0].status, "Cancelled");
    assert!(state.audit_tracker.lock().unwrap().progress.is_empty());
    assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_nobody_could_settle_is_signalled_then_released_by_the_cleanup() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    state
        .db
        .with_conn(|conn| {
            insert_run(conn, "r-stuck", 30)?;
            conn.execute_batch(
                "CREATE TRIGGER audit_runs_down BEFORE UPDATE ON audit_runs
                 BEGIN SELECT RAISE(ABORT, 'database down'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    {
        let mut tracker = state.audit_tracker.lock().unwrap();
        assert!(tracker.try_acquire_lease("p1"));
        tracker.start_progress("p1", 16, "full_audit");
    }
    {
        let mut guard = super::super::full::AuditDropGuard::new(
            state.db.clone(),
            state.audit_tracker.clone(),
            Some("r-stuck".into()),
            "p1".into(),
        );
        guard.hold_lease();
        // Dropped armed: the pipeline died and its run cannot be settled.
    }
    let orphaned = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if state.audit_tracker.lock().unwrap().lease_orphaned("p1") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(orphaned.is_ok(), "the unsettled lease is flagged");
    assert!(
        state.audit_tracker.lock().unwrap().progress.is_empty(),
        "no live progress"
    );
    assert!(
        timeline(&state).await.lease_orphaned,
        "the timeline signals it"
    );
    assert!(
        !state.audit_tracker.lock().unwrap().try_acquire_lease("p1"),
        "still fail-closed"
    );

    // A live audit on another project is never touched by the cleanup.
    state
        .db
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER audit_runs_down;")?;
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p2', 'Q', '/nowhere', ?1, ?1)",
                rusqlite::params![now],
            )?;
            crate::db::audit_runs::insert_running(
                conn,
                "r-live",
                "p2",
                "Full",
                "ClaudeCode",
                chrono::Utc::now(),
            )
        })
        .await
        .unwrap();
    assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p2"));

    let cleaned = super::audit_runs_cleanup(State(state.clone())).await.0;
    assert_eq!(cleaned.data, Some(1));
    let data = timeline(&state).await;
    assert!(!data.lease_orphaned);
    assert_eq!(data.runs[0].status, "Interrupted");
    let live = state
        .db
        .with_conn(|conn| crate::db::audit_runs::get_by_id(conn, "r-live"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.status, "Running", "a live run is spared");
    let mut tracker = state.audit_tracker.lock().unwrap();
    assert!(tracker.leased.contains("p2"), "a live lease is kept");
    assert!(
        tracker.try_acquire_lease("p1"),
        "the project can be audited again"
    );
}

/// A validation discussion of run `r1` with one queued agent turn.
pub(super) async fn queued_validation(
    state: &AppState,
) -> crate::db::agent_dispatch::AgentDispatchJob {
    state
        .db
        .with_conn(|conn| {
            insert_run(conn, "r1", 60)?;
            crate::db::audit_runs::update_last_completed_step(conn, "r1", 3)?;
            crate::db::audit_runs::mark_interrupted(conn, "r1", "test")?;
            insert_validation(conn, "d-val", Some("r1"))?;
            crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: "job-val",
                    discussion_id: "d-val",
                    dedupe_key: "runtime:d-val:job-val",
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )
        })
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_audit_launch_stops_the_validation_agent_still_answering() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    queued_validation(&state).await;

    let stopped = crate::api::audit::full::stop_validation_agents(&state, "p1").await;
    assert_eq!(stopped, vec!["d-val".to_string()]);
    let active = state
        .db
        .with_conn(|conn| crate::db::agent_dispatch::has_active_for_discussion(conn, "d-val"))
        .await
        .unwrap();
    assert!(!active, "the validation agent no longer runs");
    assert!(
        crate::api::audit::full::stop_validation_agents(&state, "p1")
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_validation_agent_waits_while_an_audit_of_its_project_runs() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    queued_validation(&state).await;
    state
        .audit_tracker
        .lock()
        .unwrap()
        .start_progress("p1", 16, "full_audit");
    let job = state
        .db
        .with_conn(|conn| crate::db::agent_dispatch::claim(conn, "job-val"))
        .await
        .unwrap()
        .expect("claimed");

    let stream =
        crate::api::discussions::runtime::stream_claimed_dispatch_job(state.clone(), job, None)
            .await;
    assert!(stream.is_none(), "no agent starts");
    let held = state
        .db
        .with_conn(|conn| crate::db::agent_dispatch::get(conn, "job-val"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        held.status,
        crate::db::agent_dispatch::DispatchStatus::Pending
    );
    assert!(held
        .last_error
        .as_deref()
        .is_some_and(|e| e.starts_with("audit_running")));
    assert_eq!(held.attempts, 0, "a held turn spends no attempt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn docs_writer_lease_holds_validation_without_progress() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    queued_validation(&state).await;
    state.audit_tracker.lock().unwrap().try_acquire_lease("p1");
    let job = state
        .db
        .with_conn(|conn| crate::db::agent_dispatch::claim(conn, "job-val"))
        .await
        .unwrap()
        .expect("claimed");

    let stream =
        crate::api::discussions::runtime::stream_claimed_dispatch_job(state.clone(), job, None)
            .await;
    assert!(stream.is_none(), "no agent starts");
    let held = state
        .db
        .with_conn(|conn| crate::db::agent_dispatch::get(conn, "job-val"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        held.status,
        crate::db::agent_dispatch::DispatchStatus::Pending
    );
    assert!(held
        .last_error
        .as_deref()
        .is_some_and(|e| e.starts_with("audit_running")));
    assert_eq!(held.attempts, 0, "a held turn spends no attempt");
}

/// KT-1169 — the launch gate: a resumable run refuses a launch that is not an
/// explicit fresh one; a partial run, which the Full launch cannot resume, never does.
#[tokio::test]
async fn only_an_explicit_fresh_launch_supersedes_a_resumable_run() {
    let state = state();
    let project = tempfile::tempdir().unwrap();
    seed_project(&state, project.path()).await;
    let latest = |state: AppState| async move {
        state
            .db
            .with_conn(|conn| crate::db::audit_runs::latest_resumable(conn, "p1"))
            .await
            .unwrap()
    };
    assert_eq!(
        crate::api::audit::full::fresh_launch_refusal(None, false),
        None
    );

    state
        .db
        .with_conn(|conn| {
            insert_run(conn, "r1", 60)?;
            crate::db::audit_runs::update_last_completed_step(conn, "r1", 3)?;
            crate::db::audit_runs::mark_interrupted(conn, "r1", "step 4 failed")
        })
        .await
        .unwrap();
    let run = latest(state.clone()).await.expect("resumable");
    let refusal =
        crate::api::audit::full::fresh_launch_refusal(Some(&run), false).expect("refused");
    assert!(
        refusal.contains("r1") && refusal.contains("fresh: true"),
        "{refusal}"
    );
    assert_eq!(
        crate::api::audit::full::fresh_launch_refusal(Some(&run), true),
        None
    );

    state
        .db
        .with_conn(|conn| {
            crate::db::audit_runs::insert_running(
                conn,
                "p-run",
                "p1",
                "Partial",
                "ClaudeCode",
                chrono::Utc::now(),
            )?;
            crate::db::audit_runs::update_last_completed_step(conn, "p-run", 1)?;
            crate::db::audit_runs::mark_interrupted(conn, "p-run", "stale")
        })
        .await
        .unwrap();
    let partial = latest(state.clone()).await.expect("latest is the partial");
    assert_eq!(partial.id, "p-run");
    assert_eq!(
        crate::api::audit::full::fresh_launch_refusal(Some(&partial), false),
        None
    );
}
