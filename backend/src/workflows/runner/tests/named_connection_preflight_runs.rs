//! KT-2030 — an Agent step on a named HTTP connection (`AgentType::Custom`)
//! passes the run preflight when its connection can be called, and is refused
//! before start, by step and connection, when it cannot. Never "install
//! Custom": no such agent exists.

use super::*;
use crate::models::{ExternalApiConnection, ExternalApiConnectionPreset};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "fixture-connection-key";

fn connection(
    id: &str,
    endpoint: String,
    preset: ExternalApiConnectionPreset,
) -> ExternalApiConnection {
    let now = chrono::Utc::now();
    ExternalApiConnection {
        id: id.into(),
        display_name: format!("{id} display"),
        mention_alias: id.into(),
        endpoint: Some(endpoint),
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
    let model = connection.default_model.clone().unwrap();
    state
        .db
        .with_conn(move |conn| {
            crate::db::external_api_connections::insert(conn, &connection)?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::Custom,
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

fn store_key(tokens: &mut TokensConfig, connection_id: &str) {
    tokens.keys.push(crate::models::ApiKey {
        id: format!("key-{connection_id}"),
        name: "fixture key".into(),
        provider: format!("credential-{connection_id}"),
        value: KEY.into(),
        active: true,
    });
}

fn custom_step(connection_id: Option<&str>) -> WorkflowStep {
    let mut step = fake_step("jeu");
    step.agent = AgentType::Custom;
    step.prompt_template = "Answer".into();
    step.agent_settings = Some(crate::models::AgentSettings {
        tier: Some(crate::models::ModelTier::Default),
        connection_id: connection_id.map(str::to_string),
        ..Default::default()
    });
    step
}

async fn answering_provider() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "data: {\"choices\":[{\"delta\":{\"content\":\"NAMED_CONNECTION_ANSWER\"}}]}\n\ndata: [DONE]\n\n",
        ))
        .mount(&server)
        .await;
    server
}

/// A fake key in the LiteLLM provider slot: a keyless named connection must
/// never send it to its own endpoint (KT-2038).
fn store_litellm_slot_key(tokens: &mut TokensConfig) {
    tokens.keys.push(crate::models::ApiKey {
        id: "key-litellm-slot".into(),
        name: "fixture litellm slot".into(),
        provider: crate::db::external_api_connections::LITELLM_CREDENTIAL_SLUG.into(),
        value: "fixture-litellm-slot-key".into(),
        active: true,
    });
}

/// Runs a one-step workflow on `step`; `with_key` stores the connection key.
async fn run_one(
    id: &str,
    step: WorkflowStep,
    seed: Option<ExternalApiConnection>,
    with_key: Option<&str>,
) -> WorkflowRun {
    run_one_with_litellm_slot(id, step, seed, with_key, false).await
}

async fn run_one_with_litellm_slot(
    id: &str,
    step: WorkflowStep,
    seed: Option<ExternalApiConnection>,
    with_key: Option<&str>,
    litellm_slot: bool,
) -> WorkflowRun {
    let (state, mut tokens, agents) = test_state_and_configs();
    if let Some(connection) = seed {
        insert_connection(&state, connection).await;
    }
    if let Some(connection_id) = with_key {
        store_key(&mut tokens, connection_id);
        store_key(&mut state.config.write().await.tokens, connection_id);
    }
    if litellm_slot {
        store_litellm_slot_key(&mut tokens);
        store_litellm_slot_key(&mut state.config.write().await.tokens);
    }
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("wf-{id}");
    wf.steps = vec![step];
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

fn preflight_refusal(run: &WorkflowRun) -> &str {
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    let first = &run.step_results[0];
    assert_eq!(first.step_name, "__preflight__", "{:?}", run.step_results);
    assert!(
        !first.output.contains("Install or enable"),
        "never asks to install an agent: {}",
        first.output
    );
    assert!(
        first.output.contains("'jeu'"),
        "names the step: {}",
        first.output
    );
    &first.output
}

#[tokio::test]
async fn a_step_on_a_named_connection_holding_its_key_passes_the_preflight_and_runs() {
    let server = answering_provider().await;
    let open_router = connection(
        "conn-or",
        server.uri(),
        ExternalApiConnectionPreset::OpenRouter,
    );
    let run = run_one(
        "named-ok",
        custom_step(Some("conn-or")),
        Some(open_router),
        Some("conn-or"),
    )
    .await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_eq!(run.step_results[0].step_name, "jeu");
    assert!(
        run.step_results[0]
            .output
            .contains("NAMED_CONNECTION_ANSWER"),
        "{}",
        run.step_results[0].output
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_named_connection_whose_preset_needs_no_key_passes_the_preflight() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "data: {\"choices\":[{\"delta\":{\"content\":\"KEYLESS_ANSWER\"}}]}\n\ndata: [DONE]\n\n",
        ))
        .mount(&server)
        .await;
    let local = connection(
        "conn-local",
        server.uri(),
        ExternalApiConnectionPreset::Other,
    );
    // The LiteLLM slot holds a key: it belongs to the LiteLLM proxy, not to
    // this connection's endpoint, so it must not be sent here (KT-2038).
    let run = run_one_with_litellm_slot(
        "named-keyless",
        custom_step(Some("conn-local")),
        Some(local),
        None,
        true,
    )
    .await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert!(run.step_results[0].output.contains("KEYLESS_ANSWER"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    // Checked for presence only: the header value is never printed.
    assert!(
        requests
            .iter()
            .all(|request| !request.headers.contains_key("authorization")),
        "a keyless connection must send no Authorization header"
    );
}

#[tokio::test]
async fn a_step_on_custom_without_a_connection_id_is_refused_before_start() {
    let run = run_one("named-none", custom_step(None), None, None).await;
    let refusal = preflight_refusal(&run);
    assert!(refusal.contains("names no connection"), "{refusal}");
}

#[tokio::test]
async fn a_step_on_a_missing_connection_is_refused_before_start() {
    let run = run_one("named-missing", custom_step(Some("conn-gone")), None, None).await;
    let refusal = preflight_refusal(&run);
    assert!(
        refusal.contains("conn-gone"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("not found"), "{refusal}");
}

#[tokio::test]
async fn a_step_on_a_connection_without_its_required_key_is_refused_before_start() {
    let server = answering_provider().await;
    let open_router = connection(
        "conn-nokey",
        server.uri(),
        ExternalApiConnectionPreset::OpenRouter,
    );
    let run = run_one(
        "named-nokey",
        custom_step(Some("conn-nokey")),
        Some(open_router),
        None,
    )
    .await;
    let refusal = preflight_refusal(&run);
    assert!(
        refusal.contains("conn-nokey display"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("no API key"), "{refusal}");
    assert!(server.received_requests().await.unwrap().is_empty());
}
