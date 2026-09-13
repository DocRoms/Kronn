use std::sync::Arc;

use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use kronn::db::{external_api_connections as connections, model_catalog as catalog, Database};
use kronn::models::{AgentType, ExternalApiConnection, ExternalApiConnectionPreset};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use serial_test::serial;
use tokio::sync::{Notify, RwLock};
use tower::ServiceExt;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

async fn fixture(endpoint: &str) -> (AppState, ExternalApiConnection) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let now = chrono::Utc::now();
    let connection = ExternalApiConnection {
        id: "saved-connection".into(),
        display_name: "Saved".into(),
        mention_alias: "saved".into(),
        endpoint: Some(endpoint.into()),
        credential_slug: "saved-key".into(),
        origin_preset: ExternalApiConnectionPreset::Other,
        economy_model: None,
        default_model: Some("saved/model".into()),
        reasoning_model: None,
        image_model: None,
        video_model: None,
        media_endpoint: None,
        created_at: now,
        updated_at: now,
    };
    let insert = connection.clone();
    db.with_conn(move |conn| {
        connections::insert(conn, &insert)?;
        for target in ["http:saved-connection", "http:other-connection"] {
            catalog::reconcile_live(
                conn,
                target,
                &AgentType::Custom,
                &[catalog::DiscoveredModel {
                    model_id: "saved/model".into(),
                    display_name: "Saved model".into(),
                    capabilities: vec!["chat".into()],
                    reasoning_modes: vec![],
                    default_reasoning_mode: None,
                }],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(kronn::core::config::default_config())),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (state, connection)
}

async fn snapshot(db: &Database) -> Value {
    db.with_read_conn(|conn| {
        let logs = ["http:saved-connection", "http:other-connection"].map(|target| {
            let log = catalog::get_refresh_log(conn, target).unwrap().unwrap();
            json!([
                target,
                log.last_live_success_at,
                log.last_attempt_at,
                log.last_error_reason,
                log.last_error_detail
            ])
        });
        Ok(json!({"models": catalog::list_all(conn)?, "logs": logs}))
    })
    .await
    .unwrap()
}

async fn probe(state: AppState, request: Value) -> Value {
    let response = build_router_with_auth(state, false)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/external-api/connections/test")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn models_response(server: &MockServer, status: u16) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(status).set_body_json(json!({"data":[{"id":"draft/model"}]})),
        )
        .mount(server)
        .await;
    // Any authentication invocation remains on this local mock, never a provider.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices":[]})))
        .mount(server)
        .await;
}

#[tokio::test]
#[serial]
async fn draft_endpoint_preset_and_key_never_change_the_saved_catalogue() {
    let server = MockServer::start().await;
    models_response(&server, 200).await;
    for mutation in ["endpoint", "preset", "key", "clear_key"] {
        let endpoint = if mutation == "endpoint" {
            "http://127.0.0.1:1".into()
        } else {
            server.uri()
        };
        let (state, connection) = fixture(&endpoint).await;
        let before = snapshot(&state.db).await;
        let response = probe(
            state.clone(),
            json!({
                "connection_id": connection.id, "endpoint": server.uri(),
                "origin_preset": if mutation == "preset" { "lite_llm" } else { "other" },
                "api_key": if mutation == "clear_key" { "" } else { "draft-secret" },
            }),
        )
        .await;
        assert_eq!(
            response["data"]["status"], "success",
            "{mutation}: {response}"
        );
        assert!(!response.to_string().contains("draft-secret"));
        assert_eq!(
            snapshot(&state.db).await,
            before,
            "draft {mutation} changed a saved target"
        );
    }
}

#[tokio::test]
#[serial]
async fn failed_draft_does_not_mark_the_saved_catalogue_cached() {
    let server = MockServer::start().await;
    models_response(&server, 401).await;
    let (state, connection) = fixture(&server.uri()).await;
    let before = snapshot(&state.db).await;
    let response = probe(
        state.clone(),
        json!({
            "connection_id": connection.id, "endpoint": server.uri(),
            "origin_preset": "other", "api_key": "rejected-draft-secret",
        }),
    )
    .await;
    assert_eq!(response["data"]["status"], "auth_error");
    assert_eq!(snapshot(&state.db).await, before);
}

#[tokio::test]
#[serial]
async fn saved_success_and_failure_roll_back_if_the_refresh_log_cannot_be_written() {
    for status in [200, 401] {
        let server = MockServer::start().await;
        models_response(&server, status).await;
        let (state, connection) = fixture(&server.uri()).await;
        let before = snapshot(&state.db).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER refuse_probe_log BEFORE INSERT ON model_catalog_refresh_log
                BEGIN SELECT RAISE(ABORT, 'injected log failure'); END",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let response = probe(
            state.clone(),
            json!({
                "connection_id": connection.id, "endpoint": server.uri(), "origin_preset": "other",
            }),
        )
        .await;
        assert_eq!(
            snapshot(&state.db).await,
            before,
            "partial catalogue write for HTTP {status}"
        );
        assert_eq!(
            response["success"], false,
            "persistence failure was hidden: {response}"
        );
    }
}

#[tokio::test]
#[serial]
async fn unchanged_saved_connection_refreshes_only_its_own_target() {
    let server = MockServer::start().await;
    models_response(&server, 200).await;
    let (state, connection) = fixture(&server.uri()).await;
    let config_before = serde_json::to_value(&*state.config.read().await).unwrap();
    let before = snapshot(&state.db).await;
    let response = probe(
        state.clone(),
        json!({
            "connection_id": connection.id, "endpoint": server.uri(), "origin_preset": "other",
        }),
    )
    .await;
    assert_eq!(response["data"]["status"], "success");
    let after = snapshot(&state.db).await;
    assert_eq!(before["logs"][1], after["logs"][1]);
    assert_eq!(before["models"][0], after["models"][0]);
    let models = after["models"].as_array().unwrap();
    assert!(models.iter().any(
        |model| model["runtime_target_id"] == "http:saved-connection"
            && model["model_id"] == "draft/model"
            && model["availability"] == "available"
    ));
    assert!(models.iter().any(
        |model| model["runtime_target_id"] == "http:saved-connection"
            && model["model_id"] == "saved/model"
            && model["unavailable_reason"] == "disappeared"
    ));
    assert_eq!(
        serde_json::to_value(&*state.config.read().await).unwrap(),
        config_before
    );
    let id = connection.id.clone();
    assert_eq!(
        state
            .db
            .with_read_conn(move |conn| connections::get(conn, &id))
            .await
            .unwrap(),
        Some(connection)
    );
}

#[tokio::test]
#[serial]
async fn saved_failure_preserves_availability_and_last_seen_with_a_normalized_diagnostic() {
    for (status, reason) in [(401, "auth_required"), (500, "provider_error")] {
        let server = MockServer::start().await;
        models_response(&server, status).await;
        let (state, connection) = fixture(&server.uri()).await;
        let before = snapshot(&state.db).await;
        let response = probe(
            state.clone(),
            json!({
                "connection_id": connection.id, "endpoint": server.uri(), "origin_preset": "other",
            }),
        )
        .await;
        assert_eq!(response["success"], true);
        assert_eq!(response["data"]["ok"], false);
        let after = snapshot(&state.db).await;
        assert_eq!(before["models"][0], after["models"][0]);
        assert_eq!(before["logs"][1], after["logs"][1]);
        assert_eq!(after["models"][1]["provenance"], "cached");
        for field in [
            "id",
            "availability",
            "last_seen_at",
            "capabilities",
            "display_name",
        ] {
            assert_eq!(
                before["models"][1][field], after["models"][1][field],
                "{field}"
            );
        }
        assert_eq!(before["logs"][0][1], after["logs"][0][1]);
        assert_eq!(after["logs"][0][3], reason);
    }
}

#[tokio::test]
#[serial]
async fn empty_then_reappearing_catalogue_preserves_operator_overlays() {
    let server = MockServer::start().await;
    let (state, connection) = fixture(&server.uri()).await;
    state.db.with_conn(|conn| {
        catalog::update_manual(conn, "http:saved-connection", "saved/model", &serde_json::from_value(json!({
            "runtime_target_id":"http:saved-connection", "agent_type":"Custom", "model_id":"saved/model",
            "display_name":"Équipe / 模型", "capabilities":["chat"], "reasoning_modes":[],
            "tier_assignment":"reasoning", "cost_hint":"paid", "privacy_note":"Operator policy",
        }))?).map(|_| ())
    }).await.unwrap();
    for available in [false, true] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data": if available {
                vec![json!({"id":"saved/model", "name":"Provider name"})]
            } else { vec![] }})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let response = probe(
            state.clone(),
            json!({
                "connection_id": connection.id, "endpoint": server.uri(), "origin_preset": "other",
            }),
        )
        .await;
        assert_eq!(response["data"]["ok"], true);
        let entry = state
            .db
            .with_read_conn(|conn| catalog::get(conn, "http:saved-connection", "saved/model"))
            .await
            .unwrap()
            .unwrap();
        let entry = serde_json::to_value(entry).unwrap();
        assert_eq!(
            entry["availability"],
            if available {
                "available"
            } else {
                "unavailable"
            }
        );
        assert_eq!(entry["display_alias"], "Équipe / 模型");
        assert_eq!(entry["tier_assignment"], "reasoning");
        assert_eq!(entry["cost_hint"], "paid");
        assert_eq!(entry["privacy_note"], "Operator policy");
    }
}

#[tokio::test]
#[serial]
async fn changed_endpoint_without_an_explicit_key_is_refused_before_network() {
    let server = MockServer::start().await;
    let (state, connection) = fixture("http://127.0.0.1:1").await;
    let before = snapshot(&state.db).await;
    let response = probe(
        state.clone(),
        json!({
            "connection_id": connection.id, "endpoint": server.uri(), "origin_preset": "other",
        }),
    )
    .await;
    assert_eq!(response["data"]["status"], "credential_required");
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(snapshot(&state.db).await, before);
}

struct HeldProvider {
    url: String,
    received: Arc<Notify>,
    release: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for HeldProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn held_provider() -> HeldProvider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (request_seen, resume) = (received.clone(), release.clone());
    let router = Router::new().route(
        "/v1/models",
        axum::routing::get(move || {
            let (request_seen, resume) = (request_seen.clone(), resume.clone());
            async move {
                request_seen.notify_one();
                resume.notified().await;
                axum::Json(json!({"data":[{"id":"obsolete/model"}]}))
            }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    HeldProvider {
        url,
        received,
        release,
        task,
    }
}

#[tokio::test]
#[serial]
async fn superseded_saved_connection_does_not_accept_the_pending_response() {
    let server = held_provider().await;
    let (state, mut connection) = fixture(&server.url).await;
    let before = snapshot(&state.db).await;
    let pending = tokio::spawn(probe(
        state.clone(),
        json!({
            "connection_id": connection.id, "endpoint": server.url, "origin_preset": "other",
        }),
    ));
    server.received.notified().await;
    connection.endpoint = Some("http://127.0.0.1:1".into());
    connection.updated_at += chrono::Duration::seconds(1);
    state
        .db
        .with_conn(move |conn| connections::update(conn, &connection))
        .await
        .unwrap();
    server.release.notify_one();
    let response = pending.await.unwrap();
    assert_eq!(response["data"]["status"], "success");
    assert_eq!(
        snapshot(&state.db).await,
        before,
        "obsolete response changed a saved target"
    );
}

#[tokio::test]
#[serial]
async fn credential_rotation_does_not_accept_the_pending_anonymous_response() {
    let server = held_provider().await;
    let (state, connection) = fixture(&server.url).await;
    let before = snapshot(&state.db).await;
    let pending = tokio::spawn(probe(
        state.clone(),
        json!({
            "connection_id": connection.id, "endpoint": server.url, "origin_preset": "other",
        }),
    ));
    server.received.notified().await;
    state
        .config
        .write()
        .await
        .tokens
        .keys
        .push(kronn::models::ApiKey {
            id: "rotated".into(),
            name: "Rotated".into(),
            provider: connection.credential_slug,
            value: "rotated-secret".into(),
            active: true,
        });
    server.release.notify_one();
    let response = pending.await.unwrap();
    assert_eq!(response["data"]["status"], "success");
    assert!(!response.to_string().contains("rotated-secret"));
    assert_eq!(snapshot(&state.db).await, before);
}
