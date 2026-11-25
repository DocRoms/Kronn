//! KT-2055 — the dispatch half of the connection/key rule: every Agent step
//! `execute_step` starts is refused before any request when its connection or
//! its provider key is unusable, in `step_connection_refusal`'s words. No
//! socket: a refused step never reaches HTTP, and every endpoint is
//! `.invalid`. Every key value is a fake.

use super::*;
use crate::db::external_api_connections::NVIDIA_CREDENTIAL_SLUG;
use crate::models::MultiAgentReviewConfig;

fn no_tokens() -> TokensConfig {
    TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    }
}

fn nvidia_keyed() -> TokensConfig {
    let mut tokens = no_tokens();
    tokens.keys.push(crate::models::ApiKey {
        id: "key-nvidia".into(),
        name: "fixture".into(),
        provider: NVIDIA_CREDENTIAL_SLUG.into(),
        value: "fake-connection-rule-key".into(),
        active: true,
    });
    tokens
}

fn agent_step(agent: AgentType, connection_id: Option<&str>) -> WorkflowStep {
    WorkflowStep {
        name: "jeu".into(),
        step_type: StepType::Agent,
        agent,
        prompt_template: "Answer".into(),
        agent_settings: Some(AgentSettings {
            tier: Some(crate::models::ModelTier::Default),
            connection_id: connection_id.map(str::to_string),
            ..Default::default()
        }),
        ..WorkflowStep::default()
    }
}

fn endpoints() -> crate::models::setup::HttpEndpoints {
    crate::models::setup::HttpEndpoints {
        lite_llm: Some("https://litellm.invalid".into()),
        nvidia: Some("https://nvidia.invalid".into()),
        tools: Default::default(),
    }
}

async fn dispatch(step: &WorkflowStep, tokens: &TokensConfig) -> StepOutcome {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_string_lossy();
    execute_step(
        step,
        &project,
        None,
        &project,
        tokens,
        false,
        &TemplateContext::new(),
        None,
        None,
        None,
        Some(&endpoints()),
        None,
        None,
        None,
        None,
        Some("test-run"),
    )
    .await
}

fn assert_refused_before_any_request(outcome: &StepOutcome) {
    assert_eq!(
        outcome.result.status,
        RunStatus::Failed,
        "{:?}",
        outcome.result
    );
    assert_eq!(
        outcome.result.step_kind.as_deref(),
        Some("preflight_failed"),
        "{:?}",
        outcome.result
    );
    assert_eq!(outcome.result.tokens_used, Some(0));
    assert!(
        !outcome.result.output.contains("invalid"),
        "no request was attempted: {}",
        outcome.result.output
    );
    assert!(
        !outcome.result.output.contains("fake-connection-rule-key"),
        "never echoes a key"
    );
}

#[tokio::test]
async fn connection_rule_a_step_without_its_provider_key_is_refused_at_dispatch() {
    // What a resumed run, an on_failure step or any step the preflight never
    // saw starts with: no NVIDIA key.
    let step = agent_step(AgentType::Nvidia, None);
    let outcome = dispatch(&step, &no_tokens()).await;
    assert_refused_before_any_request(&outcome);
    let expected = step_connection_refusal(&step, None, &no_tokens())
        .await
        .expect("the rule refuses it");
    assert_eq!(
        outcome.result.output,
        format!("preflight_failed:{expected}")
    );
    assert!(expected.contains("no NVIDIA API key"), "{expected}");
}

#[tokio::test]
async fn connection_rule_a_step_on_a_gone_connection_is_refused_at_dispatch() {
    let db = crate::db::Database::open_in_memory().unwrap();
    let step = agent_step(AgentType::Nvidia, Some("conn-gone"));
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_string_lossy();
    let outcome = execute_step(
        &step,
        &project,
        None,
        &project,
        &nvidia_keyed(),
        false,
        &TemplateContext::new(),
        None,
        None,
        None,
        Some(&endpoints()),
        None,
        None,
        Some(&db),
        None,
        Some("test-run"),
    )
    .await;
    assert_eq!(outcome.result.status, RunStatus::Failed);
    assert_eq!(
        outcome.result.step_kind.as_deref(),
        Some("preflight_failed")
    );
    assert!(
        outcome.result.output.contains("'jeu'") && outcome.result.output.contains("conn-gone"),
        "{}",
        outcome.result.output
    );
}

#[tokio::test]
async fn connection_rule_a_keyless_reviewer_is_refused_before_the_author_runs() {
    let mut step = agent_step(AgentType::ClaudeCode, None);
    step.multi_agent_review = Some(MultiAgentReviewConfig {
        reviewer_agent: AgentType::Nvidia,
        reviewer_tier: None,
        debate_prompt: "Review it".into(),
        max_rounds: Some(1),
    });
    let outcome = dispatch(&step, &no_tokens()).await;
    assert_refused_before_any_request(&outcome);
    assert!(
        outcome
            .result
            .output
            .contains("multi_agent_review reviewer (NVIDIA)")
            && outcome.result.output.contains("no NVIDIA API key"),
        "{}",
        outcome.result.output
    );
    // No attempt of the author was recorded: it never started.
    assert!(
        outcome
            .result
            .agent_provenance
            .as_ref()
            .is_none_or(|provenance| provenance.attempts.is_empty()),
        "{:?}",
        outcome.result.agent_provenance
    );
}

#[tokio::test]
async fn connection_rule_the_reviewer_shares_only_its_own_agents_connection() {
    let mut step = agent_step(AgentType::Nvidia, Some("conn-nv"));
    let review = |agent| MultiAgentReviewConfig {
        reviewer_agent: agent,
        reviewer_tier: None,
        debate_prompt: "Review it".into(),
        max_rounds: Some(1),
    };
    let same = reviewer_step_for(&step, &review(AgentType::Nvidia));
    assert_eq!(step_connection_id(&same), Some("conn-nv"));
    let other = reviewer_step_for(&step, &review(AgentType::LiteLlm));
    assert_eq!(step_connection_id(&other), None);
    // A blank connection is never handed on.
    step.agent_settings.as_mut().unwrap().connection_id = Some("  ".into());
    let same = reviewer_step_for(&step, &review(AgentType::Nvidia));
    assert_eq!(
        same.agent_settings.unwrap().connection_id,
        None,
        "a blank id is no connection"
    );
}

#[tokio::test]
async fn connection_rule_a_blank_connection_id_is_no_connection() {
    for blank in ["", "   ", "\t"] {
        let step = agent_step(AgentType::Nvidia, Some(blank));
        assert_eq!(step_connection_id(&step), None);
        assert!(
            resolve_step_connection(&step, None)
                .await
                .unwrap()
                .is_none(),
            "{blank:?} resolves to no connection"
        );
        // Judged on its provider key, as a step naming none.
        let refusal = step_connection_refusal(&step, None, &no_tokens())
            .await
            .expect("no NVIDIA key");
        assert!(refusal.contains("no NVIDIA API key"), "{refusal}");
        assert!(!refusal.contains("not found"), "{refusal}");
        assert!(
            step_connection_refusal(&step, None, &nvidia_keyed())
                .await
                .is_none(),
            "{blank:?} with the NVIDIA key set runs on it"
        );
        // A named-connection step with a blank id names none.
        let custom = agent_step(AgentType::Custom, Some(blank));
        let refusal = step_connection_refusal(&custom, None, &no_tokens())
            .await
            .expect("Custom needs a connection");
        assert!(refusal.contains("names no connection"), "{refusal}");
    }
}

#[tokio::test]
async fn connection_rule_never_refuses_cli_agents_ollama_or_a_keyless_litellm_proxy() {
    for agent in [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Ollama,
        AgentType::LiteLlm,
    ] {
        let step = agent_step(agent.clone(), None);
        assert!(
            step_connection_refusal(&step, None, &no_tokens())
                .await
                .is_none(),
            "{agent:?} must not be refused"
        );
    }
}
