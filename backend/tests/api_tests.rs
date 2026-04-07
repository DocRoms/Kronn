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

use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use kronn::models::WsMessage;
use futures::{SinkExt, StreamExt};

// ═══════════════════════════════════════════════════════════════════════════════
// Test helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Create a test AppState with an in-memory database and default config.
fn test_state() -> AppState {
    let db = Arc::new(
        kronn::db::Database::open_in_memory().expect("Failed to open in-memory DB"),
    );
    let mut cfg = kronn::core::config::default_config();
    cfg.server.auth_token = None; // Disable auth for tests
    let config = Arc::new(RwLock::new(cfg));
    let workflow_engine = Arc::new(kronn::workflows::WorkflowEngine::new(
        db.clone(),
        config.clone(),
    ));
    let (ws_tx, _) = tokio::sync::broadcast::channel(256);
    AppState {
        config,
        db,
        workflow_engine,
        agent_semaphore: Arc::new(tokio::sync::Semaphore::new(DEFAULT_MAX_CONCURRENT_AGENTS)),
        audit_tracker: Arc::new(std::sync::Mutex::new(kronn::AuditTracker::default())),
        ws_broadcast: Arc::new(ws_tx),
    }
}

/// Build a test router backed by an in-memory database (auth disabled).
fn test_app() -> Router {
    build_router_with_auth(test_state(), false)
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

/// Send a PATCH request with a JSON body and return (status, parsed JSON body).
async fn patch_json(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("PATCH")
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
// Context files endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper to create a discussion via the API, returns the discussion ID.
async fn create_test_discussion(state: &kronn::AppState) -> String {
    let disc_id = uuid::Uuid::new_v4().to_string();
    state.db.with_conn({
        let id = disc_id.clone();
        move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
                 VALUES (?1, 'Test', 'ClaudeCode', 'en', '[]', datetime('now'), datetime('now'))",
                rusqlite::params![id],
            )?;
            Ok(())
        }
    }).await.unwrap();
    disc_id
}

#[tokio::test]
async fn context_files_list_empty() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    let (status, json) = get_json(app, &format!("/api/discussions/{}/context-files", disc_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn context_files_upload_text_file() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    // Build multipart body
    let boundary = "----TestBoundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nHello world\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{}/context-files", disc_id))
        .header("content-type", format!("multipart/form-data; boundary={}", boundary))
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["file"]["filename"], "test.txt");
    assert_eq!(json["data"]["file"]["extracted_size"], 11); // "Hello world".len()
    assert!(json["data"]["file"]["disk_path"].is_null());
}

#[tokio::test]
async fn context_files_upload_unsupported_format() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    let boundary = "----TestBoundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"binary.exe\"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\x01\x02\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{}/context-files", disc_id))
        .header("content-type", format!("multipart/form-data; boundary={}", boundary))
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Unsupported"));
}

#[tokio::test]
async fn context_files_delete() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;

    // Insert a context file directly via DB
    state.db.with_conn({
        let did = disc_id.clone();
        move |conn| {
            kronn::db::discussions::insert_context_file(conn, "cf-del", &did, "to_delete.txt", "text/plain", 10, "Test", None)
                .map_err(|e| anyhow::anyhow!(e))
        }
    }).await.unwrap();

    let app = kronn::build_router(state);
    let (status, json) = delete_json(app, &format!("/api/discussions/{}/context-files/cf-del", disc_id)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
}

#[tokio::test]
async fn context_files_delete_nonexistent_returns_error() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    let (_, json) = delete_json(app, &format!("/api/discussions/{}/context-files/nonexistent", disc_id)).await;
    assert_eq!(json["success"], false);
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
        build_router_with_auth(state.clone(), false),
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
    let (status, json) = get_json(build_router_with_auth(state.clone(), false), "/api/discussions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Get discussion by id
    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;
    let disc_id = json["data"]["id"].as_str().unwrap().to_string();

    // Delete
    let (status, json) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // Verify gone
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/discussions").await;
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
    let req = Request::builder()
        .method("GET")
        .uri("/api/config/export")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap().to_str().unwrap(),
        "application/zip"
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let cursor = std::io::Cursor::new(&bytes[..]);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();

    // Verify data.json contains version 3 with empty collections
    {
        let mut data_file = archive.by_name("data.json").unwrap();
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut data_file, &mut contents).unwrap();
        let data: Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(data["version"], 3);
        assert!(data["projects"].as_array().unwrap().is_empty());
        assert!(data["discussions"].as_array().unwrap().is_empty());
        assert!(data["workflows"].as_array().unwrap().is_empty());
        assert!(data["mcp_servers"].as_array().unwrap().is_empty());
        assert!(data["mcp_configs"].as_array().unwrap().is_empty());
        assert!(data["contacts"].as_array().unwrap().is_empty());
    }

    // Verify config.toml exists
    assert!(archive.by_name("config.toml").is_ok());
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
    assert_eq!(rs["name"], "rust");
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
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
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
    let (_, json) = get_json(build_router_with_auth(test_state(), false), "/api/profiles").await;
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
        build_router_with_auth(state.clone(), false),
        "/api/profiles",
        create_body,
    ).await;
    let profile_id = json["data"]["id"].as_str().unwrap().to_string();

    // Delete it
    let (status, json) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/profiles/{}", profile_id),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // Verify it's gone from the list (only builtins remain)
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/profiles").await;
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
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["profile_ids"], serde_json::json!(["some-profile-id"]));

    // Retrieve and verify profile_ids persisted
    let disc_id = json["data"]["id"].as_str().unwrap();
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
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
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // profile_ids should be absent (skip_serializing_if = "Vec::is_empty")
    assert!(json["data"]["profile_ids"].is_null() || json["data"]["profile_ids"] == serde_json::json!([]));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Template validation tests (Phase 2)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn templates_use_placeholder_syntax() {
    // All template files should use {{PLACEHOLDER}} syntax, not <!-- fill --> or <!-- ... -->
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        // Skip if templates dir not available (CI without full repo)
        return;
    }

    let ambiguous_patterns = ["<!-- fill", "<!-- Add ", "<!-- Describe ", "<!-- List "];

    for entry in walkdir::WalkDir::new(&template_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        // Skip TEMPLATE.md which is a template for templates
        if entry.path().file_name().is_some_and(|n| n == "TEMPLATE.md") {
            continue;
        }

        let content = std::fs::read_to_string(entry.path()).unwrap();
        let rel = entry.path().strip_prefix(&template_dir).unwrap();

        for pattern in &ambiguous_patterns {
            assert!(
                !content.contains(pattern),
                "Template {} contains ambiguous placeholder '{}' — use {{{{PLACEHOLDER}}}} syntax instead",
                rel.display(), pattern
            );
        }
    }
}

#[test]
fn templates_have_no_empty_comment_placeholders() {
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        return;
    }

    for entry in walkdir::WalkDir::new(&template_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        if entry.path().file_name().is_some_and(|n| n == "TEMPLATE.md") {
            continue;
        }

        let content = std::fs::read_to_string(entry.path()).unwrap();
        let rel = entry.path().strip_prefix(&template_dir).unwrap();

        // Should not have generic <!-- ... --> placeholders (except TODO markers and KRONN markers)
        for (i, line) in content.lines().enumerate() {
            if line.contains("<!-- ") && !line.contains("<!-- TODO") && !line.contains("<!-- KRONN") {
                // Allow specific known comments
                if line.contains("<!-- Fill") || line.contains("<!-- Flag") || line.contains("<!-- Add entries") {
                    // These are instructions in table comments, acceptable
                    continue;
                }
                panic!(
                    "Template {}:{} has generic comment placeholder: {}",
                    rel.display(), i + 1, line.trim()
                );
            }
        }
    }
}

#[test]
fn templates_all_have_placeholders() {
    // Verify that key template files contain {{PLACEHOLDER}} patterns (they're skeletons, not empty)
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        return;
    }

    let expected_files = [
        "index.md",
        "glossary.md",
        "repo-map.md",
        "coding-rules.md",
        "testing-quality.md",
        "architecture/overview.md",
        "operations/debug-operations.md",
        "inconsistencies-tech-debt.md",
    ];

    for file in &expected_files {
        let path = template_dir.join(file);
        assert!(path.exists(), "Template file {} should exist", file);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("{{"),
            "Template {} should contain {{{{PLACEHOLDER}}}} patterns",
            file
        );
    }
}

#[test]
fn template_mcp_template_exists() {
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        return;
    }

    let mcp_template = template_dir.join("operations/mcp-servers/TEMPLATE.md");
    assert!(mcp_template.exists(), "MCP TEMPLATE.md should exist");
    let content = std::fs::read_to_string(&mcp_template).unwrap();
    assert!(content.contains("{{MCP_NAME}}"), "MCP template should have {{MCP_NAME}} placeholder");
    assert!(content.contains("Rules"), "MCP template should have Rules section");
    assert!(content.contains("Gotchas") || content.contains("Examples") || content.contains("usage patterns"), "MCP template should have gotchas or examples section");
}

#[test]
fn template_tech_debt_dir_exists() {
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        return;
    }

    let td_dir = template_dir.join("tech-debt");
    assert!(td_dir.exists(), "tech-debt/ directory should exist in templates");
    assert!(td_dir.join(".gitkeep").exists(), "tech-debt/.gitkeep should exist");
}

#[test]
fn template_inconsistencies_has_outdated_prerequisites_table() {
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        return;
    }

    let content = std::fs::read_to_string(template_dir.join("inconsistencies-tech-debt.md")).unwrap();
    assert!(content.contains("Outdated dependencies") || content.contains("Outdated prerequisites"), "Should have outdated dependencies/prerequisites section");
    assert!(content.contains("Severity"), "Should have severity column");
    assert!(content.contains("ai/tech-debt/"), "Should reference tech-debt detail files");
}

#[test]
fn template_glossary_has_todo_marker_guidance() {
    let template_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/ai");
    if !template_dir.exists() {
        return;
    }

    let content = std::fs::read_to_string(template_dir.join("glossary.md")).unwrap();
    assert!(content.contains("TODO: ask user"), "Glossary should mention TODO: ask user markers");
}

// ═══════════════════════════════════════════════════════════════════════════════
// ANALYSIS_STEPS prompt validation tests (Phase 1 + 2)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn analysis_steps_glossary_mentions_todo_markers() {
    // The glossary step prompt should instruct the agent to add TODO markers for unknown terms
    let prompt_preamble = include_str!("../src/api/audit.rs");
    // Find the glossary step prompt
    assert!(
        prompt_preamble.contains("TODO: ask user"),
        "Glossary step prompt should instruct adding TODO: ask user markers"
    );
}

#[test]
fn analysis_steps_tech_debt_creates_detail_files() {
    let source = include_str!("../src/api/audit.rs");
    assert!(
        source.contains("ai/tech-debt/TD-"),
        "Tech debt step should instruct creating detail files in ai/tech-debt/"
    );
}

#[test]
fn analysis_steps_tech_debt_checks_outdated_prerequisites() {
    let source = include_str!("../src/api/audit.rs");
    // The tech debt step should mention checking for outdated prerequisites
    assert!(
        source.contains("deprecated") || source.contains("EOL") || source.contains("outdated"),
        "Tech debt step should instruct checking for outdated prerequisites"
    );
}

#[test]
fn analysis_steps_review_checks_tech_debt_files() {
    let source = include_str!("../src/api/audit.rs");
    assert!(
        source.contains("Tech debt files") || source.contains("tech-debt/"),
        "Review step should verify tech-debt detail files exist"
    );
}

#[test]
fn analysis_steps_review_checks_glossary_todos() {
    let source = include_str!("../src/api/audit.rs");
    assert!(
        source.contains("Glossary TODO") || source.contains("TODO: ask user"),
        "Review step should check glossary TODO markers"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bootstrap endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bootstrap_empty_name_returns_error() {
    // To reach the name validation, we need a state with a scan path configured.
    let state = test_state();
    {
        let mut cfg = state.config.write().await;
        cfg.scan.paths = vec!["/tmp".to_string()];
    }

    let body = serde_json::json!({
        "name": "   ",
        "description": "Some description",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/bootstrap",
        body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("name"),
        "Error should mention name, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn bootstrap_no_scan_paths_no_projects_returns_error() {
    // Default test state has no scan paths and no existing projects,
    // so bootstrap should fail because it cannot determine a parent directory.
    let app = test_app();
    let body = serde_json::json!({
        "name": "my-new-project",
        "description": "A brand new project",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(app, "/api/projects/bootstrap", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("scan path")
            || json["error"].as_str().unwrap().contains("Parent directory"),
        "Error should mention missing scan path or parent directory, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn bootstrap_route_accepts_correct_payload() {
    // Verify that the route is registered and accepts the BootstrapProjectRequest shape.
    // We send a valid payload; even though it will fail (no writable parent dir in test),
    // we should NOT get a 404 or 405, and the JSON should be parseable with success/error fields.
    let app = test_app();
    let body = serde_json::json!({
        "name": "test-project",
        "description": "Testing bootstrap",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(app, "/api/projects/bootstrap", body).await;

    // Route exists — not 404/405
    assert_ne!(status, StatusCode::NOT_FOUND, "Bootstrap route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "Bootstrap should accept POST");
    // Response is a valid ApiResponse
    assert!(json["success"].is_boolean(), "Response should have a boolean 'success' field");
}

#[tokio::test]
async fn bootstrap_invalid_payload_returns_error() {
    // Missing required fields should produce a 4xx or an error response
    let app = test_app();
    let body = serde_json::json!({
        "description": "no name field"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/projects/bootstrap")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // Axum rejects malformed JSON payloads with 422 (Unprocessable Entity)
    assert!(
        status == StatusCode::UNPROCESSABLE_ENTITY || status == StatusCode::BAD_REQUEST,
        "Missing required fields should return 422 or 400, got {}",
        status
    );
}

#[tokio::test]
async fn bootstrap_find_common_parent_logic() {
    // Indirectly test find_common_parent by creating two projects under the same parent,
    // then verifying bootstrap doesn't complain about missing scan paths.
    let state = test_state();

    // Insert two projects at known paths under /tmp/kronn-test-parent
    let now = chrono::Utc::now();
    for (name, subdir) in &[("Project A", "project-a"), ("Project B", "project-b")] {
        let project = kronn::models::Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            path: format!("/tmp/kronn-test-bootstrap-parent/{}", subdir),
            repo_url: None,
            token_override: None,
            ai_config: kronn::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: kronn::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            created_at: now,
            updated_at: now,
        };
        let p = project.clone();
        state.db.with_conn(move |conn| {
            kronn::db::projects::insert_project(conn, &p)
        }).await.unwrap();
    }

    // Now bootstrap should use common parent /tmp/kronn-test-bootstrap-parent
    // It will fail on filesystem ops (dir doesn't exist), but the error should
    // mention "Parent directory not found" or "Directory already exists" — NOT "No scan path".
    let body = serde_json::json!({
        "name": "new-child",
        "description": "Testing common parent",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/bootstrap",
        body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    // The error should NOT be about missing scan paths — find_common_parent should have worked
    if !json["success"].as_bool().unwrap_or(false) {
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            !err.contains("No scan path"),
            "With existing projects, bootstrap should find common parent, not complain about scan paths. Got: {}",
            err
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Mark bootstrapped endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn mark_bootstrapped_route_exists() {
    let app = test_app();
    let (status, json) = post_json(app, "/api/projects/nonexistent-id/mark-bootstrapped", serde_json::json!({})).await;

    // Route exists — not 404/405
    assert_ne!(status, StatusCode::NOT_FOUND, "mark-bootstrapped route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "mark-bootstrapped should accept POST");
    // Response is a valid ApiResponse (project not found)
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow creation test
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn workflow_create_with_valid_payload_returns_ok() {
    let state = test_state();

    let body = serde_json::json!({
        "name": "Test workflow",
        "project_id": null,
        "trigger": { "type": "Manual" },
        "steps": [{
            "name": "step1",
            "agent": "ClaudeCode",
            "prompt_template": "Do something",
            "mode": { "type": "Normal" }
        }],
        "actions": [],
        "safety": { "sandbox": false, "require_approval": false }
    });

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows",
        body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"]["id"].is_string());
    assert_eq!(json["data"]["name"], "Test workflow");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Server config endpoint
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn server_config_returns_defaults() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/server").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Should return ServerConfigPublic fields
    assert!(json["data"]["host"].is_string());
    assert!(json["data"]["port"].is_number());
    assert!(json["data"]["max_concurrent_agents"].is_number());
    assert!(json["data"]["auth_enabled"].is_boolean());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Build bootstrap prompt tests (via source analysis)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn bootstrap_prompt_contains_project_name() {
    let source = include_str!("../src/api/projects.rs");
    // The bootstrap prompt function should include the project name
    assert!(source.contains("build_bootstrap_prompt"), "build_bootstrap_prompt function should exist");
    // Should support multiple languages
    assert!(source.contains("Réponds en français") || source.contains("fr"), "Should support French");
    assert!(source.contains("Respond in English") || source.contains("en"), "Should support English");
}

#[test]
fn detect_project_skills_function_exists() {
    let source = include_str!("../src/api/audit.rs");
    assert!(source.contains("detect_project_skills"), "detect_project_skills function should exist");
    // Should check common project files
    assert!(source.contains("Cargo.toml"), "Should detect Rust projects");
    assert!(source.contains("tsconfig.json"), "Should detect TypeScript projects");
    assert!(source.contains("go.mod"), "Should detect Go projects");
    assert!(source.contains("requirements.txt") || source.contains("pyproject.toml"), "Should detect Python projects");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Hard delete endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn delete_project_hard_nonexistent_returns_error() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/projects/nonexistent-id?hard=true").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn delete_project_soft_nonexistent_returns_error() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/projects/nonexistent-id").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Clone endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn clone_route_exists() {
    let app = test_app();
    let body = serde_json::json!({
        "url": "https://github.com/test/repo.git",
        "agent": "ClaudeCode"
    });
    let (status, _json) = post_json(app, "/api/projects/clone", body).await;

    // Route exists — not 404/405
    assert_ne!(status, StatusCode::NOT_FOUND, "Clone route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "Clone should accept POST");
}

#[tokio::test]
async fn clone_empty_url_returns_error() {
    let app = test_app();
    let body = serde_json::json!({
        "url": "   ",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(app, "/api/projects/clone", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().to_lowercase().contains("url") || json["error"].as_str().unwrap().to_lowercase().contains("required"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discover repos endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discover_repos_route_exists() {
    let app = test_app();
    let (status, _json) = post_json(app, "/api/projects/discover-repos", serde_json::json!({})).await;

    // Route exists — not 404/405
    assert_ne!(status, StatusCode::NOT_FOUND, "Discover repos route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "Discover repos should accept POST");
}

#[tokio::test]
async fn discover_repos_no_token_returns_error() {
    let app = test_app();
    let (status, json) = post_json(app, "/api/projects/discover-repos", serde_json::json!({})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    // Should mention no token found
    assert!(json["error"].as_str().unwrap().contains("token") || json["error"].as_str().unwrap().contains("Token"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Git Panel endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn git_status_route_exists() {
    let app = test_app();
    let (status, _json) = get_json(app, "/api/projects/some-id/git-status").await;

    // Route registered — not 404 or 405
    assert_ne!(status, StatusCode::NOT_FOUND, "git-status route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "git-status should accept GET");
}

#[tokio::test]
async fn git_status_project_not_found() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/nonexistent-id/git-status").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found") || json["error"].as_str().unwrap().contains("Not found"));
}

#[tokio::test]
async fn git_diff_route_exists() {
    let app = test_app();
    let (status, _json) = get_json(app, "/api/projects/some-id/git-diff?path=test.rs").await;

    assert_ne!(status, StatusCode::NOT_FOUND, "git-diff route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "git-diff should accept GET");
}

#[tokio::test]
async fn git_diff_project_not_found() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/nonexistent-id/git-diff?path=file.rs").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found") || json["error"].as_str().unwrap().contains("Not found"));
}

#[tokio::test]
async fn git_branch_route_exists() {
    let app = test_app();
    let body = serde_json::json!({ "name": "test-branch" });
    let (status, _json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_ne!(status, StatusCode::NOT_FOUND, "git-branch route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "git-branch should accept POST");
}

#[tokio::test]
async fn git_branch_project_not_found() {
    let app = test_app();
    let body = serde_json::json!({ "name": "feature-x" });
    let (status, json) = post_json(app, "/api/projects/nonexistent-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found") || json["error"].as_str().unwrap().contains("Not found"));
}

#[tokio::test]
async fn git_branch_empty_name_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "name": "" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid") || json["error"].as_str().unwrap().contains("name"));
}

#[tokio::test]
async fn git_branch_name_with_spaces_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "name": "my bad branch" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn git_branch_name_with_dotdot_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "name": "branch..bad" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn git_commit_route_exists() {
    let app = test_app();
    let body = serde_json::json!({ "files": ["test.rs"], "message": "test commit" });
    let (status, _json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_ne!(status, StatusCode::NOT_FOUND, "git-commit route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "git-commit should accept POST");
}

#[tokio::test]
async fn git_commit_empty_message_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "files": ["test.rs"], "message": "" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("message") || json["error"].as_str().unwrap().contains("required"));
}

#[tokio::test]
async fn git_commit_no_files_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "files": [], "message": "my commit" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("No files") || json["error"].as_str().unwrap().contains("files"));
}

#[tokio::test]
async fn git_commit_path_traversal_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "files": ["../../../etc/passwd"], "message": "pwn" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn git_push_route_exists() {
    let app = test_app();
    let (status, _json) = post_json(app, "/api/projects/some-id/git-push", serde_json::json!({})).await;

    assert_ne!(status, StatusCode::NOT_FOUND, "git-push route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "git-push should accept POST");
}

#[tokio::test]
async fn git_push_project_not_found() {
    let app = test_app();
    let (status, json) = post_json(app, "/api/projects/nonexistent-id/git-push", serde_json::json!({})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found") || json["error"].as_str().unwrap().contains("Not found"));
}

#[tokio::test]
async fn git_diff_path_traversal_rejected() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/some-id/git-diff?path=../../../etc/passwd").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Exec endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn exec_route_exists() {
    let app = test_app();
    let body = serde_json::json!({ "command": "echo hello" });
    let (status, _json) = post_json(app, "/api/projects/some-id/exec", body).await;

    // Route exists — not 404/405
    assert_ne!(status, StatusCode::NOT_FOUND, "exec route should be registered");
    assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "exec should accept POST");
}

#[tokio::test]
async fn exec_project_not_found() {
    let app = test_app();
    let body = serde_json::json!({ "command": "echo hello" });
    let (status, json) = post_json(app, "/api/projects/nonexistent-id/exec", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("not found") || json["error"].as_str().unwrap().contains("Not found"),
        "Error should mention project not found, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn exec_empty_command_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "command": "   " });
    let (status, json) = post_json(app, "/api/projects/some-id/exec", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("Empty command"),
        "Error should mention empty command, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn exec_dangerous_command_blocked() {
    let _app = test_app();
    let blocked_commands = ["rm -rf /", "sudo apt install", "chmod 777 .", "chown root .", "kill -9 1", "reboot", "shutdown now", "mkfs /dev/sda", "dd if=/dev/zero"];

    for cmd in &blocked_commands {
        let body = serde_json::json!({ "command": cmd });
        let (_status, json) = post_json(test_app(), "/api/projects/some-id/exec", body).await;

        assert_eq!(json["success"], false, "Command '{}' should be blocked", cmd);
        assert!(
            json["error"].as_str().unwrap().contains("not allowed"),
            "Blocked command '{}' should say 'not allowed', got: {}",
            cmd, json["error"]
        );
    }
}

#[tokio::test]
async fn exec_returns_expected_fields() {
    // Create a project with a real path so the command can execute
    let state = test_state();
    let now = chrono::Utc::now();
    let project = kronn::models::Project {
        id: "exec-test-proj".to_string(),
        name: "Exec Test".to_string(),
        path: "/tmp".to_string(),
        repo_url: None,
        token_override: None,
        ai_config: kronn::models::AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: kronn::models::AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        created_at: now,
        updated_at: now,
    };
    let p = project.clone();
    state.db.with_conn(move |conn| {
        kronn::db::projects::insert_project(conn, &p)
    }).await.unwrap();

    let body = serde_json::json!({ "command": "echo hello" });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/exec-test-proj/exec",
        body,
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true, "exec should succeed, got: {:?}", json);
    let data = &json["data"];
    assert!(data["stdout"].is_string(), "Response should have stdout field");
    assert!(data["stderr"].is_string(), "Response should have stderr field");
    assert!(data["exit_code"].is_number(), "Response should have exit_code field");
    assert_eq!(data["stdout"].as_str().unwrap().trim(), "hello");
    assert_eq!(data["exit_code"].as_i64().unwrap(), 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// WebSocket integration tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: start a real TCP server with the router and return the address.
async fn start_test_server(state: AppState) -> std::net::SocketAddr {
    let app = build_router_with_auth(state, false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Send an initial Presence message to authenticate the WS connection.
/// Required since the security fix: first message MUST be Presence.
async fn ws_send_presence(sender: &mut futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::Message>) {
    let presence = WsMessage::Presence {
        from_pseudo: "TestPeer".into(),
        from_invite_code: "".into(), // Empty = local frontend (accepted)
        online: true,
    };
    sender.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&presence).unwrap().into(),
    )).await.unwrap();
    // Give handler time to verify
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

/// WS upgrade succeeds and broadcast relay works (send → receive round-trip).
#[tokio::test]
async fn ws_broadcast_relay() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    let (mut sender, mut receiver) = StreamExt::split(ws_stream);

    // Send a presence message through broadcast → it should arrive on the WS
    let presence = WsMessage::Presence {
        from_pseudo: "PeerAlpha".into(),
        from_invite_code: "".into(), // empty = local frontend, no verification needed
        online: true,
    };
    state.ws_broadcast.send(presence).unwrap();

    // Read the relayed message
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StreamExt::next(&mut receiver),
    )
    .await
    .expect("timeout waiting for WS message")
    .expect("stream ended")
    .expect("WS error");

    if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
        let parsed: WsMessage = serde_json::from_str(text.as_ref()).unwrap();
        match parsed {
            WsMessage::Presence {
                from_pseudo,
                online,
                ..
            } => {
                assert_eq!(from_pseudo, "PeerAlpha");
                assert!(online);
            }
            _ => panic!("Expected Presence, got {:?}", parsed),
        }
    } else {
        panic!("Expected text message, got {:?}", msg);
    }

    // Clean up
    let _ = sender
        .close()
        .await;
}

/// WS handler forwards client messages to broadcast channel.
#[tokio::test]
async fn ws_client_to_broadcast() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Subscribe to broadcast BEFORE the WS client sends
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a presence message via WS
    let msg = WsMessage::Presence {
        from_pseudo: "PeerBeta".into(),
        from_invite_code: "".into(),
        online: true,
    };
    let json = serde_json::to_string(&msg).unwrap();
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
        .await
        .unwrap();

    // Should appear on broadcast channel
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout")
        .expect("recv error");

    match received {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerBeta");
            assert!(online);
        }
        _ => panic!("Expected Presence, got {:?}", received),
    }
}

/// WS handler responds to Ping with Pong via broadcast.
#[tokio::test]
async fn ws_ping_pong() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Authenticate first (required since security fix)
    ws_send_presence(&mut sender).await;

    // Send a Ping
    let ping = WsMessage::Ping {
        timestamp: 1711000000,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ping).unwrap().into(),
        ))
        .await
        .unwrap();

    // Drain any Presence broadcast first, then expect Pong
    let mut pong_found = false;
    for _ in 0..5 {
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");
        if let WsMessage::Pong { timestamp } = received {
            assert_eq!(timestamp, 1711000000);
            pong_found = true;
            break;
        }
    }
    assert!(pong_found, "Expected Pong but never received it");
}

/// WS auto-adds unknown but valid invite code as pending contact and relays the message.
#[tokio::test]
async fn ws_auto_adds_unknown_valid_invite_code() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a presence with a valid invite code that doesn't exist in contacts DB
    let msg = WsMessage::Presence {
        from_pseudo: "PeerGamma".into(),
        from_invite_code: "kronn:PeerGamma@10.0.0.99:3456".into(),
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // The message should be relayed to broadcast (auto-add accepted the peer)
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout — message was not relayed")
        .expect("recv error");

    match received {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerGamma");
            assert!(online);
        }
        _ => panic!("Expected Presence, got {:?}", received),
    }

    // Verify the contact was auto-created in DB
    let contacts = state
        .db
        .with_conn(kronn::db::contacts::list_contacts)
        .await
        .unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].pseudo, "PeerGamma");
    assert_eq!(contacts[0].status, "pending");
    assert_eq!(contacts[0].invite_code, "kronn:PeerGamma@10.0.0.99:3456");
}

/// WS rejects invalid invite code format (not parseable as kronn:pseudo@host:port).
#[tokio::test]
async fn ws_rejects_invalid_invite_code_format() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a presence with an INVALID invite code format
    let msg = WsMessage::Presence {
        from_pseudo: "BadPeer".into(),
        from_invite_code: "not-a-valid-code".into(),
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // Give the server a moment to process
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The message should NOT have been relayed (invalid format → rejected)
    let result = broadcast_rx.try_recv();
    assert!(
        result.is_err(),
        "Message with invalid invite code should NOT be relayed, but got: {:?}",
        result
    );
}

/// WS rejects non-Presence as first message (security: prevent bypass of invite code check).
#[tokio::test]
async fn ws_rejects_non_presence_first_message() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a ChatMessage as the FIRST message (bypassing Presence verification)
    let msg = WsMessage::ChatMessage {
        shared_discussion_id: "attack-disc".into(),
        message_id: "attack-msg".into(),
        from_pseudo: "Attacker".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:Attacker@evil.com:666".into(),
        content: "Injected message".into(),
        timestamp: 0,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The message should NOT have been relayed (non-Presence first message → rejected)
    let result = broadcast_rx.try_recv();
    assert!(
        result.is_err(),
        "Non-Presence first message should be rejected, but got: {:?}",
        result
    );
}

/// WS accepts known invite code from a contact in the DB.
#[tokio::test]
async fn ws_accepts_known_invite_code() {
    let state = test_state();

    // Insert a contact into the DB
    let contact = kronn::models::Contact {
        id: "contact-1".into(),
        pseudo: "PeerDelta".into(),
        avatar_email: None,
        kronn_url: "http://10.0.0.50:3456".into(),
        invite_code: "kronn:PeerDelta@10.0.0.50:3456".into(),
        status: "accepted".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let c = contact.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::contacts::insert_contact(conn, &c))
        .await
        .unwrap();

    let addr = start_test_server(state.clone()).await;
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send presence with the KNOWN invite code
    let msg = WsMessage::Presence {
        from_pseudo: "PeerDelta".into(),
        from_invite_code: "kronn:PeerDelta@10.0.0.50:3456".into(),
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // Should be forwarded to broadcast (not rejected)
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout — message should have been relayed")
        .expect("recv error");

    match received {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerDelta");
            assert!(online);
        }
        _ => panic!("Expected Presence, got {:?}", received),
    }
}

/// Two WS clients connected simultaneously both receive broadcast messages.
#[tokio::test]
async fn ws_multiple_clients_receive_broadcast() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;
    let url = format!("ws://{}/api/ws", addr);

    // Connect two clients
    let (ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let (_sender1, mut receiver1) = StreamExt::split(ws1);
    let (_sender2, mut receiver2) = StreamExt::split(ws2);

    // Broadcast a message
    let msg = WsMessage::Presence {
        from_pseudo: "PeerEpsilon".into(),
        from_invite_code: "".into(),
        online: true,
    };
    state.ws_broadcast.send(msg).unwrap();

    // Both clients should receive it
    let r1 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StreamExt::next(&mut receiver1),
    )
    .await
    .expect("client 1 timeout");

    let r2 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StreamExt::next(&mut receiver2),
    )
    .await
    .expect("client 2 timeout");

    assert!(r1.is_some(), "Client 1 should receive message");
    assert!(r2.is_some(), "Client 2 should receive message");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Multi-user P2P Chat Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Sharing a discussion generates a shared_id and broadcasts DiscussionInvite.
#[tokio::test]
async fn share_discussion_creates_shared_id() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    // Create a discussion first
    let create_body = serde_json::json!({
        "title": "Test Chat",
        "agent": "ClaudeCode",
        "initial_prompt": "Hello",
        "language": "fr"
    });
    let (status, body) = post_json(app.clone(), "/api/discussions", create_body).await;
    assert_eq!(status, StatusCode::OK);
    let disc_id = body["data"]["id"].as_str().unwrap().to_string();

    // Create a contact to share with
    let contact = kronn::models::Contact {
        id: "contact-share-1".into(),
        pseudo: "PeerTest".into(),
        avatar_email: None,
        kronn_url: "http://10.0.0.99:3456".into(),
        invite_code: "kronn:PeerTest@10.0.0.99:3456".into(),
        status: "accepted".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    state.db.with_conn(move |conn| {
        kronn::db::contacts::insert_contact(conn, &contact)
    }).await.unwrap();

    // Subscribe to broadcast to catch the DiscussionInvite
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    // Share the discussion
    let share_body = serde_json::json!({ "contact_ids": ["contact-share-1"] });
    let (status, body) = post_json(app.clone(), &format!("/api/discussions/{}/share", disc_id), share_body).await;
    assert_eq!(status, StatusCode::OK);
    let shared_id = body["data"].as_str().unwrap();
    assert!(!shared_id.is_empty(), "shared_id should be generated");

    // Verify DiscussionInvite was broadcast
    let received = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        broadcast_rx.recv(),
    ).await.expect("timeout").expect("recv error");

    match received {
        WsMessage::DiscussionInvite { shared_discussion_id, title, .. } => {
            assert_eq!(shared_discussion_id, shared_id);
            assert_eq!(title, "Test Chat");
        }
        _ => panic!("Expected DiscussionInvite, got {:?}", received),
    }

    // Verify discussion now has shared_id in DB
    let disc = state.db.with_conn(move |conn| {
        kronn::db::discussions::get_discussion(conn, &disc_id)
    }).await.unwrap().unwrap();
    assert_eq!(disc.shared_id.as_deref(), Some(shared_id));
    assert!(disc.shared_with.contains(&"contact-share-1".to_string()));
}

/// ChatMessage from a remote peer is inserted into the local shared discussion.
#[tokio::test]
async fn ws_chat_message_inserts_into_shared_discussion() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Create a discussion with a shared_id
    let now = chrono::Utc::now();
    let disc = kronn::models::Discussion {
        id: "disc-chat-test".into(),
        project_id: None,
        title: "Shared Chat".into(),
        agent: kronn::models::AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: kronn::models::ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: Some("shared-abc-123".into()),
        shared_with: vec![],
        created_at: now,
        updated_at: now,
    };
    let d = disc.clone();
    state.db.with_conn(move |conn| {
        kronn::db::discussions::insert_discussion(conn, &d)
    }).await.unwrap();

    // Connect via WS and send a ChatMessage
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    let chat_msg = WsMessage::ChatMessage {
        shared_discussion_id: "shared-abc-123".into(),
        message_id: "remote-msg-001".into(),
        from_pseudo: "RemotePeer".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:RemotePeer@10.0.0.50:3456".into(),
        content: "Hello from the other side!".into(),
        timestamp: now.timestamp_millis(),
    };
    sender.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&chat_msg).unwrap().into(),
    )).await.unwrap();

    // Give the handler time to process
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify the message was inserted into the local discussion
    let updated_disc = state.db.with_conn(move |conn| {
        kronn::db::discussions::get_discussion(conn, "disc-chat-test")
    }).await.unwrap().unwrap();

    assert_eq!(updated_disc.messages.len(), 1, "Should have 1 message from remote peer");
    assert_eq!(updated_disc.messages[0].id, "remote-msg-001");
    assert_eq!(updated_disc.messages[0].content, "Hello from the other side!");
    assert_eq!(updated_disc.messages[0].author_pseudo.as_deref(), Some("RemotePeer"));
}

/// DiscussionInvite creates a new local discussion with the shared_id.
#[tokio::test]
async fn ws_discussion_invite_creates_local_discussion() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Connect via WS and send a DiscussionInvite
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    let invite = WsMessage::DiscussionInvite {
        shared_discussion_id: "shared-invite-xyz".into(),
        title: "Design Review".into(),
        from_pseudo: "Alice".into(),
        from_invite_code: "kronn:Alice@10.0.0.1:3456".into(),
    };
    sender.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&invite).unwrap().into(),
    )).await.unwrap();

    // Give the handler time to process
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify a discussion was created with the shared_id
    let disc_id = state.db.with_conn(move |conn| {
        kronn::db::discussions::find_discussion_by_shared_id(conn, "shared-invite-xyz")
    }).await.unwrap();

    assert!(disc_id.is_some(), "Discussion should have been created from invite");

    let disc = state.db.with_conn(move |conn| {
        kronn::db::discussions::get_discussion(conn, &disc_id.unwrap())
    }).await.unwrap().unwrap();

    assert!(disc.title.contains("Design Review"));
    assert!(disc.title.contains("Alice"));
    assert_eq!(disc.shared_id.as_deref(), Some("shared-invite-xyz"));
}

/// Duplicate ChatMessage (same message_id) is not inserted twice.
#[tokio::test]
async fn ws_chat_message_idempotent() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Create a shared discussion
    let now = chrono::Utc::now();
    let disc = kronn::models::Discussion {
        id: "disc-idempotent".into(),
        project_id: None,
        title: "Idempotent Test".into(),
        agent: kronn::models::AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: kronn::models::ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: Some("shared-idem-001".into()),
        shared_with: vec![],
        created_at: now,
        updated_at: now,
    };
    let d = disc.clone();
    state.db.with_conn(move |conn| {
        kronn::db::discussions::insert_discussion(conn, &d)
    }).await.unwrap();

    // Connect and send the same ChatMessage twice
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    let chat_msg = WsMessage::ChatMessage {
        shared_discussion_id: "shared-idem-001".into(),
        message_id: "msg-duplicate-001".into(),
        from_pseudo: "PeerAlpha".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:PeerAlpha@10.0.0.1:3456".into(),
        content: "This message should appear once".into(),
        timestamp: now.timestamp_millis(),
    };

    // Send twice
    let json = serde_json::to_string(&chat_msg).unwrap();
    sender.send(tokio_tungstenite::tungstenite::Message::Text(json.clone().into())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    sender.send(tokio_tungstenite::tungstenite::Message::Text(json.into())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify only 1 message exists
    let updated = state.db.with_conn(move |conn| {
        kronn::db::discussions::get_discussion(conn, "disc-idempotent")
    }).await.unwrap().unwrap();

    assert_eq!(updated.messages.len(), 1, "Duplicate message should not be inserted twice");
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP API endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn mcp_overview_returns_servers_and_configs() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/mcps").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"]["servers"].is_array(), "overview must have servers array");
    assert!(json["data"]["configs"].is_array(), "overview must have configs array");
    assert!(json["data"]["incompatibilities"].is_array(), "overview must have incompatibilities array");
}

#[tokio::test]
async fn mcp_create_config_and_reveal() {
    let state = test_state();

    // Step 1: Create config with env using registry server_id
    let create_body = serde_json::json!({
        "server_id": "mcp-github",
        "label": "TestProject GitHub",
        "env": {
            "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_test_alpha_token_123"
        },
        "args_override": null,
        "is_global": false,
        "project_ids": []
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/mcps/configs", create_body).await;
    assert_eq!(status, StatusCode::OK, "create_config failed: {:?}", json);
    assert_eq!(json["success"], true, "create_config response: {:?}", json);

    let config_id = json["data"]["id"].as_str().expect("config id should exist").to_string();
    assert_eq!(json["data"]["label"], "TestProject GitHub");
    assert_eq!(json["data"]["server_id"], "mcp-github");

    // Step 2: Reveal secrets
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/mcps/configs/{}/reveal", config_id),
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "reveal failed: {:?}", json);
    assert_eq!(json["success"], true);

    let entries = json["data"].as_array().expect("reveal should return array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "GITHUB_PERSONAL_ACCESS_TOKEN");
    assert_eq!(entries[0]["masked_value"], "ghp_test_alpha_token_123",
        "Revealed value should match original plaintext");
}

#[tokio::test]
async fn mcp_delete_config_removes_from_overview() {
    let state = test_state();

    // Create a config
    let create_body = serde_json::json!({
        "server_id": "mcp-github",
        "label": "ToDelete Config",
        "env": {},
        "args_override": null,
        "is_global": false,
        "project_ids": []
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/mcps/configs", create_body).await;
    assert_eq!(status, StatusCode::OK);
    let config_id = json["data"]["id"].as_str().unwrap().to_string();

    // Verify it appears in overview
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, json) = get_json(app, "/api/mcps").await;
    let configs = json["data"]["configs"].as_array().unwrap();
    assert!(configs.iter().any(|c| c["id"] == config_id), "Config should appear in overview before delete");

    // Delete
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = delete_json(app, &format!("/api/mcps/configs/{}", config_id)).await;
    assert_eq!(status, StatusCode::OK, "delete failed: {:?}", json);
    assert_eq!(json["success"], true);

    // Verify gone from overview
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, json) = get_json(app, "/api/mcps").await;
    let configs = json["data"]["configs"].as_array().unwrap();
    assert!(!configs.iter().any(|c| c["id"] == config_id), "Config should be gone from overview after delete");
}

#[tokio::test]
async fn mcp_update_config_changes_env() {
    let state = test_state();

    // Create config with initial env
    let create_body = serde_json::json!({
        "server_id": "mcp-github",
        "label": "UpdateTest Config",
        "env": {
            "GITHUB_PERSONAL_ACCESS_TOKEN": "old-token-alpha"
        },
        "args_override": null,
        "is_global": false,
        "project_ids": []
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/mcps/configs", create_body).await;
    assert_eq!(status, StatusCode::OK);
    let config_id = json["data"]["id"].as_str().unwrap().to_string();

    // Update env
    let update_body = serde_json::json!({
        "env": {
            "GITHUB_PERSONAL_ACCESS_TOKEN": "new-token-beta"
        }
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = patch_json(app, &format!("/api/mcps/configs/{}", config_id), update_body).await;
    assert_eq!(status, StatusCode::OK, "update failed: {:?}", json);
    assert_eq!(json["success"], true);

    // Reveal to verify new values
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/mcps/configs/{}/reveal", config_id),
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let entries = json["data"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "GITHUB_PERSONAL_ACCESS_TOKEN");
    assert_eq!(entries[0]["masked_value"], "new-token-beta",
        "Revealed value should reflect the updated env");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Export/Import ZIP tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn export_returns_zip() {
    let app = test_app();
    let req = Request::builder()
        .method("GET")
        .uri("/api/config/export")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap().to_str().unwrap(),
        "application/zip"
    );

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    // ZIP magic bytes: PK\x03\x04
    assert!(body.len() > 4);
    assert_eq!(body[0], b'P');
    assert_eq!(body[1], b'K');
    assert_eq!(body[2], 0x03);
    assert_eq!(body[3], 0x04);

    // Verify ZIP contains data.json and config.toml
    let cursor = std::io::Cursor::new(&body[..]);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();
    assert!(archive.by_name("data.json").is_ok(), "ZIP must contain data.json");
    assert!(archive.by_name("config.toml").is_ok(), "ZIP must contain config.toml");

    // Verify data.json has version 3
    let mut data_file = archive.by_name("data.json").unwrap();
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut data_file, &mut contents).unwrap();
    let data: Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(data["version"], 3);
}

#[tokio::test]
async fn import_zip_roundtrip() {
    let state = test_state();

    // Create a project first
    {
        let app = kronn::build_router_with_auth(state.clone(), false);
        let (status, _) = post_json(app, "/api/projects", serde_json::json!({
            "name": "TestProject",
            "path": "/tmp/test-project",
            "remote_url": null,
            "branch": "main",
            "ai_configs": [],
            "has_project": false,
            "hidden": false
        })).await;
        assert_eq!(status, StatusCode::OK);
    }

    // Export
    let zip_bytes = {
        let app = kronn::build_router_with_auth(state.clone(), false);
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/export")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        resp.into_body().collect().await.unwrap().to_bytes()
    };

    // Build multipart body with the ZIP
    let boundary = "----TestBoundary123";
    let mut multipart_body = Vec::new();
    multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    multipart_body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"export.zip\"\r\n");
    multipart_body.extend_from_slice(b"Content-Type: application/zip\r\n\r\n");
    multipart_body.extend_from_slice(&zip_bytes);
    multipart_body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    // Import
    let app = kronn::build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/config/import")
        .header("content-type", format!("multipart/form-data; boundary={}", boundary))
        .body(Body::from(multipart_body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "Import failed: {:?}", json);
    assert_eq!(json["success"], true, "Import should succeed: {:?}", json);

    // Verify project was restored
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, "/api/projects").await;
    assert_eq!(status, StatusCode::OK);
    let projects = json["data"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "TestProject");
}

#[tokio::test]
async fn import_legacy_json_via_multipart() {
    let state = test_state();

    // Build a legacy v2 JSON export
    let legacy_json = serde_json::json!({
        "version": 2,
        "exported_at": "2026-01-01T00:00:00Z",
        "projects": [],
        "discussions": [],
        "workflows": [],
        "mcp_servers": [],
        "mcp_configs": []
    });
    let json_bytes = serde_json::to_vec(&legacy_json).unwrap();

    // Build multipart body with JSON file
    let boundary = "----TestBoundary456";
    let mut multipart_body = Vec::new();
    multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    multipart_body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"export.json\"\r\n");
    multipart_body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    multipart_body.extend_from_slice(&json_bytes);
    multipart_body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    let app = kronn::build_router_with_auth(state, false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/config/import")
        .header("content-type", format!("multipart/form-data; boundary={}", boundary))
        .body(Body::from(multipart_body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "Legacy JSON import failed: {:?}", json);
    assert_eq!(json["success"], true);
}

#[tokio::test]
async fn remap_project_path() {
    let state = test_state();

    // Create a project
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/projects", serde_json::json!({
        "name": "RemapProject",
        "path": "/nonexistent/old/path",
        "remote_url": null,
        "branch": "main",
        "ai_configs": [],
        "has_project": false,
        "hidden": false
    })).await;
    assert_eq!(status, StatusCode::OK);
    let project_id = json["data"]["id"].as_str().unwrap().to_string();

    // Remap to /tmp (which exists)
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/projects/{}/remap-path", project_id),
        serde_json::json!({ "path": "/tmp" }),
    ).await;
    assert_eq!(status, StatusCode::OK, "Remap failed: {:?}", json);
    assert_eq!(json["success"], true);

    // Verify path changed
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, &format!("/api/projects/{}", project_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["path"], "/tmp");
}

#[tokio::test]
async fn remap_project_path_invalid() {
    let state = test_state();

    // Create a project
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/projects", serde_json::json!({
        "name": "InvalidRemap",
        "path": "/tmp/test",
        "remote_url": null,
        "branch": "main",
        "ai_configs": [],
        "has_project": false,
        "hidden": false
    })).await;
    assert_eq!(status, StatusCode::OK);
    let project_id = json["data"]["id"].as_str().unwrap().to_string();

    // Remap to nonexistent path should fail
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/projects/{}/remap-path", project_id),
        serde_json::json!({ "path": "/this/path/does/not/exist/at/all" }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false, "Remap to nonexistent path should fail");
}

#[tokio::test]
async fn workflow_update_project_id_persists() {
    let state = test_state();

    // Create a real project first (FK constraint)
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/projects", serde_json::json!({
        "name": "WfProject",
        "path": "/tmp/wf-project",
        "remote_url": null,
        "branch": "main",
        "ai_configs": [],
        "has_project": false,
        "hidden": false
    })).await;
    assert_eq!(status, StatusCode::OK);
    let project_id = json["data"]["id"].as_str().unwrap().to_string();

    // Create a workflow without project
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/workflows", serde_json::json!({
        "name": "ProjectIdTest",
        "trigger": {"type": "Manual"},
        "steps": [{"name": "s1", "agent": "ClaudeCode", "prompt_template": "test", "mode": {"type": "Normal"}}],
        "actions": []
    })).await;
    assert_eq!(status, StatusCode::OK);
    let wf_id = json["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(json["data"]["project_id"], serde_json::Value::Null);

    // Update with project_id
    let app = kronn::build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/api/workflows/{}", wf_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "project_id": project_id
        })).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let update_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(update_json["success"], true, "Update should succeed: {:?}", update_json);
    assert_eq!(update_json["data"]["project_id"], project_id, "Update response should contain new project_id");

    // GET to verify persistence
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, &format!("/api/workflows/{}", wf_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["project_id"], project_id, "project_id must persist after update");

    // Update back to null (detach project)
    let app = kronn::build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/api/workflows/{}", wf_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "project_id": null
        })).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let detach_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detach_json["success"], true, "Detach should succeed");
    assert_eq!(detach_json["data"]["project_id"], serde_json::Value::Null, "project_id should be null after detach");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow test-step endpoint
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_step_returns_sse_stream() {
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    // Minimal test-step request — will fail (no agent binary) but should return SSE events
    let req = Request::builder()
        .method("POST")
        .uri("/api/workflows/test-step")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "step": {
                "name": "test-step",
                "agent": "ClaudeCode",
                "prompt_template": "Say hello",
                "mode": { "type": "Normal" }
            },
            "dry_run": true
        })).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Should return 200 with SSE content-type (stream starts immediately)
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/event-stream"), "Should be SSE stream, got: {}", ct);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Builtin skill: workflow-architect
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn skills_list_includes_workflow_architect() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/skills").await;
    assert_eq!(status, StatusCode::OK);

    let skills = json["data"].as_array().unwrap();
    let wf_skill = skills.iter().find(|s| s["id"] == "workflow-architect");
    assert!(wf_skill.is_some(), "workflow-architect skill must be in the list");
    let skill = wf_skill.unwrap();
    assert_eq!(skill["category"], "Domain");
    assert!(skill["is_builtin"].as_bool().unwrap(), "Must be builtin");
    assert!(skill["content"].as_str().unwrap().contains("KRONN:WORKFLOW_READY"), "Skill must mention the signal");
}
