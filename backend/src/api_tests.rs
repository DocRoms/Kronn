/// Integration tests for the HTTP API layer.
///
/// Each test spins up a real Axum router backed by an in-memory SQLite database,
/// sends HTTP requests via `tower::ServiceExt::oneshot`, and asserts on the JSON
/// responses — exactly the same way Axum's own examples do it.
#[cfg(all(test, feature = "full-tests"))]
pub mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt; // for `oneshot`

    use crate::{
        build_router_with_auth, core::config::default_config, db::Database, AppState,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    };
    use serial_test::serial;

    // ─── Helper: build a test AppState with an in-memory DB ──────────────────

    /// Handlers under test call config::save — without KRONN_DATA_DIR that
    /// writes the developer's REAL config.toml (2026-07-13 incident; the
    /// persist_atomic guard now panics instead).
    fn isolate_config_dir() {
        // Called ONLY by the #[serial] tests whose handlers SAVE config —
        // a global call from test_state() would mutate KRONN_DATA_DIR from
        // non-serial tests and race the serialized env family. Re-set on
        // every call because config env tests legitimately remove it.
        let dir = std::env::temp_dir().join(format!("kronn-libtest-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        crate::core::child_env::set_var("KRONN_DATA_DIR", &dir);
    }

    /// A project directory of this test process only: parallel runs never share it.
    fn tmp_project(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("{name}-{}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    fn test_state() -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let config_arc = Arc::new(RwLock::new(default_config()));
        AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    #[tokio::test]
    async fn workflow_step_schema_route_returns_the_shared_contract() {
        let (status, body) = send(
            test_state(),
            false,
            Request::builder()
                .uri("/api/workflows/step-schema")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // The file plus what the code owns: the step types and the doctrine.
        let expected = crate::api::workflows::canonical_step_schema();
        assert_eq!(body["data"], expected);
        assert_eq!(
            body["data"]["step_types_closed_set"],
            serde_json::json!(crate::models::StepType::names())
        );
        assert!(body["data"]["authoring_doctrine"]
            .as_str()
            .is_some_and(|doctrine| doctrine.starts_with("## Authoring doctrine")));
    }

    /// KT-1681 — the default read is lean; the route narrows it to one step
    /// type or one shared section and refuses what it does not know.
    #[tokio::test]
    async fn workflow_step_schema_route_narrows_to_a_type_or_a_section() {
        let read = |query: &'static str| async move {
            send(
                test_state(),
                false,
                Request::builder()
                    .uri(format!("/api/workflows/step-schema{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .1
        };
        let full = crate::api::workflows::full_step_schema();
        let exec = read("?step_type=Exec").await;
        assert_eq!(exec["data"]["contract"], full["fields_by_type"]["Exec"]);
        let vars = read("?section=template_vars").await;
        assert_eq!(vars["data"]["contract"], full["template_vars"]);
        assert_eq!(read("?step_type=Nope").await["success"], false);
        assert_eq!(read("?section=Nope").await["success"], false);
    }

    #[tokio::test]
    async fn quota_agent_route_refuses_an_unknown_run() {
        let (status, body) = send(
            test_state(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/workflow-runs/nope/quota-agent")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"agent":"Codex"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);
        assert_eq!(body["error"], "Run not found");
    }

    /// Build a test AppState with a specific auth token configured.
    fn test_state_with_token(token: &str) -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let mut config = default_config();
        config.server.auth_token = Some(token.to_string());
        config.server.auth_enabled = true;
        let config_arc = Arc::new(RwLock::new(config));
        AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    /// Send a request and collect the response body as parsed JSON.
    async fn send(state: AppState, enable_auth: bool, req: Request<Body>) -> (StatusCode, Value) {
        let app = build_router_with_auth(state, enable_auth);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }
}
