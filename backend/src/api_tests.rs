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
}
