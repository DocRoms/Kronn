//! KT-1518 — an Agent step whose provider cannot serve it continues on the
//! agent's fallback chain; a failure of the work itself never does.

use super::*;
use crate::core::agent_fallback as fallback;
use crate::models::{AgentFallback, AgentSettings, ModelTier, RunStatus, StepResult};
use std::sync::Arc;

fn link(agent: AgentType) -> AgentFallback {
    AgentFallback {
        agent,
        model: None,
        mode: None,
    }
}

fn everyone() -> Vec<AgentType> {
    vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::LiteLlm,
    ]
}

fn state_with(first: Option<AgentFallback>) -> (AppState, AgentsConfig) {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain = first.into_iter().collect();
    // The chain of 0.15.3, carried onto the rows the way a start-up does.
    fallback::migrate_legacy_chains(&mut config.agents);
    let agents = config.agents.clone();
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (state, agents)
}

/// An economy step on Claude Code with an explicit model of its own.
fn step() -> WorkflowStep {
    WorkflowStep {
        name: "analyse".into(),
        step_type: StepType::Agent,
        agent: AgentType::ClaudeCode,
        prompt_template: "analyse the diff".into(),
        agent_settings: Some(AgentSettings {
            model: Some("claude-opus-4".into()),
            tier: Some(ModelTier::Economy),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn outcome(status: RunStatus, output: &str) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: "analyse".into(),
            status,
            output: output.into(),
            tokens_used: Some(0),
            duration_ms: 0,
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: None,
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
            output_bytes: None,
        },
        condition_action: None,
    }
}

/// A provider probe that records what it was asked and answers `answer`.
struct FakeProbe {
    answer: Option<bool>,
    asked: std::sync::Mutex<Vec<(AgentType, Option<String>)>>,
}

impl FakeProbe {
    fn answering(answer: Option<bool>) -> Self {
        Self {
            answer,
            asked: Default::default(),
        }
    }

    fn asked(&self) -> Vec<(AgentType, Option<String>)> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl QuotaProbe for FakeProbe {
    async fn quota_is_back(
        &self,
        agent: &AgentType,
        _project_path: &str,
        _tokens: &crate::models::TokensConfig,
        _full_access: bool,
        model: Option<&str>,
    ) -> Option<bool> {
        self.asked
            .lock()
            .unwrap()
            .push((agent.clone(), model.map(str::to_string)));
        self.answer
    }
}

const FABLE: &str = "claude-fable-5-2[1m]";

/// The refusal the provider sent for Fable on 2026-11-23 (KT-2048).
const FABLE_REFUSAL: &str = "[Agent provider error] You've hit your Fable limit · resets Nov 28 at 8pm (Europe/Paris). Switch to another model, or manage usage credits at https://claude.ai/admin-settings/usage, to continue. (HTTP 429; terminal_reason=api_error)";

/// A Claude Code step on `model`, the tier left at its default.
fn claude_step_on(model: &str) -> WorkflowStep {
    WorkflowStep {
        agent_settings: Some(AgentSettings {
            model: Some(model.into()),
            ..Default::default()
        }),
        ..step()
    }
}

async fn cooldown(state: &AppState, agent: AgentType) -> Option<fallback::Cooldown> {
    state
        .db
        .with_read_conn(move |conn| fallback::active_cooldown(conn, &agent, chrono::Utc::now()))
        .await
        .unwrap()
}

fn rerun(next: Next) -> (WorkflowStep, String) {
    match next {
        Next::Rerun { step, notice } => (*step, notice),
        Next::Keep => panic!("expected a rerun, the outcome was kept"),
        Next::Stopped { notice } => panic!("expected a rerun, stopped: {notice}"),
    }
}

#[tokio::test]
async fn each_provider_side_trigger_reruns_the_step_on_the_fallback_with_the_same_tier() {
    let triggers = [
        (
            "Provider quota or session limit: You've hit your limit · resets 11:59pm (UTC)",
            "quota exhausted",
        ),
        (
            "HTTP 429 Too Many Requests: rate_limit_exceeded",
            "rate limited",
        ),
        ("API Error: 503 Service Unavailable", "provider unavailable"),
        (
            "Invalid authentication credentials (api error: 401)",
            "not authenticated",
        ),
    ];
    for (error, words) in triggers {
        let (state, agents) = state_with(Some(link(AgentType::Codex)));
        let failed = outcome(RunStatus::Failed, error);
        let (moved, notice) = rerun(
            continue_after_among(
                &state,
                &agents,
                &AgentType::ClaudeCode,
                &step(),
                &failed,
                Some(everyone()),
            )
            .await,
        );
        assert_eq!(moved.agent, AgentType::Codex, "{error}");
        let settings = moved.agent_settings.as_ref().unwrap();
        assert_eq!(
            settings.tier,
            Some(ModelTier::Economy),
            "{error}: same tier"
        );
        assert_eq!(
            settings.model, None,
            "{error}: the refused agent's model is never carried over"
        );
        assert_eq!(moved.prompt_template, "analyse the diff");
        assert!(
            notice.starts_with(&format!("Claude Code · claude-opus-4: {words}"))
                && notice.ends_with("→ continued with Codex · eco mode [CLI]"),
            "{notice}"
        );
        assert!(
            cooldown(&state, AgentType::ClaudeCode).await.is_some(),
            "{error}"
        );
    }
}

#[tokio::test]
async fn the_links_own_model_replaces_the_tier_on_the_rerun() {
    let named = AgentFallback {
        model: Some("gpt-5.6-sol".into()),
        ..link(AgentType::Codex)
    };
    let (state, agents) = state_with(Some(named));
    let failed = outcome(RunStatus::Failed, "HTTP 429 Too Many Requests");
    let (moved, notice) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &failed,
            Some(everyone()),
        )
        .await,
    );
    assert_eq!(
        moved.agent_settings.unwrap().model.as_deref(),
        Some("gpt-5.6-sol")
    );
    assert!(
        notice.ends_with("continued with Codex · gpt-5.6-sol [CLI]"),
        "{notice}"
    );
}

#[tokio::test]
async fn a_task_failure_a_refusal_a_test_failure_or_a_work_timeout_never_triggers_a_fallback() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    for failure in [
        "test result: FAILED. 12 passed; 3 failed; 0 ignored",
        "I can't help with that request.",
        "Document audit preflight failed; no agent launched, files preserved: nothing",
        "Agent stalled (no output for 300s)",
        "Agent stalled (no output for 300s) — last line: HTTP 429 Too Many Requests",
        "Step failed after 3 attempts: exit status 1",
    ] {
        let failed = outcome(RunStatus::Failed, failure);
        assert!(
            matches!(
                continue_after_among(
                    &state,
                    &agents,
                    &AgentType::ClaudeCode,
                    &step(),
                    &failed,
                    Some(everyone()),
                )
                .await,
                Next::Keep
            ),
            "{failure}"
        );
    }
    assert!(cooldown(&state, AgentType::ClaudeCode).await.is_none());
    // A step that succeeded is never moved, whatever its output says.
    let succeeded = outcome(RunStatus::Success, "HTTP 429 is what the report is about");
    assert!(matches!(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &succeeded,
            Some(everyone()),
        )
        .await,
        Next::Keep
    ));
}

#[tokio::test]
async fn a_step_whose_agent_has_no_fallback_fails_or_waits_as_before() {
    let (state, agents) = state_with(None);
    let failed = outcome(RunStatus::Failed, "HTTP 429 Too Many Requests");
    assert!(matches!(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &failed,
            Some(everyone()),
        )
        .await,
        Next::Keep
    ));
    assert!(cooldown(&state, AgentType::ClaudeCode).await.is_none());
}

#[tokio::test]
async fn a_fallback_billed_by_api_key_is_used_without_any_opt_in() {
    let (state, agents) = state_with(Some(link(AgentType::LiteLlm)));
    let failed = outcome(RunStatus::Failed, "HTTP 429 Too Many Requests");
    let (moved, notice) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &failed,
            Some(everyone()),
        )
        .await,
    );
    assert_eq!(moved.agent, AgentType::LiteLlm);
    assert!(
        notice.ends_with("→ continued with LiteLLM · eco mode [HTTP]"),
        "{notice}"
    );
}

#[tokio::test]
async fn a_fallback_on_its_own_row_chains_when_the_first_one_is_unavailable() {
    // Claude Code → Codex, and Codex → Gemini CLI on Codex's own row.
    let (state, mut agents) = state_with(Some(link(AgentType::Codex)));
    fallback::set_row(
        &mut agents,
        &AgentType::Codex,
        crate::models::FallbackMode::Eco,
        Some(link(AgentType::GeminiCli)),
    );
    let failed = outcome(RunStatus::Failed, "HTTP 429 Too Many Requests");
    let (moved, _) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &failed,
            Some(everyone()),
        )
        .await,
    );
    assert_eq!(moved.agent, AgentType::Codex);
    // Codex refuses in its turn: the fallback of ITS row takes over.
    let mut on_codex = moved.clone();
    on_codex.agent = AgentType::Codex;
    let (next, notice) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &on_codex,
            &failed,
            Some(everyone()),
        )
        .await,
    );
    assert_eq!(next.agent, AgentType::GeminiCli);
    assert!(notice.contains("→ continued with Gemini CLI"), "{notice}");
}

#[tokio::test]
async fn a_step_starts_on_the_fallback_while_its_agent_cools_down_and_returns_after() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let silent = FakeProbe::answering(None);
    assert!(
        start_on_fallback_with(&state, &silent, &agents, &step(), Some(everyone()), "")
            .await
            .is_none()
    );

    let now = chrono::Utc::now();
    state
        .db
        .with_conn(move |conn| {
            fallback::set_cooldown(
                conn,
                &AgentType::ClaudeCode,
                fallback::Trigger::QuotaExhausted,
                Some(now + chrono::Duration::hours(2)),
                now,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (moved, notice) =
        start_on_fallback_with(&state, &silent, &agents, &step(), Some(everyone()), "")
            .await
            .expect("straight to the fallback");
    assert_eq!(moved.agent, AgentType::Codex);
    assert!(
        notice.starts_with("Claude Code · claude-opus-4: quota exhausted until ")
            && notice.ends_with("→ continued with Codex · eco mode [CLI]"),
        "{notice}"
    );

    // The reset passes: the step's own agent again, nothing to undo.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE agent_fallback_cooldowns SET until = ?1",
                [(chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        start_on_fallback_with(&state, &silent, &agents, &step(), Some(everyone()), "")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_model_refusal_cools_only_the_model_it_names_and_the_other_models_run_as_configured() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let fable = claude_step_on("claude-fable-5-2[1m]");
    let (after, _) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &fable,
            &outcome(RunStatus::Failed, FABLE_REFUSAL),
            Some(everyone()),
        )
        .await,
    );
    assert_eq!(after.agent, AgentType::Codex);

    // The row is scoped to the refused model, not to the agent.
    assert!(cooldown(&state, AgentType::ClaudeCode).await.is_none());
    let refused = state
        .db
        .with_read_conn(|conn| {
            fallback::active_cooldown_for(
                conn,
                &AgentType::ClaudeCode,
                "claude-fable-5-2[1m]",
                chrono::Utc::now(),
            )
        })
        .await
        .unwrap();
    assert!(refused.is_some(), "the refused model is cooled");

    // A step on another model of the same agent starts on Claude Code.
    let probe = FakeProbe::answering(Some(false));
    let opus = claude_step_on("claude-opus-5-5");
    assert!(
        start_on_fallback_with(&state, &probe, &agents, &opus, Some(everyone()), "")
            .await
            .is_none()
    );
    assert!(
        probe.asked().is_empty(),
        "nothing cools that model: no probe"
    );

    // The Fable step itself is asked on Fable before it leaves Claude Code.
    let (moved, notice) =
        start_on_fallback_with(&state, &probe, &agents, &fable, Some(everyone()), "")
            .await
            .expect("the refused model still refuses: the fallback");
    assert_eq!(moved.agent, AgentType::Codex);
    assert_eq!(
        probe.asked(),
        vec![(
            AgentType::ClaudeCode,
            Some("claude-fable-5-2[1m]".to_string())
        )]
    );
    assert!(notice.contains("claude-fable-5-2[1m]"), "{notice}");
}

/// KT-2048 — a Claude Code step left on the tier default: no model and no tier
/// named, so it resolves its model from the tier (opus on the default tier).
#[tokio::test]
async fn a_step_on_the_default_tier_is_not_held_by_a_fable_refusal() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let fable = claude_step_on(FABLE);
    rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &fable,
            &outcome(RunStatus::Failed, FABLE_REFUSAL),
            Some(everyone()),
        )
        .await,
    );

    let default_step = WorkflowStep {
        agent_settings: None,
        ..step()
    };
    let resolved = crate::workflows::agent_fallback_step::scope_model(&default_step, &agents);
    assert_ne!(resolved, FABLE, "the default tier is not the refused model");

    // Not held: the step runs as configured, and nothing is asked or moved.
    let probe = FakeProbe::answering(Some(false));
    assert!(
        start_on_fallback_with(&state, &probe, &agents, &default_step, Some(everyone()), "")
            .await
            .is_none()
    );
    assert!(probe.asked().is_empty(), "no provider question about Fable");
}

#[tokio::test]
async fn a_step_that_would_leave_its_agent_is_asked_on_its_own_model_first() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let model = "claude-fable-probe-a";
    let now = chrono::Utc::now();
    state
        .db
        .with_conn(move |conn| {
            fallback::set_scoped_cooldown(
                conn,
                &AgentType::ClaudeCode,
                model,
                fallback::Trigger::QuotaExhausted,
                Some(now + chrono::Duration::hours(2)),
                now,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let step = claude_step_on(model);

    // The provider answers on that model: the cooldown clears, the step runs as configured.
    let answering = FakeProbe::answering(Some(true));
    assert!(start_on_fallback_with(
        &state,
        &answering,
        &agents,
        &step,
        Some(everyone()),
        "/repo"
    )
    .await
    .is_none());
    assert_eq!(
        answering.asked(),
        vec![(AgentType::ClaudeCode, Some(model.to_string()))]
    );
    assert!(state
        .db
        .with_read_conn(move |conn| fallback::active_cooldown_for(
            conn,
            &AgentType::ClaudeCode,
            model,
            chrono::Utc::now()
        ))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_probe_that_still_refuses_keeps_the_step_on_the_fallback_and_is_not_repeated() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let model = "claude-fable-probe-b";
    let now = chrono::Utc::now();
    state
        .db
        .with_conn(move |conn| {
            fallback::set_scoped_cooldown(
                conn,
                &AgentType::ClaudeCode,
                model,
                fallback::Trigger::QuotaExhausted,
                Some(now + chrono::Duration::hours(2)),
                now,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let step = claude_step_on(model);
    let refusing = FakeProbe::answering(Some(false));
    let (moved, _) =
        start_on_fallback_with(&state, &refusing, &agents, &step, Some(everyone()), "")
            .await
            .expect("a refusing provider keeps the fallback");
    assert_eq!(moved.agent, AgentType::Codex);
    assert_eq!(refusing.asked().len(), 1);

    // Asked once per interval: the next start does not ask again.
    let again = FakeProbe::answering(Some(true));
    let (moved, _) = start_on_fallback_with(&state, &again, &agents, &step, Some(everyone()), "")
        .await
        .expect("still on the fallback: the provider is not asked twice");
    assert_eq!(moved.agent, AgentType::Codex);
    assert!(again.asked().is_empty());
}

#[tokio::test]
async fn a_paused_run_whose_provider_still_refuses_is_offered_the_same_chain() {
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let offered = link_for_waiting_step_among(&state, &agents, &step(), Some(everyone())).await;
    assert_eq!(offered, Some(link(AgentType::Codex)));
    // Nothing installed to take it: no offer, the run keeps waiting.
    let alone =
        link_for_waiting_step_among(&state, &agents, &step(), Some(vec![AgentType::ClaudeCode]))
            .await;
    assert_eq!(alone, None);
    let (state, agents) = state_with(None);
    assert_eq!(
        link_for_waiting_step_among(&state, &agents, &step(), Some(everyone())).await,
        None
    );
}

#[test]
fn the_notices_lead_the_steps_output_as_quoted_lines() {
    assert_eq!(with_notices("result".into(), &[]), "result");
    assert_eq!(
        with_notices(
            "result".into(),
            &[
                "Claude Code · eco mode: rate limited → continued with Codex · eco mode [CLI]"
                    .into()
            ]
        ),
        "> Claude Code · eco mode: rate limited → continued with Codex · eco mode [CLI]\n\nresult"
    );
}

#[tokio::test]
async fn a_steps_own_chain_replaces_the_default_and_an_empty_one_cancels_it() {
    use crate::models::AgentSettings;
    // The agent's default chain is Codex; this step says Gemini CLI.
    let (state, agents) = state_with(Some(link(AgentType::Codex)));
    let mut own = step();
    own.agent_settings = Some(AgentSettings {
        fallback: Some(vec![link(AgentType::GeminiCli)]),
        ..own.agent_settings.clone().unwrap()
    });
    let failed = outcome(RunStatus::Failed, "HTTP 429 Too Many Requests");
    let (moved, notice) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &own,
            &failed,
            Some(everyone()),
        )
        .await,
    );
    assert_eq!(moved.agent, AgentType::GeminiCli);
    assert!(notice.contains("continued with Gemini CLI"), "{notice}");

    // `Some([])` cancels the fallback for this step: the failure stands.
    let mut cancelled = step();
    cancelled.agent_settings = Some(AgentSettings {
        fallback: Some(Vec::new()),
        ..cancelled.agent_settings.clone().unwrap()
    });
    assert!(matches!(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &cancelled,
            &failed,
            Some(everyone()),
        )
        .await,
        Next::Keep
    ));
}

// ─── KT-2055: a link the dispatch rule refuses is skipped ───────────────

/// Claude Code falling back along `chain`; NVIDIA keyed or not.
async fn nvidia_chain(nvidia_key: bool, chain: Vec<AgentFallback>) -> (AppState, AgentsConfig) {
    let (state, _) = state_with(None);
    let agents = {
        let mut config = state.config.write().await;
        config.agents.claude_code.fallback_chain = chain;
        fallback::migrate_legacy_chains(&mut config.agents);
        if nvidia_key {
            config.tokens.keys.push(crate::models::ApiKey {
                id: "key-nvidia".into(),
                name: "fixture".into(),
                provider: crate::db::external_api_connections::NVIDIA_CREDENTIAL_SLUG.into(),
                value: "fake-fallback-nvidia-key".into(),
                active: true,
            });
        }
        config.agents.clone()
    };
    (state, agents)
}

fn with_nvidia() -> Vec<AgentType> {
    let mut usable = everyone();
    usable.push(AgentType::Nvidia);
    usable
}

const QUOTA: &str = "Provider quota or session limit: You've hit your limit · resets 11:59pm (UTC)";

#[tokio::test]
async fn a_fallback_link_without_its_provider_key_is_skipped_like_one_that_cannot_launch() {
    let failed = outcome(RunStatus::Failed, QUOTA);
    let (state, agents) =
        nvidia_chain(false, vec![link(AgentType::Nvidia), link(AgentType::Codex)]).await;
    let (moved, _) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &failed,
            Some(with_nvidia()),
        )
        .await,
    );
    assert_eq!(moved.agent, AgentType::Codex, "keyless NVIDIA is skipped");

    // NVIDIA alone: nothing can take over, and the stop says why.
    let (state, agents) = nvidia_chain(false, vec![link(AgentType::Nvidia)]).await;
    match continue_after_among(
        &state,
        &agents,
        &AgentType::ClaudeCode,
        &step(),
        &failed,
        Some(with_nvidia()),
    )
    .await
    {
        Next::Stopped { notice } => {
            assert!(notice.contains("no NVIDIA API key"), "{notice}");
        }
        Next::Keep => panic!("a keyless NVIDIA link must stop the walk, not keep"),
        Next::Rerun { step, .. } => panic!("a keyless NVIDIA link was taken: {:?}", step.agent),
    }

    // The same link with its key is taken.
    let (state, agents) =
        nvidia_chain(true, vec![link(AgentType::Nvidia), link(AgentType::Codex)]).await;
    let (moved, _) = rerun(
        continue_after_among(
            &state,
            &agents,
            &AgentType::ClaudeCode,
            &step(),
            &failed,
            Some(with_nvidia()),
        )
        .await,
    );
    assert_eq!(moved.agent, AgentType::Nvidia, "a keyed NVIDIA link runs");
}

#[tokio::test]
async fn a_waiting_step_is_never_offered_a_link_without_its_provider_key() {
    let (state, agents) = nvidia_chain(false, vec![link(AgentType::Nvidia)]).await;
    assert!(
        link_for_waiting_step_among(&state, &agents, &step(), Some(with_nvidia()))
            .await
            .is_none()
    );
    let (state, agents) = nvidia_chain(true, vec![link(AgentType::Nvidia)]).await;
    assert_eq!(
        link_for_waiting_step_among(&state, &agents, &step(), Some(with_nvidia()))
            .await
            .map(|link| link.agent),
        Some(AgentType::Nvidia)
    );
}
