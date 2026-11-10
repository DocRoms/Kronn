//! KT-1058 — the CORS allowlist built by `build_cors`, through the real router.
//! CORS is not a CSRF defence (simple cross-origin requests still run); these
//! tests only pin that a foreign origin is never granted read access.

use std::sync::Arc;

use axum::{body::Body, http::Request, Router};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use tokio::sync::RwLock;
use tower::ServiceExt;

const PORT: u16 = 3456;

fn router(domain: Option<&str>) -> Router {
    let mut config = kronn::core::config::default_config();
    config.server.port = PORT;
    config.server.runtime_port = None;
    config.server.domain = domain.map(str::to_string);
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    build_router_with_auth(
        AppState::new_defaults(
            Arc::new(RwLock::new(config)),
            db,
            DEFAULT_MAX_CONCURRENT_AGENTS,
        ),
        false,
    )
}

/// `Access-Control-Allow-Origin` granted to `origin`, for a GET and a preflight.
async fn granted(app: &Router, origin: &str) -> [Option<String>; 2] {
    let get = Request::builder()
        .uri("/api/health")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap();
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/api/discussions")
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .body(Body::empty())
        .unwrap();
    let mut out = [None, None];
    for (slot, request) in out.iter_mut().zip([get, preflight]) {
        let response = app.clone().oneshot(request).await.unwrap();
        *slot = response
            .headers()
            .get("access-control-allow-origin")
            .map(|value| value.to_str().unwrap().to_string());
    }
    out
}

async fn assert_allowed(app: &Router, origin: &str) {
    let expected = Some(origin.to_string());
    assert_eq!(
        granted(app, origin).await,
        [expected.clone(), expected],
        "{origin}"
    );
}

async fn assert_refused(app: &Router, origin: &str) {
    assert_eq!(granted(app, origin).await, [None, None], "{origin}");
}

#[tokio::test]
async fn without_a_domain_only_local_origins_are_allowed() {
    let app = router(None);
    assert_allowed(&app, &format!("http://localhost:{PORT}")).await;
    assert_allowed(&app, &format!("http://127.0.0.1:{PORT}")).await;
    // The nginx gateway ports belong to the Docker deployment only (KT-1033).
    if kronn::core::env::is_docker() {
        assert_allowed(&app, "http://localhost:3141").await;
    } else {
        assert_refused(&app, "http://localhost:3141").await;
    }
    for foreign in [
        "https://evil.example",
        "http://localhost:9999",
        "http://localhost",
        "null",
    ] {
        assert_refused(&app, foreign).await;
    }
}

/// KT-1596 — a declared TLS proxy serves the UI from its public origin: CORS
/// grants exactly that origin, read live (the declaration is made from Settings
/// without a restart), and still refuses every other page.
#[tokio::test]
async fn a_declared_proxy_origin_is_allowed_exactly_and_live() {
    let mut config = kronn::core::config::default_config();
    config.server.port = PORT;
    config.server.runtime_port = None;
    let config = Arc::new(RwLock::new(config));
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let app = build_router_with_auth(
        AppState::new_defaults(config.clone(), db, DEFAULT_MAX_CONCURRENT_AGENTS),
        false,
    );
    // Undeclared: the proxy's origin is just another foreign page.
    assert_refused(&app, "https://kronn.example.com").await;

    {
        let mut live = config.write().await;
        live.server.tls_terminated_by_proxy = true;
        live.server.public_origin = Some("https://kronn.example.com".into());
    }
    assert_allowed(&app, "https://kronn.example.com").await;
    for foreign in [
        "http://kronn.example.com",
        "https://kronn.example.com:8443",
        "https://kronn.example.com.evil.example",
        "https://evil.example",
        "null",
    ] {
        assert_refused(&app, foreign).await;
    }
    // The local origins keep working next to it.
    assert_allowed(&app, &format!("http://localhost:{PORT}")).await;

    config.write().await.server.tls_terminated_by_proxy = false;
    assert_refused(&app, "https://kronn.example.com").await;
}

#[tokio::test]
async fn with_a_domain_only_that_domain_is_allowed() {
    let app = router(Some("kronn.example"));
    assert_allowed(&app, "https://kronn.example").await;
    assert_allowed(&app, "http://kronn.example").await;
    assert_allowed(&app, &format!("https://kronn.example:{PORT}")).await;
    for foreign in [
        "https://evil.example",
        "https://kronn.example.evil.example",
        &format!("http://localhost:{PORT}"),
    ] {
        assert_refused(&app, foreign).await;
    }
}
