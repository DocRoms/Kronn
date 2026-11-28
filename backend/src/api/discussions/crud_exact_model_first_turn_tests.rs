//! KT-2023 — the first turn of a discussion created with an exact model runs
//! that model. These tests start from what the creation writes (the discussion
//! row and the first message's targets) and, for an HTTP connection, run the
//! turn through `/run` against a local upstream: the unit tests of
//! `turn_exact_model` never started from those targets and missed it.

use super::{create, drop_creation_mode_of_own_target};
use crate::api::discussions::messaging::run_agent;
use crate::api::discussions::runtime::dispatch_target_tier;
use crate::api::discussions::streaming::resolve_turn_model;
use crate::models::{
    AgentType, CreateDiscussionRequest, MessageRole, MessageTarget, MessageTargetKind, ModelTier,
};
use crate::AppState;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The exact model of the report, an id with a slash and a `:free` suffix.
const EXACT: &str = "google/gemma-4-26b-a4b-it:free";
const CONNECTION: &str = "openrouter-named";

/// A local OpenAI-compatible upstream answering every chat request with one
/// streamed reply.
async fn upstream() -> MockServer {
    let server = MockServer::start().await;
    let frame = json!({"choices":[{"index":0,"delta":{"content":"Hello from the upstream."},"finish_reason":"stop"}]});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("data: {frame}\n\ndata: [DONE]\n\n")),
        )
        .mount(&server)
        .await;
    server
}

/// A named Custom connection on `endpoint` whose three modes map to models of
/// their own (the Standard one is `standard-model`), and the `qp-exact` Quick
/// Prompt on it, in reasoning mode, pinned to `saved-model`.
async fn state_with_connection(endpoint: String) -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    db.with_conn(move |conn| {
        let connection = serde_json::from_value(json!({
            "id": CONNECTION, "display_name": "OpenRouter", "mention_alias": "openrouter",
            "endpoint": endpoint, "credential_slug": "openrouter-named",
            "origin_preset": "other",
            "economy_model": "economy-model",
            "default_model": "standard-model",
            "reasoning_model": "reasoning-model",
            "created_at": "2026-11-23T00:00:00Z", "updated_at": "2026-11-23T00:00:00Z"
        }))?;
        crate::db::external_api_connections::insert(conn, &connection)?;
        let prompt = serde_json::from_value(json!({
            "id": "qp-exact", "name": "Résumé", "icon": "Q", "description": "Summarize a topic",
            "prompt_template": "Summarize {{topic}}", "project_id": null,
            "variables": [{"name": "topic", "label": "Topic", "placeholder": "Kronn", "required": true}],
            "agent": "Custom", "connection_id": CONNECTION, "tier": "reasoning",
            "agent_settings": {"model": "saved-model"},
            "skill_ids": [], "profile_ids": [], "directive_ids": [],
            "created_at": "2026-11-23T00:00:00Z", "updated_at": "2026-11-23T00:00:00Z"
        }))?;
        crate::db::quick_prompts::insert_quick_prompt(conn, &prompt)?;
        Ok(())
    })
    .await
    .expect("fixture seeded");
    let mut config = crate::core::config::default_config();
    config.encryption_secret = Some(crate::core::crypto::generate_secret());
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn create_discussion(state: &AppState, body: Value) -> crate::models::Discussion {
    let request: CreateDiscussionRequest =
        serde_json::from_value(body).expect("valid create payload");
    let response = create(State(state.clone()), Json(request)).await;
    assert!(response.0.success, "create refused: {:?}", response.0.error);
    response.0.data.expect("discussion created")
}

async fn first_message_targets(
    state: &AppState,
    discussion: &crate::models::Discussion,
) -> Vec<MessageTarget> {
    let message_id = discussion
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .expect("first message")
        .id
        .clone();
    state
        .db
        .with_conn(move |conn| crate::db::discussions::list_message_targets(conn, &message_id))
        .await
        .expect("targets loaded")
}

/// Runs the first turn through `/run`, as the new-discussion form does, until
/// its stream ends.
async fn run_first_turn(state: &AppState, discussion_id: &str) -> String {
    let response = run_agent(State(state.clone()), Path(discussion_id.to_string()), None).await;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        axum::body::to_bytes(response.into_response().into_body(), usize::MAX),
    )
    .await
    .expect("bounded first turn")
    .expect("SSE body");
    String::from_utf8_lossy(&body).into_owned()
}

/// The `model` of every chat request the upstream received.
async fn requested_models(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("request recording")
        .iter()
        .filter(|request| request.url.path() == "/v1/chat/completions")
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).expect("JSON body");
            body["model"].as_str().expect("model field").to_string()
        })
        .collect()
}

/// The model the reply of the turn records, once it is persisted.
async fn reply_model(state: &AppState, discussion_id: &str) -> Option<String> {
    for _ in 0..200 {
        let id = discussion_id.to_string();
        let discussion = state
            .db
            .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &id))
            .await
            .expect("discussion loaded")
            .expect("discussion exists");
        if let Some(reply) = discussion
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::Agent)
        {
            return reply.model.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the first turn left no reply");
}

/// The new-discussion form's payload for a named connection and the picker's
/// « Custom » exact model: the discussion's own target carries the form's mode.
fn named_connection_request(target_tier: &str) -> Value {
    json!({
        "project_id": null,
        "title": "Exact model on a named connection",
        "agent": "Custom",
        "connection_id": CONNECTION,
        "language": "en",
        "initial_prompt": "Who are you?",
        "model": EXACT,
        "initial_targets": [{
            "kind": "discussion_agent",
            "agent_type": "Custom",
            "connection_id": CONNECTION,
            "tier": target_tier
        }]
    })
}

#[tokio::test]
#[serial_test::serial] // HOME is process-wide: the discussion folder lands in a scratch home
async fn first_turn_on_a_named_connection_requests_the_exact_model() {
    let _home = crate::agents::runner::ScratchHome::enter();
    let server = upstream().await;
    let state = state_with_connection(server.uri()).await;
    let discussion = create_discussion(&state, named_connection_request("default")).await;
    assert_eq!(discussion.model.as_deref(), Some(EXACT));
    assert_eq!(discussion.tier, ModelTier::Default);

    let events = run_first_turn(&state, &discussion.id).await;
    assert!(events.contains("Hello from the upstream."), "{events}");
    assert_eq!(
        requested_models(&server).await,
        vec![EXACT.to_string()],
        "the first turn asks the upstream for the exact model, not the Standard one"
    );
    assert_eq!(
        reply_model(&state, &discussion.id).await.as_deref(),
        Some(EXACT),
        "the reply records the exact model"
    );

    let targets = first_message_targets(&state, &discussion).await;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].kind, MessageTargetKind::DiscussionAgent);
    assert_eq!(
        targets[0].tier, None,
        "the creation mode of the discussion's own target is not a per-message mode"
    );
}

/// KT-1993 stays: a mode chosen for the first message (here one that is not the
/// discussion's) is a per-message mode and drops the exact model.
#[tokio::test]
#[serial_test::serial] // HOME is process-wide: the discussion folder lands in a scratch home
async fn first_turn_with_a_mode_chosen_for_it_runs_the_connection_model_of_that_mode() {
    let _home = crate::agents::runner::ScratchHome::enter();
    let server = upstream().await;
    let state = state_with_connection(server.uri()).await;
    let discussion = create_discussion(&state, named_connection_request("economy")).await;
    assert_eq!(discussion.model.as_deref(), Some(EXACT));

    let targets = first_message_targets(&state, &discussion).await;
    assert_eq!(targets[0].tier, Some(ModelTier::Economy));

    let events = run_first_turn(&state, &discussion.id).await;
    assert!(events.contains("Hello from the upstream."), "{events}");
    assert_eq!(
        requested_models(&server).await,
        vec!["economy-model".to_string()]
    );
    assert_eq!(
        reply_model(&state, &discussion.id).await.as_deref(),
        Some("economy-model")
    );
}

/// A Quick Prompt pinned to a model, launched on its connection with the
/// creation mode on the discussion's own target: the first turn runs the
/// Quick Prompt's model, not the connection's model for that mode.
#[tokio::test]
#[serial_test::serial] // HOME is process-wide: the discussion folder lands in a scratch home
async fn first_turn_of_a_quick_prompt_launch_requests_the_prompt_model() {
    let _home = crate::agents::runner::ScratchHome::enter();
    let server = upstream().await;
    let state = state_with_connection(server.uri()).await;
    let discussion = create_discussion(
        &state,
        json!({
            "project_id": null,
            "title": "Quick Prompt launch",
            "agent": "Custom",
            "connection_id": CONNECTION,
            "language": "en",
            "initial_prompt": "",
            "tier": "reasoning",
            "originating_qp_id": "qp-exact",
            "launch_variables": { "topic": "Kronn" },
            "initial_targets": [{
                "kind": "discussion_agent",
                "agent_type": "Custom",
                "connection_id": CONNECTION,
                "tier": "reasoning"
            }]
        }),
    )
    .await;
    assert_eq!(discussion.model.as_deref(), Some("saved-model"));

    let events = run_first_turn(&state, &discussion.id).await;
    assert!(events.contains("Hello from the upstream."), "{events}");
    assert_eq!(
        requested_models(&server).await,
        vec!["saved-model".to_string()],
        "the Quick Prompt's model, not the connection's reasoning model"
    );
    assert_eq!(
        reply_model(&state, &discussion.id).await.as_deref(),
        Some("saved-model")
    );
    assert_eq!(
        first_message_targets(&state, &discussion).await[0].tier,
        None
    );
}

/// The model the dispatch of `target` would run, from the first message's
/// persisted targets — the path `stream_claimed_dispatch_job` and
/// `make_agent_stream` take for a CLI agent (no connection).
fn cli_turn_model(
    discussion: &crate::models::Discussion,
    targets: &[MessageTarget],
    agent_override: Option<&AgentType>,
) -> Option<String> {
    let tier_override = dispatch_target_tier(targets, agent_override, None);
    let agent = agent_override.unwrap_or(&discussion.agent);
    let model = resolve_turn_model(discussion, agent, None, None, tier_override, None);
    crate::agents::runner::effective_model_flag(
        model.as_deref(),
        agent,
        tier_override.unwrap_or(discussion.tier),
        None,
    )
}

/// A CLI discussion (no named connection) created with an exact model and the
/// creation mode on its own target, plus a mention: the discussion's own first
/// turn runs the exact model, the mention never receives it.
#[tokio::test]
async fn first_turn_of_a_cli_discussion_runs_the_exact_model_and_a_mention_its_own() {
    let state = state_with_connection("http://127.0.0.1:1".into()).await;
    let discussion = create_discussion(
        &state,
        json!({
            "project_id": null,
            "title": "Exact model on a CLI agent",
            "agent": "ClaudeCode",
            "language": "en",
            "initial_prompt": "hello @codex",
            "tier": "reasoning",
            "model": "claude-exact-cli",
            "initial_targets": [
                {"kind": "discussion_agent", "agent_type": "ClaudeCode", "tier": "reasoning"},
                {"kind": "agent", "agent_type": "Codex", "tier": "reasoning"}
            ]
        }),
    )
    .await;
    assert_eq!(discussion.model.as_deref(), Some("claude-exact-cli"));
    let targets = first_message_targets(&state, &discussion).await;
    assert_eq!(
        cli_turn_model(&discussion, &targets, None).as_deref(),
        Some("claude-exact-cli"),
        "the discussion's own first turn runs the exact model"
    );
    assert_eq!(
        targets[0].tier, None,
        "the own target's creation mode is gone"
    );
    assert_eq!(
        targets[1].tier,
        Some(ModelTier::Reasoning),
        "a mention keeps its mode"
    );
    let mention = cli_turn_model(&discussion, &targets, Some(&AgentType::Codex));
    assert_ne!(mention.as_deref(), Some("claude-exact-cli"));
    assert_eq!(
        mention,
        crate::agents::runner::effective_model_flag(
            None,
            &AgentType::Codex,
            ModelTier::Reasoning,
            None
        ),
        "the mention runs its own model for its mode"
    );

    // Without targets (the form's shape for a CLI agent) the exact model runs too.
    let bare = create_discussion(
        &state,
        json!({
            "project_id": null, "title": "Bare", "agent": "ClaudeCode", "language": "en",
            "initial_prompt": "hello", "model": "claude-exact-cli"
        }),
    )
    .await;
    let targets = first_message_targets(&state, &bare).await;
    assert!(targets.is_empty());
    assert_eq!(
        cli_turn_model(&bare, &targets, None).as_deref(),
        Some("claude-exact-cli")
    );
}

#[test]
fn only_the_own_target_s_creation_mode_is_dropped_and_only_with_an_exact_model() {
    let own = |tier| {
        MessageTarget::discussion_agent(AgentType::Custom)
            .with_connection(CONNECTION)
            .with_tier(tier)
    };
    let mut targets = vec![
        own(ModelTier::Default),
        MessageTarget::discussion_agent(AgentType::Custom)
            .with_connection("other-connection")
            .with_tier(ModelTier::Default),
        MessageTarget::agent(AgentType::Custom)
            .with_connection(CONNECTION)
            .with_tier(ModelTier::Default),
        MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Default),
    ];
    let untouched = targets.clone();

    drop_creation_mode_of_own_target(
        &mut targets,
        &AgentType::Custom,
        Some(CONNECTION),
        ModelTier::Default,
        None,
    );
    assert_eq!(targets, untouched, "without an exact model nothing changes");

    drop_creation_mode_of_own_target(
        &mut targets,
        &AgentType::Custom,
        Some(CONNECTION),
        ModelTier::Default,
        Some(EXACT),
    );
    assert_eq!(targets[0].tier, None);
    assert_eq!(
        targets[1..],
        untouched[1..],
        "other targets keep their mode"
    );

    let mut chosen = vec![own(ModelTier::Economy)];
    drop_creation_mode_of_own_target(
        &mut chosen,
        &AgentType::Custom,
        Some(CONNECTION),
        ModelTier::Default,
        Some(EXACT),
    );
    assert_eq!(
        chosen[0].tier,
        Some(ModelTier::Economy),
        "a mode other than the discussion's was chosen for the message"
    );
}
