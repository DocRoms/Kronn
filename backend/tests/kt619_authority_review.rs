//! Credential issuance is part of publication authority.
//!
//! Written by the principal during the KT-619 volet A review and taken over
//! here verbatim in shape; the two properties it proved together are split
//! below, because one of them is now fixed and should start protecting the
//! fix immediately rather than waiting for the other.
//!
//! No live backend, secrets, provider processes or network sockets are used.
use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn post(app: &Router, path: &str, body: Value) -> Value {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    // A local worker can issue requests from loopback. No bearer, Human or
    // pre-existing Orchestrator credential is supplied by this test.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 32123))));
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn append_body(room: &str, key: &str) -> Value {
    let spec = json!({
        "version": 1,
        "category": "decision",
        "dedup_key": key,
        "title": "Isolated authority regression",
        "highlight": "No operator authorized this publication.",
        "impact": "An ordinary session must not grant a privileged role.",
        "action_required": {"required": false}
    });
    json!({
        "disc_id": room,
        "messages": [{
            "source_msg_id": key,
            "role": "Agent",
            "agent_type": "Codex",
            "content": format!("```kronn-important\n{spec}\n```\n")
        }]
    })
}

async fn exercise_anonymous_issuance_chain(include_declared_session: bool) {
    let (_append, persisted) = exercise_issuance_chain(include_declared_session).await;
    assert_eq!(
        persisted, 0,
        "anonymous invitation/join must not turn an unverified caller into a privileged publisher"
    );
}

/// Walk the chain and hand back what happened, so each test asserts the one
/// property it is named for.
async fn exercise_issuance_chain(include_declared_session: bool) -> (Value, u32) {
    let data_dir = tempfile::tempdir().unwrap();
    std::env::set_var("KRONN_DATA_DIR", data_dir.path());
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some("test-only-operator-bearer-not-sent".into());
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    // Production middleware is ON; this is not the auth-disabled test router.
    let app = build_router_with_auth(state, true);
    let created = post(
        &app,
        "/api/disc/create",
        json!({"title": "Isolated review", "agent": "Codex", "no_agent": true}),
    )
    .await;
    assert_eq!(created["success"], true);
    let room = created["data"]["disc_id"].as_str().unwrap();

    let before = post(&app, "/api/disc/append", append_body(room, "before-join")).await;
    assert_eq!(before["success"], true);
    assert_eq!(before["data"]["important"]["refused_unverified"], 1);
    assert_eq!(before["data"]["important"]["published"], 0);

    let invite = post(
        &app,
        &format!("/api/discussions/{room}/invite-peer"),
        json!({}),
    )
    .await;
    assert_eq!(invite["success"], true);
    let joined = post(
        &app,
        "/api/discussions/peer-join",
        json!({
            "token": invite["data"]["token"],
            "agent_type": "Codex",
            "session_id": "review-self-issued-principal"
        }),
    )
    .await;
    assert_eq!(
        joined["success"], true,
        "join error only: {}",
        joined["error"]
    );
    assert!(joined["data"]["resume_token"].as_str().is_some());

    let mut body = append_body(room, "after-anonymous-join");
    if include_declared_session {
        body["session_id"] = json!("review-self-issued-principal");
    }
    body["session_credential"] = joined["data"]["resume_token"].clone();
    let after = post(&app, "/api/disc/append", body).await;
    let room = room.to_string();
    let persisted = db
        .with_conn(move |conn| kronn::db::discussion_important::count(conn, &room))
        .await
        .unwrap();
    (after, persisted)
}

/// The bypass, closed. An anonymous `invite` + `join` still yields a resume
/// credential — that is what those endpoints do — but a credential is no longer
/// an authority. Publication needs an enrolled grant, which is its own row with
/// its own secret and is attached to no session, so nothing acquired by
/// inviting, joining, transferring or rebinding can reach it.
///
/// This failed on `left: 1, right: 0` — a card really persisted — until the
/// grant landed.
#[tokio::test]
#[serial_test::serial]
async fn anonymous_local_invite_and_join_must_not_mint_important_publication_authority() {
    exercise_anonymous_issuance_chain(false).await;
}

/// The other half, and it passes: an append from a session whose id actually
/// resolves used to die on "cannot start a transaction within a transaction",
/// because the card ingest wrapped a SAVEPOINT around a call that opens its own
/// transaction. Asserted on its own so it guards the fix now instead of being
/// held hostage by the authority hole above.
#[tokio::test]
#[serial_test::serial]
async fn joined_cli_important_append_must_not_fail_with_a_nested_transaction() {
    let (append, _persisted) = exercise_issuance_chain(true).await;
    assert_eq!(
        append["success"], true,
        "a joined CLI must reach the handler at all: {}",
        append["error"]
    );
    assert!(
        !append["error"]
            .as_str()
            .unwrap_or_default()
            .contains("transaction within a transaction"),
        "the card ingest must not nest a transaction: {}",
        append["error"]
    );
}
