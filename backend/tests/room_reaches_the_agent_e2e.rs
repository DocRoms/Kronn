//! KT-374 DoD-5 — the scenario the ticket was opened for, end to end.
//!
//! On 2026-08-21 a peer announced it was taking over a task, the agent kept
//! working without reading the room, and the same function was written twice.
//! The unit tests on either side of the bridge each prove a piece of the fix;
//! this one plays the actual sequence: a peer writes through the route the
//! bridge really uses, and the busy agent — which never calls a blocking wait —
//! is handed that announcement by a peek before it can commit the duplicate.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};

const PROVIDER: &str = "ClaudeCode";

fn isolate_config_dir() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("kronn-peek-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("KRONN_DATA_DIR", &dir);
    });
}

fn test_app() -> (Router, Arc<kronn::db::Database>) {
    isolate_config_dir();
    let db = Arc::new(kronn::db::Database::open_in_memory().expect("in-memory DB"));
    let mut cfg = kronn::core::config::default_config();
    cfg.server.auth_token = None;
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(cfg)),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (build_router_with_auth(state, false), db)
}

async fn call(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn join_cli(db: &kronn::db::Database, pk: i64, disc_id: &str, session_id: &str) {
    let (disc, sid) = (disc_id.to_string(), session_id.to_string());
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (?1, ?2, ?3, ?4, 'peer', 'active', ?5)",
            rusqlite::params![pk, disc, PROVIDER, sid, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn create_discussion(app: Router) -> String {
    let (status, body) = call(
        app,
        "POST",
        "/api/discussions",
        Some(json!({
            "title": "room awareness",
            "agent": PROVIDER,
            "initial_prompt": "peer takes over a task",
            "language": "fr",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "discussion creation failed: {body}");
    body["data"]["id"]
        .as_str()
        .expect("discussion id")
        .to_string()
}

/// What the bridge does on the return of an unrelated tool: `timeout_secs=0`.
async fn peek_as(app: Router, disc_id: &str, session_id: &str, since: i64) -> Value {
    let uri = format!(
        "/api/discussions/{disc_id}/wait?since_sort_order={since}&timeout_secs=0\
         &exclude_agent_type={PROVIDER}&session_id={session_id}"
    );
    let (status, body) = call(app, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "peek failed: {body}");
    body["data"].clone()
}

/// The peer announces a takeover, addressing this exact session, through the
/// same endpoint `disc_append` posts to.
async fn announce_takeover(app: Router, disc: &str, author_session: &str, target_pk: i64) {
    let (status, posted) = call(
        app,
        "POST",
        "/api/disc/append",
        Some(json!({
            "disc_id": disc,
            "session_id": author_session,
            "messages": [{
                "source_msg_id": "m-takeover",
                "role": "Agent",
                "content": "@claude-cli-1 je reprends KT-320 pendant ton rate limit",
                "agent_type": PROVIDER,
                "targets": [{
                    "kind": "cli",
                    "agent_type": PROVIDER,
                    "cli_session_id": target_pk,
                }],
            }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "append failed: {posted}");
}

#[tokio::test]
async fn a_busy_agent_is_handed_the_takeover_it_never_asked_for() {
    let (app, db) = test_app();
    let disc = create_discussion(app.clone()).await;
    join_cli(&db, 101, &disc, "sess-busy").await; // the absorbed agent
    join_cli(&db, 102, &disc, "sess-peer").await; // the peer taking over

    announce_takeover(app.clone(), &disc, "sess-peer", 101).await;

    // The busy session never calls a blocking wait — it only ever peeks, the
    // way the bridge does when some unrelated tool returns.
    let started = std::time::Instant::now();
    let peeked = peek_as(app.clone(), &disc, "sess-busy", 0).await;
    let elapsed = started.elapsed();

    // The batch legitimately mixes two kinds: the turn aimed at this session,
    // and room context riding along as `awareness`. That mix is precisely why
    // the bridge splits them into `attention_required` and `context` rather
    // than handing back one flat list.
    let messages = peeked["messages"].as_array().expect("messages array");
    let addressed: Vec<&Value> = messages
        .iter()
        .filter(|message| message["addressed_to_caller"] == true)
        .collect();
    assert_eq!(
        addressed.len(),
        1,
        "the takeover must reach a session that never asked: {peeked}",
    );
    assert!(
        addressed[0]["content"]
            .as_str()
            .unwrap()
            .contains("je reprends KT-320"),
        "the announcement arrives as content, not as a count",
    );
    assert!(
        messages.iter().any(|message| message["awareness"] == true),
        "ambient context still travels, and must stay distinguishable from the debt",
    );
    assert!(
        elapsed < std::time::Duration::from_millis(900),
        "a peek the bridge runs on every tool return must not block; took {elapsed:?}",
    );
}

#[tokio::test]
async fn peeking_does_not_consume_the_turn_a_later_wait_still_owes() {
    // The peek shows; the bridge's durable cursor is what marks as read. A
    // server-side peek must therefore leave the message exactly as available
    // as it found it — otherwise an agent whose turn was interrupted between
    // the peek and reading it would lose the announcement for good.
    let (app, db) = test_app();
    let disc = create_discussion(app.clone()).await;
    join_cli(&db, 101, &disc, "sess-busy").await;
    join_cli(&db, 102, &disc, "sess-peer").await;

    announce_takeover(app.clone(), &disc, "sess-peer", 101).await;

    let addressed_turns = |peeked: &Value| -> usize {
        peeked["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["addressed_to_caller"] == true)
            .count()
    };

    let first = peek_as(app.clone(), &disc, "sess-busy", 0).await;
    assert_eq!(addressed_turns(&first), 1);

    let second = peek_as(app.clone(), &disc, "sess-busy", 0).await;
    assert_eq!(
        addressed_turns(&second),
        1,
        "the server holds no read state of its own: from the same cursor, the \
         same turn comes back",
    );
}

#[tokio::test]
async fn a_peek_never_makes_the_room_believe_the_agent_is_listening() {
    // An agent piggybacking peeks on unrelated tool calls would otherwise show
    // up as attentive to everyone watching the participants panel, while it is
    // in fact three minutes into a compile.
    let (app, db) = test_app();
    let disc = create_discussion(app.clone()).await;
    join_cli(&db, 101, &disc, "sess-busy").await;

    peek_as(app.clone(), &disc, "sess-busy", 0).await;

    let (status, participants) = call(
        app.clone(),
        "GET",
        &format!("/api/discussions/{disc}/participants"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "participants failed: {participants}"
    );
    let rows = participants["data"].as_array().expect("participant rows");
    let busy = rows
        .iter()
        .find(|row| row["session_id"] == "sess-busy")
        .expect("the busy session is listed");
    assert!(
        busy["activity"].is_null(),
        "a peek claims no activity at all, got {}",
        busy["activity"],
    );

    // …and a genuine wait on the same session still records one, so the guard
    // narrowed nothing beyond the peek itself.
    let uri = format!(
        "/api/discussions/{disc}/wait?since_sort_order=0&timeout_secs=1\
         &exclude_agent_type={PROVIDER}&session_id=sess-busy"
    );
    call(app.clone(), "GET", &uri, None).await;
    let (_, participants) = call(
        app,
        "GET",
        &format!("/api/discussions/{disc}/participants"),
        None,
    )
    .await;
    let busy = participants["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["session_id"] == "sess-busy")
        .unwrap()
        .clone();
    assert!(
        !busy["activity"].is_null(),
        "a real wait still records presence the way it always did",
    );
}
