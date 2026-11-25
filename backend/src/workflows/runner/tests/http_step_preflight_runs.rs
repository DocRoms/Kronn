//! KT-2045 — an Agent step on a native HTTP provider (`LiteLlm`, `Nvidia`) is
//! refused by the run preflight, before any step runs, when the run would
//! refuse it for its named connection (gone, endpoint-less, keyless where the
//! preset needs a key) or, without a connection, for its provider key. A
//! valid pair still starts; a CLI step keeps today's check.
//!
//! No socket and no mock server (the gate runs in a sandbox that refuses
//! loopback): a refused run never reaches HTTP, and the "still starts" run
//! pauses on a Gate placed before its HTTP steps. Every key value is a fake.

use super::*;
use crate::db::external_api_connections::NVIDIA_CREDENTIAL_SLUG;
use crate::models::{ExternalApiConnection, ExternalApiConnectionPreset};

const FAKE_KEY: &str = "fake-http-step-preflight-key";

fn connection(
    id: &str,
    endpoint: Option<&str>,
    preset: ExternalApiConnectionPreset,
) -> ExternalApiConnection {
    let now = chrono::Utc::now();
    ExternalApiConnection {
        id: id.into(),
        display_name: format!("{id} display"),
        mention_alias: id.into(),
        endpoint: endpoint.map(str::to_string),
        credential_slug: format!("credential-{id}"),
        origin_preset: preset,
        economy_model: None,
        default_model: Some("vendor/model-fixture".into()),
        reasoning_model: None,
        created_at: now,
        updated_at: now,
        image_model: None,
        video_model: None,
        decision_model: None,
        tools_enabled: Some(false),
        media_endpoint: None,
    }
}

async fn insert_connection(state: &crate::AppState, connection: ExternalApiConnection) {
    let target = crate::db::model_catalog::http_runtime_target_id(&connection.id);
    let agent = crate::db::external_api_connections::target_for_connection(&connection).agent_type;
    let model = connection.default_model.clone().unwrap();
    state
        .db
        .with_conn(move |conn| {
            crate::db::external_api_connections::insert(conn, &connection)?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &agent,
                &[crate::db::model_catalog::DiscoveredModel {
                    model_id: model.clone(),
                    display_name: model,
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: Vec::new(),
                    default_reasoning_mode: None,
                }],
            )
        })
        .await
        .unwrap();
}

fn api_key(provider: &str) -> crate::models::ApiKey {
    crate::models::ApiKey {
        id: format!("key-{provider}"),
        name: "fixture key".into(),
        provider: provider.into(),
        value: FAKE_KEY.into(),
        active: true,
    }
}

fn no_tokens() -> TokensConfig {
    TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    }
}

fn http_step(name: &str, agent: AgentType, connection_id: Option<&str>) -> WorkflowStep {
    let mut step = fake_step(name);
    step.agent = agent;
    step.prompt_template = "Answer".into();
    step.agent_settings = Some(crate::models::AgentSettings {
        tier: Some(crate::models::ModelTier::Default),
        connection_id: connection_id.map(str::to_string),
        ..Default::default()
    });
    step
}

fn gate(name: &str) -> WorkflowStep {
    let mut gate = fake_step(name);
    gate.step_type = StepType::Gate;
    gate
}

struct Setup {
    connections: Vec<ExternalApiConnection>,
    /// Providers (credential slugs) holding a fake active key.
    keys: Vec<String>,
}

/// Runs `steps` after a Gate named `porte`: a step that ran before the
/// preflight refused would leave the Gate's pause, never `__preflight__` alone.
async fn run_after_gate(id: &str, steps: Vec<WorkflowStep>, setup: Setup) -> WorkflowRun {
    let (state, mut tokens, mut agents) = test_state_and_configs();
    // A declared LiteLLM proxy makes `LiteLlm` launchable without a local
    // binary, so the refusals below are about the connection or the key only.
    agents.lite_llm.base_url = Some("https://litellm-proxy.invalid".into());
    for connection in setup.connections {
        insert_connection(&state, connection).await;
    }
    for provider in &setup.keys {
        tokens.keys.push(api_key(provider));
        state
            .config
            .write()
            .await
            .tokens
            .keys
            .push(api_key(provider));
    }
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("wf-{id}");
    wf.steps = std::iter::once(gate("porte")).chain(steps).collect();
    let mut run = pending_run(&format!("run-{id}"), &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    run
}

/// The refusal, asserting it came before any step ran and names `step`.
fn refused_before_any_step<'a>(run: &'a WorkflowRun, step: &str) -> &'a str {
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert_eq!(
        run.step_results.len(),
        1,
        "no step may run before the refusal: {:?}",
        run.step_results
    );
    let refusal = &run.step_results[0];
    assert_eq!(refusal.step_name, "__preflight__", "{:?}", run.step_results);
    assert!(
        !refusal.output.contains("Install or enable"),
        "never asks to install an HTTP provider: {}",
        refusal.output
    );
    assert!(
        refusal.output.contains(&format!("'{step}'")),
        "names the step: {}",
        refusal.output
    );
    assert!(
        !refusal.output.contains(FAKE_KEY),
        "never echoes a key: {}",
        refusal.output
    );
    &refusal.output
}

#[tokio::test]
async fn http_step_preflight_litellm_step_on_a_deleted_connection_is_refused_before_any_step_runs()
{
    let run = run_after_gate(
        "litellm-gone",
        vec![http_step("jeu", AgentType::LiteLlm, Some("conn-gone"))],
        Setup {
            connections: Vec::new(),
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-gone"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("not found"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_litellm_step_on_an_endpoint_less_connection_is_refused_before_any_step_runs(
) {
    let run = run_after_gate(
        "litellm-no-endpoint",
        vec![http_step("jeu", AgentType::LiteLlm, Some("conn-noend"))],
        Setup {
            connections: vec![connection(
                "conn-noend",
                None,
                ExternalApiConnectionPreset::LiteLlm,
            )],
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-noend"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("no HTTP endpoint"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_nvidia_step_on_a_keyless_nvidia_connection_is_refused_before_any_step_runs(
) {
    // The NVIDIA provider slot holds a key: it is not this connection's.
    let run = run_after_gate(
        "nvidia-keyless-connection",
        vec![http_step("jeu", AgentType::Nvidia, Some("conn-nv"))],
        Setup {
            connections: vec![connection(
                "conn-nv",
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            )],
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-nv display"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("no API key"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_nvidia_step_without_any_key_is_refused_before_any_step_runs() {
    let run = run_after_gate(
        "nvidia-no-key",
        vec![http_step("jeu", AgentType::Nvidia, None)],
        Setup {
            connections: Vec::new(),
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(refusal.contains("NVIDIA"), "names the provider: {refusal}");
    assert!(refusal.contains("no NVIDIA API key"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_valid_http_steps_still_start() {
    let run = run_after_gate(
        "valid-pairs",
        vec![
            // A LiteLLM proxy may run keyless.
            http_step("proxy", AgentType::LiteLlm, Some("conn-proxy")),
            http_step("nv-named", AgentType::Nvidia, Some("conn-nv-ok")),
            http_step("nv-slot", AgentType::Nvidia, None),
        ],
        Setup {
            connections: vec![
                connection(
                    "conn-proxy",
                    Some("https://proxy.invalid/v1"),
                    ExternalApiConnectionPreset::LiteLlm,
                ),
                connection(
                    "conn-nv-ok",
                    Some("https://nvidia.invalid/v1"),
                    ExternalApiConnectionPreset::Nvidia,
                ),
            ],
            keys: vec![
                "credential-conn-nv-ok".into(),
                NVIDIA_CREDENTIAL_SLUG.into(),
            ],
        },
    )
    .await;
    assert!(
        run.step_results
            .iter()
            .all(|result| result.step_name != "__preflight__"),
        "a valid pair passes the preflight: {:?}",
        run.step_results
    );
    // It started: the first step (the Gate) ran and paused the run.
    assert_eq!(
        run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        run.step_results
    );
    assert_eq!(run.step_results[0].step_name, "porte");
}

#[tokio::test]
async fn http_step_preflight_reason_names_step_connection_and_what_is_missing() {
    let state = test_state_and_configs().0;
    insert_connection(
        &state,
        connection(
            "conn-nv-reason",
            Some("https://nvidia.invalid/v1"),
            ExternalApiConnectionPreset::Nvidia,
        ),
    )
    .await;
    let tokens = no_tokens();
    let named = http_step("jeu", AgentType::Nvidia, Some("conn-nv-reason"));
    let refusal = crate::workflows::steps::step_connection_refusal(&named, &state.db, &tokens)
        .await
        .expect("a keyless NVIDIA connection is refused");
    for part in ["'jeu'", "conn-nv-reason display", "no API key"] {
        assert!(refusal.contains(part), "{part} missing from: {refusal}");
    }
    let bare = http_step("jeu", AgentType::Nvidia, None);
    let refusal = crate::workflows::steps::step_connection_refusal(&bare, &state.db, &tokens)
        .await
        .expect("an NVIDIA step without a key is refused");
    for part in ["'jeu'", "NVIDIA", "API key"] {
        assert!(refusal.contains(part), "{part} missing from: {refusal}");
    }
}

#[tokio::test]
async fn http_step_preflight_blank_provider_key_counts_as_missing() {
    let state = test_state_and_configs().0;
    let mut tokens = no_tokens();
    tokens.keys.push(crate::models::ApiKey {
        value: "   ".into(),
        ..api_key(NVIDIA_CREDENTIAL_SLUG)
    });
    let step = http_step("jeu", AgentType::Nvidia, None);
    assert!(
        crate::workflows::steps::step_connection_refusal(&step, &state.db, &tokens)
            .await
            .is_some(),
        "a whitespace-only NVIDIA key is no key"
    );
}

#[tokio::test]
async fn http_step_preflight_keyless_litellm_without_connection_is_not_refused() {
    // A LiteLLM proxy may run without a master key (KT-2031): its readiness
    // stays true, so the preflight does not refuse it on its key.
    let state = test_state_and_configs().0;
    let step = http_step("jeu", AgentType::LiteLlm, None);
    assert!(
        crate::workflows::steps::step_connection_refusal(&step, &state.db, &no_tokens())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn http_step_preflight_cli_steps_keep_todays_check() {
    // A CLI owns its login: no key, no connection, still never refused here;
    // its install/enable check stays `agent_can_launch`.
    let state = test_state_and_configs().0;
    for agent in [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::OpenCode,
        AgentType::Vibe,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::Ollama,
    ] {
        let step = http_step("jeu", agent.clone(), None);
        assert!(
            crate::workflows::steps::step_connection_refusal(&step, &state.db, &no_tokens())
                .await
                .is_none(),
            "{agent:?} must not be refused on a key"
        );
    }
}
