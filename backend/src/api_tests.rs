/// Integration tests for the HTTP API layer.
///
/// Each test spins up a real Axum router backed by an in-memory SQLite database,
/// sends HTTP requests via `tower::ServiceExt::oneshot`, and asserts on the JSON
/// responses — exactly the same way Axum's own examples do it.
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tokio::sync::{RwLock, Semaphore};
    use tower::ServiceExt; // for `oneshot`

    use crate::{
        build_router_with_auth,
        core::config::default_config,
        db::Database,
        workflows::WorkflowEngine,
        AppState, AuditTracker, DEFAULT_MAX_CONCURRENT_AGENTS,
    };

    // ─── Helper: build a test AppState with an in-memory DB ──────────────────

    fn test_state() -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let config = default_config();
        let config_arc = Arc::new(RwLock::new(config));
        let workflow_engine = Arc::new(WorkflowEngine::new(db.clone(), config_arc.clone()));
        AppState {
            config: config_arc,
            db,
            workflow_engine,
            agent_semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_AGENTS)),
            audit_tracker: Arc::new(std::sync::Mutex::new(AuditTracker::default())),
        }
    }

    /// Build a test AppState with a specific auth token configured.
    fn test_state_with_token(token: &str) -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let mut config = default_config();
        config.server.auth_token = Some(token.to_string());
        config.server.auth_enabled = true;
        let config_arc = Arc::new(RwLock::new(config));
        let workflow_engine = Arc::new(WorkflowEngine::new(db.clone(), config_arc.clone()));
        AppState {
            config: config_arc,
            db,
            workflow_engine,
            agent_semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_AGENTS)),
            audit_tracker: Arc::new(std::sync::Mutex::new(AuditTracker::default())),
        }
    }

    /// Send a request and collect the response body as parsed JSON.
    async fn send(
        state: AppState,
        enable_auth: bool,
        req: Request<Body>,
    ) -> (StatusCode, Value) {
        let app = build_router_with_auth(state, enable_auth);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body collect").to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    // ─── Q1: Workflow execution integration test ──────────────────────────────

    /// Create a workflow, trigger it, and verify a run is recorded.
    ///
    /// Because no real agent binary is available in tests, the run ends with
    /// `Failed` status (or stays `Pending` if the runner exits immediately).
    /// Either outcome proves the full create→trigger→list-runs path works.
    #[tokio::test]
    async fn workflow_create_trigger_and_list_runs() {
        let state = test_state();

        // 1. Create a workflow via POST /api/workflows
        // WorkflowTrigger and StepMode use #[serde(tag = "type")], so
        // "Manual" → { "type": "Manual" }, "Normal" → { "type": "Normal" }.
        let create_body = serde_json::json!({
            "name": "Test Integration Workflow",
            "trigger": { "type": "Manual" },
            "steps": [
                {
                    "name": "step1",
                    "agent": "ClaudeCode",
                    "prompt_template": "Say hello",
                    "mode": { "type": "Normal" }
                }
            ],
            "actions": [],
            "safety": {
                "sandbox": false,
                "max_files": null,
                "max_lines": null,
                "require_approval": false
            }
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();

        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "create workflow: {body}");
        assert!(body["success"].as_bool().unwrap_or(false), "create workflow ok: {body}");

        let workflow_id = body["data"]["id"].as_str().expect("workflow id").to_string();
        assert!(!workflow_id.is_empty(), "workflow id should not be empty");

        // 2. Trigger the workflow via POST /api/workflows/{id}/trigger
        //    The trigger endpoint returns SSE — we fire-and-forget it,
        //    then check the runs list.  We give the background task a brief moment
        //    to insert the run record before we query.
        let trigger_req = Request::builder()
            .method("POST")
            .uri(format!("/api/workflows/{}/trigger", workflow_id))
            .body(Body::empty())
            .unwrap();

        let app = build_router_with_auth(state.clone(), false);
        let trigger_resp = app.oneshot(trigger_req).await.expect("trigger oneshot");
        // SSE always returns 200, even if execution later fails
        assert_eq!(trigger_resp.status(), StatusCode::OK, "trigger should return 200 (SSE)");

        // Consume the SSE body so the background task completes
        let _ = trigger_resp.into_body().collect().await;

        // Small sleep to let the spawned runner task update the DB
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // 3. List runs via GET /api/workflows/{id}/runs
        let list_req = Request::builder()
            .method("GET")
            .uri(format!("/api/workflows/{}/runs", workflow_id))
            .body(Body::empty())
            .unwrap();

        let (status, runs_body) = send(state.clone(), false, list_req).await;
        assert_eq!(status, StatusCode::OK, "list runs: {runs_body}");
        assert!(runs_body["success"].as_bool().unwrap_or(false), "list runs ok: {runs_body}");

        let runs = runs_body["data"].as_array().expect("runs array");
        assert!(!runs.is_empty(), "at least one run should exist after trigger");

        // 4. The run status must be Pending, Failed, or Success.
        //    (No real agent binary is available in tests; the runner may fast-fail
        //    or complete immediately depending on the environment.)
        let run_status = runs[0]["status"].as_str().expect("run status");
        assert!(
            run_status == "Pending" || run_status == "Failed" || run_status == "Success",
            "expected Pending, Failed, or Success, got: {run_status}"
        );
    }

    // ─── Q2: Auth middleware tests ────────────────────────────────────────────

    /// Health endpoint bypasses auth even when auth is enabled.
    #[tokio::test]
    async fn auth_health_bypasses_auth() {
        let state = test_state_with_token("secret-test-token");

        let req = Request::builder()
            .method("GET")
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(status, StatusCode::OK, "GET /api/health should return 200 even with auth enabled");
    }

    /// A request without an Authorization header returns 401 when auth is enabled.
    #[tokio::test]
    async fn auth_missing_header_returns_401() {
        let state = test_state_with_token("secret-test-token");

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "missing auth header should return 401");
    }

    /// A request with the correct Bearer token returns 200.
    #[tokio::test]
    async fn auth_valid_token_returns_200() {
        let token = "my-valid-token";
        let state = test_state_with_token(token);

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();

        let (status, body) = send(state, true, req).await;
        assert_eq!(status, StatusCode::OK, "valid token should return 200: {body}");
    }

    /// A request with a wrong token returns 401.
    #[tokio::test]
    async fn auth_wrong_token_returns_401() {
        let state = test_state_with_token("correct-token");

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .header("Authorization", "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "wrong token should return 401");
    }

    // ─── Q3: Projects API integration tests ───────────────────────────────────

    #[tokio::test]
    async fn projects_list_empty() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/projects")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn projects_crud_lifecycle() {
        let state = test_state();

        // Create a project directly in DB (projects are created via scan, not POST)
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let project = crate::models::Project {
                id: "test-proj".into(),
                name: "Test Project".into(),
                path: "/tmp/test-project".into(),
                repo_url: Some("https://github.com/test/repo".into()),
                token_override: None,
                ai_config: crate::models::AiConfigStatus { detected: false, configs: vec![] },
                audit_status: crate::models::AiAuditStatus::NoTemplate,
                ai_todo_count: 0,
                default_skill_ids: vec![],
                default_profile_id: None,
                briefing_notes: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::projects::insert_project(conn, &project)?;
            Ok(())
        }).await.unwrap();

        // GET /api/projects — should list it
        let req = Request::builder()
            .method("GET").uri("/api/projects")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        let projects = body["data"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"].as_str().unwrap(), "Test Project");

        // GET /api/projects/:id — should return it
        let req = Request::builder()
            .method("GET").uri("/api/projects/test-proj")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["id"].as_str().unwrap(), "test-proj");

        // DELETE /api/projects/:id
        let req = Request::builder()
            .method("DELETE").uri("/api/projects/test-proj")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());

        // GET /api/projects — should be empty now
        let req = Request::builder()
            .method("GET").uri("/api/projects")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn projects_get_nonexistent_returns_error() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/projects/nonexistent-id")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK); // API returns 200 with success=false
        assert!(!body["success"].as_bool().unwrap_or(true));
    }

    // ─── Q4: Config API integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn config_language_get_default() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/config/language")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // Default language is "fr" (defined in default_config)
        let lang = body["data"].as_str().unwrap();
        assert!(!lang.is_empty(), "Language should have a default value");
    }

    #[tokio::test]
    async fn config_language_set_and_get() {
        let state = test_state();

        // Set language to "en"
        let req = Request::builder()
            .method("POST").uri("/api/config/language")
            .header("Content-Type", "application/json")
            .body(Body::from("\"en\"")).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "set language: {body}");

        // Get language — should be "en"
        let req = Request::builder()
            .method("GET").uri("/api/config/language")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_str().unwrap(), "en");
    }

    // ─── Q5: MCP API integration tests ────────────────────────────────────────

    #[tokio::test]
    async fn mcps_overview_returns_servers_and_configs() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/mcps")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // Overview should have servers and configs arrays
        assert!(body["data"]["servers"].is_array(), "Overview should include servers");
        assert!(body["data"]["configs"].is_array(), "Overview should include configs");
        let servers = body["data"]["servers"].as_array().unwrap();
        assert!(servers.is_empty(), "No servers in DB initially (registry is not auto-imported)");
    }

    #[tokio::test]
    async fn mcps_registry_lists_builtin_servers() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/mcps/registry")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let registry = body["data"].as_array().unwrap();
        assert!(registry.len() >= 30, "Registry should have at least 30 entries, got {}", registry.len());
    }

    // ─── Q6: Setup API integration tests ──────────────────────────────────────

    #[tokio::test]
    async fn setup_status_returns_valid_response() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/setup/status")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert!(body["data"]["agents_detected"].is_array(), "Setup status should include agents_detected");
    }

    // ─── Q7: Stats API ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn stats_token_usage_returns_ok() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/stats/tokens")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    // ─── Q8: Discussions API integration tests ────────────────────────────────

    #[tokio::test]
    async fn discussions_list_empty() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/discussions")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn discussions_create_and_get() {
        let state = test_state();

        // Create discussion (will fail to run agent but should persist in DB)
        let create_body = serde_json::json!({
            "title": "Test Discussion",
            "agent": "ClaudeCode",
            "language": "fr",
            "initial_prompt": "Hello, test prompt"
        });

        let req = Request::builder()
            .method("POST").uri("/api/discussions")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        // Create returns SSE stream, status should be 200
        assert_eq!(status, StatusCode::OK, "create discussion: {body}");

        // Wait for background task to persist
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // List discussions — should have 1
        let req = Request::builder()
            .method("GET").uri("/api/discussions")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        let discussions = body["data"].as_array().unwrap();
        assert_eq!(discussions.len(), 1, "Should have 1 discussion after create");
        let disc_id = discussions[0]["id"].as_str().unwrap().to_string();
        assert_eq!(discussions[0]["title"].as_str().unwrap(), "Test Discussion");
        assert_eq!(discussions[0]["language"].as_str().unwrap(), "fr");

        // Get by ID
        let req = Request::builder()
            .method("GET").uri(format!("/api/discussions/{}", disc_id))
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["id"].as_str().unwrap(), disc_id);
        // Should have at least the initial user message
        let messages = body["data"]["messages"].as_array().unwrap();
        assert!(!messages.is_empty(), "Discussion should have at least 1 message");
        assert_eq!(messages[0]["role"].as_str().unwrap(), "User");
    }

    #[tokio::test]
    async fn discussions_update_title_and_archive() {
        let state = test_state();

        // Create via DB directly (faster, no SSE)
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let disc = crate::models::Discussion {
                id: "disc-1".into(),
                project_id: None,
                title: "Original Title".into(),
                agent: crate::models::AgentType::ClaudeCode,
                language: "en".into(),
                participants: vec![crate::models::AgentType::ClaudeCode],
                message_count: 0,
                messages: vec![],
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                archived: false,
                workspace_mode: "Direct".into(),
                workspace_path: None,
                worktree_branch: None,
                tier: crate::models::ModelTier::Default,
                pin_first_message: false,
                summary_cache: None,
                summary_up_to_msg_idx: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::discussions::insert_discussion(conn, &disc)?;
            Ok(())
        }).await.unwrap();

        // PATCH — update title
        let update_body = serde_json::json!({ "title": "Updated Title" });
        let req = Request::builder()
            .method("PATCH").uri("/api/discussions/disc-1")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "update discussion: {body}");
        assert!(body["success"].as_bool().unwrap());

        // Verify title changed
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-1")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["title"].as_str().unwrap(), "Updated Title");

        // PATCH — archive
        let archive_body = serde_json::json!({ "archived": true });
        let req = Request::builder()
            .method("PATCH").uri("/api/discussions/disc-1")
            .header("Content-Type", "application/json")
            .body(Body::from(archive_body.to_string())).unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        // Verify archived
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-1")
            .body(Body::empty()).unwrap();
        let (_, body) = send(state, false, req).await;
        assert!(body["data"]["archived"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn discussions_create_with_profile_and_directive_ids() {
        let state = test_state();

        // Create discussion with profile_ids and directive_ids
        let create_body = serde_json::json!({
            "title": "Discussion with extras",
            "agent": "ClaudeCode",
            "language": "en",
            "initial_prompt": "Hello with profiles",
            "profile_ids": ["profile-dev", "profile-reviewer"],
            "directive_ids": ["directive-eco", "directive-security"]
        });

        let req = Request::builder()
            .method("POST").uri("/api/discussions")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string())).unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "create discussion with profiles/directives");

        // Wait for background persistence
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // List and verify stored profile_ids / directive_ids
        let req = Request::builder()
            .method("GET").uri("/api/discussions")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let discussions = body["data"].as_array().unwrap();
        assert_eq!(discussions.len(), 1);
        let disc = &discussions[0];
        let profile_ids = disc["profile_ids"].as_array().unwrap();
        let directive_ids = disc["directive_ids"].as_array().unwrap();
        assert_eq!(profile_ids.len(), 2, "Should store 2 profile_ids");
        assert_eq!(directive_ids.len(), 2, "Should store 2 directive_ids");
        assert!(profile_ids.iter().any(|v| v.as_str() == Some("profile-dev")));
        assert!(directive_ids.iter().any(|v| v.as_str() == Some("directive-eco")));
    }

    #[tokio::test]
    async fn discussions_patch_title() {
        let state = test_state();
        insert_test_discussion(&state, "disc-patch-title", "Old Title").await;

        let update_body = serde_json::json!({ "title": "New Title" });
        let req = Request::builder()
            .method("PATCH").uri("/api/discussions/disc-patch-title")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "PATCH title: {body}");
        assert!(body["success"].as_bool().unwrap());

        // Verify title changed
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-patch-title")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["title"].as_str().unwrap(), "New Title");
    }

    #[tokio::test]
    async fn discussions_delete() {
        let state = test_state();

        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let disc = crate::models::Discussion {
                id: "disc-del".into(),
                project_id: None,
                title: "To Delete".into(),
                agent: crate::models::AgentType::Vibe,
                language: "fr".into(),
                participants: vec![],
                message_count: 0, messages: vec![],
                skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
                archived: false, workspace_mode: "Direct".into(),
                workspace_path: None, worktree_branch: None,
                tier: crate::models::ModelTier::Default,
                pin_first_message: false,
                summary_cache: None, summary_up_to_msg_idx: None,
                created_at: now, updated_at: now,
            };
            crate::db::discussions::insert_discussion(conn, &disc)?;
            Ok(())
        }).await.unwrap();

        // DELETE
        let req = Request::builder()
            .method("DELETE").uri("/api/discussions/disc-del")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());

        // Verify gone
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-del")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body["success"].as_bool().unwrap_or(true), "Deleted discussion should return error");
    }

    #[tokio::test]
    async fn discussions_create_validates_title_length() {
        let state = test_state();

        let long_title = "x".repeat(501);
        let create_body = serde_json::json!({
            "title": long_title,
            "agent": "ClaudeCode",
            "initial_prompt": "test"
        });

        let req = Request::builder()
            .method("POST").uri("/api/discussions")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string())).unwrap();
        let (status, body) = send(state, false, req).await;
        // Should reject with validation error
        assert_eq!(status, StatusCode::OK);
        assert!(!body["success"].as_bool().unwrap_or(true),
            "Title >500 chars should be rejected: {body}");
    }

    // ─── Q9: Agents API integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn agents_detect_returns_list() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/agents")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let agents = body["data"].as_array().unwrap();
        // Should detect at least some agents (even if not installed)
        assert!(!agents.is_empty(), "Agent detection should return at least one entry");
        // Each agent should have required fields
        for agent in agents {
            assert!(agent["name"].is_string(), "Agent should have name");
            assert!(agent["agent_type"].is_string(), "Agent should have agent_type");
        }
    }

    #[tokio::test]
    async fn agents_toggle_changes_state() {
        let state = test_state();

        // Toggle Vibe off
        let req = Request::builder()
            .method("POST").uri("/api/agents/toggle")
            .header("Content-Type", "application/json")
            .body(Body::from("\"Vibe\"")).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "toggle agent: {body}");
        assert!(body["success"].as_bool().unwrap());
        let enabled = body["data"].as_bool().unwrap();

        // Toggle again — should flip
        let req = Request::builder()
            .method("POST").uri("/api/agents/toggle")
            .header("Content-Type", "application/json")
            .body(Body::from("\"Vibe\"")).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let new_enabled = body["data"].as_bool().unwrap();
        assert_ne!(enabled, new_enabled, "Toggle should flip the enabled state");
    }

    // ─── Q10: Skills API integration tests ────────────────────────────────────

    #[tokio::test]
    async fn skills_list_returns_builtins() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/skills")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let skills = body["data"].as_array().unwrap();
        assert!(!skills.is_empty(), "Should have built-in skills");
        // Verify structure
        let first = &skills[0];
        assert!(first["id"].is_string());
        assert!(first["name"].is_string());
    }

    // ─── Q11: Profiles API integration tests ──────────────────────────────────

    #[tokio::test]
    async fn profiles_list_returns_builtins() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/profiles")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let profiles = body["data"].as_array().unwrap();
        assert!(!profiles.is_empty(), "Should have built-in profiles");
    }

    // ─── Q12: Directives API integration tests ───────────────────────────────

    #[tokio::test]
    async fn directives_list_returns_builtins() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/directives")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let directives = body["data"].as_array().unwrap();
        assert!(!directives.is_empty(), "Should have built-in directives");
    }

    // ─── Q13: Config API additional tests ─────────────────────────────────────

    #[tokio::test]
    async fn config_server_get_and_set() {
        let state = test_state();

        // GET current server config
        let req = Request::builder()
            .method("GET").uri("/api/config/server")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());

        // POST new server config — enable auth
        let new_config = serde_json::json!({
            "auth_enabled": true,
            "auth_token": null,
            "max_concurrent_agents": 3
        });
        let req = Request::builder()
            .method("POST").uri("/api/config/server")
            .header("Content-Type", "application/json")
            .body(Body::from(new_config.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "set server config: {body}");
    }

    #[tokio::test]
    async fn config_scan_paths_get() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/config/scan-paths")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn config_tokens_get() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/config/tokens")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn config_db_info() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/config/db-info")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    // ─── Q14: Discussion message operations ───────────────────────────────────

    /// Helper: insert a discussion directly in DB for fast test setup
    async fn insert_test_discussion(state: &AppState, id: &str, title: &str) {
        state.db.with_conn({
            let id = id.to_string();
            let title = title.to_string();
            move |conn| {
                let now = chrono::Utc::now();
                let disc = crate::models::Discussion {
                    id: id.clone(),
                    project_id: None,
                    title,
                    agent: crate::models::AgentType::ClaudeCode,
                    language: "en".into(),
                    participants: vec![crate::models::AgentType::ClaudeCode],
                    message_count: 0, messages: vec![],
                    skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
                    archived: false, workspace_mode: "Direct".into(),
                    workspace_path: None, worktree_branch: None,
                    tier: crate::models::ModelTier::Default,
                    pin_first_message: false,
                    summary_cache: None, summary_up_to_msg_idx: None,
                    created_at: now, updated_at: now,
                };
                crate::db::discussions::insert_discussion(conn, &disc)?;
                Ok(())
            }
        }).await.unwrap();
    }

    /// Helper: insert a message directly in DB
    async fn insert_test_message(state: &AppState, disc_id: &str, role: &str, content: &str) {
        state.db.with_conn({
            let disc_id = disc_id.to_string();
            let role = role.to_string();
            let content = content.to_string();
            move |conn| {
                let msg = crate::models::DiscussionMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: match role.as_str() {
                        "User" => crate::models::MessageRole::User,
                        "Agent" => crate::models::MessageRole::Agent,
                        _ => crate::models::MessageRole::System,
                    },
                    content,
                    agent_type: if role == "Agent" { Some(crate::models::AgentType::ClaudeCode) } else { None },
                    timestamp: chrono::Utc::now(),
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: None,
                };
                crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
                Ok(())
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn discussions_delete_last_agent_messages() {
        let state = test_state();
        insert_test_discussion(&state, "disc-msg", "Message Test").await;
        insert_test_message(&state, "disc-msg", "User", "Hello").await;
        insert_test_message(&state, "disc-msg", "Agent", "Agent reply").await;
        insert_test_message(&state, "disc-msg", "Agent", "Agent follow up").await;

        // DELETE last agent messages
        let req = Request::builder()
            .method("DELETE").uri("/api/discussions/disc-msg/messages/last")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "delete last agent messages: {body}");
        assert!(body["success"].as_bool().unwrap());

        // Verify: discussion should only have the user message now
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-msg")
            .body(Body::empty()).unwrap();
        let (_, body) = send(state, false, req).await;
        let messages = body["data"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "Only user message should remain");
        assert_eq!(messages[0]["role"].as_str().unwrap(), "User");
    }

    #[tokio::test]
    async fn discussions_edit_last_user_message() {
        let state = test_state();
        insert_test_discussion(&state, "disc-edit", "Edit Test").await;
        insert_test_message(&state, "disc-edit", "User", "Original message").await;
        insert_test_message(&state, "disc-edit", "Agent", "Agent reply").await;

        // PATCH last user message
        let edit_body = serde_json::json!({ "content": "Edited message" });
        let req = Request::builder()
            .method("PATCH").uri("/api/discussions/disc-edit/messages/last")
            .header("Content-Type", "application/json")
            .body(Body::from(edit_body.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "edit last user message: {body}");

        // Verify: user message content updated, agent messages removed
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-edit")
            .body(Body::empty()).unwrap();
        let (_, body) = send(state, false, req).await;
        let messages = body["data"]["messages"].as_array().unwrap();
        // After edit, agent messages should be deleted and user message updated
        let user_msgs: Vec<_> = messages.iter()
            .filter(|m| m["role"].as_str() == Some("User"))
            .collect();
        assert!(!user_msgs.is_empty(), "Should have at least one user message");
        assert_eq!(user_msgs.last().unwrap()["content"].as_str().unwrap(), "Edited message");
    }

    #[tokio::test]
    async fn discussions_update_skill_ids() {
        let state = test_state();
        insert_test_discussion(&state, "disc-skills", "Skills Test").await;

        let update_body = serde_json::json!({
            "skill_ids": ["skill-rust", "skill-testing"]
        });
        let req = Request::builder()
            .method("PATCH").uri("/api/discussions/disc-skills")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string())).unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        // Verify
        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-skills")
            .body(Body::empty()).unwrap();
        let (_, body) = send(state, false, req).await;
        let skill_ids = body["data"]["skill_ids"].as_array().unwrap();
        assert_eq!(skill_ids.len(), 2);
    }

    #[tokio::test]
    async fn discussions_update_tier() {
        let state = test_state();
        insert_test_discussion(&state, "disc-tier", "Tier Test").await;

        let update_body = serde_json::json!({ "tier": "economy" });
        let req = Request::builder()
            .method("PATCH").uri("/api/discussions/disc-tier")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string())).unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        let req = Request::builder()
            .method("GET").uri("/api/discussions/disc-tier")
            .body(Body::empty()).unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["data"]["tier"].as_str().unwrap(), "economy");
    }

    // ─── Q15: Workflow CRUD API tests ─────────────────────────────────────────

    #[tokio::test]
    async fn workflows_list_empty() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/workflows")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn workflows_create_and_get() {
        let state = test_state();

        let create_body = serde_json::json!({
            "name": "Nightly Audit",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "audit",
                "agent": "ClaudeCode",
                "prompt_template": "Run audit on project",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "safety": {
                "sandbox": false,
                "max_files": null,
                "max_lines": null,
                "require_approval": false
            }
        });

        let req = Request::builder()
            .method("POST").uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "create workflow: {body}");
        let wf_id = body["data"]["id"].as_str().unwrap().to_string();

        // GET by ID
        let req = Request::builder()
            .method("GET").uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["name"].as_str().unwrap(), "Nightly Audit");
        assert_eq!(body["data"]["steps"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn workflows_update_and_delete() {
        let state = test_state();

        // Create
        let create_body = serde_json::json!({
            "name": "To Update",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "s1",
                "agent": "Vibe",
                "prompt_template": "test",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "safety": { "sandbox": false, "max_files": null, "max_lines": null, "require_approval": false }
        });

        let req = Request::builder()
            .method("POST").uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string())).unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        let wf_id = body["data"]["id"].as_str().unwrap().to_string();

        // Update
        let update_body = serde_json::json!({
            "name": "Updated Name",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "s1-updated",
                "agent": "ClaudeCode",
                "prompt_template": "updated prompt",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "safety": { "sandbox": false, "max_files": null, "max_lines": null, "require_approval": false }
        });

        let req = Request::builder()
            .method("PUT").uri(format!("/api/workflows/{}", wf_id))
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string())).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "update workflow: {body}");

        // Verify updated
        let req = Request::builder()
            .method("GET").uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty()).unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(body["data"]["name"].as_str().unwrap(), "Updated Name");

        // Delete
        let req = Request::builder()
            .method("DELETE").uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty()).unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        // Verify gone
        let req = Request::builder()
            .method("GET").uri("/api/workflows")
            .body(Body::empty()).unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    // ─── Q15b: Config model-tiers API ─────────────────────────────────────────

    #[tokio::test]
    async fn config_model_tiers_returns_config() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/config/model-tiers")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // model-tiers should return an object with tier configuration
        assert!(body["data"].is_object(), "model-tiers should return a config object");
    }

    // ─── Q16: Export/Import API ───────────────────────────────────────────────

    #[tokio::test]
    async fn config_export_returns_data() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/config/export")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // Export should contain at least the data key
        assert!(body["data"].is_object(), "Export should return an object");
    }

    // ─── Q17: Agent usage stats ───────────────────────────────────────────────

    #[tokio::test]
    async fn stats_agent_usage_returns_ok() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/stats/agent-usage")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    // ─── MCP overview includes incompatibilities ──────────────────────────────

    #[tokio::test]
    async fn mcp_overview_includes_incompatibilities_field() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/mcps")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // incompatibilities field should exist (may be empty if no gitlab server in test DB)
        assert!(body["data"]["incompatibilities"].is_array(),
            "McpOverview must include incompatibilities array");
    }

    // ─── Error hint detection ────────────────────────────────────────────────

    #[tokio::test]
    async fn detect_error_hint_mcp_config() {
        use crate::api::discussions::detect_agent_error_hint;
        let hint = detect_agent_error_hint(
            "Error: Invalid MCP configuration: MCP config file not found: /host-home/Repositories/test/"
        );
        assert!(hint.is_some(), "Should detect MCP config error");
        assert!(hint.unwrap().contains("MCP"), "Hint should mention MCP");
    }

    #[tokio::test]
    async fn detect_error_hint_auth() {
        use crate::api::discussions::detect_agent_error_hint;
        let hint = detect_agent_error_hint("authentication_error: invalid API key");
        assert!(hint.is_some(), "Should detect auth error");
    }

    #[tokio::test]
    async fn detect_error_hint_no_match() {
        use crate::api::discussions::detect_agent_error_hint;
        let hint = detect_agent_error_hint("Everything is fine, no errors here");
        assert!(hint.is_none(), "Should not detect error in normal output");
    }

    // ─── Drift detection API tests ──────────────────────────────────────────

    #[tokio::test]
    async fn drift_check_no_project() {
        let state = test_state();
        let req = Request::builder()
            .method("GET").uri("/api/projects/nonexistent/drift")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK); // API returns 200 with success=false
        assert!(!body["success"].as_bool().unwrap_or(true),
            "Drift check on nonexistent project should return error: {body}");
    }

    #[tokio::test]
    async fn drift_check_route_exists() {
        let state = test_state();

        // Insert a project with a real path so check_drift can run
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let project = crate::models::Project {
                id: "drift-proj".into(),
                name: "Drift Test Project".into(),
                path: "/tmp/kronn-drift-test".into(),
                repo_url: None,
                token_override: None,
                ai_config: crate::models::AiConfigStatus { detected: false, configs: vec![] },
                audit_status: crate::models::AiAuditStatus::NoTemplate,
                ai_todo_count: 0,
                default_skill_ids: vec![],
                default_profile_id: None,
                briefing_notes: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::projects::insert_project(conn, &project)?;
            Ok(())
        }).await.unwrap();

        // Ensure the path exists (even if empty)
        std::fs::create_dir_all("/tmp/kronn-drift-test").ok();

        let req = Request::builder()
            .method("GET").uri("/api/projects/drift-proj/drift")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK, "drift check route should return 200: {body}");
        assert!(body["success"].as_bool().unwrap_or(false),
            "drift check should succeed (empty drift): {body}");
    }

    #[tokio::test]
    async fn partial_audit_invalid_steps() {
        let state = test_state();

        // Insert a project
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let project = crate::models::Project {
                id: "partial-proj".into(),
                name: "Partial Audit Test".into(),
                path: "/tmp/kronn-partial-test".into(),
                repo_url: None,
                token_override: None,
                ai_config: crate::models::AiConfigStatus { detected: false, configs: vec![] },
                audit_status: crate::models::AiAuditStatus::NoTemplate,
                ai_todo_count: 0,
                default_skill_ids: vec![],
                default_profile_id: None,
                briefing_notes: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::projects::insert_project(conn, &project)?;
            Ok(())
        }).await.unwrap();

        std::fs::create_dir_all("/tmp/kronn-partial-test").ok();

        // POST with invalid step number (99)
        let body_json = serde_json::json!({
            "agent": "ClaudeCode",
            "steps": [99]
        });
        let req = Request::builder()
            .method("POST").uri("/api/projects/partial-proj/partial-audit")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string())).unwrap();

        let app = build_router_with_auth(state, false);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        assert_eq!(resp.status(), StatusCode::OK, "SSE endpoint returns 200");

        // Consume SSE body and check for error event about invalid step
        let bytes = resp.into_body().collect().await.expect("body collect").to_bytes();
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(body_str.contains("Invalid step"),
            "Should contain error about invalid step: {body_str}");
    }

    #[tokio::test]
    async fn partial_audit_route_exists() {
        let state = test_state();

        // Insert a project
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let project = crate::models::Project {
                id: "partial-ok-proj".into(),
                name: "Partial OK Test".into(),
                path: "/tmp/kronn-partial-ok-test".into(),
                repo_url: None,
                token_override: None,
                ai_config: crate::models::AiConfigStatus { detected: false, configs: vec![] },
                audit_status: crate::models::AiAuditStatus::NoTemplate,
                ai_todo_count: 0,
                default_skill_ids: vec![],
                default_profile_id: None,
                briefing_notes: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::projects::insert_project(conn, &project)?;
            Ok(())
        }).await.unwrap();

        std::fs::create_dir_all("/tmp/kronn-partial-ok-test").ok();

        // POST with valid step number (1)
        let body_json = serde_json::json!({
            "agent": "ClaudeCode",
            "steps": [1]
        });
        let req = Request::builder()
            .method("POST").uri("/api/projects/partial-ok-proj/partial-audit")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string())).unwrap();

        let app = build_router_with_auth(state, false);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        // SSE always returns 200
        assert_eq!(resp.status(), StatusCode::OK, "partial-audit route should return 200 (SSE)");
    }

    #[tokio::test]
    async fn briefing_get_set() {
        let state = test_state();

        // Create a project
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let project = crate::models::Project {
                id: "briefing-proj".into(),
                name: "Briefing Test".into(),
                path: "/tmp/briefing-test".into(),
                repo_url: None,
                token_override: None,
                ai_config: crate::models::AiConfigStatus { detected: false, configs: vec![] },
                audit_status: crate::models::AiAuditStatus::NoTemplate,
                ai_todo_count: 0,
                default_skill_ids: vec![],
                default_profile_id: None,
                briefing_notes: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::projects::insert_project(conn, &project)?;
            Ok(())
        }).await.unwrap();

        // GET briefing — should be null initially
        let req = Request::builder()
            .method("GET").uri("/api/projects/briefing-proj/briefing")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or(false));
        assert!(body["data"].is_null(), "Briefing should be null initially: {body}");

        // PUT briefing — set notes
        let req = Request::builder()
            .method("PUT").uri("/api/projects/briefing-proj/briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"notes":"This is a Node.js monorepo with React frontend"}"#)).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or(false), "Set briefing should succeed: {body}");

        // GET briefing — should return the notes
        let req = Request::builder()
            .method("GET").uri("/api/projects/briefing-proj/briefing")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_str().unwrap(), "This is a Node.js monorepo with React frontend");

        // PUT briefing — clear notes
        let req = Request::builder()
            .method("PUT").uri("/api/projects/briefing-proj/briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"notes":null}"#)).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or(false));

        // GET briefing — should be null again
        let req = Request::builder()
            .method("GET").uri("/api/projects/briefing-proj/briefing")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"].is_null(), "Briefing should be null after clearing: {body}");
    }

    #[tokio::test]
    async fn briefing_nonexistent_project() {
        let state = test_state();

        // GET briefing for nonexistent project — should return null (no project row found)
        let req = Request::builder()
            .method("GET").uri("/api/projects/nonexistent/briefing")
            .body(Body::empty()).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"].is_null(), "Briefing for nonexistent project should be null");

        // PUT briefing for nonexistent project — should fail
        let req = Request::builder()
            .method("PUT").uri("/api/projects/nonexistent/briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"notes":"test"}"#)).unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body["success"].as_bool().unwrap_or(true), "Set briefing on nonexistent project should fail: {body}");
    }

    // ─── Start briefing tests ────────────────────────────────────────────

    #[tokio::test]
    async fn start_briefing_route_exists() {
        let state = test_state();

        // Create a project in DB
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let project = crate::models::Project {
                id: "briefing-start-proj".into(),
                name: "Start Briefing Test".into(),
                path: "/tmp/kronn-start-briefing-test".into(),
                repo_url: None,
                token_override: None,
                ai_config: crate::models::AiConfigStatus { detected: false, configs: vec![] },
                audit_status: crate::models::AiAuditStatus::NoTemplate,
                ai_todo_count: 0,
                default_skill_ids: vec![],
                default_profile_id: None,
                briefing_notes: None,
                created_at: now,
                updated_at: now,
            };
            crate::db::projects::insert_project(conn, &project)?;
            Ok(())
        }).await.unwrap();

        let body_json = serde_json::json!({ "agent": "ClaudeCode" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects/briefing-start-proj/start-briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK, "start-briefing should return 200: {body}");
        assert!(body["success"].as_bool().unwrap_or(false), "start-briefing should succeed: {body}");
        assert!(body["data"]["discussion_id"].is_string(), "Response should contain discussion_id: {body}");
        let disc_id = body["data"]["discussion_id"].as_str().unwrap();
        assert!(!disc_id.is_empty(), "discussion_id should not be empty");
    }

    /// Discussions created for validation/bootstrap/briefing should have pin_first_message=true.
    /// This test verifies that a discussion with pin_first_message=true roundtrips correctly
    /// through DB insert and retrieval via the GET API.
    #[tokio::test]
    async fn validation_discussion_has_pin_first_message() {
        let state = test_state();

        // Insert a discussion with pin_first_message=true (simulating what validation creates)
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now();
            let disc = crate::models::Discussion {
                id: "disc-pin".into(),
                project_id: None,
                title: "Validation audit AI".into(),
                agent: crate::models::AgentType::ClaudeCode,
                language: "en".into(),
                participants: vec![crate::models::AgentType::ClaudeCode],
                message_count: 0, messages: vec![],
                skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
                archived: false, workspace_mode: "Direct".into(),
                workspace_path: None, worktree_branch: None,
                tier: crate::models::ModelTier::Default,
                pin_first_message: true,
                summary_cache: None, summary_up_to_msg_idx: None,
                created_at: now, updated_at: now,
            };
            crate::db::discussions::insert_discussion(conn, &disc)?;
            Ok(())
        }).await.unwrap();

        // GET the discussion and verify pin_first_message is true
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-pin")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap(), "GET disc-pin must succeed: {body}");
        assert_eq!(body["data"]["pin_first_message"], true,
            "pin_first_message must be true for validation discussions: {body}");

        // Also verify via list endpoint
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let discs = body["data"].as_array().unwrap();
        let pin_disc = discs.iter().find(|d| d["id"] == "disc-pin").unwrap();
        assert_eq!(pin_disc["pin_first_message"], true,
            "pin_first_message must be true in list view too: {pin_disc}");
    }

    #[tokio::test]
    async fn start_briefing_nonexistent_project() {
        let state = test_state();

        let body_json = serde_json::json!({ "agent": "ClaudeCode" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects/nonexistent/start-briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK, "start-briefing on nonexistent project: {body}");
        assert!(!body["success"].as_bool().unwrap_or(true),
            "start-briefing on nonexistent project should return error: {body}");
    }
}
