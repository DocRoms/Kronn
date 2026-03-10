//! Integration tests for the Kronn backend API.
//!
//! These tests exercise the full HTTP layer (router + handlers + DB)
//! using `tower::ServiceExt::oneshot` with an in-memory SQLite database.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

use kronn::{build_router, AppState};

// ═══════════════════════════════════════════════════════════════════════════════
// Test helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Create a test AppState with an in-memory database and default config.
fn test_state() -> AppState {
    let db = Arc::new(
        kronn::db::Database::open_in_memory().expect("Failed to open in-memory DB"),
    );
    let config = Arc::new(RwLock::new(kronn::core::config::default_config()));
    let workflow_engine = Arc::new(kronn::workflows::WorkflowEngine::new(
        db.clone(),
        config.clone(),
    ));
    AppState {
        config,
        db,
        workflow_engine,
    }
}

/// Build a test router backed by an in-memory database.
fn test_app() -> Router {
    build_router(test_state())
}

/// Send a GET request and return (status, parsed JSON body).
async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

/// Send a POST request with a JSON body and return (status, parsed JSON body).
async fn post_json(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

/// Send a DELETE request and return (status, parsed JSON body).
async fn delete_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Stats endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn stats_tokens_empty_db() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/tokens").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["total_tokens"], 0);
    assert!(json["data"]["by_provider"].as_array().unwrap().is_empty());
    assert!(json["data"]["by_project"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn stats_agent_usage_empty_db() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/agent-usage").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Config endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_tokens_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/tokens").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"]["keys"].as_array().unwrap().is_empty());
    assert!(json["data"]["disabled_overrides"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn config_language_default() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/language").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Default language from default_config() is "fr"
    assert_eq!(json["data"], "fr");
}

#[tokio::test]
async fn config_agent_access_default() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/agent-access").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Default config has full_access = false for all agents
    assert_eq!(json["data"]["claude_code"]["full_access"], false);
    assert_eq!(json["data"]["codex"]["full_access"], false);
}

#[tokio::test]
async fn config_db_info_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/db-info").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["project_count"], 0);
    assert_eq!(json["data"]["discussion_count"], 0);
    assert_eq!(json["data"]["message_count"], 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_list_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/discussions").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn discussions_create_and_list() {
    let state = test_state();

    // Create a discussion
    let create_body = serde_json::json!({
        "title": "Test discussion",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello, world!"
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["title"], "Test discussion");
    assert_eq!(json["data"]["agent"], "ClaudeCode");
    assert_eq!(json["data"]["language"], "en");
    // Should have the initial message
    assert_eq!(json["data"]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"]["messages"][0]["role"], "User");
    assert_eq!(json["data"]["messages"][0]["content"], "Hello, world!");

    let disc_id = json["data"]["id"].as_str().unwrap().to_string();

    // List discussions — should have one
    let (status, json) = get_json(build_router(state.clone()), "/api/discussions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Get discussion by id
    let (status, json) = get_json(
        build_router(state.clone()),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["id"], disc_id);
    assert_eq!(json["data"]["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn discussions_create_uses_default_language() {
    let state = test_state();

    // Create without specifying language — should use config default ("fr")
    let create_body = serde_json::json!({
        "title": "No lang specified",
        "agent": "ClaudeCode",
        "language": "",
        "initial_prompt": "Bonjour"
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["language"], "fr");
}

#[tokio::test]
async fn discussions_get_not_found() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/discussions/nonexistent-id").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn discussions_delete() {
    let state = test_state();

    // Create
    let create_body = serde_json::json!({
        "title": "To delete",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Delete me"
    });
    let (_, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    )
    .await;
    let disc_id = json["data"]["id"].as_str().unwrap().to_string();

    // Delete
    let (status, json) = delete_json(
        build_router(state.clone()),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // Verify gone
    let (_, json) = get_json(build_router(state.clone()), "/api/discussions").await;
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn discussions_delete_not_found() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/discussions/nonexistent-id").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Projects endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn projects_list_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflows endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn workflows_list_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/workflows").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Export endpoint test
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_export_empty_db() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/export").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["version"], 1);
    assert!(json["data"]["projects"].as_array().unwrap().is_empty());
    assert!(json["data"]["discussions"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Cross-cutting: discussion creation validates project reference
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_create_with_invalid_project() {
    let app = test_app();
    let body = serde_json::json!({
        "project_id": "nonexistent-project",
        "title": "Bad ref",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Test"
    });
    let (status, json) = post_json(app, "/api/discussions", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Project not found"));
}
