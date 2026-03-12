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
// Health endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn health_returns_ok() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], true);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Setup status endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn setup_status_returns_ok() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/setup/status").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Should have all required fields
    assert!(json["data"]["is_first_run"].is_boolean());
    assert!(json["data"]["current_step"].is_string());
    assert!(json["data"]["agents_detected"].is_array());
    assert!(json["data"]["repos_detected"].is_array());
    assert!(json["data"]["scan_paths_set"].is_boolean());
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
// Skills endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn skills_list_returns_builtins() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/skills").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let skills = json["data"].as_array().unwrap();
    assert!(skills.len() >= 14, "Expected at least 14 builtin skills, got {}", skills.len());

    // Verify a known builtin skill
    let rust = skills.iter().find(|s| s["id"] == "rust");
    assert!(rust.is_some(), "rust skill not found");
    let rs = rust.unwrap();
    assert_eq!(rs["name"], "Rust");
    assert_eq!(rs["icon"], "🦀");
    assert_eq!(rs["category"], "Language");
    assert_eq!(rs["is_builtin"], true);
    assert!(!rs["content"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn skills_list_has_all_categories() {
    let app = test_app();
    let (_, json) = get_json(app, "/api/skills").await;

    let skills = json["data"].as_array().unwrap();
    let categories: std::collections::HashSet<&str> = skills.iter()
        .filter_map(|s| s["category"].as_str())
        .collect();

    assert!(categories.contains("Language"), "No Language skills found");
    assert!(categories.contains("Domain"), "No Domain skills found");
    assert!(categories.contains("Business"), "No Business skills found");
}

#[tokio::test]
async fn skills_delete_builtin_rejected() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/skills/rust").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("builtin"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions with skills
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_create_with_skill_ids() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "Skill discussion",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello",
        "skill_ids": ["rust", "typescript"]
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let skill_ids = json["data"]["skill_ids"].as_array().unwrap();
    assert_eq!(skill_ids.len(), 2);
    assert_eq!(skill_ids[0], "rust");
    assert_eq!(skill_ids[1], "typescript");

    // Retrieve and verify skill_ids persisted
    let disc_id = json["data"]["id"].as_str().unwrap();
    let (_, json) = get_json(
        build_router(state.clone()),
        &format!("/api/discussions/{}", disc_id),
    ).await;
    let skill_ids = json["data"]["skill_ids"].as_array().unwrap();
    assert_eq!(skill_ids.len(), 2);
}

#[tokio::test]
async fn discussions_create_without_skill_ids_defaults_empty() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "No skills",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello"
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // skill_ids should either be absent or empty array
    let skill_ids = json["data"]["skill_ids"].as_array();
    if let Some(ids) = skill_ids {
        assert!(ids.is_empty());
    }
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

// ═══════════════════════════════════════════════════════════════════════════════
// Profiles endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn profiles_list_returns_builtins() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/profiles").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let profiles = json["data"].as_array().unwrap();
    assert!(!profiles.is_empty(), "Expected at least 1 profile");

    // At least one builtin profile should be present
    let builtins: Vec<_> = profiles.iter().filter(|p| p["is_builtin"] == true).collect();
    assert!(!builtins.is_empty(), "Expected at least 1 builtin profile");

    // All profiles should have required fields
    for p in profiles {
        assert!(!p["name"].as_str().unwrap().is_empty());
        assert!(!p["persona_prompt"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn profiles_create_and_get() {
    let state = test_state();

    let create_body = serde_json::json!({
        "name": "Test Profile",
        "role": "Test Assistant",
        "avatar": "🤖",
        "color": "#ff0000",
        "category": "Technical",
        "persona_prompt": "You are a test assistant."
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/profiles",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["name"], "Test Profile");
    assert_eq!(json["data"]["category"], "Technical");
    assert_eq!(json["data"]["is_builtin"], false);
    assert_eq!(json["data"]["persona_prompt"], "You are a test assistant.");

    let profile_id = json["data"]["id"].as_str().unwrap().to_string();

    // Get by id
    let (status, json) = get_json(
        build_router(state.clone()),
        &format!("/api/profiles/{}", profile_id),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["id"], profile_id);
    assert_eq!(json["data"]["name"], "Test Profile");
}

#[tokio::test]
async fn profiles_delete_builtin_rejected() {
    let app = test_app();
    // Get a builtin profile id first
    let (_, json) = get_json(build_router(test_state()), "/api/profiles").await;
    let profiles = json["data"].as_array().unwrap();
    if profiles.is_empty() {
        return; // No builtins to test
    }
    let builtin_id = profiles[0]["id"].as_str().unwrap();

    let (status, json) = delete_json(app, &format!("/api/profiles/{}", builtin_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("builtin"));
}

#[tokio::test]
async fn profiles_delete_custom() {
    let state = test_state();

    // Create a custom profile
    let create_body = serde_json::json!({
        "name": "To Delete",
        "role": "Temporary",
        "avatar": "🗑️",
        "color": "#999999",
        "category": "Meta",
        "persona_prompt": "Temporary."
    });
    let (_, json) = post_json(
        build_router(state.clone()),
        "/api/profiles",
        create_body,
    ).await;
    let profile_id = json["data"]["id"].as_str().unwrap().to_string();

    // Delete it
    let (status, json) = delete_json(
        build_router(state.clone()),
        &format!("/api/profiles/{}", profile_id),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // Verify it's gone from the list (only builtins remain)
    let (_, json) = get_json(build_router(state.clone()), "/api/profiles").await;
    let ids: Vec<&str> = json["data"].as_array().unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert!(!ids.contains(&profile_id.as_str()), "Deleted profile should not appear in list");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions with profile_ids
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_create_with_profile_id() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "Profile discussion",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello",
        "profile_ids": ["some-profile-id"]
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["profile_ids"], serde_json::json!(["some-profile-id"]));

    // Retrieve and verify profile_ids persisted
    let disc_id = json["data"]["id"].as_str().unwrap();
    let (_, json) = get_json(
        build_router(state.clone()),
        &format!("/api/discussions/{}", disc_id),
    ).await;
    assert_eq!(json["data"]["profile_ids"], serde_json::json!(["some-profile-id"]));
}

#[tokio::test]
async fn discussions_create_without_profile_id_defaults_null() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "No profile",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello"
    });
    let (status, json) = post_json(
        build_router(state.clone()),
        "/api/discussions",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // profile_ids should be absent (skip_serializing_if = "Vec::is_empty")
    assert!(json["data"]["profile_ids"].is_null() || json["data"]["profile_ids"] == serde_json::json!([]));
}
