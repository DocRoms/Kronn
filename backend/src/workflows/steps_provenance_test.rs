use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn step() -> WorkflowStep {
    WorkflowStep {
        name: "provenance".into(),
        step_type: StepType::Agent,
        agent: AgentType::LiteLlm,
        prompt_template: "Return a result".into(),
        agent_settings: Some(AgentSettings {
            model: Some("proxy-alias".into()),
            tier: None,
            connection_id: None,
            reasoning_effort: None,
            max_tokens: None,
        }),
        ..WorkflowStep::default()
    }
}

fn reply(text: &str, model: &str) -> (u16, serde_json::Value) {
    (
        200,
        json!({"model":model,"choices":[{"message":{"content":text},"finish_reason":"stop"}]}),
    )
}

fn typed(step: &mut WorkflowStep, policy: OnInvalid) {
    step.output_format = StepOutputFormat::TypedSchema {
        schema: json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}),
        on_invalid: policy,
    };
}

async fn run(step: &WorkflowStep, replies: Vec<(u16, serde_json::Value)>) -> StepResult {
    let server = MockServer::start().await;
    let count = replies.len();
    let next = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            let index = next.fetch_add(1, Ordering::SeqCst);
            let (status, body) = replies.get(index).expect("unexpected provider request");
            ResponseTemplate::new(*status).set_body_json(body)
        })
        .expect(count as u64)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_string_lossy();
    let tokens = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![],
        disabled_overrides: vec![],
    };
    let mut tiers = crate::models::setup::ModelTiersConfig::default();
    tiers.lite_llm.default = Some("default-tier-model".into());
    tiers.lite_llm.reasoning = Some("reasoning-tier-model".into());
    let mut result = execute_step(
        step,
        &project,
        &project,
        &tokens,
        false,
        &TemplateContext::new(),
        "",
        None,
        Some(&tiers),
        Some(&crate::models::setup::HttpEndpoints {
            lite_llm: Some(server.uri()),
            nvidia: None,
        }),
        None,
        None,
        None,
    )
    .await
    .result;
    // A later config change must not overwrite what these attempts captured.
    tiers.lite_llm.default = Some("changed-after-run".into());
    super::super::runner::apply_step_snapshot(step, &mut result, Some(&tiers));
    let persisted = serde_json::to_string(&result).unwrap();
    assert!(
        !persisted.contains(&server.uri()),
        "endpoint is not provenance"
    );
    serde_json::from_str(&persisted).unwrap()
}

#[tokio::test]
async fn provenance_distinguishes_override_inherited_tier_and_observed_model() {
    for (explicit, tier, expected) in [
        (Some("proxy-alias"), ModelTier::Reasoning, "proxy-alias"),
        (None, ModelTier::Reasoning, "reasoning-tier-model"),
        (None, ModelTier::Default, "default-tier-model"),
    ] {
        let mut step = step();
        let settings = step.agent_settings.as_mut().unwrap();
        settings.model = explicit.map(str::to_owned);
        settings.tier = Some(tier);
        let result = run(&step, vec![reply("answer", "served-model")]).await;
        let provenance = result.agent_provenance.as_ref().unwrap();
        assert_eq!(provenance.selected_attempt, Some(1));
        let attempt = &provenance.attempts[0];
        assert_eq!(attempt.requested_model.as_deref(), explicit);
        assert_eq!(attempt.resolved_model.as_deref(), Some(expected));
        assert_eq!(attempt.observed_models, ["served-model"]);
        assert_eq!(attempt.model_applied, Some(true));
        assert!(attempt.succeeded);
        assert!(result
            .step_model
            .as_deref()
            .unwrap()
            .starts_with("served-model"));
    }
}

#[tokio::test]
async fn provenance_selects_repair_only_when_its_output_is_retained() {
    for policy in [OnInvalid::Continue, OnInvalid::Fail] {
        for valid_repair in [false, true] {
            let mut step = step();
            typed(&mut step, policy);
            let text = if valid_repair {
                r#"{"data":{"ok":true},"status":"OK"}"#
            } else {
                "still invalid"
            };
            let result = run(
                &step,
                vec![
                    reply("initial invalid", "initial-model"),
                    reply(text, "repair-model"),
                ],
            )
            .await;
            let provenance = result.agent_provenance.as_ref().unwrap();
            assert_eq!(
                provenance
                    .attempts
                    .iter()
                    .map(|a| a.role)
                    .collect::<Vec<_>>(),
                [
                    WorkflowAgentAttemptRole::Initial,
                    WorkflowAgentAttemptRole::Repair
                ]
            );
            assert_eq!(
                provenance.selected_attempt,
                Some(if valid_repair { 2 } else { 1 })
            );
            assert_eq!(
                result.step_model.as_deref(),
                Some(if valid_repair {
                    "repair-model"
                } else {
                    "initial-model"
                })
            );
            assert_eq!(
                result.status,
                if !valid_repair && policy == OnInvalid::Fail {
                    RunStatus::Failed
                } else {
                    RunStatus::Success
                }
            );
        }
    }
}

#[tokio::test]
async fn provenance_keeps_failed_launch_and_format_negotiation_in_one_attempt() {
    let rejected = (400, json!({"error":{"message":"fixture request refused"}}));
    let result = run(&step(), vec![rejected.clone()]).await;
    let provenance = result.agent_provenance.as_ref().unwrap();
    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(provenance.selected_attempt, None);
    assert_eq!(provenance.attempts.len(), 1);
    assert!(!provenance.attempts[0].succeeded);
    assert_eq!(
        provenance.attempts[0].resolved_model.as_deref(),
        Some("proxy-alias")
    );
    assert_eq!(result.step_agent, Some(AgentType::LiteLlm));
    assert_eq!(result.step_model.as_deref(), Some("proxy-alias"));
    for fallback_succeeds in [false, true] {
        let mut step = step();
        typed(&mut step, OnInvalid::Fail);
        let fallback = if fallback_succeeds {
            reply(r#"{"data":{"ok":true},"status":"OK"}"#, "served-model")
        } else {
            rejected.clone()
        };
        let result = run(
            &step,
            vec![
                (
                    501,
                    json!({"error":{"message":"structured output is unavailable"}}),
                ),
                fallback,
            ],
        )
        .await;
        let provenance = result.agent_provenance.as_ref().unwrap();
        assert_eq!(provenance.attempts.len(), 1);
        assert!(provenance.attempts[0].format_fallback);
        assert_eq!(provenance.attempts[0].succeeded, fallback_succeeds);
        assert_eq!(provenance.selected_attempt, fallback_succeeds.then_some(1));
        assert_eq!(result.step_agent, Some(AgentType::LiteLlm));
        assert_eq!(
            result.step_model.as_deref(),
            Some(if fallback_succeeds {
                "served-model"
            } else {
                "proxy-alias"
            })
        );
    }
}

#[tokio::test]
async fn provenance_debate_never_attributes_a_reviewer_to_the_retained_output() {
    for accepted in [false, true] {
        let mut step = step();
        typed(&mut step, OnInvalid::Fail);
        step.multi_agent_review = Some(MultiAgentReviewConfig {
            reviewer_agent: AgentType::LiteLlm,
            reviewer_tier: None,
            debate_prompt: "Review it".into(),
            max_rounds: Some(1),
        });
        let author = if accepted {
            r#"{"data":{"ok":false},"status":"OK"}"#
        } else {
            "invalid author output"
        };
        let result = run(
            &step,
            vec![
                reply(r#"{"data":{"ok":true},"status":"OK"}"#, "initial-model"),
                reply("Please revise", "review-model"),
                reply(author, "author-model"),
            ],
        )
        .await;
        let provenance = result.agent_provenance.as_ref().unwrap();
        assert_eq!(
            provenance
                .attempts
                .iter()
                .map(|a| a.role)
                .collect::<Vec<_>>(),
            [
                WorkflowAgentAttemptRole::Initial,
                WorkflowAgentAttemptRole::Review,
                WorkflowAgentAttemptRole::Author
            ]
        );
        assert_eq!(
            provenance.selected_attempt,
            Some(if accepted { 3 } else { 1 })
        );
        assert_eq!(
            result.step_model.as_deref(),
            Some(if accepted {
                "author-model"
            } else {
                "initial-model"
            })
        );
        assert_eq!(
            provenance.attempts[1].resolved_model.as_deref(),
            Some("default-tier-model")
        );
    }
}

#[test]
fn provenance_is_absent_in_a_legacy_row() {
    let result: StepResult = serde_json::from_value(json!({"step_name":"old", "status":"Success",
        "output":"old answer", "duration_ms":0, "tokens_used":0, "step_agent":"Ollama", "step_model":"historic-model"})).unwrap();
    assert!(result.agent_provenance.is_none());
    assert_eq!(result.step_model.as_deref(), Some("historic-model"));
    assert!(serde_json::to_value(result)
        .unwrap()
        .get("agent_provenance")
        .is_none());
}

#[test]
fn provenance_snapshot_keeps_escalation_after_config_changes_and_unknown_acp_defaults() {
    let mut result: StepResult =
        serde_json::from_value(json!({"step_name":"provenance", "status":"Success",
        "output":"retained escalation", "duration_ms":0, "tokens_used":0}))
        .unwrap();
    result.agent_provenance = Some(Box::new(WorkflowAgentProvenance {
        selected_attempt: Some(3),
        attempts: vec![WorkflowAgentAttempt {
            id: 3,
            role: WorkflowAgentAttemptRole::Escalation,
            retry: 1,
            agent: AgentType::ClaudeCode,
            tier: ModelTier::Reasoning,
            connection_id: None,
            requested_model: None,
            resolved_model: Some("launch-alias".into()),
            model_applied: Some(true),
            observed_models: vec!["served-claude".into()],
            format_fallback: false,
            started_at: chrono::Utc::now(),
            duration_ms: 20,
            succeeded: true,
        }],
    }));
    let mut step = step();
    step.agent = AgentType::Ollama;
    step.agent_settings.as_mut().unwrap().model = Some("new-config-local-model".into());
    let mut ctx = TemplateContext::new();
    super::super::runner::record_step_completion(&step, &mut result, &mut ctx, None);
    assert_eq!(result.step_agent, Some(AgentType::ClaudeCode));
    assert_eq!(
        result.step_model.as_deref(),
        Some("served-claude · reasoning")
    );
    assert_eq!(
        ctx.render("{{steps.provenance.agent}}").unwrap(),
        "ClaudeCode"
    );
    let attempt = &mut result.agent_provenance.as_mut().unwrap().attempts[0];
    attempt.agent = AgentType::OpenCode;
    attempt.observed_models.clear();
    attempt.model_applied = Some(false);
    super::super::runner::apply_step_snapshot(&step, &mut result, None);
    assert_eq!(result.step_agent, Some(AgentType::OpenCode));
    assert!(
        result.step_model.is_none(),
        "unapplied request is not an observed native ACP default"
    );
    let provenance = result.agent_provenance.as_mut().unwrap();
    provenance.selected_attempt = None;
    provenance.attempts.clear();
    super::super::runner::apply_step_snapshot(&step, &mut result, None);
    assert!(result.step_agent.is_none());
    assert!(
        result.step_model.is_none(),
        "no launch means no tried model"
    );
}

#[tokio::test]
async fn provenance_retains_a_failed_repair_without_selecting_it() {
    let mut step = step();
    typed(&mut step, OnInvalid::Continue);
    let result = run(
        &step,
        vec![
            reply("initial invalid", "initial-model"),
            (400, json!({"error":{"message":"repair refused"}})),
        ],
    )
    .await;
    let provenance = result.agent_provenance.unwrap();
    assert_eq!(provenance.selected_attempt, Some(1));
    assert_eq!(provenance.attempts.len(), 2);
    assert_eq!(
        provenance.attempts[1].role,
        WorkflowAgentAttemptRole::Repair
    );
    assert!(!provenance.attempts[1].succeeded);
    assert!(provenance.attempts[1].observed_models.is_empty());
    assert_eq!(result.output, "initial invalid");
}

#[tokio::test]
async fn provenance_retains_failed_retry_before_the_successful_launch() {
    let mut step = step();
    step.retry = Some(RetryConfig {
        max_retries: 1,
        backoff: "exponential".into(),
    });
    let result = run(
        &step,
        vec![
            (400, json!({"error":{"message":"first attempt refused"}})),
            reply("retry answer", "served-model"),
        ],
    )
    .await;
    let provenance = result.agent_provenance.unwrap();
    assert_eq!(provenance.selected_attempt, Some(2));
    assert_eq!(
        provenance
            .attempts
            .iter()
            .map(|a| (a.retry, a.succeeded))
            .collect::<Vec<_>>(),
        [(1, false), (2, true)]
    );
}
