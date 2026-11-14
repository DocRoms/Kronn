//! KT-1518 — a task worker whose provider cannot serve it continues on the
//! agent's fallback chain, in its own room and worktree; a task failure never
//! moves it. The quota-back decision point of KT-1664 offers the same chain.

use super::*;
use crate::api::discussions::agent_fallback::{
    intercept_failed_turn_among, offer_to_escalated_execution_among, worker_after_quota_among,
};
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
    ]
}

/// A state whose config chains Claude Code to `first`.
fn state_with_chain(db: &std::sync::Arc<Database>, first: Option<AgentFallback>) -> AppState {
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain = first.into_iter().collect();
    // The chain of 0.15.3, carried onto the rows the way a start-up does.
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

async fn fallback_notice(db: &Database, was: &TaskExecution, dispatch: &str) -> Option<String> {
    messages_of(db, &was.parent_discussion_id)
        .await
        .into_iter()
        .find(|message| message.id == format!("agent-fallback:{dispatch}"))
        .map(|message| message.content)
}

#[tokio::test]
async fn a_quota_refused_worker_continues_on_the_fallback_in_its_own_room_with_the_same_tier() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::GeminiCli)));

    assert!(
        worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("GeminiCli"));
    assert_eq!(
        after.worker_model_tier.as_deref(),
        Some("economy"),
        "same tier on the next agent"
    );
    assert_eq!(
        after.worker_model, None,
        "the refused agent's model is dropped"
    );
    assert_eq!(after.sub_discussion_id, was.sub_discussion_id, "same room");
    assert_eq!(after.workspace_id, was.workspace_id, "same worktree");
    assert_eq!(after.child_branch, was.child_branch);
    assert_ne!(
        after.dispatch_job_id, was.dispatch_job_id,
        "a fresh dispatch"
    );

    // The parent room says so, in the agreed words.
    let notice = fallback_notice(&db, &was, &job.id)
        .await
        .expect("room line");
    assert!(
        notice.starts_with(&format!(
            "Task worker execution {}: Claude Code · eco mode: quota exhausted until ",
            was.id
        )) && notice.ends_with(" → continued with Gemini CLI · eco mode [CLI]"),
        "{notice}"
    );
    // The refused agent is on cooldown, so new turns start on the fallback.
    let cooling = db
        .with_conn(|conn| {
            fallback::active_cooldown(conn, &AgentType::ClaudeCode, chrono::Utc::now())
        })
        .await
        .unwrap()
        .expect("cooldown");
    assert_eq!(cooling.reason, "quota_exhausted");
    // The provider switch is journaled like any fallback (KT-1564).
    let id = was.id.clone();
    assert!(!db
        .with_conn(move |conn| crate::db::orchestration::provider_changes(conn, &id))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_rate_limited_or_unauthenticated_worker_continues_on_the_fallback_too() {
    for response in [
        "HTTP 429 Too Many Requests: rate_limit_exceeded",
        "Invalid authentication credentials (api error: 401)",
        // A saturated provider (503/529) is retried on the same agent first;
        // any other outage goes straight to the chain for a worker.
        "[Agent provider error] API Error: 500 Internal Server Error",
    ] {
        let (_repo, db, was, job) = the_economy_worker().await;
        let state = state_with_chain(&db, Some(link(AgentType::GeminiCli)));
        assert!(
            intercept_failed_turn_among(&state, &job, response, Some(usable())).await,
            "{response}"
        );
        let after = exec_of(&db, &was.id).await;
        assert_eq!(
            after.worker_agent_type.as_deref(),
            Some("GeminiCli"),
            "{response}"
        );
        assert_eq!(
            after.worker_model_tier.as_deref(),
            Some("economy"),
            "{response}"
        );
        assert!(
            fallback_notice(&db, &was, &job.id).await.is_some(),
            "{response}"
        );
    }
}

/// KT-1758 — the outage chain takes the worker over from `Working`: the
/// principal is not told to « reassign or cancel » an execution Kronn is
/// reassigning, and the per-execution notice is not spent by it — it still
/// fires, once, if the replacement later ends without a delivery.
#[tokio::test]
async fn an_outage_move_neither_parks_the_worker_nor_spends_the_undelivered_notice() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::GeminiCli)));
    let undelivered = format!("orch-worker-undelivered:{}", was.id);
    let notice = |db: std::sync::Arc<Database>, parent: String, id: String| async move {
        messages_of(&db, &parent)
            .await
            .into_iter()
            .find(|message| message.id == id)
            .map(|message| message.content)
    };

    assert!(
        intercept_failed_turn_among(
            &state,
            &job,
            "[Agent provider error] API Error: 500 Internal Server Error",
            Some(usable())
        )
        .await
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("GeminiCli"));
    assert_eq!(
        after.status,
        TaskExecutionStatus::Working,
        "taken over, never parked"
    );
    assert!(
        notice(
            db.clone(),
            was.parent_discussion_id.clone(),
            undelivered.clone()
        )
        .await
        .is_none(),
        "no « reassign or cancel » before an automatic move"
    );
    assert!(fallback_notice(&db, &was, &job.id).await.is_some());

    // The replacement ends without a delivery: now the principal is told, once.
    let replacement = after.dispatch_job_id.clone().unwrap();
    let (claim_id, child) = (
        replacement.clone(),
        after.sub_discussion_id.clone().unwrap(),
    );
    db.with_conn(move |conn| {
        crate::db::agent_dispatch::claim(conn, &claim_id)?.expect("the replacement is due now");
        crate::api::discussions::runtime::persist_dispatch_settlement(
            conn,
            &claim_id,
            &child,
            None,
            crate::db::workflows::BatchChildOutcome::Failed,
            Some("the replacement crashed"),
            None,
        )
    })
    .await
    .unwrap();
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Interrupted
    );
    assert!(
        notice(db.clone(), was.parent_discussion_id.clone(), undelivered)
            .await
            .is_some_and(|content| content.contains("réaffecte ou annule")),
        "the notice was kept for the replacement's own end"
    );
}

#[tokio::test]
async fn a_task_failure_a_refusal_a_test_failure_or_a_work_timeout_never_moves_a_worker() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::GeminiCli)));
    for failure in [
        "test result: FAILED. 12 passed; 3 failed; 0 ignored",
        "I can't help with that request.",
        "error[E0308]: mismatched types — the build failed",
        "Agent stalled (no output for 300s)",
        "Agent stalled (no output for 300s) — last line: HTTP 429 Too Many Requests",
    ] {
        assert!(
            !intercept_failed_turn_among(&state, &job, failure, Some(usable())).await,
            "{failure}"
        );
        assert!(
            !worker_after_quota_among(&state, &job, failure, Some(usable()))
                .await
                .unwrap(),
            "{failure}"
        );
    }
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(after.dispatch_job_id, was.dispatch_job_id);
    assert!(fallback_notice(&db, &was, &job.id).await.is_none());
}

#[tokio::test]
async fn a_worker_whose_agent_has_no_fallback_or_whose_fallback_is_unusable_stays_put() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let none = state_with_chain(&db, None);
    assert!(
        !worker_after_quota_among(&none, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    // A fallback that is not installed is skipped, and the room is told why.
    let missing = state_with_chain(&db, Some(link(AgentType::LiteLlm)));
    assert!(
        !worker_after_quota_among(&missing, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    let stop = messages_of(&db, &was.parent_discussion_id)
        .await
        .into_iter()
        .find(|message| message.id == format!("agent-fallback-stop:{}", job.id))
        .expect("the stop is said");
    assert!(
        stop.content
            .contains("Claude Code: quota exhausted → no fallback")
            && stop
                .content
                .contains("LiteLLM is not installed or not enabled"),
        "{}",
        stop.content
    );
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode")
    );
}

#[tokio::test]
async fn the_quota_back_decision_point_offers_the_fallback_when_the_provider_still_refuses() {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let was = escalate_claude_without_reset(&db, repo.path(), 1)
        .await
        .remove(0);
    let state = state_with_chain(&db, Some(link(AgentType::GeminiCli)));
    let probe = StubQuotaProbe::new(Some(false));
    let rechecker = rechecker_with(&probe).with_usable_agents(usable());
    let t0 = chrono::Utc::now();

    // Just refused: not asked yet, so nothing moves.
    rechecker.recheck_pass(&state, t0).await;
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Escalated
    );

    // Asked: Claude Code still refuses, so the chain is the alternative to
    // waiting, through the same selection as a failing turn.
    let outcome = rechecker
        .recheck_pass(&state, t0 + chrono::Duration::minutes(21))
        .await;
    assert!(
        outcome.executions.is_empty(),
        "nothing was RESUMED on Claude Code"
    );
    assert_eq!(probe.asked(), vec![AgentType::ClaudeCode]);
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("GeminiCli"));
    assert_eq!(after.worker_model_tier.as_deref(), Some("economy"));
    assert_ne!(after.status, TaskExecutionStatus::Escalated);
    assert_eq!(after.sub_discussion_id, was.sub_discussion_id);
}

#[tokio::test]
async fn the_quota_back_decision_point_waits_when_there_is_no_chain_or_the_provider_is_silent() {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let was = escalate_claude_without_reset(&db, repo.path(), 1)
        .await
        .remove(0);
    let t0 = chrono::Utc::now();
    // No chain configured: the escalation waits as it always did.
    let no_chain = state_with_chain(&db, None);
    let still = StubQuotaProbe::new(Some(false));
    let rechecker = rechecker_with(&still).with_usable_agents(usable());
    rechecker.recheck_pass(&no_chain, t0).await;
    rechecker
        .recheck_pass(&no_chain, t0 + chrono::Duration::minutes(21))
        .await;
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Escalated
    );
    // A chain, but the provider said nothing: inconclusive is not "refused".
    let chained = state_with_chain(&db, Some(link(AgentType::GeminiCli)));
    let silent = StubQuotaProbe::new(None);
    let rechecker = rechecker_with(&silent).with_usable_agents(usable());
    rechecker.recheck_pass(&chained, t0).await;
    rechecker
        .recheck_pass(&chained, t0 + chrono::Duration::minutes(21))
        .await;
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode")
    );
    // Offered directly with nothing usable to take it: no move either.
    assert!(!offer_to_escalated_execution_among(
        &chained,
        &was.id,
        Some(vec![AgentType::ClaudeCode])
    )
    .await
    .unwrap());
}
