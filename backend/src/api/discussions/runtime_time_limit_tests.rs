//! KT-2037 — a task worker Kronn stopped at the run time limit ends with that
//! cause in its dispatch `last_error`, and its prose is never read as a provider
//! refusal that would move it to a fallback agent.

use super::finish_dispatch_turn;
use crate::core::agent_fallback as fallback;
use crate::models::{AgentFallback, AgentType, TaskExecutionStatus};
use std::sync::Arc;

const PARENT: &str = "d-limit-parent";
const CHILD: &str = "d-limit-child";
const JOB: &str = "job-limit";

/// A `Working` Claude Code execution whose worker runs the claimed dispatch
/// `JOB`, stopped by the limit after 240 of 240 minutes. `chain` is the fallback
/// of Claude Code, when the test needs one.
async fn stopped_worker(
    chain: Option<AgentFallback>,
) -> (
    crate::AppState,
    String,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    stopped_worker_by(chain, crate::api::worker_time_limit::StopKind::TimeLimit).await
}

/// [`stopped_worker`], stopped by the limit of `kind`: 240 of 240 minutes for
/// the time limit, 20 minutes without output after 95 for inactivity.
async fn stopped_worker_by(
    chain: Option<AgentFallback>,
    kind: crate::api::worker_time_limit::StopKind,
) -> (
    crate::AppState,
    String,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    worker_on_dispatch(chain, Some(kind)).await
}

/// The worker execution of the fixtures above; `stop`, when given, is the limit
/// that stopped its run.
async fn worker_on_dispatch(
    chain: Option<AgentFallback>,
    stop: Option<crate::api::worker_time_limit::StopKind>,
) -> (
    crate::AppState,
    String,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    let (execution_id, job) = db
        .with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            for (id, title) in [(PARENT, "Parent"), (CHILD, "Child")] {
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?3)",
                    rusqlite::params![id, title, now],
                )?;
            }
            conn.execute(
                "INSERT INTO planning_tasks
                 (id, task_number, title, status, created_at, updated_at)
                 VALUES ('t-limit', 9905, 'Limit', 'todo', ?1, ?1)",
                [&now],
            )?;
            let actor = crate::models::PlanningActor {
                kind: crate::models::PlanningActorKind::Backend,
                id: Some("orchestrator".into()),
                session_id: None,
                source_message_id: None,
                token_declared: false,
            };
            let mut launch = crate::models::LaunchSingleTaskInput::new("t-limit", PARENT);
            launch.worker_agent_type = Some("ClaudeCode".into());
            let execution =
                crate::db::orchestration::launch_single_task(conn, &launch, &actor)?.execution;
            for status in [
                TaskExecutionStatus::Provisioning,
                TaskExecutionStatus::Working,
            ] {
                crate::db::orchestration::transition_execution(
                    conn,
                    &execution.id,
                    status,
                    &actor,
                    serde_json::json!({}),
                )?;
            }
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u-limit', ?1, 'User', 'go', ?2, 1, ?2)",
                rusqlite::params![CHILD, now],
            )?;
            crate::db::agent_dispatch::enqueue_for_latest_user(
                conn,
                crate::db::agent_dispatch::NewLatestUserDispatch {
                    id: JOB,
                    discussion_id: CHILD,
                    dedupe_key: "message:u-limit",
                    agent_override: Some(&AgentType::ClaudeCode),
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            let job = crate::db::agent_dispatch::claim(conn, JOB)?
                .ok_or_else(|| anyhow::anyhow!("dispatch was not claimable"))?;
            crate::db::orchestration::attach_execution_dispatch(conn, &execution.id, JOB)?;
            let minutes = |n: u64| std::time::Duration::from_secs(n * 60);
            if let Some(kind) = stop {
                let (elapsed, limit) = match kind {
                    crate::api::worker_time_limit::StopKind::TimeLimit => {
                        (minutes(240), minutes(240))
                    }
                    crate::api::worker_time_limit::StopKind::Inactivity => {
                        (minutes(95), minutes(20))
                    }
                };
                crate::api::worker_time_limit::record_stop(
                    conn,
                    &execution.id,
                    JOB,
                    kind,
                    elapsed,
                    limit,
                    None,
                )?;
            }
            Ok((execution.id, job))
        })
        .await
        .unwrap();
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain = chain.into_iter().collect();
    fallback::migrate_legacy_chains(&mut config.agents);
    let state = crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (state, execution_id, job)
}

async fn dispatch_status(state: &crate::AppState) -> (String, Option<String>, Option<String>) {
    state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row(
                "SELECT status, last_error, failure_kind FROM agent_dispatch_jobs WHERE id = ?1",
                [JOB],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .await
        .unwrap()
}

/// The reply of a run cut at the limit: the worker's prose, then the notice the
/// stream appends.
fn cut_reply(prose: &str) -> String {
    format!(
        "{prose}\n\n⚠️ **Agent interrupted by Kronn after reaching the 240-minute global execution limit.** Retry the turn to resume from the durable discussion context."
    )
}

/// KT-2037 — the dispatch of a worker stopped at the limit names that cause and
/// both durations in `last_error`, and its execution is parked with it.
#[tokio::test]
async fn a_worker_stream_that_reached_the_global_timeout_ends_with_that_failure_kind() {
    let (state, execution_id, job) = stopped_worker(None).await;

    finish_dispatch_turn(&state, job, cut_reply("Running the full suite."), false).await;

    let (status, last_error, failure_kind) = dispatch_status(&state).await;
    assert_eq!(status, "Failed");
    let last_error = last_error.expect("the stop is the dispatch's last error");
    assert!(
        last_error.contains("failure_kind=execution_time_limit"),
        "{last_error}"
    );
    assert!(last_error.contains("240 of 240 min"), "{last_error}");
    assert!(
        !last_error.contains("worker_reported_failure"),
        "{last_error}"
    );
    assert_eq!(
        failure_kind, None,
        "the text is the diagnostic, as for any completion"
    );

    let recovery = state
        .db
        .with_read_conn(move |conn| {
            crate::db::orchestration::get_execution_recovery(conn, &execution_id)
        })
        .await
        .unwrap()
        .expect("the execution is parked for a decision");
    assert_eq!(
        recovery.recovery_reason,
        "worker_timed_out_without_delivery"
    );
}

/// KT-2037 — prose that quotes a provider error does not move a worker Kronn
/// stopped itself to a fallback agent, nor put its provider on cooldown.
#[tokio::test]
async fn a_timed_out_reply_that_quotes_a_rate_limit_is_not_moved_to_a_fallback() {
    let (state, execution_id, job) = stopped_worker(Some(AgentFallback {
        agent: AgentType::Codex,
        model: None,
        mode: None,
    }))
    .await;

    finish_dispatch_turn(
        &state,
        job,
        cut_reply("The client answered HTTP 429 Too Many Requests: rate_limit_exceeded."),
        false,
    )
    .await;

    let (status, last_error, _) = dispatch_status(&state).await;
    assert_eq!(status, "Failed");
    assert!(
        last_error
            .as_deref()
            .is_some_and(|error| error.contains("failure_kind=execution_time_limit")),
        "{last_error:?}"
    );
    let (execution, cooling, fallback_notices) = state
        .db
        .with_read_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            let cooling = fallback::cooling_lookup(conn, chrono::Utc::now())?;
            let fallback_notices: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE id LIKE 'agent-fallback%'",
                [],
                |row| row.get(0),
            )?;
            Ok((execution, cooling, fallback_notices))
        })
        .await
        .unwrap();
    assert_eq!(execution.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(execution.status, TaskExecutionStatus::Interrupted);
    assert_eq!(execution.dispatch_job_id.as_deref(), Some(JOB));
    assert!(
        cooling.is_empty(),
        "no provider is put on cooldown: {cooling:?}"
    );
    assert_eq!(fallback_notices, 0, "no fallback line is posted");
}

/// KT-2107 — a worker Kronn stopped for lack of output ends with that cause,
/// and its prose quoting a rate limit moves it to no fallback agent.
#[tokio::test]
async fn an_inactivity_stopped_reply_that_quotes_a_rate_limit_is_not_moved_to_a_fallback() {
    let (state, execution_id, job) = stopped_worker_by(
        Some(AgentFallback {
            agent: AgentType::Codex,
            model: None,
            mode: None,
        }),
        crate::api::worker_time_limit::StopKind::Inactivity,
    )
    .await;

    finish_dispatch_turn(
        &state,
        job,
        "The client answered HTTP 429 Too Many Requests: rate_limit_exceeded.\n\n---\n⚠️ **Agent interrupted by Kronn after 20 min without output.** Retry the turn."
            .to_string(),
        false,
    )
    .await;

    let (status, last_error, _) = dispatch_status(&state).await;
    assert_eq!(status, "Failed");
    let last_error = last_error.expect("the stop is the dispatch's last error");
    let head: String = last_error.chars().take(160).collect();
    assert!(
        head.contains("failure_kind=inactivity_timeout"),
        "{last_error}"
    );
    assert!(
        !last_error.contains("worker_reported_failure"),
        "{last_error}"
    );
    let (execution, cooling, recovery) = state
        .db
        .with_read_conn(move |conn| {
            let execution = crate::db::orchestration::get_task_execution(conn, &execution_id)?
                .expect("execution must remain queryable");
            let cooling = fallback::cooling_lookup(conn, chrono::Utc::now())?;
            let recovery = crate::db::orchestration::get_execution_recovery(conn, &execution_id)?;
            Ok((execution, cooling, recovery))
        })
        .await
        .unwrap();
    assert_eq!(execution.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(execution.status, TaskExecutionStatus::Interrupted);
    assert!(
        cooling.is_empty(),
        "no provider is put on cooldown: {cooling:?}"
    );
    assert_eq!(
        recovery.map(|recovery| recovery.recovery_reason).as_deref(),
        Some(crate::db::orchestration::WORKER_INACTIVE_REASON)
    );
}

/// KT-2107 — a rate-limited worker that no fallback link can take ends with the
/// rate limit, and its announced reset, as the dispatch's cause; it is parked.
#[tokio::test]
async fn a_rate_limited_worker_with_no_link_ends_with_the_rate_limit_named() {
    for (reply, reset) in [
        ("HTTP 429 Too Many Requests: rate_limit_exceeded.", false),
        ("HTTP 429 Too Many Requests; try again in 3 hours", true),
    ] {
        let (state, execution_id, job) = worker_on_dispatch(None, None).await;

        finish_dispatch_turn(&state, job, reply.to_string(), false).await;

        let (status, last_error, _) = dispatch_status(&state).await;
        assert_eq!(status, "Failed", "{reply}");
        let last_error = last_error.expect("the cause is the dispatch's last error");
        let head: String = last_error.chars().take(160).collect();
        assert!(head.contains("failure_kind=rate_limit"), "{last_error}");
        assert!(
            !last_error.contains("worker_reported_failure"),
            "{last_error}"
        );
        assert_eq!(
            last_error.contains("announced reset"),
            reset,
            "{last_error}"
        );
        assert!(last_error.contains("task_exec_resume"), "{last_error}");
        let execution = state
            .db
            .with_read_conn(move |conn| {
                crate::db::orchestration::get_task_execution(conn, &execution_id)
            })
            .await
            .unwrap()
            .expect("execution must remain queryable");
        assert_eq!(
            execution.status,
            TaskExecutionStatus::Interrupted,
            "{reply}"
        );
    }
}
