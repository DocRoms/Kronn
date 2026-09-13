//! Real upload/import routes and SQLite, with no runtime agent or operator data.
use std::sync::{Arc, OnceLock};

use axum::{body::Body, http::Request, Router};
use base64::Engine;
use http_body_util::BodyExt;
use kronn::core::context_files::build_context_prompt;
use kronn::{AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

fn data_dir() -> &'static tempfile::TempDir {
    static DIRECTORY: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIRECTORY.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = directory.path().join("data");
        let host_home = directory.path().join("host-home");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&host_home).unwrap();
        std::env::set_var("KRONN_DATA_DIR", data_dir);
        std::env::set_var("KRONN_HOST_HOME", host_home);
        directory
    })
}

async fn fixture() -> (AppState, Router, String) {
    data_dir();
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let mut config = kronn::core::config::default_config();
    config.server.auth_token = None;
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let id = uuid::Uuid::new_v4().to_string();
    let did = id.clone();
    state.db.with_conn(move |conn| {
        conn.execute("INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
            VALUES (?1, 'Attachment fixture', 'ClaudeCode', 'en', '[]', datetime('now'), datetime('now'))", [&did])?;
        Ok(())
    }).await.unwrap();
    let app = kronn::build_router_with_auth(state.clone(), false);
    (state, app, id)
}

async fn response(app: Router, mut request: Request<Body>) -> Value {
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let result = app.oneshot(request).await.unwrap();
    assert!(result.status().is_success(), "{}", result.status());
    let bytes = result.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn upload(app: Router, id: &str, bytes: &[u8]) -> Value {
    let mut body = b"--kt633-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.json\"\r\nContent-Type: application/json\r\n\r\n".to_vec();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--kt633-boundary--\r\n");
    let result = response(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/discussions/{id}/context-files"))
            .header(
                "content-type",
                "multipart/form-data; boundary=kt633-boundary",
            )
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    assert_eq!(result["success"], true, "{result}");
    result
}

#[tokio::test]
async fn upload_utf16_and_binary_keeps_original_bytes_and_safe_durable_previews() {
    let (state, app, id) = fixture().await;
    let text = "{\"hello\":\"été 中文 😀\"}";
    let mut little = vec![0xff, 0xfe];
    little.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut big = vec![0xfe, 0xff];
    big.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    for (bytes, expected) in [
        (little, Some(text)),
        (big, Some(text)),
        (vec![0; 100], None),
        (b"{\0\"\0x\0".to_vec(), None),
        (text.as_bytes().to_vec(), Some(text)),
    ] {
        let result = upload(app.clone(), &id, &bytes).await;
        let file_id = result["data"]["file"]["id"].as_str().unwrap().to_string();
        let (preview, path): (String, String) = state
            .db
            .with_read_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT extracted_text, disk_path FROM context_files WHERE id = ?1",
                    [&file_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert!(std::path::Path::new(&path).starts_with(data_dir().path()));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!preview.contains('\0'));
        match expected {
            Some(text) => assert_eq!(preview, text),
            None => assert!(preview.contains("no text preview")),
        }
    }
    let entries = state
        .db
        .with_read_conn(move |conn| {
            Ok(kronn::db::discussions::get_context_files_for_prompt(
                conn, &id,
            )?)
        })
        .await
        .unwrap();
    let prompt = build_context_prompt(&entries);
    assert!(std::ffi::CString::new(prompt).is_ok());
}

#[tokio::test]
async fn imported_legacy_preview_and_history_are_safe_without_rewriting_the_archive() {
    let (state, app, id) = fixture().await;
    let raw = b"{\0\"\0x\0";
    let uploaded = upload(app.clone(), &id, raw).await;
    let file_id = uploaded["data"]["file"]["id"].as_str().unwrap().to_string();
    let did = id.clone();
    // Simulate an old stored preview, not a newly processed upload.
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE context_files SET extracted_text = ?2 WHERE id = ?1",
                rusqlite::params![file_id, "old\0preview"],
            )?;
            conn.execute(
                "UPDATE discussions SET title = ?2 WHERE id = ?1",
                rusqlite::params![did, "legacy\0title"],
            )?;
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
            VALUES ('legacy-message', ?1, 'User', ?2, datetime('now'), 0)",
                rusqlite::params![did, "hello\0world é😀"],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let envelope = response(
        app.clone(),
        Request::builder()
            .uri(format!("/api/discussions/{id}/export"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(envelope["attachments"][0]["extracted_text"], "old\0preview");
    let imported = response(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri("/api/discussions/import")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"content": envelope.to_string()}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(imported["success"], true, "{imported}");
    let new_id = imported["data"]["discussion_id"]
        .as_str()
        .unwrap()
        .to_string();
    let did = new_id.clone();
    let (discussion, entries) = state
        .db
        .with_read_conn(move |conn| {
            Ok((
                kronn::db::discussions::get_discussion(conn, &did)?.unwrap(),
                kronn::db::discussions::get_context_files_for_prompt(conn, &did)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(entries[0].text, "old\0preview");
    assert_eq!(discussion.messages[0].content, "hello\0world é😀");
    let context = build_context_prompt(&entries);
    assert!(!context.contains('\0'));
    assert!(context.contains("no text preview"));
    let prompt = kronn::api::disc_prompts::build_agent_prompt(
        &discussion,
        &kronn::models::AgentType::ClaudeCode,
        context.len(),
    );
    assert!(!prompt.contains('\0'));
    assert!(prompt.contains("hello world é😀"));
    let path = entries[0].disk_path.as_ref().unwrap();
    assert!(std::path::Path::new(path).starts_with(data_dir().path()));
    assert_eq!(std::fs::read(path).unwrap(), raw);
    let exported_again = response(
        app,
        Request::builder()
            .uri(format!("/api/discussions/{new_id}/export"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        exported_again["attachments"][0]["extracted_text"],
        "old\0preview"
    );
    assert_eq!(exported_again["messages"][0]["content"], "hello\0world é😀");
    let encoded = exported_again["attachments"][0]["data_base64"]
        .as_str()
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
        raw
    );
}
