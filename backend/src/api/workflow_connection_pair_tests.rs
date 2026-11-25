//! KT-2032 — saving a workflow refuses an agent / named-connection pair the
//! run would refuse ("no longer matches agent …"), on a step, a shadow and the
//! shadow merge, in `steps` and `on_failure`, on create and update; the
//! readiness check names the pair on a workflow saved before.

use super::*;
use crate::db::external_api_connections::{LEGACY_LITELLM_ID, LEGACY_NVIDIA_ID};
use crate::models::{ExternalApiConnection, ExternalApiConnectionPreset};

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

/// A state with the two legacy connections and one OpenRouter connection.
async fn seeded() -> AppState {
    let state = state();
    state
        .db
        .with_conn(|conn| {
            crate::db::external_api_connections::backfill_legacy_config(
                conn,
                &crate::core::config::default_config(),
            )?;
            let now = chrono::Utc::now();
            crate::db::external_api_connections::insert(
                conn,
                &ExternalApiConnection {
                    id: "conn-or".into(),
                    display_name: "OpenRouter".into(),
                    mention_alias: "conn-or".into(),
                    endpoint: Some("https://openrouter.example/api".into()),
                    credential_slug: "credential-conn-or".into(),
                    origin_preset: ExternalApiConnectionPreset::OpenRouter,
                    economy_model: None,
                    default_model: Some("vendor/model".into()),
                    reasoning_model: None,
                    created_at: now,
                    updated_at: now,
                    image_model: None,
                    video_model: None,
                    media_endpoint: None,
                    decision_model: None,
                    tools_enabled: None,
                },
            )
        })
        .await
        .unwrap();
    state
}

fn on(agent: &str, connection: &str) -> serde_json::Value {
    serde_json::json!({"agent": agent, "model": "gemini-4-argon", "connection_id": connection})
}

fn agent_step(name: &str, agent: &str, connection: Option<&str>) -> serde_json::Value {
    let mut step = serde_json::json!({
        "name": name, "step_type": {"type": "Agent"}, "agent": agent,
        "prompt_template": "Answer",
    });
    if let Some(connection) = connection {
        step["agent_settings"] = serde_json::json!({"connection_id": connection});
    }
    step
}

fn shadowed(shadow_agent: serde_json::Value) -> serde_json::Value {
    let mut step = agent_step("review", "ClaudeCode", None);
    step["shadows"] = serde_json::json!([{"id": "argon", "agent": shadow_agent}]);
    step
}

fn merged(merge_agent: serde_json::Value) -> serde_json::Value {
    let mut step = shadowed(serde_json::json!({"agent": "Codex"}));
    step["shadow_merge"] = serde_json::json!({"agent": merge_agent});
    step
}

fn request(steps: serde_json::Value, on_failure: serde_json::Value) -> CreateWorkflowRequest {
    serde_json::from_value(serde_json::json!({
        "name": "kt-2032", "project_id": null, "trigger": {"type": "Manual"},
        "steps": steps, "on_failure": on_failure,
    }))
    .unwrap()
}

async fn create(
    state: &AppState,
    steps: serde_json::Value,
    on_failure: serde_json::Value,
) -> ApiResponse<Workflow> {
    create_as(
        state.clone(),
        request(steps, on_failure),
        WorkflowWriter::Human,
    )
    .await
    .0
}

fn refusal(response: ApiResponse<Workflow>) -> String {
    assert!(!response.success, "saved: {:?}", response.data);
    response.error.unwrap_or_default()
}

#[tokio::test]
async fn a_shadow_on_custom_with_the_litellm_connection_is_refused_naming_litellm() {
    let state = seeded().await;
    let error = refusal(
        create(
            &state,
            serde_json::json!([shadowed(on("Custom", LEGACY_LITELLM_ID))]),
            serde_json::json!([]),
        )
        .await,
    );
    assert!(error.contains("`review`"), "names the step: {error}");
    assert!(
        error.contains("shadow `argon`"),
        "names the shadow: {error}"
    );
    assert!(
        error.contains("use agent LiteLlm for connection external-api-litellm"),
        "{error}"
    );
}

#[tokio::test]
async fn every_pair_is_checked_on_create_and_a_matching_one_saves() {
    let state = seeded().await;
    let none = serde_json::json!([]);
    // The step's own connection.
    let error = refusal(
        create(
            &state,
            serde_json::json!([agent_step("ask", "Custom", Some(LEGACY_NVIDIA_ID))]),
            none.clone(),
        )
        .await,
    );
    assert!(error.contains("Step `ask`:"), "{error}");
    assert!(
        error.contains("use agent Nvidia for connection external-api-nvidia"),
        "{error}"
    );
    // The merge's agent.
    let error = refusal(
        create(
            &state,
            serde_json::json!([merged(on("LiteLlm", LEGACY_NVIDIA_ID))]),
            none.clone(),
        )
        .await,
    );
    assert!(error.contains("Step `review`, merge:"), "{error}");
    assert!(
        error.contains("use agent Nvidia for connection external-api-nvidia"),
        "{error}"
    );
    // A rollback step, a Custom connection named for a legacy agent.
    let error = refusal(
        create(
            &state,
            serde_json::json!([agent_step("ok", "ClaudeCode", None)]),
            serde_json::json!([agent_step("undo", "LiteLlm", Some("conn-or"))]),
        )
        .await,
    );
    assert!(error.contains("Step `undo`:"), "{error}");
    assert!(
        error.contains("use agent Custom for connection conn-or"),
        "{error}"
    );
    // A connection that does not exist.
    let error = refusal(
        create(
            &state,
            serde_json::json!([shadowed(on("Custom", "conn-gone"))]),
            none.clone(),
        )
        .await,
    );
    assert!(
        error.contains("connection conn-gone does not exist"),
        "{error}"
    );

    // Matching pairs save.
    let saved = create(
        &state,
        serde_json::json!([
            agent_step("ask", "Custom", Some("conn-or")),
            merged(on("Nvidia", LEGACY_NVIDIA_ID)),
            shadowed(on("LiteLlm", LEGACY_LITELLM_ID)),
        ]),
        serde_json::json!([agent_step("undo", "LiteLlm", Some(LEGACY_LITELLM_ID))]),
    )
    .await;
    assert!(saved.success, "{:?}", saved.error);
}

#[tokio::test]
async fn an_update_refuses_the_same_pairs_and_saves_a_matching_one() {
    let state = seeded().await;
    let saved = create(
        &state,
        serde_json::json!([agent_step("ask", "ClaudeCode", None)]),
        serde_json::json!([]),
    )
    .await;
    let id = saved.data.expect("created").id;
    let update = |body: serde_json::Value| {
        let state = state.clone();
        let id = id.clone();
        async move {
            update_as(
                state,
                id,
                serde_json::from_value(body).unwrap(),
                WorkflowWriter::Human,
            )
            .await
            .0
        }
    };
    let error = refusal(
        update(serde_json::json!({"steps": [shadowed(on("Custom", LEGACY_LITELLM_ID))]})).await,
    );
    assert!(
        error.contains("use agent LiteLlm for connection external-api-litellm"),
        "{error}"
    );
    let error = refusal(
        update(serde_json::json!({"on_failure": [agent_step("undo", "Custom", Some(LEGACY_NVIDIA_ID))]})).await,
    );
    assert!(error.contains("Step `undo`:"), "{error}");
    assert!(
        error.contains("use agent Nvidia for connection external-api-nvidia"),
        "{error}"
    );
    let ok =
        update(serde_json::json!({"steps": [shadowed(on("LiteLlm", LEGACY_LITELLM_ID))]})).await;
    assert!(ok.success, "{:?}", ok.error);
}

#[tokio::test]
async fn the_readiness_of_a_workflow_saved_before_names_the_mismatch() {
    let state = seeded().await;
    let wf: Workflow = serde_json::from_value(serde_json::json!({
        "id": "wf-kt-2032", "name": "kt-2032", "project_id": null,
        "trigger": {"type": "Manual"}, "enabled": true,
        "steps": [shadowed(on("Custom", LEGACY_LITELLM_ID))],
        "on_failure": [agent_step("undo", "Custom", Some(LEGACY_NVIDIA_ID))],
        "actions": [], "safety": {"sandbox": false, "require_approval": false},
        "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now(),
    }))
    .unwrap();
    let readiness = state
        .db
        .with_read_conn(move |conn| {
            Ok(crate::workflows::readiness::assess_in_db(
                conn,
                &wf,
                &std::collections::HashMap::new(),
            ))
        })
        .await
        .unwrap();
    assert!(!readiness.ready, "{readiness:?}");
    let shadow = readiness
        .blockers
        .iter()
        .find(|b| b.step.as_deref() == Some("review"))
        .expect("the shadow's pair is a blocker");
    assert!(
        shadow
            .message
            .contains("use agent LiteLlm for connection external-api-litellm"),
        "{shadow:?}"
    );
    assert!(
        shadow.human_only,
        "an agent cannot change a shadow: {shadow:?}"
    );
    let rollback = readiness
        .blockers
        .iter()
        .find(|b| b.step.as_deref() == Some("undo"))
        .expect("the rollback step's pair is a blocker");
    assert!(rollback.on_failure);
    assert!(
        rollback
            .message
            .contains("use agent Nvidia for connection external-api-nvidia"),
        "{rollback:?}"
    );
    assert!(!rollback.human_only, "{rollback:?}");
}
