//! KT-1026 — an API access policy through the production router: only a person
//! sets or widens it, a bridge token never does, agents see it, and the broker
//! enforces it for a bridge token's own identity.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::core::api_access::AgentIdentity;
use kronn::core::bridge_token::{mint, BridgeScope};
use kronn::models::*;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

const POLICY_PATH: &str = "/api/mcps/servers/custom-notion/access-policy";

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40404))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn fixture() -> (Router, Arc<kronn::db::Database>) {
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let secret = kronn::core::crypto::generate_secret();
    let env = HashMap::from([("NOTION_TOKEN".to_string(), "kt1026-token".to_string())]);
    let encrypted = kronn::db::mcps::encrypt_env(&env, &secret).unwrap();
    db.with_conn(move |conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects(id, name, path, created_at, updated_at) VALUES ('p1', 'p1', '/tmp/kt1026-p1', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO discussions(id, title, project_id, created_at, updated_at) VALUES ('room-a', 'room-a', 'p1', ?1, ?1)",
            [&now],
        )?;
        kronn::db::mcps::upsert_server(
            conn,
            &McpServer {
                id: "custom-notion".into(),
                name: "Notion".into(),
                description: String::new(),
                transport: McpTransport::ApiOnly,
                source: McpSource::Manual,
                api_spec: Some(ApiSpec {
                    base_url: "https://api.notion.com/v1".into(),
                    auth: ApiAuthKind::Bearer {
                        env_key: "NOTION_TOKEN".into(),
                    },
                    endpoints: vec![ApiEndpoint {
                        method: "GET".into(),
                        path: "/users/me".into(),
                        description: String::new(),
                    }],
                    docs_url: None,
                    config_keys: vec![],
                    default_headers: vec![],
                    test_endpoint: None,
                }),
            },
        )?;
        kronn::db::mcps::insert_config(
            conn,
            &McpConfig {
                id: "cfg-notion".into(),
                server_id: "custom-notion".into(),
                label: "Notion".into(),
                env_keys: vec!["NOTION_TOKEN".into()],
                env_encrypted: encrypted,
                args_override: None,
                is_global: true,
                include_general: true,
                config_hash: "kt1026".into(),
                project_ids: Vec::new(),
                host_sync: HostSyncMode::None,
            },
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some("operator-bearer-never-sent-here".into());
    config.encryption_secret = Some(secret);
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (build_router_with_auth(state, true), db)
}

fn scope() -> BridgeScope {
    BridgeScope {
        discussion_ids: vec!["room-a".into()],
        agent: Some(AgentIdentity {
            agent_type: AgentType::ClaudeCode,
            model: None,
        }),
        ..Default::default()
    }
}

async fn stored(db: &Arc<kronn::db::Database>) -> Option<ApiAccessPolicy> {
    db.with_conn(|conn| kronn::db::api_access_policies::get(conn, "custom-notion"))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_bridge_token_can_neither_set_nor_widen_nor_remove_a_policy() {
    let (app, db) = fixture().await;
    let guard = mint(scope()).unwrap();
    let token = guard.value().to_owned();

    let open = json!({"policy": {"access": {"kind": "all"}, "endpoints": []}});
    let (status, body) = call(&app, "PUT", POLICY_PATH, Some(&token), Some(open.clone())).await;
    assert_eq!(status, 403, "a bridge token set a policy: {body}");
    assert_eq!(stored(&db).await, None);

    // A person sets a restrictive policy; the token cannot widen or drop it.
    let (status, body) = call(
        &app,
        "PUT",
        POLICY_PATH,
        None,
        Some(json!({"policy": {"access": {"kind": "blocked"}}})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], true, "{body}");
    let blocked = Some(ApiAccessPolicy {
        access: ApiAccessRule::Blocked,
        endpoints: vec![],
    });
    assert_eq!(stored(&db).await, blocked);
    for attempt in [open, json!({"policy": null})] {
        let (status, body) = call(&app, "PUT", POLICY_PATH, Some(&token), Some(attempt)).await;
        assert_eq!(status, 403, "a bridge token changed a policy: {body}");
    }
    assert_eq!(stored(&db).await, blocked);
}

#[tokio::test]
async fn agents_see_the_policy_and_the_broker_refuses_their_identity() {
    let (app, db) = fixture().await;
    db.with_conn(|conn| {
        kronn::db::api_access_policies::set(
            conn,
            "custom-notion",
            &ApiAccessPolicy {
                access: ApiAccessRule::Agents {
                    agents: vec![ApiAccessSubject {
                        agent: AgentType::Codex,
                        model: None,
                    }],
                },
                endpoints: vec![],
            },
        )
    })
    .await
    .unwrap();
    let guard = mint(scope()).unwrap();
    let token = guard.value().to_owned();

    let (status, overview) = call(&app, "GET", "/api/mcps", Some(&token), None).await;
    assert_eq!(status, 200);
    let entry = &overview["data"]["access_policies"][0];
    assert_eq!(entry["server_id"], "custom-notion", "{overview}");
    assert_eq!(entry["policy"]["access"]["kind"], "agents");

    let (status, body) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(&token),
        Some(json!({
            "api_plugin_slug": "custom-notion",
            "api_config_id": "cfg-notion",
            "endpoint_path": "/users/me",
        })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let error = body["data"]["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("Access policy") && error.contains("reserved to Codex"),
        "{body}"
    );
    assert!(!body.to_string().contains("kt1026-token"));

    // No token: an agent Kronn did not launch is never a named agent.
    let (_, body) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        None,
        Some(json!({
            "api_plugin_slug": "custom-notion",
            "api_config_id": "cfg-notion",
            "endpoint_path": "/users/me",
        })),
    )
    .await;
    let error = body["data"]["error"].as_str().unwrap_or_default();
    assert!(error.contains("no Kronn-issued identity"), "{body}");
}

#[tokio::test]
async fn an_invalid_policy_is_refused() {
    let (app, db) = fixture().await;
    let (_, body) = call(
        &app,
        "PUT",
        POLICY_PATH,
        None,
        Some(json!({"policy": {"access": {"kind": "all"},
            "endpoints": [{"method": "GET", "path": "/a/../b", "access": {"kind": "all"}}]}})),
    )
    .await;
    assert_eq!(body["success"], false, "{body}");
    assert_eq!(stored(&db).await, None);
}
