//! The library is built without cfg(test): migration-only unit-test defaults
//! cannot mask production resolution. Each case owns an isolated SQLite catalog;
//! serial execution protects its process-local runtime projection. All endpoints
//! are explicit loopback fixtures, never ambient provider configuration.
use std::sync::OnceLock;

use kronn::agents::runner::{start_agent_with_config, AgentStartConfig, ExternalHttpRuntime};
use kronn::core::model_catalog::{
    assigned_model_for_agent, preflight_check, refresh_runtime_cache,
};
use kronn::db::{model_catalog as store, Database};
use kronn::models::{
    AgentType, ModelTier, ModelTiersConfig, ModelUnavailableReason, TokensConfig,
    UpsertManualModelRequest,
};
use serde_json::{json, Value};
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fixture() -> (Database, MockServer, ExternalHttpRuntime, TokensConfig) {
    static DIRECTORY: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIRECTORY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        std::env::set_var("KRONN_DATA_DIR", directory.path());
        directory
    });
    let db = Database::open_in_memory().unwrap();
    refresh_runtime_cache(&db).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("{\"message\":{\"content\":\"fixture answer\"},\"done\":true}\n"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fixture answer\"}}]}\n\ndata: [DONE]\n\n",
        ))
        .mount(&server)
        .await;
    let runtime = ExternalHttpRuntime {
        display_name: "Isolated fixture".into(),
        mention_alias: "fixture".into(),
        endpoint: server.uri(),
        api_key: None,
    };
    let tokens = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    };
    (db, server, runtime, tokens)
}

async fn launch(config: AgentStartConfig<'_>) -> Result<String, String> {
    let mut process = start_agent_with_config(config).await?;
    let mut output = String::new();
    while let Some(chunk) = process.next_line().await {
        output.push_str(&chunk);
    }
    assert!(process.child.wait().await.unwrap().success());
    Ok(output)
}

async fn chat_models(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() != "/api/show")
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            body["model"].as_str().unwrap().to_string()
        })
        .collect()
}

#[tokio::test]
#[serial]
async fn unassigned_http_models_refuse_before_any_provider_request() {
    let (_db, server, runtime, tokens) = fixture().await;
    let mut failures = Vec::new();
    for agent in [
        AgentType::Ollama,
        AgentType::LiteLlm,
        AgentType::Nvidia,
        AgentType::Custom,
    ] {
        let result = launch(AgentStartConfig {
            external_http: Some(&runtime),
            ..AgentStartConfig::new(&agent, "", "hello", &tokens)
        })
        .await;
        match result {
            Err(error) if error.contains("model") && error.contains("Settings") => {}
            other => failures.push(format!("{agent:?}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn whitespace_tier_values_do_not_become_provider_model_ids() {
    let (_db, server, runtime, tokens) = fixture().await;
    let mut tiers = ModelTiersConfig::default();
    tiers.ollama.default = Some(" \t ".into());
    tiers.ollama.reasoning = Some("\n ".into());
    let result = launch(AgentStartConfig {
        external_http: Some(&runtime),
        model_tiers: Some(&tiers),
        tier: ModelTier::Reasoning,
        ..AgentStartConfig::new(&AgentType::Ollama, "", "hello", &tokens)
    })
    .await;
    assert!(
        matches!(result, Err(ref error) if error.contains("No Ollama model configured")),
        "{result:?}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn removing_a_persisted_model_does_not_revive_a_migration_seed() {
    let (db, server, runtime, tokens) = fixture().await;
    let model = "operator/Éclair:v2";
    let request: UpsertManualModelRequest = serde_json::from_value(json!({
        "runtime_target_id": "agent:ollama", "agent_type": "Ollama",
        "model_id": model, "display_name": "Operator model", "tier_assignment": "default"
    }))
    .unwrap();
    db.with_conn(move |conn| {
        assert!(store::create_manual(conn, &request)?.is_some());
        Ok(())
    })
    .await
    .unwrap();
    refresh_runtime_cache(&db).await.unwrap();
    assert_eq!(
        assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default).as_deref(),
        Some(model)
    );
    assert!(launch(AgentStartConfig {
        external_http: Some(&runtime),
        tier: ModelTier::Reasoning,
        ..AgentStartConfig::new(&AgentType::Ollama, "", "hello", &tokens)
    })
    .await
    .unwrap()
    .contains("fixture answer"));
    let before_removal = server.received_requests().await.unwrap().len();
    db.with_conn(move |conn| {
        assert!(store::delete_manual(conn, "agent:ollama", model)?);
        assert!(store::get(conn, "agent:ollama", model)?.is_none());
        Ok(())
    })
    .await
    .unwrap();
    refresh_runtime_cache(&db).await.unwrap();
    assert_eq!(
        assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default),
        None
    );
    let result = launch(AgentStartConfig {
        external_http: Some(&runtime),
        ..AgentStartConfig::new(&AgentType::Ollama, "", "hello", &tokens)
    })
    .await;
    assert!(
        matches!(result, Err(ref error) if error.contains("No Ollama model configured")),
        "{result:?}"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        before_removal
    );
    // A later explicit operator choice stays exact; no silent replacement.
    assert!(launch(AgentStartConfig {
        external_http: Some(&runtime),
        model_override: Some(model),
        ..AgentStartConfig::new(&AgentType::Ollama, "", "hello", &tokens)
    })
    .await
    .unwrap()
    .contains("fixture answer"));
    assert_eq!(chat_models(&server).await, [model, model]);
}

#[tokio::test]
#[serial]
async fn configured_http_default_tier_and_explicit_override_keep_exact_identity() {
    let (_db, server, runtime, tokens) = fixture().await;
    let mut tiers = ModelTiersConfig::default();
    tiers.ollama.default = Some("ollama-default".into());
    tiers.lite_llm.default = Some("lite-default".into());
    tiers.nvidia.default = Some("nvidia-default".into());
    let mut expected = Vec::new();
    for (agent, configured) in [
        (AgentType::Ollama, "ollama-default"),
        (AgentType::LiteLlm, "lite-default"),
        (AgentType::Nvidia, "nvidia-default"),
    ] {
        for tier in [ModelTier::Economy, ModelTier::Reasoning] {
            assert!(launch(AgentStartConfig {
                external_http: Some(&runtime),
                model_tiers: Some(&tiers),
                tier,
                ..AgentStartConfig::new(&agent, "", "hello", &tokens)
            })
            .await
            .unwrap()
            .contains("fixture answer"));
            expected.push(configured);
        }
    }
    for agent in [
        AgentType::Ollama,
        AgentType::LiteLlm,
        AgentType::Nvidia,
        AgentType::Custom,
    ] {
        assert!(launch(AgentStartConfig {
            external_http: Some(&runtime),
            model_tiers: Some(&tiers),
            model_override: Some("explicit/Éclair:v3"),
            tier: ModelTier::Reasoning,
            ..AgentStartConfig::new(&agent, "", "hello", &tokens)
        })
        .await
        .unwrap()
        .contains("fixture answer"));
        expected.push("explicit/Éclair:v3");
    }
    assert_eq!(chat_models(&server).await, expected);
}

#[tokio::test]
#[serial]
async fn unavailable_assigned_tier_cannot_disappear_behind_an_available_http_default() {
    let (db, server, _runtime, _tokens) = fixture().await;
    for (model, tier) in [
        ("known-reasoning", "reasoning"),
        ("other-default", "default"),
    ] {
        let request: UpsertManualModelRequest = serde_json::from_value(json!({
            "runtime_target_id": "agent:ollama", "agent_type": "Ollama",
            "model_id": model, "display_name": model, "tier_assignment": tier
        }))
        .unwrap();
        db.with_conn(move |conn| {
            assert!(store::create_manual(conn, &request)?.is_some());
            Ok(())
        })
        .await
        .unwrap();
    }
    db.with_conn(|conn| {
        store::mark_unavailable(
            conn,
            "agent:ollama",
            "known-reasoning",
            ModelUnavailableReason::Disappeared,
            Some("removed from the provider catalogue"),
        )
    })
    .await
    .unwrap();
    refresh_runtime_cache(&db).await.unwrap();
    assert_eq!(
        assigned_model_for_agent(&AgentType::Ollama, ModelTier::Reasoning),
        None
    );
    assert_eq!(
        assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default).as_deref(),
        Some("other-default")
    );
    let failure = preflight_check(
        &db,
        None,
        AgentType::Ollama,
        ModelTier::Reasoning,
        None,
        None,
    )
    .await
    .expect("a known unavailable assignment must not silently use Default");
    assert_eq!(failure.model_id.as_deref(), Some("known-reasoning"));
    assert_eq!(failure.reason, ModelUnavailableReason::Disappeared);
    assert_eq!(failure.recommended_action, "choose_replacement");
    // The operator may explicitly choose that available Default instead.
    assert!(preflight_check(
        &db,
        None,
        AgentType::Ollama,
        ModelTier::Reasoning,
        Some("other-default"),
        None
    )
    .await
    .is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn preflight_resolves_assignments_only_in_the_exact_connection_namespace() {
    let (db, server, _runtime, _tokens) = fixture().await;
    for target in ["http:connection-one", "http:connection-two"] {
        let request: UpsertManualModelRequest = serde_json::from_value(json!({
            "runtime_target_id": target, "agent_type": "Custom",
            "model_id": "same-id", "display_name": "Same label", "tier_assignment": "default"
        }))
        .unwrap();
        db.with_conn(move |conn| {
            assert!(store::create_manual(conn, &request)?.is_some());
            Ok(())
        })
        .await
        .unwrap();
    }
    db.with_conn(|conn| {
        store::mark_unavailable(
            conn,
            "http:connection-one",
            "same-id",
            ModelUnavailableReason::Disappeared,
            Some("no longer listed"),
        )
    })
    .await
    .unwrap();
    refresh_runtime_cache(&db).await.unwrap();
    let failure = preflight_check(
        &db,
        Some("http:connection-one"),
        AgentType::Custom,
        ModelTier::Reasoning,
        None,
        None,
    )
    .await
    .expect("the first connection's Default is unavailable");
    assert_eq!(failure.runtime_target_id, "http:connection-one");
    assert_eq!(failure.model_id.as_deref(), Some("same-id"));
    assert_eq!(failure.reason, ModelUnavailableReason::Disappeared);
    assert!(preflight_check(
        &db,
        Some("http:connection-two"),
        AgentType::Custom,
        ModelTier::Reasoning,
        None,
        None
    )
    .await
    .is_none());
    assert!(preflight_check(
        &db,
        Some("http:absent"),
        AgentType::Custom,
        ModelTier::Reasoning,
        None,
        None
    )
    .await
    .is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn preflight_assignment_read_error_fails_closed_without_provider_request() {
    let (db, server, _runtime, _tokens) = fixture().await;
    // Corrupt only this disposable in-memory fixture, never an instance DB.
    db.with_conn(|conn| {
        conn.execute("DROP TABLE model_catalog_entries", [])?;
        Ok(())
    })
    .await
    .unwrap();
    let failure = preflight_check(&db, None, AgentType::Ollama, ModelTier::Default, None, None)
        .await
        .expect("a failed catalogue read must not approve a launch");
    assert_eq!(failure.reason, ModelUnavailableReason::ProviderError);
    assert_eq!(failure.recommended_action, "recheck_catalog");
    assert!(failure
        .detail
        .starts_with("catalog assignment lookup failed:"));
    assert!(server.received_requests().await.unwrap().is_empty());
}
