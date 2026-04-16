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
    let (ws_tx, _) = tokio::sync::broadcast::channel(256);
    AppState {
        config,
        db,
        agent_semaphore: Arc::new(tokio::sync::Semaphore::new(DEFAULT_MAX_CONCURRENT_AGENTS)),
        audit_tracker: Arc::new(std::sync::Mutex::new(kronn::AuditTracker::default())),
        ws_broadcast: Arc::new(ws_tx),
        cancel_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
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
async fn discussions_stop_returns_cancelled_false_when_nothing_running() {
    // Endpoint smoke test: POST /api/discussions/:id/stop on a disc that has
    // no token in the cancel registry (never started, or already finished)
    // must return success with `cancelled: false` — not a fake success.
    let state = test_state();
    let (status, _) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        serde_json::json!({
            "title": "idle", "agent": "ClaudeCode", "language": "en", "initial_prompt": "x",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    // Any disc id — the endpoint doesn't validate existence, it just looks
    // up the registry (by design: even a just-finished disc won't be in the
    // registry anymore because of the CancelGuard Drop).
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/discussions/nonexistent-disc/stop",
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["cancelled"], false);
}

#[tokio::test]
async fn config_ui_language_round_trip() {
    // The Tauri WebView2 localStorage wipe bug: frontend must be able to
    // recover the user's UI locale from the backend after a wipe. This test
    // proves the backend stores + returns the value.
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    // Default: fr
    let (_, json) = get_json(app.clone(), "/api/config/ui-language").await;
    assert_eq!(json["data"], "fr");

    // Save "en"
    let (status, json) = post_json(app.clone(), "/api/config/ui-language",
        serde_json::json!("en")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // GET returns the new value
    let (_, json) = get_json(app.clone(), "/api/config/ui-language").await;
    assert_eq!(json["data"], "en");

    // Reject invalid value
    let (_, json) = post_json(app.clone(), "/api/config/ui-language",
        serde_json::json!("klingon")).await;
    assert_eq!(json["success"], false);

    // Previous valid value is still there
    let (_, json) = get_json(app, "/api/config/ui-language").await;
    assert_eq!(json["data"], "en");
}

#[tokio::test]
async fn config_global_context_round_trip() {
    let state = test_state();
    let app = build_router_with_auth(state, false);

    // Default: empty
    let (_, json) = get_json(app.clone(), "/api/config/global-context").await;
    assert_eq!(json["data"], "");

    // Save markdown content
    let content = "## Glossary\n- CMS: our custom CMS\n\n## Stack\n- Rust + React";
    let (status, json) = post_json(app.clone(), "/api/config/global-context",
        serde_json::json!(content)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // GET returns saved content
    let (_, json) = get_json(app.clone(), "/api/config/global-context").await;
    assert_eq!(json["data"], content);

    // Empty string clears it
    let (_, json) = post_json(app.clone(), "/api/config/global-context",
        serde_json::json!("   ")).await;
    assert_eq!(json["success"], true);
    let (_, json) = get_json(app, "/api/config/global-context").await;
    assert_eq!(json["data"], "");
}

#[tokio::test]
async fn config_stt_model_round_trip() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    // Default: null (never set)
    let (_, json) = get_json(app.clone(), "/api/config/stt-model").await;
    assert!(json["data"].is_null());

    // Save
    let (_, _) = post_json(app.clone(), "/api/config/stt-model",
        serde_json::json!("onnx-community/whisper-tiny")).await;
    let (_, json) = get_json(app.clone(), "/api/config/stt-model").await;
    assert_eq!(json["data"], "onnx-community/whisper-tiny");

    // Empty string clears it
    let (_, _) = post_json(app.clone(), "/api/config/stt-model",
        serde_json::json!("")).await;
    let (_, json) = get_json(app, "/api/config/stt-model").await;
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn config_tts_voices_per_language() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    let (_, json) = get_json(app.clone(), "/api/config/tts-voices").await;
    assert_eq!(json["data"].as_object().unwrap().len(), 0);

    // Save 2 voices for different languages
    post_json(app.clone(), "/api/config/tts-voice",
        serde_json::json!({"lang": "fr", "voice_id": "voice-fr-alpha"})).await;
    post_json(app.clone(), "/api/config/tts-voice",
        serde_json::json!({"lang": "en", "voice_id": "voice-en-beta"})).await;

    let (_, json) = get_json(app.clone(), "/api/config/tts-voices").await;
    let voices = json["data"].as_object().unwrap();
    assert_eq!(voices["fr"], "voice-fr-alpha");
    assert_eq!(voices["en"], "voice-en-beta");

    // Overwrite fr
    post_json(app.clone(), "/api/config/tts-voice",
        serde_json::json!({"lang": "fr", "voice_id": "voice-fr-new"})).await;
    let (_, json) = get_json(app.clone(), "/api/config/tts-voices").await;
    assert_eq!(json["data"]["fr"], "voice-fr-new");

    // Empty voice_id removes the entry
    post_json(app.clone(), "/api/config/tts-voice",
        serde_json::json!({"lang": "fr", "voice_id": ""})).await;
    let (_, json) = get_json(app, "/api/config/tts-voices").await;
    assert!(json["data"].as_object().unwrap().get("fr").is_none());
    assert_eq!(json["data"]["en"], "voice-en-beta");
}

#[tokio::test]
async fn workflow_test_batch_step_dry_run_preview() {
    // The wizard's "Tester" button on a BatchQuickPrompt step must let the
    // user spot-check what would happen WITHOUT spawning anything. We hit
    // the endpoint with a step + mock previous output, get back the parsed
    // items + sample prompt, no DB writes.
    let state = test_state();

    // Seed a Quick Prompt with one variable.
    let (status, qp_resp) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Analyse ticket",
            "icon": "🎯",
            "prompt_template": "Analyse {{ticket}} en profondeur",
            "variables": [{ "name": "ticket", "label": "Ticket", "placeholder": "EW-1" }],
            "agent": "ClaudeCode",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let qp_id = qp_resp["data"]["id"].as_str().unwrap().to_string();

    // Hit the dry-run endpoint with a mock previous output (the kind of
    // string a fetch step would produce: a JSON array of ticket ids).
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch-tickets",
                "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode",
                "prompt_template": "",
                "mode": { "type": "Normal" },
                "batch_quick_prompt_id": qp_id,
                // {{steps.X.data}} resolves to the data envelope object
                // (JSON-stringified). parse_items unwraps the inner array.
                "batch_items_from": "{{steps.fetch.data}}",
                "batch_max_items": 50
            },
            "mock_previous_output": "{\"data\":{\"tickets\":[\"EW-100\",\"EW-101\",\"EW-102\"]},\"status\":\"OK\",\"summary\":\"3 tickets\"}",
            "previous_step_name": "fetch"
        }),
    ).await;
    assert_eq!(status, StatusCode::OK, "preview endpoint must return 200, got: {:?}", json);
    assert_eq!(json["success"], true);
    let preview = &json["data"];
    assert_eq!(preview["total_items"], 3);
    let sample = preview["sample_items"].as_array().unwrap();
    assert_eq!(sample.len(), 3);
    assert_eq!(sample[0], "EW-100");
    assert_eq!(preview["quick_prompt_name"], "Analyse ticket");
    assert_eq!(preview["quick_prompt_icon"], "🎯");
    assert_eq!(preview["first_variable_name"], "ticket");
    assert_eq!(preview["sample_rendered_prompt"], "Analyse EW-100 en profondeur");
    let prompts = preview["sample_rendered_prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 3, "One rendered prompt per sample item");
    assert_eq!(prompts[0], "Analyse EW-100 en profondeur");
    assert_eq!(prompts[1], "Analyse EW-101 en profondeur");
    assert_eq!(prompts[2], "Analyse EW-102 en profondeur");
    assert_eq!(preview["workspace_mode"], "Direct");
    assert_eq!(preview["wait_for_completion"], true);
    assert!(preview["errors"].as_array().unwrap().is_empty());

    // No DB writes — discussions table should still be empty
    let (_, discs) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
    ).await;
    assert!(discs["data"].as_array().unwrap().is_empty(),
        "Dry-run must not create any discussion");
}

#[tokio::test]
async fn workflow_test_batch_step_freetext_with_data_template_warns_but_continues() {
    // Marie's bug 2026-04-13: she wires `{{steps.main.data}}` but step main
    // is in FreeText mode → at runtime, `.data` is never populated, the
    // template stays literal, batch fails. The dry-run must:
    //   1. Inject a fallback so the user CAN see what their items would be
    //   2. Warn them clearly that this won't work in production
    let state = test_state();
    let (_, qp_resp) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Auto-PR", "icon": "🤯",
            "prompt_template": "Code {{ticketId}}",
            "variables": [{ "name": "ticketId", "label": "id", "placeholder": "x" }],
            "agent": "ClaudeCode",
        }),
    ).await;
    let qp_id = qp_resp["data"]["id"].as_str().unwrap().to_string();

    let (_, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch", "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode", "prompt_template": "",
                "mode": { "type": "Normal" },
                "batch_quick_prompt_id": qp_id,
                "batch_items_from": "{{steps.main.data}}",
            },
            // Mock is plain text (FreeText output from step main) — no envelope
            "mock_previous_output": "EW-2687,EW-3055",
            "previous_step_name": "main"
        }),
    ).await;
    assert_eq!(json["success"], true);

    // Errors must stay empty: the dry-run succeeded thanks to the fallback
    let errors = json["data"]["errors"].as_array().unwrap();
    assert!(errors.is_empty(), "Expected no blocking errors with fallback. Got: {:?}", errors);

    // Items got extracted (2 tickets via comma-split)
    assert_eq!(json["data"]["total_items"], 2);
    let sample = json["data"]["sample_items"].as_array().unwrap();
    assert_eq!(sample[0], "EW-2687");
    assert_eq!(sample[1], "EW-3055");

    // BUT: warnings must explain the production gap
    let warnings = json["data"]["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "Must surface a warning about FreeText + .data");
    let warn_text: String = warnings.iter().map(|w| w.as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(" ");
    assert!(warn_text.contains("Structured") || warn_text.contains(".output"),
        "Warning should suggest Structured mode or .output. Got: {}", warn_text);
    assert!(warn_text.contains("main"), "Warning should name the step. Got: {}", warn_text);
}

#[tokio::test]
async fn workflow_test_batch_step_rejects_unresolved_template() {
    // Regression: user reported running the preview without providing a
    // mock_previous_output → the `{{steps.main.data}}` placeholder stayed
    // literal, parse_items treated it as a single item, and the UI showed
    // "1 item would be launched" with the raw template shown as sample.
    // Worse than useless — it hid the config bug. Now the endpoint detects
    // the unresolved braces and returns an explicit error.
    let state = test_state();
    let (_, qp_resp) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "QP", "prompt_template": "Do {{id}}",
            "variables": [{ "name": "id", "label": "id", "placeholder": "x" }],
            "agent": "ClaudeCode",
        }),
    ).await;
    let qp_id = qp_resp["data"]["id"].as_str().unwrap().to_string();

    let (_, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch", "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode", "prompt_template": "",
                "mode": { "type": "Normal" },
                "batch_quick_prompt_id": qp_id,
                "batch_items_from": "{{steps.main.data}}",
            },
            // NO mock_previous_output — the template can't resolve
        }),
    ).await;
    assert_eq!(json["success"], true);
    let errors = json["data"]["errors"].as_array().unwrap();
    assert!(!errors.is_empty(), "Must surface error on unresolved template");
    let err_text: String = errors.iter().map(|e| e.as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(" ");
    assert!(err_text.contains("non résolue") || err_text.contains("unresolved"),
        "Error should mention unresolved variable. Got: {}", err_text);
    // The preview should NOT report a bogus "1 item" count
    assert_eq!(json["data"]["total_items"], 0);
}

#[tokio::test]
async fn workflow_test_batch_step_surfaces_validation_errors() {
    // Missing items_from + bad QP id → endpoint returns errors[] populated
    // and total_items=0, never crashes.
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch-broken",
                "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode",
                "prompt_template": "",
                "mode": { "type": "Normal" }
                // No batch_quick_prompt_id, no batch_items_from
            }
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let errors = json["data"]["errors"].as_array().unwrap();
    assert!(!errors.is_empty(), "Should report missing required fields");
    assert!(errors.iter().any(|e| e.as_str().unwrap().contains("batch_quick_prompt_id")),
        "Error should mention missing QP id: {:?}", errors);
}

#[tokio::test]
async fn workflow_cancel_run_triggers_token_and_cascades_to_children() {
    // Pre-register a token for a fake run id + 3 fake child disc ids.
    // Then hit the cancel endpoint — all 4 tokens must be triggered.
    let state = test_state();
    let run_token = tokio_util::sync::CancellationToken::new();
    let d1 = tokio_util::sync::CancellationToken::new();
    let d2 = tokio_util::sync::CancellationToken::new();
    let d3 = tokio_util::sync::CancellationToken::new();
    {
        let mut map = state.cancel_registry.lock().unwrap();
        map.insert("run-cancel-me".into(), run_token.clone());
        map.insert("disc-A".into(), d1.clone());
        map.insert("disc-B".into(), d2.clone());
        map.insert("disc-C".into(), d3.clone());
    }

    // Seed the DB: parent linear run, one child batch, 3 child discs linked.
    // The endpoint's cascade query walks discussions.workflow_run_id →
    // workflow_runs where parent_run_id = target.
    state.db.with_conn(|conn| {
        // Minimal workflow row so the FK is satisfied.
        conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
             safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at)
             VALUES ('wf-1', 'W', NULL, '\"Manual\"', '[]', '[]', '{}', NULL, NULL, 0,
             datetime('now'), datetime('now'))",
            [],
        )?;
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
             started_at, run_type, batch_total, batch_completed, batch_failed)
             VALUES ('run-cancel-me', 'wf-1', 'Running', '[]', 0, datetime('now'),
             'linear', 0, 0, 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
             started_at, run_type, batch_total, batch_completed, batch_failed, parent_run_id)
             VALUES ('batch-child', 'wf-1', 'Running', '[]', 0, datetime('now'),
             'batch', 3, 0, 0, 'run-cancel-me')",
            [],
        )?;
        for disc_id in ["disc-A", "disc-B", "disc-C"] {
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language,
                 participants_json, skill_ids_json, profile_ids_json, directive_ids_json,
                 created_at, updated_at, archived, workflow_run_id, message_count,
                 workspace_mode)
                 VALUES (?1, NULL, 'T', 'ClaudeCode', 'en', '[]', '[]', '[]', '[]',
                 datetime('now'), datetime('now'), 0, 'batch-child', 1, 'Direct')",
                rusqlite::params![disc_id],
            )?;
        }
        Ok(())
    }).await.unwrap();

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows/wf-1/runs/run-cancel-me/cancel",
        serde_json::json!({}),
    ).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["run_cancelled"], true);
    assert_eq!(json["data"]["child_discs_cancelled"], 3);

    // Every token we pre-registered is now cancelled
    assert!(run_token.is_cancelled(), "parent run token must fire");
    assert!(d1.is_cancelled() && d2.is_cancelled() && d3.is_cancelled(),
        "all child disc tokens must fire");

    // The child batch run row must be marked Cancelled in DB
    let batch_status: String = state.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT status FROM workflow_runs WHERE id = 'batch-child'",
            [],
            |r| r.get::<_, String>(0),
        )?)
    }).await.unwrap();
    assert_eq!(batch_status, "Cancelled");
}

#[tokio::test]
async fn workflow_cancel_run_idempotent_on_already_finished() {
    // Calling cancel on a run that never registered (or finished already)
    // must not error — returns `{run_cancelled: false, child_discs_cancelled: 0}`.
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/anything/runs/ghost-run/cancel",
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["run_cancelled"], false);
    assert_eq!(json["data"]["child_discs_cancelled"], 0);
}

#[tokio::test]
async fn discussions_stop_triggers_registered_token() {
    // Full path: pre-register a token in the cancel registry, hit the stop
    // endpoint, verify the token was triggered.
    let state = test_state();
    let token = tokio_util::sync::CancellationToken::new();
    {
        let mut map = state.cancel_registry.lock().unwrap();
        map.insert("live-disc".into(), token.clone());
    }
    assert!(!token.is_cancelled());

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions/live-disc/stop",
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["cancelled"], true);
    assert!(token.is_cancelled());

    // Registry was cleaned: a second stop returns cancelled=false
    let (_, json2) = post_json(
        build_router_with_auth(state, false),
        "/api/discussions/live-disc/stop",
        serde_json::json!({}),
    ).await;
    assert_eq!(json2["data"]["cancelled"], false);
}

// ─── Partial response recovery — HTTP layer ─────────────────────────────────
// DB-level behavior is covered in backend/src/db/tests.rs. These tests cover
// the HTTP contract: dismiss-partial endpoint, WS broadcast, and the
// send-message guard that refused a new run while a partial is pending.
// Regression net for the 2026-04-13 "double Agent response" bug.

#[tokio::test]
async fn discussions_dismiss_partial_recovers_and_broadcasts_ws() {
    let state = test_state();
    // Subscribe BEFORE we call the endpoint so we don't miss the broadcast.
    let mut ws_rx = state.ws_broadcast.subscribe();

    // Seed: disc with a dangling partial checkpoint.
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, agent, language,
             participants_json, skill_ids_json, profile_ids_json, directive_ids_json,
             created_at, updated_at, archived, message_count, workspace_mode,
             partial_response, partial_response_started_at)
             VALUES ('disc-dismiss-1', NULL, 'T', 'ClaudeCode', 'fr', '[]', '[]', '[]', '[]',
             datetime('now'), datetime('now'), 0, 0, 'Direct',
             'partial draft v1', datetime('now', '-1 minute'))",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions/disc-dismiss-1/dismiss-partial",
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["recovered"], true,
        "dismiss must return recovered=true when the targeted disc had a partial");

    // The partial was converted to an Agent message and the column cleared.
    let (partial_col, msg_count): (Option<String>, i64) = state.db.with_conn(|conn| {
        let p = conn.query_row(
            "SELECT partial_response FROM discussions WHERE id = 'disc-dismiss-1'",
            [], |r| r.get::<_, Option<String>>(0),
        )?;
        let n = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id = 'disc-dismiss-1'",
            [], |r| r.get::<_, i64>(0),
        )?;
        Ok((p, n))
    }).await.unwrap();
    assert!(partial_col.is_none(), "partial_response must be cleared after dismiss");
    assert_eq!(msg_count, 1, "recovery must produce exactly one Agent message");

    // WS broadcast fired with the recovered id.
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv())
        .await
        .expect("WS broadcast not received within 2s")
        .expect("broadcast recv error");
    match received {
        WsMessage::PartialResponseRecovered { discussion_ids } => {
            assert!(discussion_ids.contains(&"disc-dismiss-1".to_string()),
                "WS must include the dismissed disc id");
        }
        other => panic!("Expected PartialResponseRecovered, got {:?}", other),
    }
}

#[tokio::test]
async fn discussions_dismiss_partial_idempotent_on_clean_disc() {
    // No partial → dismiss is a cheap no-op that returns recovered=false and
    // does NOT broadcast (subscribers are just there for "real" events).
    let state = test_state();
    let mut ws_rx = state.ws_broadcast.subscribe();

    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode)
             VALUES ('disc-clean', 'T', 'ClaudeCode', 'fr', '[]',
             datetime('now'), datetime('now'), 0, 'Direct')",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/discussions/disc-clean/dismiss-partial",
        serde_json::json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["recovered"], false);

    // Drain any non-recovery event the dispatch might emit (e.g. telemetry)
    // and assert that NO PartialResponseRecovered is among them.
    let mut got_recovery = false;
    for _ in 0..5 {
        match tokio::time::timeout(std::time::Duration::from_millis(100), ws_rx.recv()).await {
            Ok(Ok(WsMessage::PartialResponseRecovered { .. })) => { got_recovery = true; break; }
            Ok(Ok(_)) => continue, // some other event — ignore
            _ => break, // timeout or channel closed — done
        }
    }
    assert!(!got_recovery,
        "PartialResponseRecovered must NOT be broadcast when nothing was recovered");
}

#[tokio::test]
async fn discussions_send_message_blocks_while_partial_pending() {
    // Full HTTP guard: POST a new user message while a partial_response
    // checkpoint exists → SSE stream emits a single `error` event with code
    // `partial_pending` and NO agent run is spawned (no new message row).
    let state = test_state();
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode,
             partial_response, partial_response_started_at)
             VALUES ('disc-blocked', 'T', 'ClaudeCode', 'fr', '[]',
             datetime('now'), datetime('now'), 0, 'Direct',
             'half a thought', datetime('now'))",
            [],
        )?;
        Ok(())
    }).await.unwrap();

    let app = build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/discussions/disc-blocked/messages")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "content": "hello again",
            "target_agent": null,
        })).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("event: error"),
        "stream must contain an `error` SSE event, got: {body_str}");
    assert!(body_str.contains("partial_pending"),
        "error payload must tag the case as partial_pending, got: {body_str}");

    // No user message was persisted (guard fired before insert).
    let msg_count: i64 = state.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id = 'disc-blocked'",
            [], |r| r.get::<_, i64>(0),
        )?)
    }).await.unwrap();
    assert_eq!(msg_count, 0,
        "blocked send must not write a user message — otherwise resending after dismiss duplicates it");
}

#[tokio::test]
async fn boot_recovery_simulation_emits_ws_event() {
    // Simulates the main.rs bootstrap sequence: seed partials, run the
    // recovery fn, assert the same WS event is broadcast as the live dismiss
    // path. Guards the exact call chain main.rs:110-124 uses.
    let state = test_state();
    let mut ws_rx = state.ws_broadcast.subscribe();

    state.db.with_conn(|conn| {
        for id in ["disc-boot-1", "disc-boot-2"] {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json,
                 created_at, updated_at, message_count, workspace_mode,
                 partial_response, partial_response_started_at)
                 VALUES (?1, 'T', 'ClaudeCode', 'fr', '[]',
                 datetime('now'), datetime('now'), 0, 'Direct',
                 'in-flight output', datetime('now', '-30 seconds'))",
                rusqlite::params![id],
            )?;
        }
        Ok(())
    }).await.unwrap();

    // This is the exact call main.rs makes.
    let ids = state.db.with_conn(|conn| {
        kronn::db::discussions::recover_partial_responses(conn)
    }).await.unwrap();
    assert_eq!(ids.len(), 2);
    let _ = state.ws_broadcast.send(kronn::models::WsMessage::PartialResponseRecovered {
        discussion_ids: ids.clone(),
    });

    let received = tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv())
        .await
        .expect("WS not received")
        .expect("broadcast error");
    match received {
        WsMessage::PartialResponseRecovered { discussion_ids } => {
            assert_eq!(discussion_ids.len(), 2);
            assert!(discussion_ids.contains(&"disc-boot-1".to_string()));
            assert!(discussion_ids.contains(&"disc-boot-2".to_string()));
        }
        other => panic!("Expected PartialResponseRecovered, got {:?}", other),
    }
}

#[tokio::test]
async fn discussions_list_no_query_params_returns_all_not_limited_to_50() {
    // Regression for the 2026-04-13 bug: `PaginationQuery.page` used to have a
    // serde default of 1, which made `Option<Query<PaginationQuery>>` always
    // succeed even on a bare `GET /api/discussions`, silently capping results
    // at 50. Users with >50 discussions lost access to their older ones as
    // soon as a 50-item batch pushed them past the boundary.
    //
    // This test creates 60 discussions and asserts that a plain list call
    // returns all 60 (not 50).
    let state = test_state();
    for i in 0..60 {
        let (status, _) = post_json(
            build_router_with_auth(state.clone(), false),
            "/api/discussions",
            serde_json::json!({
                "title": format!("Disc {}", i),
                "agent": "ClaudeCode",
                "language": "en",
                "initial_prompt": "test",
            }),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
    ).await;
    assert_eq!(status, StatusCode::OK);
    let discs = json["data"].as_array().expect("data must be an array");
    assert_eq!(
        discs.len(), 60,
        "Bare GET /api/discussions must return ALL discussions, not a paginated slice. \
         Got {} items. Check that PaginationQuery.page has NO serde default.",
        discs.len()
    );
}

#[tokio::test]
async fn discussions_list_explicit_pagination_still_works() {
    // When the caller DOES pass pagination params, the server must honor them.
    // Counterpart to the regression test above.
    let state = test_state();
    for i in 0..10 {
        let (status, _) = post_json(
            build_router_with_auth(state.clone(), false),
            "/api/discussions",
            serde_json::json!({
                "title": format!("Disc {}", i),
                "agent": "ClaudeCode",
                "language": "en",
                "initial_prompt": "test",
            }),
        ).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions?page=1&per_page=3",
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 3);
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
// Add folder (project without git)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn projects_add_folder_creates_project_with_no_repo_url() {
    let state = test_state();
    // Use the actual temp directory (always exists).
    let tmp = std::env::temp_dir();
    let path = tmp.to_str().unwrap().to_string();

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": path }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Name auto-inferred from last path component.
    assert!(!json["data"]["name"].as_str().unwrap().is_empty());
    // No git → repo_url is null.
    assert!(json["data"]["repo_url"].is_null());
    assert!(!json["data"]["id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn projects_add_folder_rejects_nonexistent_path() {
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": "/does/not/exist/xyzzy" }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("does not exist"));
}

#[tokio::test]
async fn projects_add_folder_rejects_path_traversal() {
    let state = test_state();
    let (_, json) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": "/home/../etc/shadow" }),
    ).await;
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains(".."));
}

#[tokio::test]
async fn projects_add_folder_rejects_duplicate_path() {
    let state = test_state();
    let tmp = std::env::temp_dir();
    let path = tmp.to_str().unwrap().to_string();

    // First add succeeds.
    let (_, json1) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": &path }),
    ).await;
    assert_eq!(json1["success"], true);

    // Second add with same path fails.
    let (_, json2) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": &path }),
    ).await;
    assert_eq!(json2["success"], false);
    assert!(json2["error"].as_str().unwrap().contains("already exists"));
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
        assert!(data["quick_prompts"].as_array().unwrap().is_empty());
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

// ═══════════════════════════════════════════════════════════════════════════════
// disc_git / ai_docs / discover — route existence smoke tests (0.3.7)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn disc_git_status_returns_error_for_nonexistent_disc() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/discussions/nonexistent/git-status").await;
    // Route exists, returns an error (not 404 — the handler runs but disc not found)
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

#[tokio::test]
async fn disc_git_diff_route_exists() {
    // git-diff may return non-JSON (raw diff text), so just verify the route exists (not 404)
    let app = test_app();
    let req = Request::builder().method("GET").uri("/api/discussions/nonexistent/git-diff").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::NOT_FOUND, "Route must exist");
}

#[tokio::test]
async fn ai_files_returns_error_for_nonexistent_project() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/nonexistent/ai-files").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

#[tokio::test]
async fn discover_repos_accepts_empty_sources() {
    // discover-repos needs MCP tokens to actually work — just verify the route accepts the payload
    let state = test_state();
    let (status, _json) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/discover-repos",
        serde_json::json!({ "source_ids": [] }),
    ).await;
    assert_eq!(status, StatusCode::OK);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Ollama endpoints (0.4.0)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ollama_health_returns_valid_status() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/ollama/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Status depends on whether Ollama is running on the dev machine
    let health_status = json["data"]["status"].as_str().unwrap();
    assert!(
        ["online", "offline", "not_installed", "unreachable"].contains(&health_status),
        "Unexpected status: '{}'", health_status
    );
    // Endpoint field is always present
    assert!(json["data"]["endpoint"].as_str().unwrap().starts_with("http"));
}

#[tokio::test]
async fn ollama_models_returns_valid_response() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/ollama/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Models array: empty if Ollama offline, populated if online
    assert!(json["data"]["models"].is_array());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Skills / Profiles / Directives / Stats — CRUD smoke tests (0.3.7)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn skills_crud_create_and_delete_custom() {
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/skills",
        serde_json::json!({
            "name": "TestSkill",
            "description": "A test skill",
            "icon": "Zap",
            "category": "Domain",
            "content": "Be excellent.",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let skill_id = json["data"]["id"].as_str().unwrap().to_string();

    // Verify it appears in list
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/skills").await;
    assert!(json["data"].as_array().unwrap().iter().any(|s| s["id"] == skill_id));

    // Delete
    let (status, _) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/skills/{}", skill_id),
    ).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn directives_list_returns_builtins() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/directives").await;
    assert_eq!(status, StatusCode::OK);
    let directives = json["data"].as_array().unwrap();
    assert!(!directives.is_empty(), "Expected at least 1 builtin directive");
}

#[tokio::test]
async fn directives_create_and_delete_custom() {
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/directives",
        serde_json::json!({
            "name": "TestDirective",
            "description": "Be terse",
            "icon": "MessageSquare",
            "category": "Output",
            "content": "Keep answers under 3 sentences.",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let id = json["data"]["id"].as_str().unwrap().to_string();

    let (status, _) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/directives/{}", id),
    ).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn stats_tokens_returns_success() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/tokens").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["total_tokens"], 0);
}

#[tokio::test]
async fn stats_agent_usage_returns_success() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/agent-usage").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
}

#[tokio::test]
async fn agents_detect_returns_list() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let agents = json["data"].as_array().unwrap();
    assert!(agents.len() >= 6, "Expected at least 6 agents, got {}", agents.len());
}

#[tokio::test]
async fn quick_prompts_crud() {
    let state = test_state();
    // List empty
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/quick-prompts").await;
    assert!(json["data"].as_array().unwrap().is_empty());

    // Create
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "TestQP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{"name": "ticket", "label": "Ticket", "placeholder": "PROJ-123", "required": true}],
            "agent": "ClaudeCode",
            "icon": null,
            "project_id": null,
            "skill_ids": [],
            "tier": "default",
            "description": "A test QP",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let qp_id = json["data"]["id"].as_str().unwrap().to_string();

    // List has 1
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/quick-prompts").await;
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Delete
    let (status, _) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/quick-prompts/{}", qp_id),
    ).await;
    assert_eq!(status, StatusCode::OK);

    // List empty again
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/quick-prompts").await;
    assert!(json["data"].as_array().unwrap().is_empty());
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
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: kronn::models::ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: Some("shared-abc-123".into()),
        shared_with: vec![],
        workflow_run_id: None,
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
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: kronn::models::ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: Some("shared-idem-001".into()),
        shared_with: vec![],
        workflow_run_id: None,
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
// Quick Prompts CRUD
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn quick_prompt_crud_api() {
    let state = test_state();

    // Create
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/quick-prompts", serde_json::json!({
        "name": "Test QP",
        "prompt_template": "Analyse {{ticket}}",
        "variables": [{"name": "ticket", "label": "Ticket", "placeholder": "PROJ-123"}]
    })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let qp_id = json["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(json["data"]["name"], "Test QP");
    assert_eq!(json["data"]["variables"].as_array().unwrap().len(), 1);

    // List
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, "/api/quick-prompts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Delete
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, _) = delete_json(app, &format!("/api/quick-prompts/{}", qp_id)).await;
    assert_eq!(status, StatusCode::OK);

    // Verify deleted
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, json) = get_json(app, "/api/quick-prompts").await;
    assert_eq!(json["data"].as_array().unwrap().len(), 0);
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

#[tokio::test]
async fn skills_list_includes_bootstrap_architect() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/skills").await;
    assert_eq!(status, StatusCode::OK);

    let skills = json["data"].as_array().unwrap();
    let skill = skills.iter().find(|s| s["id"] == "bootstrap-architect");
    assert!(skill.is_some(), "bootstrap-architect skill must be in the list");
    let skill = skill.unwrap();
    assert_eq!(skill["category"], "Domain");
    assert!(skill["is_builtin"].as_bool().unwrap());
    // Must mention all 4 gated signals (v4 — canonical *_READY family)
    let content = skill["content"].as_str().unwrap();
    assert!(content.contains("KRONN:REPO_READY"), "Must mention REPO_READY");
    assert!(content.contains("KRONN:ARCHITECTURE_READY"), "Must mention ARCHITECTURE_READY");
    assert!(content.contains("KRONN:PLAN_READY"), "Must mention PLAN_READY");
    assert!(content.contains("KRONN:ISSUES_READY"), "Must mention ISSUES_READY");
    // Hard guardrails: runaway agent prevention
    assert!(content.contains("STOP HERE") || content.contains("STOP IMMEDIATELY"),
        "Must have explicit STOP between stages");
    assert!(content.to_lowercase().contains("no retries") || content.contains("NO RETRIES"),
        "Must have no-retries rule");
    // v3+ enforces stories-as-checklists, not separate issues
    assert!(content.to_lowercase().contains("checklist"),
        "Must instruct stories live as checklists inside epics");
    // v4 guardrails added after disc 8716ae79 debrief:
    // - Stage 0 must call get_me first (not search globally)
    assert!(content.contains("get_me") || content.contains("authenticated user"),
        "Stage 0 must start by identifying the authenticated user");
    // - GitHub Projects v2 limitation must be acknowledged (MCP can't create them)
    assert!(content.contains("Projects v2") || content.contains("project board"),
        "Must mention Projects v2 / project board limitation");
    // - Fallback cascade explicitly forbidden (no plan A/B/C menus)
    assert!(content.contains("NO FALLBACK") || content.to_lowercase().contains("no fallback"),
        "Must forbid fallback cascades (no plan A/B/C menus)");
    // - Tool pivoting forbidden mid-stage (no switching from MCP to gh CLI)
    assert!(content.contains("pivot") || content.contains("switch tool"),
        "Must forbid pivoting between tools mid-stage");
    // - Stage 0 and Stage 3 are execution stages, no multi-profile discussion
    assert!(content.contains("no multi-profile") || content.contains("no-multi-profile") ||
            content.contains("do NOT use the multi-profile"),
        "Must disable multi-profile format for Stage 0 and Stage 3");
    // - Auto-configure git identity from get_me, never ask the user
    //   (regression check for the "Quel nom et email?" prompt bug)
    assert!(content.contains("git config user.name") || content.contains("user.email"),
        "Stage 0 must explicitly set git user.name/email from get_me");
    assert!(content.contains("DO NOT ASK THE USER") || content.contains("MUST NOT ask"),
        "Stage 0 must forbid asking the user for git identity");
    assert!(content.contains("users.noreply.github.com"),
        "Must document the noreply fallback email");
}

#[tokio::test]
async fn bootstrap_request_accepts_skill_ids() {
    // Verify the endpoint accepts skill_ids without deserialization error.
    // The actual bootstrap creates directories so it will fail in test (no scan paths),
    // but the important thing is that the request is parsed correctly.
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    let (status, json) = post_json(app, "/api/projects/bootstrap", serde_json::json!({
        "name": "TestBootstrap",
        "description": "A test project",
        "agent": "ClaudeCode",
        "skill_ids": ["bootstrap-architect"]
    })).await;
    // Will fail because no scan paths configured, but should NOT return 422 (deserialization error)
    assert_eq!(status, StatusCode::OK, "Should not be a deserialization error: {:?}", json);
    // The error should be about scan paths, not about unknown field "skill_ids"
    if json["success"] == false {
        let err = json["error"].as_str().unwrap_or("");
        assert!(!err.contains("skill_ids"), "skill_ids should be accepted by the schema");
    }
}

#[tokio::test]
async fn batch_run_isolated_without_project_id_fails_early() {
    // Safety check: POST /api/quick-prompts/:id/batch with workspace_mode=Isolated
    // must be rejected if neither the QP nor the request has a project_id —
    // otherwise the child discussions would crash at run time when the
    // worktree code tries to locate a non-existent repo.
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    // 1. Create a Quick Prompt WITHOUT project_id
    let (status, json) = post_json(
        app.clone(),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Test QP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{ "name": "ticket", "label": "Ticket", "placeholder": "EW-1" }],
            "agent": "ClaudeCode",
            // project_id intentionally omitted
        }),
    ).await;
    assert_eq!(status, StatusCode::OK, "QP creation failed: {:?}", json);
    assert_eq!(json["success"], true, "QP creation returned error: {:?}", json);
    let qp_id = json["data"]["id"].as_str().expect("QP id missing").to_string();

    // 2. Hit /batch with workspace_mode=Isolated → must fail with a clear message
    let (status, json) = post_json(
        app,
        &format!("/api/quick-prompts/{}/batch", qp_id),
        serde_json::json!({
            "items": [
                { "title": "EW-1", "prompt": "Analyse EW-1" },
                { "title": "EW-2", "prompt": "Analyse EW-2" },
            ],
            "batch_name": "Should-fail batch",
            "workspace_mode": "Isolated",
            // No project_id in request either — qp also has none
        }),
    ).await;
    assert_eq!(status, StatusCode::OK); // HTTP 200 with success=false (Kronn error envelope)
    assert_eq!(json["success"], false, "Should have been rejected: {:?}", json);
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.to_lowercase().contains("isolated") && err.to_lowercase().contains("project"),
        "Error should mention Isolated + project requirement: got {:?}", err
    );
}

#[tokio::test]
async fn batch_run_direct_mode_works_without_project_id() {
    // Regression guard: the Isolated-mode safety check must NOT block Direct
    // mode runs on project-less QPs. This is the legacy path used by the
    // manual batch button for analysis-only Quick Prompts (Jira cadrage, etc.).
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    let (status, json) = post_json(
        app.clone(),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Analysis QP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{ "name": "ticket", "label": "Ticket", "placeholder": "EW-1" }],
            "agent": "ClaudeCode",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let qp_id = json["data"]["id"].as_str().unwrap().to_string();

    // Direct mode on a projectless QP is legit — analysis batches don't need a repo.
    let (status, json) = post_json(
        app,
        &format!("/api/quick-prompts/{}/batch", qp_id),
        serde_json::json!({
            "items": [{ "title": "EW-1", "prompt": "Analyse EW-1" }],
            "batch_name": "Analysis batch",
            // workspace_mode omitted → defaults to Direct on the backend
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true, "Direct mode without project should succeed: {:?}", json);
    assert_eq!(json["data"]["batch_total"], 1);
}

// ─── Audit status endpoint (P1 — resume on nav) ─────────────────────────────

#[tokio::test]
async fn audit_status_returns_null_when_no_audit_is_running() {
    // Baseline: a project with no live audit → data: null, success: true.
    // The UI uses this to detect "nothing to resume" on ProjectCard mount.
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/p-unknown/audit-status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(
        json["data"].is_null(),
        "data must be null when no audit runs; got {:?}", json["data"]
    );
}

#[tokio::test]
async fn audit_status_reflects_advances_and_clears_when_done() {
    // End-to-end check of the tracker → endpoint pipeline. Seed progress,
    // advance through a few steps, then clear — the endpoint must mirror
    // each transition exactly, without the UI having to know about the
    // internal Mutex / HashMap layout.
    let state = test_state();
    let app = kronn::build_router_with_auth(state.clone(), false);

    {
        let mut t = state.audit_tracker.lock().unwrap();
        t.start_progress("proj-x", 10, "full");
        t.advance_step("proj-x", 3, Some("repo-map.md".into()));
    }

    let (status, json) = get_json(app.clone(), "/api/projects/proj-x/audit-status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["project_id"], "proj-x");
    assert_eq!(json["data"]["total_steps"], 10);
    assert_eq!(json["data"]["step_index"], 3);
    assert_eq!(json["data"]["current_file"], "repo-map.md");
    assert_eq!(json["data"]["phase"], "auditing");
    assert_eq!(json["data"]["kind"], "full");

    // Mark validating (phase 3).
    state.audit_tracker.lock().unwrap().mark_validating("proj-x");
    let (_, json) = get_json(app.clone(), "/api/projects/proj-x/audit-status").await;
    assert_eq!(json["data"]["phase"], "validating");

    // Audit finishes → entry cleared → endpoint must report null again.
    state.audit_tracker.lock().unwrap().clear_progress("proj-x");
    let (_, json) = get_json(app, "/api/projects/proj-x/audit-status").await;
    assert!(
        json["data"].is_null(),
        "data must return to null once the audit cleared; got {:?}", json["data"]
    );
}

#[tokio::test]
async fn audit_status_isolates_projects() {
    // Two concurrent audits on different projects must not bleed into each
    // other. Covers the "user has two tabs open on two projects" case.
    let state = test_state();
    let app = kronn::build_router_with_auth(state.clone(), false);

    {
        let mut t = state.audit_tracker.lock().unwrap();
        t.start_progress("proj-a", 10, "full");
        t.advance_step("proj-a", 7, Some("decisions.md".into()));
        t.start_progress("proj-b", 3, "partial");
    }

    let (_, a) = get_json(app.clone(), "/api/projects/proj-a/audit-status").await;
    let (_, b) = get_json(app, "/api/projects/proj-b/audit-status").await;

    assert_eq!(a["data"]["step_index"], 7);
    assert_eq!(a["data"]["kind"], "full");
    assert_eq!(b["data"]["step_index"], 0);
    assert_eq!(b["data"]["kind"], "partial");
}
