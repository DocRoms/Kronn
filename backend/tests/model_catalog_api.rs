//! Integration tests for the KT-531/KT-543 dynamic model catalog HTTP surface
//! (`/api/model-catalogs*`) — end to end through the real router + handlers +
//! in-memory DB. Focused on OpenCode (KT-543): the catalog must treat it as a
//! first-class, catalog-managed CLI target, never silently as `Custom`, and
//! its Zen-routed models must carry an honest, catalog-driven cost/privacy
//! overlay instead of a hardcoded model list.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};

/// See api_tests.rs — without this, handler-level config saves during tests
/// write the developer's REAL config.toml (2026-07-13 incident).
fn isolate_config_dir() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("kronn-inttest-mccfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("KRONN_DATA_DIR", &dir);
    });
}

fn test_app() -> Router {
    isolate_config_dir();
    let db = Arc::new(kronn::db::Database::open_in_memory().expect("in-memory DB"));
    let mut cfg = kronn::core::config::default_config();
    cfg.server.auth_token = None;
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(cfg)),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    build_router_with_auth(state, false)
}

async fn request(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(body) => Body::from(serde_json::to_vec(&body).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get(app: Router, uri: &str) -> (StatusCode, Value) {
    request(app, "GET", uri, None).await
}

async fn post(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(app, "POST", uri, Some(body)).await
}

async fn put(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    request(app, "PUT", uri, Some(body)).await
}

/// DoD 1/4: OpenCode must appear as its own catalog-managed CLI target
/// (`agent:opencode`) in the shared snapshot every selector reads — never
/// silently absent, and never merged into a generic `Custom` bucket.
#[tokio::test]
async fn list_includes_opencode_as_a_first_class_cli_target() {
    let app = test_app();
    let (status, json) = get(app, "/api/model-catalogs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let targets = json["data"]["targets"].as_array().expect("targets array");
    let opencode = targets
        .iter()
        .find(|t| t["runtime_target_id"] == "agent:opencode")
        .expect("agent:opencode must be present among the CLI targets");
    assert_eq!(opencode["agent_type"], "OpenCode");
    assert_ne!(
        opencode["agent_type"], "Custom",
        "OpenCode must never be represented as a Custom fallback"
    );
}

fn manual_request(overrides: Value) -> Value {
    let mut base = json!({
        "runtime_target_id": "agent:opencode",
        "agent_type": "OpenCode",
        "model_id": "opencode/big-pickle",
        "display_name": "Big Pickle",
        "capabilities": ["chat"],
        "reasoning_modes": [],
        "default_reasoning_mode": null,
        "tier_assignment": null,
    });
    for (key, value) in overrides.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// DoD 5: an operator can record a cost/privacy assessment for an OpenCode
/// Zen model through the same generic manual-entry endpoint every other
/// runtime uses — no Zen-specific route, no hardcoded model list.
#[tokio::test]
async fn create_manual_persists_opencode_zen_cost_hint_and_privacy_note() {
    let app = test_app();
    let (status, json) = post(
        app,
        "/api/model-catalogs/manual",
        manual_request(json!({
            "cost_hint": "unknown",
            "privacy_note": "Routed through OpenCode Zen, a third-party gateway.",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true, "creation failed: {json:?}");
    assert_eq!(json["data"]["cost_hint"], "unknown");
    assert_eq!(
        json["data"]["privacy_note"],
        "Routed through OpenCode Zen, a third-party gateway."
    );
}

/// DoD 5: cost_hint/privacy_note default to absent (rendered "unknown" by
/// the frontend) rather than a guessed value when the operator doesn't send
/// them — the catalog never invents a cost.
#[tokio::test]
async fn create_manual_without_cost_fields_leaves_them_absent() {
    let app = test_app();
    let (status, json) = post(app, "/api/model-catalogs/manual", manual_request(json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"]["cost_hint"].is_null());
    assert!(json["data"]["privacy_note"].is_null());
}

/// DoD 5: an unrelated follow-up edit (e.g. correcting the display name)
/// must not silently wipe a previously recorded cost/privacy assessment.
#[tokio::test]
async fn update_manual_preserves_cost_hint_when_not_resent() {
    let app = test_app();
    let (status, _) = post(
        app.clone(),
        "/api/model-catalogs/manual",
        manual_request(json!({
            "cost_hint": "free",
            "privacy_note": "Temporary free promotion.",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = put(
        app,
        "/api/model-catalogs/manual",
        manual_request(json!({ "display_name": "Big Pickle (renamed)" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true, "update failed: {json:?}");
    assert_eq!(json["data"]["display_name"], "Big Pickle (renamed)");
    assert_eq!(
        json["data"]["cost_hint"], "free",
        "an edit that doesn't mention cost_hint must not clear it"
    );
    assert_eq!(
        json["data"]["privacy_note"], "Temporary free promotion.",
        "an edit that doesn't mention privacy_note must not clear it"
    );
}

/// DoD 1/4: a runtime_target_id of `agent:opencode` paired with a mismatched
/// agent_type is rejected — the projection check never lets a caller smuggle
/// an OpenCode-shaped identity in as another agent (or vice versa), so
/// OpenCode can never be silently reclassified as Custom through this path.
#[tokio::test]
async fn create_manual_rejects_mismatched_opencode_projection() {
    let app = test_app();
    let (status, json) = post(
        app,
        "/api/model-catalogs/manual",
        manual_request(json!({ "agent_type": "Custom" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert_eq!(json["error_code"], "validation");
}

/// DoD 6: deleting a manual OpenCode entry works through the same generic
/// endpoint as any other runtime.
#[tokio::test]
async fn delete_manual_removes_the_opencode_entry() {
    let app = test_app();
    post(app.clone(), "/api/model-catalogs/manual", manual_request(json!({}))).await;
    let (status, json) = post(
        app,
        "/api/model-catalogs/manual/delete",
        json!({ "runtime_target_id": "agent:opencode", "model_id": "opencode/big-pickle" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true, "delete failed: {json:?}");
}

/// DoD 1/6: end-to-end through `/api/agents` — OpenCode must surface as its
/// own identity (never dropped, never merged into `Custom`) with a real,
/// actionable setup hint rather than a generic always-ready default. The
/// `installed` flag itself depends on the host running this test (a dev
/// machine may have `opencode` on PATH), so it is deliberately not asserted
/// here — the auth-signal cases (missing/invalid file stays "unknown, not
/// blocked"; a real auth file is confirmed ready) are covered at the unit
/// level (`agents::tests::opencode_missing_auth_file_is_unknown_not_blocked`,
/// `agents::tests::opencode_with_auth_file_is_confirmed_ready`).
#[tokio::test]
async fn agents_endpoint_reports_opencode_with_a_real_auth_hint() {
    let app = test_app();
    let (status, json) = get(app, "/api/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let agents = json["data"].as_array().expect("agents array");
    let opencode = agents
        .iter()
        .find(|a| a["agent_type"] == "OpenCode")
        .expect("OpenCode must be present in agent detection");
    assert_eq!(
        opencode["auth_setup_command"], "opencode auth login",
        "OpenCode must carry its own auth diagnostic, not a generic default"
    );
    assert_eq!(opencode["install_command"], "npm install -g opencode-ai");
}
