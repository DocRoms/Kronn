//! KT-325 DoD-4 / KT-372 — an explicit handoff to a joined CLI must wake that
//! session and only that session.
//!
//! The 2026-08-21 incident is the reason this file exists: a message addressed
//! to `@claude-cli-2` was delivered to the NATIVE ClaudeCode agent instead. Both
//! identities were real, so nothing failed — the message simply arrived
//! somewhere else and the intended session never woke. A unit test on alias
//! parsing cannot see that: the substitution happens across the whole routing
//! path, so the proof has to travel it.

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
        let dir = std::env::temp_dir().join(format!("kronn-handoff-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("KRONN_DATA_DIR", &dir);
    });
}

/// The router plus the database behind it: sessions are seeded directly, the
/// way a real join writes them, because the point under test is the routing.
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

/// Join a CLI session the way the bridge does, so the room really has two
/// sessions of the same provider — the situation the incident needed.
///
/// `cli_ordinal` is derived from join order, not stored, so the rows are simply
/// inserted in the order the aliases should follow.
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
            "title": "handoff",
            "agent": PROVIDER,
            "initial_prompt": "handoff routing",
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

/// Poll as one specific session and report what it was actually handed.
async fn wait_as(app: Router, disc_id: &str, session_id: &str, since: i64) -> Value {
    let uri = format!(
        "/api/discussions/{disc_id}/wait?since_sort_order={since}&timeout_secs=1\
         &exclude_agent_type={PROVIDER}&session_id={session_id}"
    );
    let (status, body) = call(app, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "wait failed: {body}");
    body["data"].clone()
}

#[tokio::test]
async fn an_explicit_cli_handoff_wakes_that_session_and_not_its_twin() {
    let (app, db) = test_app();
    let disc = create_discussion(app.clone()).await;
    join_cli(&db, 101, &disc, "sess-one").await;
    join_cli(&db, 102, &disc, "sess-two").await;

    // The handoff names the SECOND session explicitly — the case that failed.
    let (status, posted) = call(
        app.clone(),
        "POST",
        "/api/disc/append",
        Some(json!({
            "disc_id": disc,
            "session_id": "sess-one",
            "messages": [{
                "source_msg_id": "m-handoff",
                "role": "Agent",
                "content": "@claude-cli-2 peux-tu relire ce commit ?",
                "agent_type": PROVIDER,
                "targets": [{
                    "kind": "cli",
                    "agent_type": PROVIDER,
                    "cli_session_id": 102,
                }],
            }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "append failed: {posted}");

    // Control for the departed-target test: a live target DOES store a row, so a
    // count of zero there means the departure caused it, not the harness.
    let disc_for_count = disc.clone();
    let stored: i64 = db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND role = 'Agent'",
                rusqlite::params![disc_for_count],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        stored, 1,
        "a live target stores exactly one agent message — the control that makes \
         the departed-target count meaningful"
    );

    // The named session is handed the message and told it is for them.
    let addressed = wait_as(app.clone(), &disc, "sess-two", -1).await;
    let delivered = addressed["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        delivered
            .iter()
            .any(|m| m["addressed_to_caller"] == json!(true)),
        "the named session must be woken and told the turn is theirs: {addressed}"
    );
    assert_eq!(
        addressed["withheld_by_routing"],
        json!(0),
        "nothing may be withheld from the session the message actually names: {addressed}"
    );

    // Its twin — same provider, same room — is not.
    let twin = wait_as(app.clone(), &disc, "sess-one", -1).await;
    let twin_addressed = twin["messages"]
        .as_array()
        .map(|m| m.iter().any(|x| x["addressed_to_caller"] == json!(true)))
        .unwrap_or(false);
    assert!(
        !twin_addressed,
        "a same-provider session that was NOT named must never be told the turn is \
         theirs — that substitution is the whole incident: {twin}"
    );
}

#[tokio::test]
async fn a_message_for_nobody_in_particular_is_context_for_everyone() {
    // The mirror case: without an explicit target, neither session may be told
    // the turn is theirs. Silence about ownership is the honest answer, and a
    // guard that only ever refuses would break ordinary room chatter.
    let (app, db) = test_app();
    let disc = create_discussion(app.clone()).await;
    join_cli(&db, 201, &disc, "sess-a").await;
    join_cli(&db, 202, &disc, "sess-b").await;

    let (status, posted) = call(
        app.clone(),
        "POST",
        "/api/disc/append",
        Some(json!({
            "disc_id": disc,
            "session_id": "sess-a",
            "messages": [{
                "source_msg_id": "m-broadcast",
                "role": "Agent",
                "content": "point d'étape, rien à faire pour personne",
                "agent_type": PROVIDER,
            }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "append failed: {posted}");

    for session in ["sess-a", "sess-b"] {
        let seen = wait_as(app.clone(), &disc, session, -1).await;
        let owned = seen["messages"]
            .as_array()
            .map(|m| m.iter().any(|x| x["addressed_to_caller"] == json!(true)))
            .unwrap_or(false);
        assert!(
            !owned,
            "{session} was told an untargeted turn was theirs: {seen}"
        );
    }
}

/// KT-372 — the joint, from the router's side.
///
/// The bridge's Python suite asserts it EMITS the payloads in this fixture; this
/// feeds the very same payloads to the router and asserts who wakes. Neither test
/// proves the traversal alone — together they make the two halves impossible to
/// change independently, which is what was missing when both suites stayed green
/// through the incident.
#[tokio::test]
async fn the_router_routes_exactly_what_the_bridge_contract_emits() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/room_routing_contract.json"),
        )
        .expect("the routing contract fixture must exist — the bridge suite pins the same file"),
    )
    .expect("valid contract fixture");

    for case in fixture["cases"].as_array().expect("cases") {
        if case["outcome"] != json!("delivers") {
            // Refusals never reach the router: the bridge stops them, and its own
            // suite proves it. Asserting them here would test nothing.
            continue;
        }
        let (app, db) = test_app();
        let disc = create_discussion(app.clone()).await;
        for participant in fixture["participants"].as_array().expect("participants") {
            let pk = participant["id"].as_i64().expect("participant pk");
            join_cli(&db, pk, &disc, &format!("sess-{pk}")).await;
        }

        let (status, posted) = call(
            app.clone(),
            "POST",
            "/api/disc/append",
            Some(json!({
                "disc_id": disc,
                // The author must never be the target: nobody is woken by their
                // own append, so a self-addressed case would prove nothing.
                "session_id": format!("sess-{}", case["never_wakes_session_pk"].as_i64().unwrap()),
                "messages": [{
                    "source_msg_id": format!("m-{}", case["name"].as_str().unwrap_or("case")),
                    "role": "Agent",
                    "content": case["mention"],
                    "agent_type": PROVIDER,
                    "targets": case["emits"],
                }],
            })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "append failed for {}: {posted}",
            case["name"]
        );

        let woken = case["wakes_session_pk"].as_i64().expect("wakes");
        let never = case["never_wakes_session_pk"]
            .as_i64()
            .expect("never wakes");

        let addressed = wait_as(app.clone(), &disc, &format!("sess-{woken}"), -1).await;
        assert!(
            addressed["messages"]
                .as_array()
                .map(|m| m.iter().any(|x| x["addressed_to_caller"] == json!(true)))
                .unwrap_or(false),
            "case {}: session {woken} must be woken: {addressed}",
            case["name"]
        );

        let other = wait_as(app.clone(), &disc, &format!("sess-{never}"), -1).await;
        assert!(
            !other["messages"]
                .as_array()
                .map(|m| m.iter().any(|x| x["addressed_to_caller"] == json!(true)))
                .unwrap_or(false),
            "case {}: session {never} must NOT be told the turn is theirs: {other}",
            case["name"]
        );
    }
}

/// Mark a session as having left, the way `disc_leave` does.
async fn leave_cli(db: &kronn::db::Database, pk: i64) {
    db.with_conn(move |conn| {
        conn.execute(
            "UPDATE discussion_sessions SET status = 'left', left_at = ?2 WHERE id = ?1",
            rusqlite::params![pk, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// KT-372 — the race the fixture cannot express.
///
/// The bridge resolves `@claude-cli-2` while session 2 is joined, then the
/// session leaves before the POST lands. The alias was never wrong; it simply
/// stopped being true between resolution and delivery. The question is what the
/// router does with a target that no longer names anybody: silently dropping it
/// would deliver the message to the room with nobody owning the turn, and
/// falling back to the provider's native agent would be the original incident
/// arriving by a different road.
#[tokio::test]
async fn a_target_whose_session_left_between_resolution_and_append_fails_closed() {
    let (app, db) = test_app();
    let disc = create_discussion(app.clone()).await;
    join_cli(&db, 301, &disc, "sess-stays").await;
    join_cli(&db, 302, &disc, "sess-leaves").await;

    // Resolution happened while it was joined; the departure happens now.
    leave_cli(&db, 302).await;

    let (status, posted) = call(
        app.clone(),
        "POST",
        "/api/disc/append",
        Some(json!({
            "disc_id": disc,
            "session_id": "sess-stays",
            "messages": [{
                "source_msg_id": "m-departed",
                "role": "Agent",
                "content": "@claude-cli-2 tu peux relire ?",
                "agent_type": PROVIDER,
                "targets": [{
                    "kind": "cli",
                    "agent_type": PROVIDER,
                    "cli_session_id": 302,
                }],
            }],
        })),
    )
    .await;

    // `send_message` answers with an SSE stream, not a JSON envelope, so the body
    // says nothing about acceptance. Ask the database what actually landed.
    assert_eq!(status, StatusCode::OK, "append transport failed: {posted}");
    let disc_for_count = disc.clone();
    let stored: i64 = db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND role = 'Agent'",
                rusqlite::params![disc_for_count],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();

    // Whatever the router decides about storing it, the one outcome that must
    // never happen is another identity inheriting the turn.
    let stayed = wait_as(app.clone(), &disc, "sess-stays", -1).await;
    let inherited = stayed["messages"]
        .as_array()
        .map(|m| m.iter().any(|x| x["addressed_to_caller"] == json!(true)))
        .unwrap_or(false);
    assert!(
        !inherited,
        "a message aimed at a departed session was handed to the session that \
         stayed — that is the original incident by another road: {stayed}"
    );

    // The contract: a target that no longer names anybody rejects the append
    // before insertion. Nothing durable, nobody woken, no sister or native agent
    // inheriting. Reached only because the author's `session_id` is sent — omit
    // it and the targets are ignored, which is how this looked like a hole.
    assert_eq!(
        stored, 0,
        "a message aimed at a departed session must not be stored at all"
    );
}
