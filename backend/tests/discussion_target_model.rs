//! Actual PATCH/SQLite regressions; no sockets, provider process or user database.
use std::sync::{Arc, OnceLock};

use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use kronn::models::{AgentType, Discussion, DiscussionMessage, ModelTier};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn fixture(
    agent: AgentType,
    connection_id: Option<&str>,
) -> (Router, Arc<kronn::db::Database>) {
    static FIXTURE_ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    FIXTURE_ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("create discussion-target fixture root");
        let data_dir = root.path().join("data");
        let host_home = root.path().join("host-home");
        std::fs::create_dir_all(&data_dir).expect("create discussion-target data fixture");
        std::fs::create_dir_all(&host_home).expect("create discussion-target host fixture");
        std::env::set_var("KRONN_DATA_DIR", data_dir);
        std::env::set_var("KRONN_HOST_HOME", host_home);
        root
    });
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let now = chrono::Utc::now();
    let disc: Discussion = serde_json::from_value(json!({
        "id": "target-review", "title": "Existing", "agent": agent,
        "connection_id": connection_id, "language": "en", "participants": [agent],
        "model": "old-provider-model", "tier": "default", "messages": [],
        "created_at": now, "updated_at": now
    }))
    .unwrap();
    let message: DiscussionMessage = serde_json::from_value(json!({
        "id": "old-answer", "role": "Agent", "content": "Historical answer",
        "agent_type": agent, "model": "old-provider-model", "timestamp": now,
        "tokens_used": 17
    }))
    .unwrap();
    db.with_conn(move |conn| {
        for (id, preset) in [("one", "other"), ("two", "other"), ("lite", "litellm")] {
            conn.execute(
                "INSERT INTO external_api_connections
                    (id, display_name, mention_alias, endpoint, credential_slug, origin_preset)
                 VALUES (?1, ?1, ?1, 'https://fixture.invalid', ?1, ?2)",
                rusqlite::params![id, preset],
            )?;
        }
        kronn::db::discussions::insert_discussion(conn, &disc)?;
        kronn::db::discussions::insert_message(conn, &disc.id, &message)?;
        Ok(())
    })
    .await
    .unwrap();
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(kronn::core::config::default_config())),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (build_router_with_auth(state, false), db)
}

async fn patch(app: &Router, body: Value) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/discussions/target-review")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn read(db: &kronn::db::Database) -> Discussion {
    let disc = db
        .with_read_conn(|conn| kronn::db::discussions::get_discussion(conn, "target-review"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disc.messages.len(), 1);
    assert_eq!(disc.messages[0].content, "Historical answer");
    assert_eq!(
        disc.messages[0].model.as_deref(),
        Some("old-provider-model")
    );
    assert_eq!(disc.messages[0].tokens_used, 17);
    disc
}

#[tokio::test]
async fn changing_agent_clears_the_previous_explicit_model() {
    let (app, db) = fixture(AgentType::ClaudeCode, None).await;
    assert_eq!(
        patch(
            &app,
            json!({"agent": "Codex", "tier": "reasoning", "connection_id": null})
        )
        .await["success"],
        true
    );
    let after = read(&db).await;
    assert_eq!(after.agent, AgentType::Codex);
    assert_eq!(after.tier, ModelTier::Reasoning);
    assert!(
        after.model.is_none(),
        "a new agent must not inherit the previous provider model"
    );
}

#[tokio::test]
async fn changing_only_tier_clears_the_explicit_model() {
    let (app, db) = fixture(AgentType::ClaudeCode, None).await;
    assert_eq!(
        patch(&app, json!({"tier": "economy"})).await["success"],
        true
    );
    let after = read(&db).await;
    assert_eq!(after.tier, ModelTier::Economy);
    assert!(
        after.model.is_none(),
        "the old explicit model must not override the selected tier"
    );
}

#[tokio::test]
async fn changing_only_named_connection_clears_the_explicit_model() {
    let (app, db) = fixture(AgentType::Custom, Some("one")).await;
    assert_eq!(
        patch(&app, json!({"connection_id": "two"})).await["success"],
        true
    );
    let after = read(&db).await;
    assert_eq!(after.connection_id.as_deref(), Some("two"));
    assert!(
        after.model.is_none(),
        "same agent family does not mean same runtime namespace"
    );
}

#[tokio::test]
async fn explicit_json_null_clears_a_named_connection_and_its_model() {
    let (app, db) = fixture(AgentType::LiteLlm, Some("lite")).await;
    assert_eq!(
        patch(&app, json!({"connection_id": null})).await["success"],
        true
    );
    let after = read(&db).await;
    assert!(
        after.connection_id.is_none(),
        "JSON null must be distinct from an absent field"
    );
    assert!(after.model.is_none());
}

#[tokio::test]
async fn ordinary_edits_same_selection_and_invalid_target_preserve_model_and_history() {
    let (app, db) = fixture(AgentType::ClaudeCode, None).await;
    for body in [
        json!({"title": "Renamed"}),
        json!({"agent": "ClaudeCode", "tier": "default"}),
    ] {
        assert_eq!(patch(&app, body).await["success"], true);
        assert_eq!(read(&db).await.model.as_deref(), Some("old-provider-model"));
    }
    assert_eq!(
        patch(
            &app,
            json!({"title": "Must not persist", "agent": "Custom", "connection_id": "missing"})
        )
        .await["success"],
        false
    );
    let after = read(&db).await;
    assert_eq!(after.title, "Renamed");
    assert_eq!(after.agent, AgentType::ClaudeCode);
    assert_eq!(after.model.as_deref(), Some("old-provider-model"));
}

#[tokio::test]
async fn same_named_agent_and_tier_do_not_implicitly_clear_the_connection() {
    let (app, db) = fixture(AgentType::LiteLlm, Some("lite")).await;
    assert_eq!(
        patch(&app, json!({"agent": "LiteLlm", "tier": "default"})).await["success"],
        true
    );
    let after = read(&db).await;
    assert_eq!(after.connection_id.as_deref(), Some("lite"));
    assert_eq!(after.model.as_deref(), Some("old-provider-model"));
}

#[tokio::test]
async fn same_custom_connection_preserves_model_and_clearing_it_is_refused() {
    let (app, db) = fixture(AgentType::Custom, Some("one")).await;
    assert_eq!(
        patch(&app, json!({"connection_id": "one"})).await["success"],
        true
    );
    assert_eq!(read(&db).await.model.as_deref(), Some("old-provider-model"));
    assert_eq!(
        patch(
            &app,
            json!({"title": "Must not persist", "connection_id": null})
        )
        .await["success"],
        false
    );
    let after = read(&db).await;
    assert_eq!(after.title, "Existing");
    assert_eq!(after.connection_id.as_deref(), Some("one"));
    assert_eq!(after.model.as_deref(), Some("old-provider-model"));
}
