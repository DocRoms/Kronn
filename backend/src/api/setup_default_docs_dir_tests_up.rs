
use super::*;

async fn docs_dir_reply(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

/// KT-263: the default AI doc folder is normalized and persisted (it only
/// pre-fills the briefing, KT-1192), and refused with a 400 when it could
/// leave a project.
#[tokio::test]
#[serial_test::serial]
async fn default_docs_dir_is_normalized_saved_and_refused_when_unsafe() {
    // Saving persists config.toml: never the developer's real one.
    let _restore =
        crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&["KRONN_DATA_DIR"]);
    let data = tempfile::tempdir().unwrap();
    crate::core::child_env::set_var("KRONN_DATA_DIR", data.path());
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config()));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

    let (status, body) = docs_dir_reply(
        save_default_docs_dir(State(state.clone()), Json(Some(" ./handbook/ ".into()))).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"], "handbook");
    assert_eq!(
        state.config.read().await.scan.default_docs_dir.as_deref(),
        Some("handbook")
    );

    for bad in ["../outside", "/etc", "C:\\docs", ".git/x"] {
        let (status, body) = docs_dir_reply(
            save_default_docs_dir(State(state.clone()), Json(Some(bad.into()))).await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert_eq!(body["error_code"], "validation");
        assert!(!body["error"].as_str().unwrap().is_empty());
        assert_eq!(
            state.config.read().await.scan.default_docs_dir.as_deref(),
            Some("handbook"),
            "a refused value leaves the saved one"
        );
    }

    let (status, body) =
        docs_dir_reply(save_default_docs_dir(State(state.clone()), Json(Some("  ".into()))).await)
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].is_null());
    assert_eq!(state.config.read().await.scan.default_docs_dir, None);
}
