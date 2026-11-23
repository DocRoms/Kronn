//! KT-1713 — a task worker is never handed to Codex (its sandbox cannot write
//! the linked worktree's git dir), and a fallback taken for a quota window is
//! temporary: the first resume after the window goes back to the declared worker.

use super::agent_fallback_runs::prelocalise;
use super::*;
use crate::api::discussions::agent_fallback::worker_after_quota_among;
use crate::core::agent_fallback as fallback;
use crate::models::AgentFallback;

const CLAUDE_LIMIT: &str = "You've hit your limit · resets 11:59pm (UTC)";

fn link(agent: AgentType) -> AgentFallback {
    AgentFallback {
        agent,
        model: None,
        mode: None,
    }
}

fn usable() -> Vec<AgentType> {
    vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Ollama,
    ]
}

fn state_with_chain(db: &std::sync::Arc<Database>, first: AgentFallback) -> AppState {
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain = vec![first];
    fallback::migrate_legacy_chains(&mut config.agents);
    AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn the_economy_worker() -> (
    tempfile::TempDir,
    std::sync::Arc<Database>,
    TaskExecution,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let was = economy_claude_executions(&db, repo.path(), 1)
        .await
        .remove(0);
    let job = dispatch_of(&db, was.dispatch_job_id.clone().unwrap()).await;
    (repo, db, was, job)
}

/// KT-1991 — a generic worker never moves to an HTTP link: the tests of a move
/// onto one start from a prelocalised execution.
async fn the_prelocalised_economy_worker() -> (
    tempfile::TempDir,
    std::sync::Arc<Database>,
    TaskExecution,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let (repo, db, was, job) = the_economy_worker().await;
    let was = prelocalise(&db, &was.id).await;
    (repo, db, was, job)
}

/// The worker's turn on the fallback ended without a delivery: the execution is
/// `Interrupted` with a pending `resume_worker` decision, as in the 12/11 run.
async fn interrupt_undelivered(db: &Database, exec_id: &str) {
    let dispatch = exec_of(db, exec_id).await.dispatch_job_id.unwrap();
    db.with_conn(move |conn| {
        crate::db::agent_dispatch::mark_completed(conn, &dispatch)?;
        crate::db::orchestration::interrupt_undelivered_execution_for_dispatch(
            conn,
            &dispatch,
            "worker_completed_without_delivery",
            &backend_actor(),
        )?
        .context("the worker ended without a delivery")?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn set_origin_reset(db: &Database, exec_id: &str, reset: chrono::DateTime<chrono::Utc>) {
    let id = exec_id.to_string();
    let stamp = crate::db::orchestration::quota_reset_stamp(reset);
    db.with_conn(move |conn| {
        conn.execute(
            "UPDATE task_execution_quota_origin SET reset_at = ?2 WHERE task_execution_id = ?1",
            rusqlite::params![id, stamp],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn codex_is_refused_as_a_task_worker_fallback_and_the_room_says_why() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let state = state_with_chain(&db, link(AgentType::Codex));

    assert!(
        !worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap(),
        "Codex cannot write the linked worktree's git dir, so it never takes a task worker"
    );

    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode")
    );
    let stop = messages_of(&db, &was.parent_discussion_id)
        .await
        .into_iter()
        .find(|message| message.id == format!("agent-fallback-stop:{}", job.id))
        .expect("the refusal is said");
    assert!(
        stop.content
            .contains(&format!("Exécution du worker de tâche {}", was.id))
            && stop
                .content
                .contains("Codex task-worker fallback is disabled")
            && stop.content.contains("linked worktree"),
        "{}",
        stop.content
    );
}

#[tokio::test]
async fn a_resume_after_the_fallback_window_goes_back_to_the_declared_worker_and_says_so() {
    let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
    let state = state_with_chain(&db, link(AgentType::Ollama));
    assert!(
        worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("Ollama")
    );
    // The window that justified the fallback is over; the alternative's turn
    // ended without a delivery.
    set_origin_reset(
        &db,
        &was.id,
        chrono::Utc::now() - chrono::Duration::minutes(1),
    )
    .await;
    interrupt_undelivered(&db, &was.id).await;
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Interrupted
    );

    let Json(resumed) =
        resume_execution(State(recovery_test_state(&db)), Path(was.id.clone())).await;
    assert!(resumed.success, "resume refused: {:?}", resumed.error);

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.status, TaskExecutionStatus::Working);
    assert_eq!(after.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(after.worker_model_tier.as_deref(), Some("economy"));
    assert_eq!(after.sub_discussion_id, was.sub_discussion_id);
    assert!(
        origin_of(&db, &was.id).await.is_none(),
        "home again: nothing left to return to"
    );
    let notice = messages_of(&db, &was.parent_discussion_id)
        .await
        .into_iter()
        .find(|message| message.id.starts_with("agent-fallback-return:"))
        .expect("the return is said in the parent room")
        .content;
    assert!(
        notice.starts_with(&format!("Task worker execution {}", was.id))
            && notice.contains("quota window")
            && notice.contains("back on its declared worker ClaudeCode"),
        "{notice}"
    );
    let child = messages_of(&db, after.sub_discussion_id.as_deref().unwrap()).await;
    assert!(
        child
            .iter()
            .any(|message| message.content.contains("back on its declared worker")),
        "the worker's own room is told too"
    );
}

#[tokio::test]
async fn a_resume_while_the_fallback_window_is_open_stays_on_the_fallback() {
    let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
    let state = state_with_chain(&db, link(AgentType::Ollama));
    assert!(
        worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    set_origin_reset(
        &db,
        &was.id,
        chrono::Utc::now() + chrono::Duration::hours(2),
    )
    .await;
    interrupt_undelivered(&db, &was.id).await;

    let Json(resumed) =
        resume_execution(State(recovery_test_state(&db)), Path(was.id.clone())).await;
    assert!(resumed.success, "resume refused: {:?}", resumed.error);

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert!(origin_of(&db, &was.id).await.is_some());
    assert!(messages_of(&db, &was.parent_discussion_id)
        .await
        .iter()
        .all(|message| !message.id.starts_with("agent-fallback-return:")));
}

// ─── KT-1749 — a fallback is never an agent the launch would refuse ──────

async fn stop_in(db: &Database, room: &str, id: String) -> Option<String> {
    messages_of(db, room)
        .await
        .into_iter()
        .find(|message| message.id == id)
        .map(|message| message.content)
}

#[tokio::test]
async fn a_native_acp_agent_is_never_picked_for_a_worker_and_both_rooms_say_why() {
    for refused in [
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::OpenCode,
        AgentType::Vibe,
    ] {
        let (_repo, db, was, job) = the_economy_worker().await;
        let state = state_with_chain(&db, link(refused.clone()));
        let mut usable = usable();
        usable.push(refused.clone());

        assert!(
            !worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable))
                .await
                .unwrap(),
            "{refused:?}"
        );
        let after = exec_of(&db, &was.id).await;
        assert_eq!(after.worker_agent_type.as_deref(), Some("ClaudeCode"));
        assert_eq!(after.dispatch_job_id, was.dispatch_job_id, "not moved");

        let parent = stop_in(
            &db,
            &was.parent_discussion_id,
            format!("agent-fallback-stop:{}", job.id),
        )
        .await
        .expect("the principal room is told");
        assert!(
            parent.starts_with(&format!("Exécution du worker de tâche {}", was.id))
                && parent.contains("cannot take this turn"),
            "{refused:?}: {parent}"
        );
        let worker_room = stop_in(
            &db,
            &job.discussion_id,
            format!("agent-fallback-stop-worker:{}", job.id),
        )
        .await
        .expect("the worker room is told");
        assert!(
            worker_room.contains("aucun agent de repli"),
            "{worker_room}"
        );
    }
}

#[tokio::test]
async fn the_walk_goes_past_a_refused_agent_to_an_http_link_built_as_a_discussion_agent() {
    let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain =
        vec![link(AgentType::GeminiCli), link(AgentType::Ollama)];
    fallback::migrate_legacy_chains(&mut config.agents);
    let state = AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    assert!(
        worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_eq!(
        after.worker_target_kind,
        Some(MessageTargetKind::DiscussionAgent),
        "an HTTP link uses the HTTP transport"
    );
}

#[tokio::test]
async fn a_refused_reassignment_is_refused_before_anything_moves() {
    let (_repo, db, was, _job) = the_economy_worker().await;
    let state = state_with_chain(&db, link(AgentType::Ollama));
    let error = crate::api::orchestration::reassign_native_execution_as(
        &state,
        &was.id,
        crate::models::CampaignWorkerSelection {
            target: crate::models::MessageTarget::agent(AgentType::GeminiCli),
            model: None,
            profile_id: None,
        },
        "test",
        crate::api::orchestration::ReassignCause::QuotaFallback { reset_at: None },
    )
    .await
    .err()
    .expect("the spawn would refuse it, so the move is refused first")
    .to_string();
    assert!(error.starts_with("worker_capability"), "{error}");
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode")
    );
}

// ─── KT-2000 — the reassignment boundary refuses a Kronn-made move to no shell ─

async fn reassign_to_ollama(
    state: &AppState,
    exec_id: &str,
    cause: crate::api::orchestration::ReassignCause,
) -> anyhow::Result<crate::api::orchestration::ExecutionRecoveryView> {
    crate::api::orchestration::reassign_native_execution_as(
        state,
        exec_id,
        crate::models::CampaignWorkerSelection {
            target: fallback::task_worker_target(&AgentType::Ollama),
            model: None,
            profile_id: None,
        },
        "test",
        cause,
    )
    .await
}

#[tokio::test]
async fn a_quota_fallback_of_a_generic_worker_to_an_http_agent_is_refused_at_the_boundary() {
    let (_repo, db, was, _job) = the_economy_worker().await;
    assert!(was.worker_scope.is_none(), "a generic worker");
    let state = state_with_chain(&db, link(AgentType::ClaudeCode));
    let error = reassign_to_ollama(
        &state,
        &was.id,
        crate::api::orchestration::ReassignCause::QuotaFallback { reset_at: None },
    )
    .await
    .err()
    .expect("Kronn never moves a generic worker to an agent without a shell")
    .to_string();
    assert!(error.contains(fallback::NO_SHELL), "{error}");
    let after = exec_of(&db, &was.id).await;
    assert_eq!(
        after.worker_agent_type, was.worker_agent_type,
        "same worker"
    );
    assert_eq!(after.worker_target_kind, was.worker_target_kind);
    assert_eq!(after.dispatch_job_id, was.dispatch_job_id, "same dispatch");
    assert_eq!(after.status, was.status, "same status");
}

#[tokio::test]
async fn a_quota_fallback_of_a_prelocalised_worker_to_an_http_agent_passes_the_boundary() {
    let (_repo, db, was, _job) = the_prelocalised_economy_worker().await;
    let state = state_with_chain(&db, link(AgentType::ClaudeCode));
    reassign_to_ollama(
        &state,
        &was.id,
        crate::api::orchestration::ReassignCause::QuotaFallback { reset_at: None },
    )
    .await
    .expect("a prelocalised edit can be served over HTTP");
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_ne!(after.dispatch_job_id, was.dispatch_job_id, "a new dispatch");
}

#[tokio::test]
async fn a_person_may_still_move_a_generic_worker_to_an_http_agent() {
    let (_repo, db, was, _job) = the_economy_worker().await;
    assert!(was.worker_scope.is_none(), "a generic worker");
    let state = state_with_chain(&db, link(AgentType::ClaudeCode));
    reassign_to_ollama(
        &state,
        &was.id,
        crate::api::orchestration::ReassignCause::Human,
    )
    .await
    .expect("a person's choice is theirs, as at launch");
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_ne!(after.dispatch_job_id, was.dispatch_job_id, "a new dispatch");
}

#[tokio::test]
async fn the_stop_reason_is_in_the_language_of_the_principal_room() {
    for (code, prefix, stopped) in [
        (
            "fr",
            "Exécution du worker de tâche",
            "quota épuisé → aucun agent de repli",
        ),
        (
            "en",
            "Task worker execution",
            "quota exhausted → no fallback",
        ),
    ] {
        let (_repo, db, was, job) = the_economy_worker().await;
        let parent = was.parent_discussion_id.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE discussions SET language = ?2 WHERE id = ?1",
                rusqlite::params![parent, code],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let state = state_with_chain(&db, link(AgentType::GeminiCli));
        let mut usable = usable();
        usable.push(AgentType::GeminiCli);
        assert!(
            !worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable))
                .await
                .unwrap()
        );
        let said = stop_in(
            &db,
            &was.parent_discussion_id,
            format!("agent-fallback-stop:{}", job.id),
        )
        .await
        .expect("said");
        assert!(
            said.starts_with(&format!("{prefix} {}", was.id)) && said.contains(stopped),
            "{code}: {said}"
        );
    }
}
