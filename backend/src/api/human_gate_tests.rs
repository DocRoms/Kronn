//! KT-1495 — an agent process on the loopback cannot take Kronn's secrets by
//! leaving its bridge token out of the request. Every test goes through the
//! production router (`build_router_with_auth`), the auth middleware included.

use axum::http::StatusCode;
use serde_json::Value;
use serial_test::serial;
use tower::ServiceExt;

use crate::AppState;

const REGENERATE: &str = "/api/config/auth-token/regenerate";

struct Instance {
    state: AppState,
    seed: Option<Vec<u8>>,
    _dir: tempfile::TempDir,
}

impl Instance {
    fn new(enrolled: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KRONN_DATA_DIR", dir.path());
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let mut config = crate::core::config::default_config();
        let secret = crate::core::crypto::generate_secret();
        let key = crate::core::crypto::parse_secret(&secret).unwrap();
        config.encryption_secret = Some(secret);
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let seed = enrolled.then(|| {
            let now = now();
            let enrolment =
                crate::core::trust_proof::begin_enrolment(dir.path(), &key, "me", now, false)
                    .unwrap();
            let seed = crate::core::totp::base32_decode(&enrolment.secret).unwrap();
            let code = crate::core::totp::code_at(&seed, crate::core::totp::step_of(now));
            crate::core::trust_proof::confirm_enrolment(dir.path(), &key, &code, now).unwrap();
            seed
        });
        Self {
            state,
            seed,
            _dir: dir,
        }
    }

    /// The code of the step after the one that confirmed the enrolment.
    fn code(&self) -> String {
        crate::core::totp::code_at(
            self.seed.as_ref().unwrap(),
            crate::core::totp::step_of(now()) + 1,
        )
    }

    fn router(&self) -> axum::Router {
        crate::build_router_with_auth(self.state.clone(), true)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn call(
    router: axum::Router,
    method: &str,
    path: &str,
    from: [u8; 4],
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder
        .body(axum::body::Body::from(if method == "POST" {
            "{}"
        } else {
            ""
        }))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            from, 40000,
        ))));
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const LOOPBACK: [u8; 4] = [127, 0, 0, 1];

#[tokio::test]
#[serial]
async fn a_loopback_caller_without_a_token_is_not_the_owner_for_the_vault_routes() {
    let instance = Instance::new(true);
    for path in super::HUMAN_PROOF_ROUTES {
        let (status, body) = call(instance.router(), "POST", path, LOOPBACK, &[]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(body["error_code"], "confirm_required", "{path}");
    }
    assert!(
        instance
            .state
            .config
            .read()
            .await
            .server
            .auth_token
            .is_none(),
        "no token was minted"
    );
}

#[tokio::test]
#[serial]
async fn a_wrong_code_is_refused_a_good_one_passes_once_and_is_not_replayed() {
    let instance = Instance::new(true);
    let (status, body) = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("x-kronn-confirm", "000000")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error_code"], "confirm_required");

    let code = instance.code();
    let (status, body) = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("x-kronn-confirm", &code)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true, "{body}");

    // The code is single use: the next call needs another.
    let (status, body) = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("x-kronn-confirm", &code)],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
#[serial]
async fn the_proof_is_journalled_with_the_vault_route() {
    let instance = Instance::new(true);
    let _ = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("x-kronn-confirm", "123456")],
    )
    .await;
    let code = instance.code();
    let _ = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("x-kronn-confirm", &code)],
    )
    .await;
    let entries = instance
        .state
        .db
        .with_conn(|conn| crate::db::trust::audit_entries(conn, 20))
        .await
        .unwrap();
    let events: Vec<&str> = entries.iter().map(|entry| entry.event.as_str()).collect();
    assert!(events.contains(&"proof"), "{events:?}");
    assert!(events.contains(&"proof_refused"), "{events:?}");
    let line = entries.iter().find(|e| e.event == "proof").unwrap();
    assert!(line.subject.contains(REGENERATE), "{}", line.subject);
    assert!(!format!("{entries:?}").contains(&code), "never the code");
}

/// Owner rule: nothing that works today is refused. No enrolment, no gate.
#[tokio::test]
#[serial]
async fn without_an_enrolment_the_vault_routes_work_as_they_did() {
    let instance = Instance::new(false);
    let (status, body) = call(instance.router(), "POST", REGENERATE, LOOPBACK, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true, "{body}");
}

#[tokio::test]
#[serial]
async fn an_ordinary_route_never_asks_for_a_code() {
    let instance = Instance::new(true);
    for path in ["/api/projects", "/api/trust", "/api/setup/status"] {
        let (status, _) = call(instance.router(), "GET", path, LOOPBACK, &[]).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{path}");
    }
}

/// The operator token is a credential of its own: the owner who set one and
/// presents it is not asked for a code on top.
#[tokio::test]
#[serial]
async fn the_operator_token_is_not_asked_a_code_on_top() {
    let instance = Instance::new(true);
    {
        let mut config = instance.state.config.write().await;
        config.server.auth_enabled = true;
        config.server.auth_token = Some("operator-token-for-the-test".into());
    }
    let (status, body) = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("authorization", "Bearer operator-token-for-the-test")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // A wrong bearer never falls back to the loopback.
    let (status, _) = call(
        instance.router(),
        "POST",
        REGENERATE,
        LOOPBACK,
        &[("authorization", "Bearer not-the-token")],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A bridge token reaches none of these routes, with or without a code.
#[test]
fn no_vault_route_is_a_bridge_route() {
    for path in super::HUMAN_PROOF_ROUTES {
        assert!(
            crate::core::bridge_token::route_for("POST", path).is_none(),
            "{path}"
        );
        assert!(!crate::core::bridge_token::trust_confirm_route(
            "POST", path
        ));
    }
}

#[test]
fn only_the_listed_posts_are_vault_routes() {
    use axum::http::Method;
    assert!(super::needs_human(&Method::POST, REGENERATE));
    assert!(!super::needs_human(&Method::GET, REGENERATE));
    assert!(!super::needs_human(&Method::POST, "/api/projects"));
    assert!(!super::needs_human(
        &Method::POST,
        "/api/config/recovery/status"
    ));
}
