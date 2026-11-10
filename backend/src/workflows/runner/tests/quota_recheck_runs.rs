//! KT-1657 — a run paused on a provider quota does not wait blindly for the
//! announced reset: a bounded re-check of its provider resumes it as soon as
//! the quota is back, and its refused step can be moved to another agent.

use super::quota_wait_runs::{fixture, restarted, run_until_paused, stored, SESSION_LIMIT};
use super::*;
use crate::models::{AgentType, QuotaParkReason, StepAgentOverride};
use crate::workflows::quota_wait::{QuotaProbe, QuotaRechecker};
use chrono::Duration;
use std::sync::{Arc, Mutex};

/// A provider that answers with a fixed verdict and remembers who it was asked about.
struct StubProbe {
    verdict: Option<bool>,
    asked: Mutex<Vec<AgentType>>,
}

impl StubProbe {
    fn new(verdict: Option<bool>) -> Arc<Self> {
        Arc::new(Self {
            verdict,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<AgentType> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl QuotaProbe for StubProbe {
    async fn quota_is_back(
        &self,
        agent: &AgentType,
        _project_path: &str,
        _tokens: &crate::models::TokensConfig,
        _full_access: bool,
    ) -> Option<bool> {
        self.asked.lock().unwrap().push(agent.clone());
        self.verdict
    }
}

const THREE_DAYS: u64 = 3 * 24 * 3600;

fn wake_of(run: &WorkflowRun) -> chrono::DateTime<Utc> {
    run.step_results[1]
        .quota_wait
        .as_ref()
        .and_then(|wait| wait.wake_at)
        .expect("a scheduled wake-up")
}

#[tokio::test]
async fn a_paused_run_resumes_as_soon_as_its_provider_answers_before_the_announced_reset() {
    let fx = fixture("recheck-back", SESSION_LIMIT, 1, THREE_DAYS).await;
    let paused = run_until_paused(&fx, "run-recheck-back").await;
    assert_eq!(paused.status, RunStatus::WaitingQuota);
    // Virtual clock, well before the announced reset.
    let t0 = wake_of(&paused) - Duration::hours(3);

    let probe = StubProbe::new(Some(true));
    let rechecker = QuotaRechecker::new(probe.clone(), Duration::minutes(20));
    let state = restarted(&fx.state);

    // Just refused: the provider is not asked about a wait it has just made.
    assert!(rechecker.recheck_due(&state, t0).await.is_empty());
    // Bounded: no new question inside the interval.
    assert!(rechecker
        .recheck_due(&state, t0 + Duration::minutes(10))
        .await
        .is_empty());
    assert!(probe.asked().is_empty());

    let resumed = rechecker
        .recheck_due(&state, t0 + Duration::minutes(21))
        .await;
    assert_eq!(resumed, vec!["run-recheck-back".to_string()]);
    assert_eq!(probe.asked(), vec![AgentType::ClaudeCode]);

    let done = wait_until_settled(&state, "run-recheck-back").await;
    assert_eq!(done.status, RunStatus::Success);
    assert_eq!(done.step_results[1].status, RunStatus::Success);
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse", "analyse"]);
}

#[tokio::test]
async fn a_provider_still_refusing_or_silent_keeps_the_run_paused_and_is_asked_at_most_once_per_interval(
) {
    let fx = fixture("recheck-still", SESSION_LIMIT, 1, THREE_DAYS).await;
    let paused = run_until_paused(&fx, "run-recheck-still").await;
    let t0 = wake_of(&paused) - Duration::hours(3);
    let state = restarted(&fx.state);

    for verdict in [Some(false), None] {
        let probe = StubProbe::new(verdict);
        let rechecker = QuotaRechecker::new(probe.clone(), Duration::minutes(20));
        rechecker.recheck_due(&state, t0).await;
        assert!(rechecker
            .recheck_due(&state, t0 + Duration::minutes(21))
            .await
            .is_empty());
        assert!(rechecker
            .recheck_due(&state, t0 + Duration::minutes(30))
            .await
            .is_empty());
        assert_eq!(probe.asked().len(), 1, "{verdict:?}: asked once in 30 min");
        rechecker
            .recheck_due(&state, t0 + Duration::minutes(42))
            .await;
        assert_eq!(
            probe.asked().len(),
            2,
            "{verdict:?}: asked again after 20 min"
        );
    }
    let still = stored(&state, "run-recheck-still").await;
    assert_eq!(still.status, RunStatus::WaitingQuota);
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse"]);
}

#[tokio::test]
async fn a_login_or_account_change_asks_the_provider_at_once() {
    let fx = fixture("recheck-login", SESSION_LIMIT, 1, THREE_DAYS).await;
    let paused = run_until_paused(&fx, "run-recheck-login").await;
    let t0 = wake_of(&paused) - Duration::hours(3);
    let state = restarted(&fx.state);

    let probe = StubProbe::new(Some(true));
    let rechecker = QuotaRechecker::new(probe.clone(), Duration::minutes(20));
    assert!(rechecker.recheck_due(&state, t0).await.is_empty());
    assert!(probe.asked().is_empty());

    crate::workflows::quota_wait::request_recheck();
    let resumed = rechecker
        .recheck_due(&state, t0 + Duration::minutes(1))
        .await;
    assert_eq!(resumed, vec!["run-recheck-login".to_string()]);
    assert_eq!(probe.asked().len(), 1);
    wait_until_settled(&state, "run-recheck-login").await;
}

#[tokio::test]
async fn a_run_parked_for_a_human_is_never_probed() {
    let refusal = "HTTP 429 Too Many Requests: rate_limit_exceeded";
    let fx = fixture("recheck-park", refusal, 1, THREE_DAYS).await;
    run_until_paused(&fx, "run-recheck-park").await;
    let state = restarted(&fx.state);
    let t0 = Utc::now();

    let probe = StubProbe::new(Some(true));
    let rechecker = QuotaRechecker::new(probe.clone(), Duration::minutes(20));
    rechecker.recheck_due(&state, t0).await;

    // Parked for a human on purpose (too many attempts): never probed.
    let loaded = stored(&state, "run-recheck-park").await;
    assert!(
        crate::workflows::quota_wait::park(
            &state,
            &loaded,
            QuotaParkReason::TooManyAttempts,
            String::new()
        )
        .await
    );
    assert!(rechecker
        .recheck_due(&state, t0 + Duration::minutes(25))
        .await
        .is_empty());
    assert!(probe.asked().is_empty());
}

#[tokio::test]
async fn a_parked_run_without_a_reset_resumes_when_its_provider_answers() {
    let refusal = "HTTP 429 Too Many Requests: rate_limit_exceeded";
    let fx = fixture("recheck-noreset", refusal, 1, THREE_DAYS).await;
    let parked = run_until_paused(&fx, "run-recheck-noreset").await;
    let wait = parked.step_results[1].quota_wait.clone().unwrap();
    assert_eq!(wait.parked, Some(QuotaParkReason::NoResetTime));
    assert_eq!(wait.wake_at, None);
    let state = restarted(&fx.state);
    let t0 = Utc::now();

    let probe = StubProbe::new(Some(true));
    let rechecker = QuotaRechecker::new(probe.clone(), Duration::minutes(20));
    rechecker.recheck_due(&state, t0).await;
    let resumed = rechecker
        .recheck_due(&state, t0 + Duration::minutes(21))
        .await;
    assert_eq!(resumed, vec!["run-recheck-noreset".to_string()]);
    let done = wait_until_settled(&state, "run-recheck-noreset").await;
    assert_eq!(done.status, RunStatus::Success);
}

#[tokio::test]
async fn the_refused_step_can_be_moved_to_another_agent_and_the_recheck_asks_that_agent() {
    let fx = fixture("recheck-switch", SESSION_LIMIT, 1, THREE_DAYS).await;
    let paused = run_until_paused(&fx, "run-recheck-switch").await;
    let t0 = wake_of(&paused) - Duration::hours(3);
    let state = restarted(&fx.state);

    // What `POST /api/workflow-runs/:id/quota-agent` stores once the agent is admitted.
    let choice = StepAgentOverride {
        agent: AgentType::Codex,
        model: None,
        reasoning_effort: None,
    };
    let context =
        crate::workflows::step_agents::context_with_choice(&paused, "analyse", Some(&choice))
            .unwrap();
    let id = paused.id.clone();
    assert!(state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::set_quota_waiting_trigger_context(conn, &id, &context)
        })
        .await
        .unwrap());

    let switched = stored(&state, "run-recheck-switch").await;
    assert_eq!(switched.status, RunStatus::WaitingQuota);
    let leaf = state
        .db
        .with_conn(move |conn| crate::workflows::quota_wait::waiting_leaf(conn, switched))
        .await
        .unwrap();
    assert_eq!(leaf.step.name, "analyse");
    assert_eq!(leaf.step.agent, AgentType::Codex);
    // The workflow as saved is untouched: the choice belongs to the run.
    assert_eq!(leaf.workflow.steps[1].agent, AgentType::ClaudeCode);

    let probe = StubProbe::new(Some(false));
    let rechecker = QuotaRechecker::new(probe.clone(), Duration::minutes(20));
    rechecker.recheck_due(&state, t0).await;
    rechecker
        .recheck_due(&state, t0 + Duration::minutes(21))
        .await;
    assert_eq!(probe.asked(), vec![AgentType::Codex]);

    // Back on its own agent: the stored choice goes away.
    let back = stored(&state, "run-recheck-switch").await;
    let context =
        crate::workflows::step_agents::context_with_choice(&back, "analyse", None).unwrap();
    assert!(context
        .get(crate::workflows::step_agents::TRIGGER_CONTEXT_KEY)
        .is_none());
}

#[tokio::test]
async fn switching_the_agent_is_refused_unless_the_run_is_paused_on_a_quota_and_the_agent_is_valid()
{
    let fx = fixture("recheck-refuse", SESSION_LIMIT, 1, THREE_DAYS).await;
    let paused = run_until_paused(&fx, "run-recheck-refuse").await;
    let state = restarted(&fx.state);

    // A named HTTP connection is chosen in the workflow, never by this switch.
    let custom = StepAgentOverride {
        agent: AgentType::Custom,
        model: None,
        reasoning_effort: None,
    };
    let refused =
        crate::api::workflows::switch_quota_agent_inner(&state, paused.id.clone(), custom)
            .await
            .unwrap_err();
    assert!(refused.contains("named HTTP connection"), "{refused}");

    let missing = crate::api::workflows::switch_quota_agent_inner(
        &state,
        "nope".into(),
        StepAgentOverride {
            agent: AgentType::Codex,
            model: None,
            reasoning_effort: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(missing, "Run not found");

    // A run that is not waiting on a quota cannot switch.
    let id = paused.id.clone();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE workflow_runs SET status = 'Success' WHERE id = ?1",
                [&id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let not_waiting = crate::api::workflows::switch_quota_agent_inner(
        &state,
        paused.id.clone(),
        StepAgentOverride {
            agent: AgentType::Codex,
            model: None,
            reasoning_effort: None,
        },
    )
    .await
    .unwrap_err();
    assert!(
        not_waiting.contains("paused on a provider quota"),
        "{not_waiting}"
    );
}
