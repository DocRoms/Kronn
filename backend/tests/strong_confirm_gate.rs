//! KT-1496 — the routes that return a secret or destroy data, through the
//! production router. With a strong confirmation enrolled each one refuses a
//! request that carries no valid single-use proof, loopback and bridge token
//! included; without an enrolment nothing changes; a grant is bound to one call.
//!
//! Each test sets `KRONN_DATA_DIR` for its process (the suite runs under
//! nextest, one process per test).

use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::core::bridge_token::{mint, BridgeScope};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

const REQUIRED: &str = "strong_confirmation_required";

struct Harness {
    app: Router,
    state: AppState,
    data: tempfile::TempDir,
    /// Ten single-use recovery codes: every one is a proof for one call.
    codes: Vec<String>,
}

impl Harness {
    /// An instance; `enrolled` confirms a strong confirmation first.
    async fn new(enrolled: bool) -> Self {
        let data = tempfile::tempdir().unwrap();
        kronn::core::child_env::set_var("KRONN_DATA_DIR", data.path());
        let secret = kronn::core::crypto::generate_secret();
        let key = kronn::core::crypto::parse_secret(&secret).unwrap();
        let mut config = kronn::core::config::default_config();
        // The production stack: auth on, a loopback caller trusted.
        config.server.auth_enabled = true;
        config.server.auth_token = Some("operator-token-never-sent-here".into());
        config.encryption_secret = Some(secret);
        let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
        let state = AppState::new_defaults(
            Arc::new(RwLock::new(config)),
            db,
            DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let mut codes = Vec::new();
        if enrolled {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let enrolment =
                kronn::core::trust_proof::begin_enrolment(data.path(), &key, "owner", now, false)
                    .unwrap();
            let seed = kronn::core::totp::base32_decode(&enrolment.secret).unwrap();
            codes = kronn::core::trust_proof::confirm_enrolment(
                data.path(),
                &key,
                &kronn::core::totp::code_at(&seed, kronn::core::totp::step_of(now)),
                now,
            )
            .unwrap();
        }
        Self {
            app: build_router_with_auth(state.clone(), true),
            state,
            data,
            codes,
        }
    }

    async fn send(&self, request: Req) -> (u16, Value) {
        let mut builder = Request::builder().method(request.method).uri(request.path);
        for (name, value) in &request.headers {
            builder = builder.header(*name, value);
        }
        let body = match request.body {
            Some(body) => {
                let bytes = serde_json::to_vec(&body).unwrap();
                builder = builder
                    .header("content-type", "application/json")
                    .header("content-length", bytes.len());
                Body::from(bytes)
            }
            None => Body::empty(),
        };
        let mut http = builder.body(body).unwrap();
        // Loopback, exactly like an agent on this machine.
        http.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40404))));
        let response = self.app.clone().oneshot(http).await.unwrap();
        let status = response.status().as_u16();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

struct Req {
    method: &'static str,
    path: String,
    body: Option<Value>,
    headers: Vec<(&'static str, String)>,
}

fn req(method: &'static str, path: &str, body: Option<Value>) -> Req {
    Req {
        method,
        path: path.to_owned(),
        body,
        headers: Vec::new(),
    }
}

impl Req {
    fn code(mut self, code: &str) -> Self {
        self.headers.push(("x-kronn-confirm", code.to_owned()));
        self
    }

    fn grant(mut self, grant: &str) -> Self {
        self.headers.push(("x-kronn-grant", grant.to_owned()));
        self
    }

    fn bearer(mut self, token: &str) -> Self {
        self.headers
            .push(("authorization", format!("Bearer {token}")));
        self
    }
}

fn asks(status: u16, body: &Value) -> bool {
    status == 403 && body["error_code"] == REQUIRED
}

/// One request per route family of the inventory (28/10).
fn families() -> Vec<(&'static str, &'static str, Option<Value>)> {
    vec![
        // 1. recovery material
        (
            "POST",
            "/api/config/recovery/set",
            Some(json!({"passphrase": "chosen by the caller", "replace_unverified": true})),
        ),
        // 2. the export
        ("GET", "/api/config/export", None),
        // 3, 4, 5. decrypted values
        ("POST", "/api/mcps/configs/c1/reveal", Some(json!({}))),
        (
            "POST",
            "/api/external-api/connections/x1/reveal",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/execution-context/workflow/r1/reveal",
            Some(json!({})),
        ),
        // 6. an export under a passphrase the caller chooses
        (
            "POST",
            "/api/mcps/bundles/export",
            Some(json!({"include_values": true, "passphrase": "mine", "config_ids": ["c1"]})),
        ),
        // 7. the operator token and who may reach it
        ("POST", "/api/config/auth-token/regenerate", Some(json!({}))),
        (
            "POST",
            "/api/config/network-exposure",
            Some(json!({"exposed": true})),
        ),
        // 8. provider keys written into other tools' files
        ("POST", "/api/config/sync-agent-tokens", Some(json!({}))),
        // 12. everything `is_destructive` lists, and the settings that widen access
        ("POST", "/api/setup/reset", Some(json!({}))),
        ("POST", "/api/config/import", None),
        ("POST", "/api/config/recovery/reencrypt", Some(json!({}))),
        // Owner decision, 28/10: instance operations. An agent CLI removed, a
        // page import (it can bring code).
        ("POST", "/api/agents/uninstall", Some(json!({}))),
        ("POST", "/api/pages/import", None),
        // 12. the sensitive deletions: value that does not come back
        ("DELETE", "/api/projects/p1", None),
        ("DELETE", "/api/mcps/configs/c1", None),
        ("DELETE", "/api/external-api/connections/x1", None),
        ("DELETE", "/api/config/api-keys/k1", None),
        (
            "POST",
            "/api/human-credentials/revoke",
            Some(json!({"authority": "x", "credential_id": "h1", "reason": "r"})),
        ),
        (
            "PUT",
            "/api/projects/p1/validations-access",
            Some(json!({"access": "host"})),
        ),
        ("POST", "/api/config/agent-access", Some(json!({}))),
        ("POST", "/api/config/server", Some(json!({}))),
        ("PUT", "/api/projects/p1/github", Some(json!({}))),
        ("POST", "/api/agents/install", Some(json!({}))),
    ]
}

/// DoD 1: every family refuses a request with no proof, a wrong one, and a
/// bridge-less loopback agent alike.
#[tokio::test]
async fn enrolled_every_protected_route_family_refuses_a_request_without_a_valid_proof() {
    let harness = Harness::new(true).await;
    for (method, path, body) in families() {
        let (status, refused) = harness.send(req(method, path, body.clone())).await;
        assert!(
            asks(status, &refused),
            "{method} {path}: {status} {refused}"
        );
        let (status, refused) = harness
            .send(req(method, path, body.clone()).code("000000"))
            .await;
        // The fifth wrong code of the run locks the confirmation: still a refusal.
        assert!(
            (status == 403 || status == 429) && refused["error_code"] == REQUIRED,
            "{method} {path} with a wrong code: {status} {refused}"
        );
        let (status, refused) = harness
            .send(req(method, path, body).grant("kgr_not-a-grant"))
            .await;
        assert!(
            asks(status, &refused),
            "{method} {path} with a wrong grant: {status} {refused}"
        );
    }
}

/// DoD 2: without an enrolment every route behaves as before: the gate never
/// answers, the handler does.
#[tokio::test]
async fn without_an_enrolment_no_route_asks_for_a_proof() {
    let harness = Harness::new(false).await;
    for (method, path, body) in families() {
        if path == "/api/setup/reset" || path.ends_with("/auth-token/regenerate") {
            // These really act on the instance; the gate's silence is shown by
            // the others and by `a_request_with_a_code_is_not_spent_without_enrolment`.
            continue;
        }
        let (status, answer) = harness.send(req(method, path, body.clone())).await;
        assert!(
            answer["error_code"] != REQUIRED,
            "{method} {path}: {status} {answer}"
        );
        // A code sent anyway changes nothing.
        let (status, answer) = harness.send(req(method, path, body).code("123456")).await;
        assert!(
            answer["error_code"] != REQUIRED,
            "{method} {path} with a code: {status} {answer}"
        );
    }
}

/// Owner decision, 28/10: an everyday deletion never asks for the code, enrolled
/// or not (the destructive gate of `auth_allows` stays what it was). One per
/// family named by the owner, plus the other everyday ones.
#[tokio::test]
async fn an_everyday_deletion_goes_through_even_when_enrolled() {
    let harness = Harness::new(true).await;
    for path in [
        // a discussion, a message
        "/api/discussions/d1",
        "/api/discussions/d1/messages/m1",
        // a task (here: leaving a task's blockers and discussion links)
        "/api/planning/tasks/t1/blockers/b1",
        "/api/planning/tasks/t1/discussions",
        // a workflow, and its runs
        "/api/workflows/w1",
        "/api/workflows/w1/runs/r1",
        // a page
        "/api/pages/pg1",
        // a Quick Prompt, a Quick API, a Quick Exec
        "/api/quick-prompts/qp1",
        "/api/quick-apis/qa1",
        "/api/quick-execs/qe1",
        // a skill, a profile, a directive, a contact
        "/api/skills/s1",
        "/api/profiles/pr1",
        "/api/directives/di1",
        "/api/contacts/ct1",
    ] {
        let (status, answer) = harness.send(req("DELETE", path, None)).await;
        assert!(
            answer["error_code"] != REQUIRED && status != 429,
            "DELETE {path} must not ask: {status} {answer}"
        );
    }
    // And it did not spend or lock anything: a sensitive one still asks, then
    // accepts the first code.
    let (status, refused) = harness.send(req("DELETE", "/api/projects/p1", None)).await;
    assert!(asks(status, &refused), "{status} {refused}");
    let (status, done) = harness
        .send(req("DELETE", "/api/projects/p1", None).code(&harness.codes[0]))
        .await;
    assert!(!asks(status, &done), "{status} {done}");
}

/// Owner decision, 28/10: the strong confirmation is for secrets and instance
/// operations. A non-secret maintenance route that the destructive gate covers
/// (logs, quotas, audit state, an allowed embed site, …) never asks for the code,
/// enrolled or not; it keeps its local-or-token gate.
#[tokio::test]
async fn a_maintenance_route_goes_through_even_when_enrolled() {
    let harness = Harness::new(true).await;
    for (method, path, body) in [
        ("POST", "/api/debug/logs/clear", Some(json!({}))),
        ("POST", "/api/api-call-logs/purge", Some(json!({}))),
        (
            "POST",
            "/api/orchestration/provider-quotas/claude/rearm",
            Some(json!({})),
        ),
        ("POST", "/api/audit-runs/cleanup", Some(json!({}))),
        ("POST", "/api/rtk/deactivate", Some(json!({"agents": []}))),
        ("POST", "/api/config/embed-origins", Some(json!({}))),
        (
            "POST",
            "/api/projects/p1/context-audit/baseline",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/projects/p1/audit-attestation",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/mcps/custom/srv-1/cleanup-orphan-env",
            Some(json!({})),
        ),
    ] {
        let (status, answer) = harness.send(req(method, path, body)).await;
        assert!(
            answer["error_code"] != REQUIRED && status != 429,
            "{method} {path} must not ask: {status} {answer}"
        );
    }
    // Nothing was spent or locked: a protected route still takes the first code.
    let (status, refused) = harness
        .send(req("POST", "/api/agents/uninstall", Some(json!({}))))
        .await;
    assert!(asks(status, &refused), "{status} {refused}");
    let (status, done) = harness
        .send(req("POST", "/api/agents/uninstall", Some(json!({}))).code(&harness.codes[0]))
        .await;
    assert!(!asks(status, &done), "{status} {done}");
}

/// A sensitive deletion is refused without the proof and accepted once with
/// it, family by family (project, plugin, connection, provider key, human
/// credential).
#[tokio::test]
async fn a_sensitive_deletion_needs_the_proof_each_family() {
    let harness = Harness::new(true).await;
    let sensitive: Vec<_> = families()
        .into_iter()
        .filter(|(method, path, _)| {
            *method == "DELETE" || path.ends_with("/human-credentials/revoke")
        })
        .collect();
    assert_eq!(sensitive.len(), 5, "{sensitive:?}");
    for (index, (method, path, body)) in sensitive.into_iter().enumerate() {
        let (status, refused) = harness.send(req(method, path, body.clone())).await;
        assert!(
            asks(status, &refused),
            "{method} {path}: {status} {refused}"
        );
        let (status, done) = harness
            .send(req(method, path, body).code(&harness.codes[index]))
            .await;
        assert!(!asks(status, &done), "{method} {path}: {status} {done}");
    }
}

/// The code is the proof on the HTTP channel: accepted once, for the one route
/// and object, journalled with its channel.
#[tokio::test]
async fn a_code_in_the_header_is_spent_on_one_route_and_one_object() {
    let harness = Harness::new(true).await;
    let code = harness.codes[0].clone();

    let (status, done) = harness
        .send(req("DELETE", "/api/projects/p1", None).code(&code))
        .await;
    assert!(!asks(status, &done), "{status} {done}");

    // Spent: the same code on the same call, and on another, is refused.
    for path in ["/api/projects/p1", "/api/projects/p2"] {
        let (status, replay) = harness.send(req("DELETE", path, None).code(&code)).await;
        assert!(asks(status, &replay), "{path}: {status} {replay}");
    }

    let journal = harness
        .state
        .db
        .with_conn(|conn| kronn::db::trust::audit_entries(conn, 50))
        .await
        .unwrap();
    assert!(
        journal.iter().any(|line| line.event == "proof"
            && line.subject.contains("DELETE /api/projects/{id}")
            && line.subject.contains("p1")
            && line.detail.contains("http channel")),
        "{journal:?}"
    );
    assert!(
        journal.iter().any(|line| line.event == "proof_refused"),
        "a replay is journalled"
    );
}

/// A call that sends no code at all is the first request of a dialog: it
/// neither locks the confirmation nor fills the journal.
#[tokio::test]
async fn a_request_without_any_code_is_not_a_failed_guess() {
    let harness = Harness::new(true).await;
    for _ in 0..8 {
        let (status, refused) = harness.send(req("DELETE", "/api/projects/p1", None)).await;
        assert!(asks(status, &refused), "{status} {refused}");
    }
    let (status, done) = harness
        .send(req("DELETE", "/api/projects/p1", None).code(&harness.codes[0]))
        .await;
    assert!(!asks(status, &done), "not locked: {status} {done}");
}

/// Five wrong codes lock the confirmation, for every route.
#[tokio::test]
async fn five_wrong_codes_lock_every_protected_route() {
    let harness = Harness::new(true).await;
    for _ in 0..4 {
        let (status, refused) = harness
            .send(req("DELETE", "/api/projects/p1", None).code("000000"))
            .await;
        assert!(asks(status, &refused), "{status} {refused}");
    }
    // The failure that starts the lockout says so.
    let (status, starts) = harness
        .send(req("DELETE", "/api/projects/p1", None).code("000000"))
        .await;
    assert_eq!(status, 429, "{starts}");
    // Locked: a code of the authenticator is refused without being verified.
    let (status, locked) = harness
        .send(req("DELETE", "/api/projects/p1", None).code("123456"))
        .await;
    assert_eq!(status, 429, "{locked}");
    assert!(
        locked["error"].as_str().unwrap().contains("locked"),
        "{locked}"
    );
    // A recovery code is the way out (KT-1492), and it opens this one call.
    let (status, done) = harness
        .send(req("DELETE", "/api/projects/p1", None).code(&harness.codes[0]))
        .await;
    assert!(!asks(status, &done) && status != 429, "{status} {done}");
}

/// The body field carries the code too, for a caller that cannot set a header.
#[tokio::test]
async fn a_body_field_carries_the_code() {
    let harness = Harness::new(true).await;
    let (status, done) = harness
        .send(req(
            "POST",
            "/api/config/network-exposure",
            Some(json!({"exposed": false, "kronn_confirm": harness.codes[0]})),
        ))
        .await;
    assert!(!asks(status, &done), "{status} {done}");
    let (status, replay) = harness
        .send(req(
            "POST",
            "/api/config/network-exposure",
            Some(json!({"exposed": false, "kronn_confirm": harness.codes[0]})),
        ))
        .await;
    assert!(asks(status, &replay), "{status} {replay}");
}

/// DoD 3 + the grant: an agent without the code cannot obtain a grant; with a
/// code, the grant is for one call only and is spent by its first use.
#[tokio::test]
async fn a_grant_needs_the_code_and_serves_one_call_once() {
    let harness = Harness::new(true).await;
    let ask = |code: Option<&str>, path: &str| {
        let mut body = json!({"method": "DELETE", "path": path});
        if let Some(code) = code {
            body["code"] = json!(code);
        }
        req("POST", "/api/trust/grants", Some(body))
    };

    // Nothing, then a wrong code: no grant.
    let (status, refused) = harness.send(ask(None, "/api/projects/p1")).await;
    assert!(asks(status, &refused), "{status} {refused}");
    let (status, refused) = harness.send(ask(Some("000000"), "/api/projects/p1")).await;
    assert!(asks(status, &refused), "{status} {refused}");

    // The code mints a grant for that call.
    let (status, minted) = harness
        .send(ask(Some(&harness.codes[0]), "/api/projects/p1"))
        .await;
    assert_eq!(status, 200, "{minted}");
    let grant = minted["data"]["grant"].as_str().unwrap().to_owned();
    assert!(grant.starts_with("kgr_"));

    // Another call: refused, and the grant is burnt.
    let (status, refused) = harness
        .send(req("DELETE", "/api/projects/p2", None).grant(&grant))
        .await;
    assert!(asks(status, &refused), "{status} {refused}");
    let (status, refused) = harness
        .send(req("DELETE", "/api/projects/p1", None).grant(&grant))
        .await;
    assert!(asks(status, &refused), "burnt: {status} {refused}");

    // A fresh grant serves its call once.
    let (_, minted) = harness
        .send(ask(Some(&harness.codes[1]), "/api/projects/p1"))
        .await;
    let grant = minted["data"]["grant"].as_str().unwrap().to_owned();
    let (status, done) = harness
        .send(req("DELETE", "/api/projects/p1", None).grant(&grant))
        .await;
    assert!(!asks(status, &done), "{status} {done}");
    let (status, replay) = harness
        .send(req("DELETE", "/api/projects/p1", None).grant(&grant))
        .await;
    assert!(asks(status, &replay), "{status} {replay}");
}

/// Without an enrolment there is nothing to prove: no grant is sold.
#[tokio::test]
async fn no_grant_is_minted_without_an_enrolment() {
    let harness = Harness::new(false).await;
    let (status, refused) = harness
        .send(req(
            "POST",
            "/api/trust/grants",
            Some(json!({"method": "DELETE", "path": "/api/projects/p1", "code": "123456"})),
        ))
        .await;
    assert_eq!(status, 409, "{refused}");
}

fn bridge() -> kronn::core::bridge_token::BridgeTokenGuard {
    mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        room_grant_discussions: vec!["room-a".into()],
        ..Default::default()
    })
    .unwrap()
}

/// A bridge token stays refused on these routes, except for one action when it
/// carries a valid code (the owner gave it); nothing enrolled, nothing accepted.
#[tokio::test]
async fn a_bridge_token_reaches_a_protected_route_only_with_a_valid_code() {
    let harness = Harness::new(true).await;
    let guard = bridge();
    let token = guard.value().to_owned();

    for (method, path, body) in families() {
        let (status, refused) = harness.send(req(method, path, body).bearer(&token)).await;
        assert_eq!(status, 403, "{method} {path}: {refused}");
        assert!(
            refused["error"].as_str().unwrap().contains("bridge token"),
            "{method} {path}: {refused}"
        );
    }

    // A wrong code reaches the gate, which refuses it.
    let (status, refused) = harness
        .send(
            req("DELETE", "/api/projects/p1", None)
                .bearer(&token)
                .code("000000"),
        )
        .await;
    assert!(asks(status, &refused), "{status} {refused}");

    // The right code: accepted for that one action, journalled as the bridge channel.
    let code = harness.codes[0].clone();
    let (status, done) = harness
        .send(
            req("DELETE", "/api/projects/p1", None)
                .bearer(&token)
                .code(&code),
        )
        .await;
    assert!(!asks(status, &done) && status != 403, "{status} {done}");
    let (status, replay) = harness
        .send(
            req("DELETE", "/api/projects/p1", None)
                .bearer(&token)
                .code(&code),
        )
        .await;
    assert!(asks(status, &replay), "replay: {status} {replay}");
    let journal = harness
        .state
        .db
        .with_conn(|conn| kronn::db::trust::audit_entries(conn, 50))
        .await
        .unwrap();
    assert!(
        journal
            .iter()
            .any(|line| line.event == "proof" && line.detail.contains("bridge channel")),
        "{journal:?}"
    );
}

/// With no enrolment a bridge token carrying a code is still refused, as today.
#[tokio::test]
async fn a_bridge_token_with_a_code_stays_refused_when_nothing_is_enrolled() {
    let harness = Harness::new(false).await;
    let guard = bridge();
    let (status, refused) = harness
        .send(
            req("DELETE", "/api/projects/p1", None)
                .bearer(guard.value())
                .code("123456"),
        )
        .await;
    assert_eq!(status, 403, "{refused}");
}

/// Conditional routes ask only when the call is sensitive.
#[tokio::test]
async fn a_bundle_export_asks_only_with_values() {
    let harness = Harness::new(true).await;
    let (status, answer) = harness
        .send(req(
            "POST",
            "/api/mcps/bundles/export",
            Some(json!({"include_values": false, "config_ids": ["c1"]})),
        ))
        .await;
    assert!(!asks(status, &answer), "{status} {answer}");
    let (status, answer) = harness
        .send(req(
            "POST",
            "/api/mcps/bundles/export",
            Some(json!({"include_values": true, "passphrase": "mine", "config_ids": ["c1"]})),
        ))
        .await;
    assert!(asks(status, &answer), "{status} {answer}");
    let (status, answer) = harness
        .send(
            req(
                "POST",
                "/api/mcps/bundles/export",
                Some(json!({"include_values": true, "passphrase": "mine", "config_ids": ["c1"]})),
            )
            .code(&harness.codes[0]),
        )
        .await;
    assert!(!asks(status, &answer), "{status} {answer}");
}

fn quick_exec(approved: bool) -> Value {
    json!({
        "name": "Collect",
        "description": "",
        "command": "aws",
        "args": ["s3", "ls", "{{bucket}}"],
        "timeout_secs": 60,
        "output_format": "json",
        "variables": [],
        "unmodelled_args_approved": approved,
    })
}

/// A token-less loopback agent cannot approve its own command.
#[tokio::test]
async fn a_quick_exec_asks_for_the_proof_when_it_sets_the_human_approval() {
    let harness = Harness::new(true).await;
    let (status, answer) = harness
        .send(req("POST", "/api/quick-execs", Some(quick_exec(false))))
        .await;
    assert!(!asks(status, &answer), "{status} {answer}");

    let (status, answer) = harness
        .send(req("POST", "/api/quick-execs", Some(quick_exec(true))))
        .await;
    assert!(asks(status, &answer), "{status} {answer}");
    let (status, answer) = harness
        .send(req("POST", "/api/quick-execs", Some(quick_exec(true))).code(&harness.codes[0]))
        .await;
    assert!(!asks(status, &answer), "{status} {answer}");
    assert_eq!(answer["data"]["unmodelled_args_approved"], true, "{answer}");

    // Saving the line a human approved, with its approval kept, asks nothing.
    let id = answer["data"]["id"].as_str().unwrap().to_owned();
    let (status, answer) = harness
        .send(req(
            "PUT",
            &format!("/api/quick-execs/{id}"),
            Some(quick_exec(true)),
        ))
        .await;
    assert!(!asks(status, &answer), "{status} {answer}");
    // Approving another line does.
    let mut other = quick_exec(true);
    other["args"] = json!(["s3", "rm", "{{bucket}}"]);
    let (status, answer) = harness
        .send(req("PUT", &format!("/api/quick-execs/{id}"), Some(other)))
        .await;
    assert!(asks(status, &answer), "{status} {answer}");
}

/// Stored credentials go to the base URL: changing it asks, saving the same one does not.
#[tokio::test]
async fn a_custom_plugin_asks_when_its_base_url_changes() {
    let harness = Harness::new(true).await;
    let spec = kronn::models::ApiSpec {
        base_url: "https://old.example/v1".into(),
        auth: Default::default(),
        endpoints: vec![],
        docs_url: None,
        config_keys: vec![],
        default_headers: vec![],
        test_endpoint: None,
    };
    let json = serde_json::to_string(&spec).unwrap();
    harness
        .state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO mcp_servers(id, name, transport, source, api_spec_json) \
                 VALUES ('custom-x', 'X', 'stdio', 'manual', ?1)",
                [json],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let payload = |url: &str| json!({"name": "X", "base_url": url});

    let (status, answer) = harness
        .send(req(
            "PUT",
            "/api/mcps/custom/custom-x",
            Some(payload("https://old.example/v1/")),
        ))
        .await;
    assert!(!asks(status, &answer), "same URL: {status} {answer}");
    let (status, answer) = harness
        .send(req(
            "PUT",
            "/api/mcps/custom/custom-x",
            Some(payload("https://attacker.example")),
        ))
        .await;
    assert!(asks(status, &answer), "{status} {answer}");
    let (status, answer) = harness
        .send(
            req(
                "PUT",
                "/api/mcps/custom/custom-x",
                Some(payload("https://new.example")),
            )
            .code(&harness.codes[0]),
        )
        .await;
    assert!(!asks(status, &answer), "{status} {answer}");
}

/// Same for the endpoint of an external API connection.
#[tokio::test]
async fn an_external_connection_asks_when_its_endpoint_changes() {
    let harness = Harness::new(true).await;
    let now = chrono::Utc::now();
    let connection = kronn::models::ExternalApiConnection {
        id: "x1".into(),
        display_name: "Provider".into(),
        mention_alias: "provider".into(),
        endpoint: Some("https://old.example".into()),
        credential_slug: "provider".into(),
        origin_preset: kronn::models::ExternalApiConnectionPreset::Other,
        economy_model: None,
        default_model: None,
        reasoning_model: None,
        created_at: now,
        updated_at: now,
        image_model: None,
        video_model: None,
        media_endpoint: None,
        decision_model: None,
    };
    harness
        .state
        .db
        .with_conn(move |conn| kronn::db::external_api_connections::insert(conn, &connection))
        .await
        .unwrap();
    let payload = |endpoint: &str| {
        json!({
            "display_name": "Provider",
            "mention_alias": "provider",
            "endpoint": endpoint,
            "origin_preset": "other",
        })
    };
    let (status, answer) = harness
        .send(req(
            "PUT",
            "/api/external-api/connections/x1",
            Some(payload("https://old.example/v1")),
        ))
        .await;
    assert!(!asks(status, &answer), "same endpoint: {status} {answer}");
    let (status, answer) = harness
        .send(req(
            "PUT",
            "/api/external-api/connections/x1",
            Some(payload("https://attacker.example")),
        ))
        .await;
    assert!(asks(status, &answer), "{status} {answer}");
}

/// A project inside the data directory, or containing it, is refused whatever
/// the enrolment: project commands would read the vault from there.
#[tokio::test]
async fn a_project_path_overlapping_the_data_dir_is_refused_enrolled_or_not() {
    for enrolled in [false, true] {
        let harness = Harness::new(enrolled).await;
        let inside = harness.data.path().join("projects");
        std::fs::create_dir_all(&inside).unwrap();
        let parent = harness.data.path().parent().unwrap().to_owned();
        for path in [harness.data.path().to_owned(), inside, parent] {
            let (status, answer) = harness
                .send(req(
                    "POST",
                    "/api/projects/add-folder",
                    Some(json!({"path": path.to_string_lossy()})),
                ))
                .await;
            assert_eq!(status, 200, "{path:?}");
            assert!(
                answer["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("data directory"),
                "enrolled={enrolled} {path:?}: {answer}"
            );
        }
    }
}
