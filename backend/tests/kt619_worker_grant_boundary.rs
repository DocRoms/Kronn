//! Principal review: a legitimately enrolled principal is later delegated.
//! No stolen secret, live instance, provider, socket or user database is used.
use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[path = "support/publication_fixture.rs"]
mod fixture_env;

async fn post(app: &Router, path: &str, body: Value) -> Value {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 32123))));
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        envelope["success"], true,
        "request failed: {}",
        envelope["error"]
    );
    envelope
}

async fn exercise(session_credential: Option<&str>) {
    let _directory = fixture_env::PublicationFixture::new();
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some("isolated-bearer-never-sent".into());
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let app = build_router_with_auth(state, true);

    // Legitimate enrolment happens BEFORE the principal becomes a worker.
    db.with_conn(|conn| {
        kronn::core::operator_secret::bootstrap(conn)?;
        Ok(())
    })
    .await
    .unwrap();
    let admin = kronn::core::operator_secret::read_delivered().unwrap();
    let enrolled = post(&app, "/api/human-credentials/enrol", json!({
        "authority": admin.expose(), "role": "orchestrator", "label": "Principal later delegated"
    })).await;
    let grant = enrolled["data"]["secret"].as_str().unwrap().to_string();
    db.with_conn(|conn| {
        for room in ["review-parent", "review-child"] {
            conn.execute(
                "INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES (?1,?1,'Codex','now','now')",
                [room],
            )?;
        }
        conn.execute(
            "INSERT INTO orchestration_runs (id,kind,discussion_id,created_at,updated_at) VALUES ('run','single_task','review-parent','now','now')", [],
        )?;
        conn.execute(
            "INSERT INTO planning_tasks (id,task_number,title,created_at,updated_at) VALUES ('task',619,'Review','now','now')", [],
        )?;
        let worker = kronn::db::discussion_sessions::create_session(
            conn, "review-child", "Codex", Some("principal-now-worker"), "peer",
        )?;
        conn.execute(
            "UPDATE discussion_sessions SET resume_token_hash=?1 WHERE id=?2",
            rusqlite::params![Sha256::digest(b"own-worker-resume").iter().map(|byte| format!("{byte:02x}")).collect::<String>(), worker],
        )?;
        conn.execute(
            "INSERT INTO task_executions (id,orchestration_run_id,task_id,parent_discussion_id,sub_discussion_id,status,worker_target_kind,worker_agent_type,worker_cli_session_id,created_at,updated_at) VALUES ('execution','run','task','review-parent','review-child','Working','cli','Codex',?1,'now','now')",
            [worker],
        )?;
        Ok(())
    }).await.unwrap();

    let spec = json!({
        "version": 1, "category": "decision", "dedup_key": "delegated-publication",
        "title": "No steering while delegated", "highlight": "Same legitimate grant, different duty",
        "impact": "Omitting a self-declared session must not restore authority",
        "action_required": {"required": false}
    });
    let content = format!("```kronn-important\n{spec}\n```\n");
    let proof = post(
        &app,
        "/api/human-credentials/proof",
        json!({
            "grant": grant, "discussion_id": "review-parent", "content": content
        }),
    )
    .await;
    let mut append = json!({
        "disc_id": "review-parent", "session_id": "principal-now-worker",
        "publication_grant": grant, "publication_proof": proof["data"],
        "messages": [{"source_msg_id": "delegated-publication", "role": "Agent", "agent_type": "Codex", "content": content}]
    });
    if let Some(secret) = session_credential {
        append["session_credential"] = json!(secret);
    }
    let response = post(&app, "/api/disc/append", append).await;
    let persisted = db
        .with_read_conn(|conn| kronn::db::discussion_important::count(conn, "review-parent"))
        .await
        .unwrap();
    assert_eq!(
        persisted, 0,
        "a legitimately enrolled principal must lose publication while delegated, even without a valid session credential; ingest={}",
        response["data"]["important"]
    );
}

#[tokio::test]
#[serial_test::serial]
async fn worker_with_its_real_session_credential_is_refused() {
    exercise(Some("own-worker-resume")).await;
}

#[tokio::test]
#[serial_test::serial]
async fn worker_cannot_restore_publication_by_omitting_its_session_credential() {
    exercise(None).await;
}

#[tokio::test]
#[serial_test::serial]
async fn worker_cannot_restore_publication_by_invalidating_its_session_credential() {
    exercise(Some("not-a-session-credential")).await;
}
