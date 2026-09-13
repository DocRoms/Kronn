use std::sync::Arc;

use axum::{body::Body, http::Request, Router};
use futures::StreamExt;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

use kronn::core::model_catalog;
use kronn::db::model_catalog as store;
use kronn::models::{
    AgentType, ModelAvailability, ModelCostHint, ModelTier, ModelUnavailableReason,
    UpsertManualModelRequest,
};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};

struct StreamingServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StreamingServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn streaming_server(stall_after_headers: bool) -> StreamingServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/api/tags",
        axum::routing::get(move || async move {
            let bytes = if stall_after_headers {
                vec![b'{']
            } else {
                vec![b'a'; 1_048_577]
            };
            let first = futures::stream::once(async move {
                Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(bytes))
            });
            let stream = if stall_after_headers {
                first.chain(futures::stream::pending()).boxed()
            } else {
                first.boxed()
            };
            axum::response::Response::new(Body::from_stream(stream))
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    StreamingServer { url, task }
}

fn app_with_db(db: Arc<kronn::db::Database>, ollama_url: &str) -> Router {
    let mut config = kronn::core::config::default_config();
    config.server.auth_token = None;
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    )
    .with_ollama_base_url(ollama_url.to_string());
    build_router_with_auth(state, false)
}

async fn get(app: Router, uri: &str) -> Value {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn post(app: Router, uri: &str, body: Value) -> Value {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
#[serial]
async fn snapshot_includes_ollama_without_network_discovery() {
    let server = MockServer::start().await;
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        store::create_manual(
            conn,
            &UpsertManualModelRequest {
                runtime_target_id: "agent:ollama".into(),
                agent_type: AgentType::Ollama,
                model_id: "offline:1".into(),
                display_name: "Offline alias".into(),
                capabilities: vec!["chat".into()],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
                tier_assignment: Some(ModelTier::Reasoning),
                cost_hint: None,
                privacy_note: None,
            },
        )
    })
    .await
    .unwrap();
    let json = get(app_with_db(db, &server.uri()), "/api/model-catalogs").await;
    let targets = json["data"]["targets"].as_array().unwrap();
    let ollama = targets
        .iter()
        .find(|target| target["runtime_target_id"] == "agent:ollama")
        .expect("Ollama target");
    assert_eq!(ollama["agent_type"], "Ollama");
    assert_eq!(ollama["models"][0]["model_id"], "offline:1");
    assert_eq!(ollama["models"][0]["provenance"], "manual");
    assert_eq!(ollama["models"][0]["tier_assignment"], "reasoning");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn models_route_reconciles_once_preserves_overlays_and_survives_reopen() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"name":"local/model:1","size":4100000000_u64,"modified_at":"2026-09-10T10:00:00Z"},
            {"name":"local/model:1","size":4100000000_u64,"modified_at":"2026-09-10T10:00:00Z"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_info": {"llama.context_length": 32768}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.sqlite");
    let db = Arc::new(kronn::db::Database::open_path(&path).unwrap());
    let manual = UpsertManualModelRequest {
        runtime_target_id: "agent:ollama".into(),
        agent_type: AgentType::Ollama,
        model_id: "local/model:1".into(),
        display_name: "Operator alias".into(),
        capabilities: vec!["chat".into()],
        reasoning_modes: vec![],
        default_reasoning_mode: None,
        tier_assignment: Some(ModelTier::Default),
        cost_hint: Some(ModelCostHint::Unknown),
        privacy_note: Some("Operator-owned privacy note".into()),
    };
    db.with_conn(move |conn| store::create_manual(conn, &manual))
        .await
        .unwrap();
    let named = UpsertManualModelRequest {
        runtime_target_id: "http:named".into(),
        agent_type: AgentType::Custom,
        model_id: "local/model:1".into(),
        display_name: "Named connection copy".into(),
        capabilities: vec!["chat".into()],
        reasoning_modes: vec![],
        default_reasoning_mode: None,
        tier_assignment: None,
        cost_hint: None,
        privacy_note: None,
    };
    db.with_conn(move |conn| store::create_manual(conn, &named))
        .await
        .unwrap();

    let json = get(app_with_db(db.clone(), &server.uri()), "/api/ollama/models").await;
    let models = json["data"]["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["name"], "local/model:1");
    assert_eq!(models[0]["size"], "4.1 GB");
    assert_eq!(models[0]["modified"], "2026-09-10T10:00:00Z");
    assert_eq!(models[0]["advertised_context"], 32768);

    let entry = db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "local/model:1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.provenance, kronn::models::ModelProvenance::Live);
    assert_eq!(entry.display_alias.as_deref(), Some("Operator alias"));
    assert_eq!(entry.tier_assignment, Some(ModelTier::Default));
    assert_eq!(entry.cost_hint, Some(ModelCostHint::Unknown));
    assert_eq!(
        entry.privacy_note.as_deref(),
        Some("Operator-owned privacy note")
    );
    let named = db
        .with_read_conn(|conn| store::get(conn, "http:named", "local/model:1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named.display_name, "Named connection copy");
    assert_eq!(named.provenance, kronn::models::ModelProvenance::Manual);
    assert_eq!(
        model_catalog::assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default).as_deref(),
        Some("local/model:1")
    );

    drop(db);
    let reopened = kronn::db::Database::open_path(&path).unwrap();
    let persisted = reopened
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "local/model:1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.display_alias.as_deref(), Some("Operator alias"));
    assert_eq!(persisted.tier_assignment, Some(ModelTier::Default));
    assert_eq!(persisted.id, entry.id);
    assert_eq!(persisted.cost_hint, entry.cost_hint);
    assert_eq!(persisted.privacy_note, entry.privacy_note);
}

#[tokio::test]
#[serial]
async fn explicit_refresh_distinguishes_empty_success_from_invalid_and_provider_failure() {
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let initial = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"models":[{"name":"gone:1"}]})),
        )
        .mount(&initial)
        .await;
    let body = json!({"runtime_target_id":"agent:ollama","agent_type":"Ollama","force":true});
    let first = post(
        app_with_db(db.clone(), &initial.uri()),
        "/api/model-catalogs/refresh",
        body.clone(),
    )
    .await;
    assert_eq!(first["success"], true);

    let empty = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":[]})))
        .mount(&empty)
        .await;
    let emptied = post(
        app_with_db(db.clone(), &empty.uri()),
        "/api/model-catalogs/refresh",
        body.clone(),
    )
    .await;
    assert_eq!(emptied["data"]["live_refresh_ok"], true);
    let gone = db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "gone:1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(gone.availability, ModelAvailability::Unavailable);
    assert_eq!(
        gone.unavailable_reason,
        Some(ModelUnavailableReason::Disappeared)
    );

    let revived = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"models":[{"name":"gone:1"}]})),
        )
        .mount(&revived)
        .await;
    post(
        app_with_db(db.clone(), &revived.uri()),
        "/api/model-catalogs/refresh",
        body.clone(),
    )
    .await;
    let live = db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "gone:1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.availability, ModelAvailability::Available);
    assert_eq!(live.id, gone.id);

    for (template, expected) in [
        (
            ResponseTemplate::new(200).set_body_raw("{\"models\":{}}", "application/json"),
            ModelUnavailableReason::InvalidCatalog,
        ),
        (
            ResponseTemplate::new(503),
            ModelUnavailableReason::ProviderError,
        ),
    ] {
        let failed = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(template)
            .mount(&failed)
            .await;
        let result = post(
            app_with_db(db.clone(), &failed.uri()),
            "/api/model-catalogs/refresh",
            body.clone(),
        )
        .await;
        assert_eq!(result["data"]["live_refresh_ok"], false);
        let reason: ModelUnavailableReason =
            serde_json::from_value(result["data"]["last_error_reason"].clone()).unwrap();
        assert_eq!(reason, expected);
        let preserved = db
            .with_read_conn(|conn| store::get(conn, "agent:ollama", "gone:1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preserved.availability, ModelAvailability::Available);
    }

    let malformed_id = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"models":[{"name":"valid:1"},{"name":"bad id"}]})),
        )
        .mount(&malformed_id)
        .await;
    let result = post(
        app_with_db(db.clone(), &malformed_id.uri()),
        "/api/model-catalogs/refresh",
        body.clone(),
    )
    .await;
    assert_eq!(result["data"]["last_error_reason"], "invalid_catalog");
    assert!(db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "valid:1"))
        .await
        .unwrap()
        .is_none());

    let timeout_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(6)))
        .mount(&timeout_server)
        .await;
    let timed_out = post(
        app_with_db(db.clone(), &timeout_server.uri()),
        "/api/model-catalogs/refresh",
        body,
    )
    .await;
    assert_eq!(timed_out["data"]["last_error_reason"], "timeout");
}

#[tokio::test]
#[serial]
async fn failed_reconciliation_rolls_back_every_row_before_publishing_a_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":[
            {"name":"first:1"},{"name":"rejected:1"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;
    let db = kronn::db::Database::open_in_memory().unwrap();
    db.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_fixture_model BEFORE INSERT ON model_catalog_entries
            WHEN NEW.model_id = 'rejected:1' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    model_catalog::refresh_runtime_cache(&db).await.unwrap();
    assert!(model_catalog::refresh_ollama_catalog_at(&db, &server.uri())
        .await
        .is_err());
    let rows = db
        .with_read_conn(|conn| store::list_for_target(conn, "agent:ollama"))
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "a failed listing must not publish its earlier rows"
    );
    assert!(db
        .with_read_conn(|conn| store::get_refresh_log(conn, "agent:ollama"))
        .await
        .unwrap()
        .is_none());
    assert!(
        model_catalog::assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default).is_none()
    );
}

#[tokio::test]
#[serial]
async fn a_body_timeout_remains_a_timeout_in_the_durable_diagnostic() {
    let server = streaming_server(true).await;
    let db = kronn::db::Database::open_in_memory().unwrap();
    let (view, outcome) = model_catalog::refresh_ollama_catalog_at(&db, &server.url)
        .await
        .unwrap();
    assert_eq!(outcome, Err(model_catalog::DiscoveryOutcome::Timeout));
    assert_eq!(
        view.last_error_reason,
        Some(ModelUnavailableReason::Timeout)
    );
    assert!(!view.live_refresh_ok);
}

#[tokio::test]
#[serial]
async fn bounded_body_and_schema_errors_preserve_last_known_catalogue_without_echoing_body() {
    let db = kronn::db::Database::open_in_memory().unwrap();
    db.with_conn(|conn| {
        store::reconcile_live(
            conn,
            "agent:ollama",
            &AgentType::Ollama,
            &[store::DiscoveredModel {
                model_id: "last-good:1".into(),
                display_name: "Last good".into(),
                capabilities: vec!["chat".into()],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )
    })
    .await
    .unwrap();
    let last_live = db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "last-good:1"))
        .await
        .unwrap()
        .unwrap();
    let sentinel = "fixture-secret-do-not-echo";
    let mut templates = vec![
        ResponseTemplate::new(200).set_body_raw(format!("invalid {sentinel}"), "application/json"),
        ResponseTemplate::new(200).set_body_json(json!({})),
        ResponseTemplate::new(200).set_body_json(json!({"models":null})),
        ResponseTemplate::new(200).set_body_json(json!({"models":{}})),
        ResponseTemplate::new(200).set_body_json(json!({"models":[{}]})),
        ResponseTemplate::new(200).set_body_json(json!({"models":[{"name":false}]})),
        ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 1_048_577]),
    ];
    for id in [
        "".to_string(),
        " bad:1".into(),
        "bad id".into(),
        "bad\0id".into(),
        "a".repeat(257),
    ] {
        templates.push(
            ResponseTemplate::new(200)
                .set_body_json(json!({"models":[{"name":"partial:1"},{"name":id}]})),
        );
    }
    for template in templates {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(template)
            .expect(1)
            .mount(&server)
            .await;
        let (view, outcome) = model_catalog::refresh_ollama_catalog_at(&db, &server.uri())
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            Err(model_catalog::DiscoveryOutcome::InvalidCatalog(_))
        ));
        assert!(!view.live_refresh_ok);
        assert!(!view.last_error_detail.unwrap().contains(sentinel));
        let rows = db
            .with_read_conn(|conn| store::list_for_target(conn, "agent:ollama"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, last_live.id);
        assert_eq!(rows[0].last_seen_at, last_live.last_seen_at);
        assert_eq!(rows[0].provenance, kronn::models::ModelProvenance::Cached);
        assert_eq!(rows[0].availability, ModelAvailability::Available);
    }
    let server = streaming_server(false).await;
    let (view, outcome) = model_catalog::refresh_ollama_catalog_at(&db, &server.url)
        .await
        .unwrap();
    assert!(
        matches!(outcome,Err(model_catalog::DiscoveryOutcome::InvalidCatalog(ref detail)) if detail.contains("size limit"))
    );
    assert_eq!(view.models.len(), 1);
    assert_eq!(view.models[0].availability, ModelAvailability::Available);
}

#[tokio::test]
#[serial]
async fn inventory_failure_is_compatible_only_after_its_diagnostic_was_saved() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(503).set_body_string("fixture-secret-do-not-echo"))
        .expect(2)
        .mount(&server)
        .await;
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let response = get(app_with_db(db.clone(), &server.uri()), "/api/ollama/models").await;
    assert_eq!(response["success"], true);
    assert_eq!(response["data"]["models"], json!([]));
    let log = db
        .with_read_conn(|conn| store::get_refresh_log(conn, "agent:ollama"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        log.last_error_reason,
        Some(ModelUnavailableReason::ProviderError)
    );
    assert!(!log
        .last_error_detail
        .unwrap()
        .contains("fixture-secret-do-not-echo"));

    db.with_conn(|conn| {
        store::reconcile_live(
            conn,
            "agent:ollama",
            &AgentType::Ollama,
            &[store::DiscoveredModel {
                model_id: "preserved:1".into(),
                display_name: "Preserved".into(),
                capabilities: vec!["chat".into()],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            }],
        )?;
        conn.execute_batch(
            "CREATE TRIGGER reject_refresh_log BEFORE INSERT ON model_catalog_refresh_log
            BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let before = db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "preserved:1"))
        .await
        .unwrap()
        .unwrap();
    let response = get(app_with_db(db.clone(), &server.uri()), "/api/ollama/models").await;
    assert_eq!(
        response["success"], false,
        "a storage failure is not an authoritative empty inventory"
    );
    let after = db
        .with_read_conn(|conn| store::get(conn, "agent:ollama", "preserved:1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after, before,
        "failed diagnostic persistence rolls back the provenance change too"
    );
}
