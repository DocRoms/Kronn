//! KT-1006 — every bridge-token effect is logged with the token id, never with
//! the token value.

use std::{
    io::Write,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{body::Body, extract::ConnectInfo, http::Request};
use kronn::core::bridge_token::{mint, BridgeScope};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_effect_is_logged_with_the_token_id_only() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);

    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects(id, name, path, created_at, updated_at) VALUES ('p1', 'p1', '/tmp', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO discussions(id, title, project_id, created_at, updated_at) VALUES ('room', 'room', 'p1', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO mcp_servers(id, name, transport) VALUES ('synthetic-api', 'Synthetic', 'stdio')",
            [],
        )?;
        conn.execute(
            "INSERT INTO mcp_configs(id, server_id, label, is_global) VALUES ('cfg', 'synthetic-api', 'cfg', 1)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_token = Some("operator".into());
    let app = build_router_with_auth(
        AppState::new_defaults(
            Arc::new(RwLock::new(config)),
            db,
            DEFAULT_MAX_CONCURRENT_AGENTS,
        ),
        true,
    );
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room".into()],
        ..Default::default()
    })
    .unwrap();
    let body = serde_json::json!({
        "api_plugin_slug": "synthetic-api", "api_config_id": "cfg",
        "endpoint_path": "/x", "method": "GET"
    });
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/agent-api/call")
        .header("authorization", format!("Bearer {}", guard.value()))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4243))));
    app.oneshot(request).await.unwrap();

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let effects: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("bridge token effect"))
        .collect();
    assert_eq!(effects.len(), 1, "{log}");
    assert!(
        effects[0].contains(&format!("token={}", guard.id())),
        "{}",
        effects[0]
    );
    assert!(
        !log.contains(guard.value()),
        "the token value never reaches a log"
    );
}
