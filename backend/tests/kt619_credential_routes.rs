//! KT-619 — the enrolment surface, through the production router.
//!
//! Not the DB functions: those are `pub` for tests and are not endpoints. What
//! matters here is that the ROUTES refuse, with the real middleware in front of
//! them, from loopback, with no bearer sent.

use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[path = "support/publication_fixture.rs"]
mod fixture_env;

async fn post(app: &Router, path: &str, body: Value) -> (u16, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    // A local caller, exactly like a worker on this machine. No bearer.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 32123))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

struct Fixture {
    app: Router,
    db: Arc<kronn::db::Database>,
    _dir: fixture_env::PublicationFixture,
}

async fn fixture() -> Fixture {
    let dir = fixture_env::PublicationFixture::new();
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
    // Production middleware ON.
    let app = build_router_with_auth(state, true);
    Fixture { app, db, _dir: dir }
}

/// Bootstrap and return the admin secret, the way an operator would read it.
async fn bootstrap(fixture: &Fixture) -> String {
    fixture
        .db
        .with_conn(|conn| {
            kronn::core::operator_secret::bootstrap(conn)?;
            Ok(())
        })
        .await
        .unwrap();
    kronn::core::operator_secret::read_delivered()
        .unwrap()
        .expose()
        .to_string()
}

async fn enrol(fixture: &Fixture, authority: &str, role: &str, label: &str) -> (u16, Value) {
    post(
        &fixture.app,
        "/api/human-credentials/enrol",
        json!({"authority": authority, "role": role, "label": label}),
    )
    .await
}

#[tokio::test]
#[serial_test::serial]
async fn an_unauthenticated_local_caller_administers_nothing() {
    let fixture = fixture().await;
    bootstrap(&fixture).await;

    // Every administration route, from loopback, with nothing to present.
    for (path, body) in [
        (
            "/api/human-credentials/enrol",
            json!({"authority": "", "role": "human", "label": "mine"}),
        ),
        (
            "/api/human-credentials/list",
            json!({"authority": "kr-admin-guessed"}),
        ),
        (
            "/api/human-credentials/revoke",
            json!({"authority": "", "credential_id": "x", "reason": "because"}),
        ),
        (
            "/api/human-credentials/rotate",
            json!({"authority": "", "credential_id": "x"}),
        ),
        (
            "/api/human-credentials/admin/rotate",
            json!({"authority": "kr-admin-guessed"}),
        ),
    ] {
        let (status, envelope) = post(&fixture.app, path, body).await;
        assert_eq!(status, 403, "{path} must refuse an unauthenticated caller");
        assert_eq!(envelope["success"], false);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn the_admin_secret_enrols_and_the_plaintext_travels_exactly_once() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (status, enrolled) = enrol(&fixture, &admin, "human", "Romu — laptop").await;
    assert_eq!(status, 200);
    let secret = enrolled["data"]["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("kr-human-"));

    // The listing is the only other place a credential appears, and it carries
    // neither the plaintext nor its hash — a hash is enough to verify a guess.
    let (status, listed) = post(
        &fixture.app,
        "/api/human-credentials/list",
        json!({"authority": &admin}),
    )
    .await;
    assert_eq!(status, 200);
    let rendered = listed.to_string();
    assert!(!rendered.contains(&secret), "the plaintext travelled twice");
    assert!(rendered.contains("Romu — laptop"));
}

#[tokio::test]
#[serial_test::serial]
async fn an_orchestrator_grant_administers_nothing_through_the_routes() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (_s, enrolled) = enrol(&fixture, &admin, "orchestrator", "principal").await;
    let orchestrator = enrolled["data"]["secret"].as_str().unwrap().to_string();
    let victim_id = enrolled["data"]["credential"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // It publishes; it does not administer. Not even to enrol its own role.
    for (path, body) in [
        (
            "/api/human-credentials/enrol",
            json!({"authority": &orchestrator, "role": "orchestrator", "label": "another"}),
        ),
        (
            "/api/human-credentials/enrol",
            json!({"authority": &orchestrator, "role": "human", "label": "escalation"}),
        ),
        (
            "/api/human-credentials/list",
            json!({"authority": &orchestrator}),
        ),
        (
            "/api/human-credentials/revoke",
            json!({"authority": &orchestrator, "credential_id": &victim_id, "reason": "self"}),
        ),
    ] {
        let (status, _envelope) = post(&fixture.app, path, body).await;
        assert_eq!(status, 403, "{path} must refuse an orchestrator grant");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn a_credential_cannot_rotate_another_ones_secret() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (_s, a) = enrol(&fixture, &admin, "orchestrator", "A").await;
    let (_s, b) = enrol(&fixture, &admin, "orchestrator", "B").await;
    let a_secret = a["data"]["secret"].as_str().unwrap().to_string();
    let b_id = b["data"]["credential"]["id"].as_str().unwrap().to_string();
    let b_secret = b["data"]["secret"].as_str().unwrap().to_string();

    // The cross test: A holds a perfectly good secret, and aims it at B.
    let (status, _envelope) = post(
        &fixture.app,
        "/api/human-credentials/rotate",
        json!({"authority": &a_secret, "credential_id": &b_id}),
    )
    .await;
    assert_eq!(status, 403, "possession of A must not reach B");

    // B's own secret still works, so the refusal was about identity and not
    // about the route being broken.
    let (status, rotated) = post(
        &fixture.app,
        "/api/human-credentials/rotate",
        json!({"authority": &b_secret, "credential_id": &b_id}),
    )
    .await;
    assert_eq!(status, 200);
    assert_ne!(rotated["data"].as_str().unwrap(), b_secret);
}

#[tokio::test]
#[serial_test::serial]
async fn rotation_kills_the_old_secret_and_keeps_the_role() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (_s, enrolled) = enrol(&fixture, &admin, "orchestrator", "principal").await;
    let id = enrolled["data"]["credential"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let old = enrolled["data"]["secret"].as_str().unwrap().to_string();

    let (_s, rotated) = post(
        &fixture.app,
        "/api/human-credentials/rotate",
        json!({"authority": &old, "credential_id": &id}),
    )
    .await;
    let new = rotated["data"].as_str().unwrap().to_string();

    // A secret read once must not be good forever.
    let (status, _e) = post(
        &fixture.app,
        "/api/human-credentials/rotate",
        json!({"authority": &old, "credential_id": &id}),
    )
    .await;
    assert_eq!(status, 403, "the pre-rotation secret must be dead");

    // And the role did not move: rotation proves possession, and possession is
    // never a path to privilege.
    let (_s, listed) = post(
        &fixture.app,
        "/api/human-credentials/list",
        json!({"authority": &admin}),
    )
    .await;
    let entry = listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id.as_str())
        .unwrap();
    assert_eq!(entry["role"], "orchestrator");
    assert!(!listed.to_string().contains(&new));
}

#[tokio::test]
#[serial_test::serial]
async fn a_revoked_credential_stops_administering_and_stops_rotating() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (_s, enrolled) = enrol(&fixture, &admin, "human", "to be revoked").await;
    let id = enrolled["data"]["credential"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let secret = enrolled["data"]["secret"].as_str().unwrap().to_string();

    // It administers while live.
    let (status, _e) = post(
        &fixture.app,
        "/api/human-credentials/list",
        json!({"authority": &secret}),
    )
    .await;
    assert_eq!(status, 200);

    let (status, _e) = post(
        &fixture.app,
        "/api/human-credentials/revoke",
        json!({"authority": &admin, "credential_id": &id, "reason": "laptop lost"}),
    )
    .await;
    assert_eq!(status, 200);

    // And nothing afterwards — including rotating itself back to life.
    for (path, body) in [
        ("/api/human-credentials/list", json!({"authority": &secret})),
        (
            "/api/human-credentials/rotate",
            json!({"authority": &secret, "credential_id": &id}),
        ),
        (
            "/api/human-credentials/enrol",
            json!({"authority": &secret, "role": "human", "label": "comeback"}),
        ),
    ] {
        let (status, _e) = post(&fixture.app, path, body).await;
        assert_eq!(status, 403, "{path} must refuse a revoked credential");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn without_a_bootstrap_no_route_authorises_anybody() {
    // The "no TOFU" property at the API boundary: an install nobody has
    // bootstrapped hands nothing to whoever asks first.
    let fixture = fixture().await;

    for guess in ["", "kr-admin-anything", "kr-human-anything"] {
        let (status, _e) = enrol(&fixture, guess, "human", "first caller").await;
        assert_eq!(status, 403, "{guess:?} must not enrol on a fresh install");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn no_refusal_or_error_ever_echoes_the_secret_presented() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    // A body is not intrinsically un-logged, and an error message is the
    // easiest way for one to escape: `error.to_string()` on a path that took
    // the secret is all it takes. These are the four routes and both outcomes.
    let (_s, enrolled) = enrol(&fixture, &admin, "human", "live").await;
    let live = enrolled["data"]["secret"].as_str().unwrap().to_string();
    let invented = "kr-human-invented-and-must-not-come-back";

    for (path, body, needle) in [
        // Refused: an unknown authority.
        (
            "/api/human-credentials/list",
            json!({"authority": invented}),
            invented.to_string(),
        ),
        (
            "/api/human-credentials/revoke",
            json!({"authority": invented, "credential_id": "x", "reason": "r"}),
            invented.to_string(),
        ),
        // Accepted authority, rejected input: the validation error is built
        // from the same request that carried the secret.
        (
            "/api/human-credentials/enrol",
            json!({"authority": &live, "role": "human", "label": "   "}),
            live.clone(),
        ),
        (
            "/api/human-credentials/rotate",
            json!({"authority": &live, "credential_id": "no-such-credential"}),
            live.clone(),
        ),
    ] {
        let (_status, envelope) = post(&fixture.app, path, body).await;
        assert!(
            !envelope.to_string().contains(&needle),
            "{path} echoed the presented secret back: {envelope}"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn a_proof_is_issued_only_to_a_live_grant() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;
    // A proof references a real room: the foreign key is part of the binding,
    // so a proof cannot be minted for a discussion that does not exist.
    let (_s, created) = post(
        &fixture.app,
        "/api/disc/create",
        json!({"title": "Room", "agent": "Codex", "no_agent": true}),
    )
    .await;
    let room = created["data"]["disc_id"].as_str().unwrap().to_string();
    let (_s, enrolled) = enrol(&fixture, &admin, "orchestrator", "principal").await;
    let grant = enrolled["data"]["secret"].as_str().unwrap().to_string();
    let id = enrolled["data"]["credential"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let ask = |authority: String| {
        let app = fixture.app.clone();
        let room_id = room.clone();
        async move {
            post(
                &app,
                "/api/human-credentials/proof",
                json!({"grant": authority, "discussion_id": room_id, "content": "body"}),
            )
            .await
        }
    };

    let (status, issued) = ask(grant.clone()).await;
    assert_eq!(status, 200);
    assert!(issued["data"].as_str().unwrap().starts_with("proof-"));

    // Nothing else gets one.
    for stranger in ["", "kr-human-invented", &admin] {
        let (status, _e) = ask(stranger.to_string()).await;
        assert_eq!(status, 403, "{stranger:?} must not be issued a proof");
    }

    // And revocation stops issuance immediately, not at the next expiry.
    let (status, _e) = post(
        &fixture.app,
        "/api/human-credentials/revoke",
        json!({"authority": &admin, "credential_id": &id, "reason": "compromised"}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _e) = ask(grant).await;
    assert_eq!(status, 403, "a revoked grant must stop being issued proofs");
}

#[tokio::test]
#[serial_test::serial]
async fn the_admin_secret_publishes_nothing_by_itself() {
    // The bootstrap authorises enrolment. It is not a publication authority,
    // and conflating the two would make the operator's file a live publisher.
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;
    let (_s, created) = post(
        &fixture.app,
        "/api/disc/create",
        json!({"title": "Room", "agent": "Codex", "no_agent": true}),
    )
    .await;
    let room = created["data"]["disc_id"].as_str().unwrap().to_string();

    let (status, _e) = post(
        &fixture.app,
        "/api/human-credentials/proof",
        json!({"grant": &admin, "discussion_id": room, "content": "body"}),
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
#[serial_test::serial]
async fn the_admin_secret_rotates_itself_and_the_replacement_never_travels() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (status, rotated) = post(
        &fixture.app,
        "/api/human-credentials/admin/rotate",
        json!({"authority": &admin}),
    )
    .await;
    assert_eq!(status, 200);

    // What comes back is where to look, not what is there.
    let path = rotated["data"]["path"].as_str().unwrap().to_string();
    let replacement = kronn::core::operator_secret::read_delivered()
        .unwrap()
        .expose()
        .to_string();
    assert_eq!(
        path,
        kronn::core::operator_secret::secret_path()
            .unwrap()
            .to_string_lossy()
    );
    assert!(
        !rotated.to_string().contains(&replacement),
        "the new admin secret travelled over the wire"
    );
    assert_ne!(replacement, admin);

    // The old one is done, the new one works: a rotation that leaves both alive
    // has widened the door rather than moved it.
    let (status, _envelope) = post(
        &fixture.app,
        "/api/human-credentials/list",
        json!({"authority": &admin}),
    )
    .await;
    assert_eq!(status, 403, "the rotated-away secret still administers");

    let (status, _envelope) = post(
        &fixture.app,
        "/api/human-credentials/list",
        json!({"authority": &replacement}),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
#[serial_test::serial]
async fn no_enrolled_grant_can_rotate_the_secret_that_governs_it() {
    let fixture = fixture().await;
    let admin = bootstrap(&fixture).await;

    let (_s, human) = enrol(&fixture, &admin, "human", "Romu — laptop").await;
    let (_s, orchestrator) = enrol(&fixture, &admin, "orchestrator", "principal").await;

    // A `human` grant administers credentials — it enrols, lists and revokes.
    // Rotating the BOOTSTRAP is a different power: allowing it would let a
    // laptop the operator enrolled lock the operator out of their own install.
    for grant in [
        human["data"]["secret"].as_str().unwrap(),
        orchestrator["data"]["secret"].as_str().unwrap(),
    ] {
        let (status, _envelope) = post(
            &fixture.app,
            "/api/human-credentials/admin/rotate",
            json!({"authority": grant}),
        )
        .await;
        assert_eq!(
            status, 403,
            "an enrolled grant must not rotate the bootstrap"
        );
    }

    // And the admin secret is untouched by the attempts.
    let (status, _envelope) = post(
        &fixture.app,
        "/api/human-credentials/list",
        json!({"authority": &admin}),
    )
    .await;
    assert_eq!(status, 200);
}
