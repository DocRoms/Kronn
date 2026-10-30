//! KT-1490 — a CLI the user launched from the host and joined through the MCP
//! bridge carries no scoped bridge token (an operator token, or nothing). The
//! bridge script marks every request with `X-Kronn-Agent`; through the
//! production router the marker makes the write an agent's: a line that passes
//! run values to a program Kronn does not model is saved inactive instead of
//! refused, and the marker can lower a request, never raise it.

use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

const OPERATOR: &str = "operator-token-of-the-host-session";
const ROOM: &str = "room-host";

struct Caller<'a> {
    operator: bool,
    marker: bool,
    room: Option<&'a str>,
}

/// A human in the UI: the operator token, no marker.
const HUMAN: Caller = Caller {
    operator: true,
    marker: false,
    room: None,
};

/// A host-launched CLI joined through the bridge, operator token exported.
const HOST_AGENT: Caller = Caller {
    operator: true,
    marker: true,
    room: Some(ROOM),
};

async fn fixture() -> (Router, Arc<kronn::db::Database>) {
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions(id, title, created_at, updated_at) VALUES (?1, ?1, ?2, ?2)",
            rusqlite::params![ROOM, now],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some(OPERATOR.into());
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (build_router_with_auth(state, true), db)
}

async fn call(
    app: &Router,
    caller: &Caller<'_>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let mut builder = Request::builder().method(method).uri(path);
    if caller.operator {
        builder = builder.header("authorization", format!("Bearer {OPERATOR}"));
    }
    if caller.marker {
        builder = builder.header("x-kronn-agent", "1");
    }
    if let Some(room) = caller.room {
        builder = builder.header("x-kronn-discussion", room);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40404))));
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn purge(approved: Option<bool>) -> Value {
    let mut body = json!({
        "name": "Purge Fastly",
        "command": "fastly",
        "args": ["purge", "--key", "{{surrogate_key}}"],
        "variables": [{"name": "surrogate_key", "label": "Key", "placeholder": ""}],
        "output_format": "text",
        "timeout_secs": 60,
    });
    if let Some(approved) = approved {
        body["unmodelled_args_approved"] = json!(approved);
    }
    body
}

async fn cards_in_room(db: &kronn::db::Database) -> i64 {
    db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND id LIKE 'qe-activation:%'",
            [ROOM],
            |row| row.get(0),
        )?)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_host_session_with_the_operator_token_saves_the_line_inactive_and_posts_the_card() {
    let (app, db) = fixture().await;
    let saved = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/quick-execs",
        Some(purge(None)),
    )
    .await;
    assert_eq!(saved["success"], true, "{saved}");
    assert_eq!(saved["data"]["agent_written"], true, "{saved}");
    assert!(
        saved["data"]["unmodelled_args_approved"]
            .as_bool()
            .is_none_or(|approved| !approved),
        "no approval: {saved}"
    );
    let notice = saved["notice"].as_str().expect("told how it is activated");
    assert!(
        notice.contains("inactive") && notice.contains("Activer"),
        "{notice}"
    );
    assert_eq!(cards_in_room(&db).await, 1, "the room gets its one card");
}

#[tokio::test]
async fn a_marked_request_without_any_credential_saves_inactive_too() {
    let (app, _db) = fixture().await;
    let anonymous = Caller {
        operator: false,
        marker: true,
        room: None,
    };
    let saved = call(
        &app,
        &anonymous,
        "POST",
        "/api/quick-execs",
        Some(purge(None)),
    )
    .await;
    assert_eq!(saved["success"], true, "{saved}");
    assert_eq!(saved["data"]["agent_written"], true, "{saved}");
}

#[tokio::test]
async fn without_the_marker_the_request_is_a_humans_as_today() {
    let (app, db) = fixture().await;
    // Unapproved, a human's line is refused where the editor asks for the approval.
    let refused = call(&app, &HUMAN, "POST", "/api/quick-execs", Some(purge(None))).await;
    assert_eq!(refused["success"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("fastly"),
        "{refused}"
    );
    // Approved by a human, it is saved approved and is not the agent's.
    let approved = call(
        &app,
        &HUMAN,
        "POST",
        "/api/quick-execs",
        Some(purge(Some(true))),
    )
    .await;
    assert_eq!(approved["success"], true, "{approved}");
    assert_eq!(
        approved["data"]["unmodelled_args_approved"], true,
        "{approved}"
    );
    assert!(approved["data"]["agent_written"].is_null(), "{approved}");
    assert_eq!(cards_in_room(&db).await, 0, "a human's save posts no card");
}

#[tokio::test]
async fn a_marked_request_cannot_approve_on_create_or_update() {
    let (app, _db) = fixture().await;
    let created = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/quick-execs",
        Some(purge(Some(true))),
    )
    .await;
    assert_eq!(created["success"], true, "{created}");
    assert_eq!(created["data"]["agent_written"], true, "{created}");
    assert!(
        created["data"]["unmodelled_args_approved"]
            .as_bool()
            .is_none_or(|approved| !approved),
        "the approval it sent is dropped: {created}"
    );
    let id = created["data"]["id"].as_str().unwrap();
    let updated = call(
        &app,
        &HOST_AGENT,
        "PUT",
        &format!("/api/quick-execs/{id}"),
        Some(purge(Some(true))),
    )
    .await;
    assert_eq!(updated["success"], true, "{updated}");
    assert!(
        updated["data"]["unmodelled_args_approved"]
            .as_bool()
            .is_none_or(|approved| !approved),
        "{updated}"
    );
}

#[tokio::test]
async fn a_marked_request_cannot_activate_even_with_the_operator_token() {
    let (app, _db) = fixture().await;
    let created = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/quick-execs",
        Some(purge(None)),
    )
    .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let line = json!({"command": "fastly", "args": ["purge", "--key", "{{surrogate_key}}"]});
    let path = format!("/api/quick-execs/{id}/activate");

    let refused = call(&app, &HOST_AGENT, "POST", &path, Some(line.clone())).await;
    assert_eq!(refused["success"], false, "{refused}");
    let listed = call(&app, &HUMAN, "GET", "/api/quick-execs", None).await;
    let stored = &listed["data"].as_array().unwrap()[0];
    assert!(
        stored["unmodelled_args_approved"]
            .as_bool()
            .is_none_or(|approved| !approved),
        "still waiting: {stored}"
    );

    // The same call without the marker is the human's one click.
    let activated = call(&app, &HUMAN, "POST", &path, Some(line)).await;
    assert_eq!(activated["success"], true, "{activated}");
    assert_eq!(
        activated["data"]["unmodelled_args_approved"], true,
        "{activated}"
    );
}

#[tokio::test]
async fn a_marked_request_cannot_refuse_and_a_humans_refusal_survives_a_reload() {
    let (app, _db) = fixture().await;
    let created = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/quick-execs",
        Some(purge(None)),
    )
    .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let line = json!({"command": "fastly", "args": ["purge", "--key", "{{surrogate_key}}"]});
    let path = format!("/api/quick-execs/{id}/refuse-activation");
    let refused_flag = |pending: &Value| pending["data"][0]["refused"].clone();

    let by_agent = call(&app, &HOST_AGENT, "POST", &path, Some(line.clone())).await;
    assert_eq!(by_agent["success"], false, "{by_agent}");
    let pending = call(
        &app,
        &HUMAN,
        "GET",
        "/api/quick-execs/pending-approvals",
        None,
    )
    .await;
    assert_eq!(refused_flag(&pending), false, "{pending}");

    let by_human = call(&app, &HUMAN, "POST", &path, Some(line.clone())).await;
    assert_eq!(by_human["success"], true, "{by_human}");
    // « After a reload »: a fresh read still says refused, and still inactive.
    let pending = call(
        &app,
        &HUMAN,
        "GET",
        "/api/quick-execs/pending-approvals",
        None,
    )
    .await;
    assert_eq!(refused_flag(&pending), true, "{pending}");
    let listed = call(&app, &HUMAN, "GET", "/api/quick-execs", None).await;
    assert!(
        listed["data"][0]["unmodelled_args_approved"]
            .as_bool()
            .is_none_or(|approved| !approved),
        "{listed}"
    );

    // A human who changes their mind activates, and the refusal is gone.
    let activated = call(
        &app,
        &HUMAN,
        "POST",
        &format!("/api/quick-execs/{id}/activate"),
        Some(line),
    )
    .await;
    assert_eq!(activated["success"], true, "{activated}");
    let pending = call(
        &app,
        &HUMAN,
        "GET",
        "/api/quick-execs/pending-approvals",
        None,
    )
    .await;
    assert_eq!(pending["data"].as_array().unwrap().len(), 0, "{pending}");
}

#[tokio::test]
async fn a_marked_workflow_with_an_exec_line_saves_inactive() {
    let (app, _db) = fixture().await;
    let workflow = |approved: bool| {
        json!({
            "name": "purge", "project_id": null, "trigger": {"type": "Manual"},
            "exec_allowlist": ["fastly"],
            "steps": [{"name": "purge", "step_type": {"type": "Exec"},
                       "exec_command": "fastly",
                       "exec_args": ["purge", "--key", "{{surrogate_key}}"],
                       "exec_unmodelled_args_approved": approved}]
        })
    };
    let agent = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/workflows",
        Some(workflow(true)),
    )
    .await;
    assert_eq!(agent["success"], true, "{agent}");
    let step = &agent["data"]["steps"][0];
    assert_eq!(step["exec_agent_written"], true, "{agent}");
    assert!(
        step["exec_unmodelled_args_approved"]
            .as_bool()
            .is_none_or(|approved| !approved),
        "the approval it sent is dropped: {step}"
    );
    assert!(
        agent["notice"].is_string(),
        "told how it is activated: {agent}"
    );

    // Without the marker the same save is a human's, and its approval stands.
    let human = call(&app, &HUMAN, "POST", "/api/workflows", Some(workflow(true))).await;
    assert_eq!(human["success"], true, "{human}");
    assert_eq!(
        human["data"]["steps"][0]["exec_unmodelled_args_approved"], true,
        "{human}"
    );
}

#[tokio::test]
async fn a_marked_request_cannot_reenable_what_kronn_disabled() {
    let (app, _db) = fixture().await;
    let refused = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/workflows/reenable",
        Some(json!({"ids": []})),
    )
    .await;
    assert_eq!(refused["success"], false, "{refused}");
    let human = call(
        &app,
        &HUMAN,
        "POST",
        "/api/workflows/reenable",
        Some(json!({"ids": []})),
    )
    .await;
    assert_eq!(human["success"], true, "{human}");
}

#[tokio::test]
async fn a_room_header_that_is_not_an_id_posts_no_card_and_still_saves_inactive() {
    let (app, db) = fixture().await;
    let odd = Caller {
        operator: true,
        marker: true,
        room: Some("room; drop table messages"),
    };
    let saved = call(&app, &odd, "POST", "/api/quick-execs", Some(purge(None))).await;
    assert_eq!(saved["success"], true, "{saved}");
    assert_eq!(saved["data"]["agent_written"], true, "{saved}");
    assert_eq!(cards_in_room(&db).await, 0);
}

/// The human-only routes outside Quick Execs: each answers the human, and
/// refuses the very same request once it carries the marker, the operator
/// token included. One entry per family of route.
#[tokio::test]
async fn every_human_only_route_refuses_a_marked_request() {
    let (app, _db) = fixture().await;
    let routes: [(&str, &str, Option<Value>, &str); 8] = [
        (
            "POST",
            "/api/defaults/todo/install",
            None,
            "Only a human can reinstall Kronn's todo board",
        ),
        (
            "POST",
            "/api/live-page-actions/some-action/trust",
            Some(json!({"fingerprint": "f"})),
            "Only a human can approve a Page action",
        ),
        (
            "DELETE",
            "/api/live-page-actions/some-action/trust",
            None,
            "Only a human can change a Page action's approval",
        ),
        (
            "GET",
            "/api/projects/some-project/template-reinstall/plan",
            None,
            "Only a human can reinstall the template files",
        ),
        (
            "POST",
            "/api/projects/some-project/template-reinstall",
            None,
            "Only a human can reinstall the template files",
        ),
        (
            "POST",
            "/api/projects/some-project/drift/acknowledge",
            Some(json!({"sections": []})),
            "human decision",
        ),
        (
            "POST",
            "/api/projects/some-project/doc-provenance/confirm",
            Some(json!({"docs": []})),
            "Only a human can confirm a documentation change",
        ),
        (
            "POST",
            "/api/projects/some-project/audit-runs/some-run/steps/1/accept",
            Some(json!({})),
            "human decision",
        ),
    ];
    for (method, path, body, refusal) in routes {
        let marked = call(&app, &HOST_AGENT, method, path, body.clone()).await;
        assert_eq!(marked["success"], false, "{method} {path}: {marked}");
        assert!(
            marked["error"]
                .as_str()
                .unwrap_or_default()
                .contains(refusal),
            "{method} {path} must refuse the marked request: {marked}"
        );
        let human = call(&app, &HUMAN, method, path, body).await;
        assert!(
            !human["error"]
                .as_str()
                .unwrap_or_default()
                .contains(refusal),
            "{method} {path} is the human's, not refused as an agent's: {human}"
        );
    }
}

#[tokio::test]
async fn a_room_that_does_not_exist_posts_no_card_and_still_saves_inactive() {
    let (app, _db) = fixture().await;
    let ghost = Caller {
        operator: true,
        marker: true,
        room: Some("no-such-room"),
    };
    let saved = call(&app, &ghost, "POST", "/api/quick-execs", Some(purge(None))).await;
    assert_eq!(saved["success"], true, "{saved}");
    assert_eq!(saved["data"]["agent_written"], true, "{saved}");
}

/// A router over a plugin reserved to a named agent (Codex) and a Quick API
/// `qa-notes` on it: a caller Kronn cannot name is refused by the access
/// policy, a human is not.
async fn policy_app() -> Router {
    use kronn::models::*;
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let secret = kronn::core::crypto::generate_secret();
    let env = std::collections::HashMap::from([("NOTES_TOKEN".to_string(), "kt1490".to_string())]);
    let encrypted = kronn::db::mcps::encrypt_env(&env, &secret).unwrap();
    db.with_conn(move |conn| {
        kronn::db::mcps::upsert_server(
            conn,
            &McpServer {
                id: "custom-notes".into(),
                name: "Notes".into(),
                description: String::new(),
                transport: McpTransport::ApiOnly,
                source: McpSource::Manual,
                api_spec: Some(ApiSpec {
                    base_url: "https://api.example.com/v1".into(),
                    auth: ApiAuthKind::Bearer {
                        env_key: "NOTES_TOKEN".into(),
                    },
                    endpoints: vec![ApiEndpoint {
                        method: "GET".into(),
                        path: "/users/me".into(),
                        description: String::new(),
                    }],
                    docs_url: None,
                    config_keys: vec![],
                    default_headers: vec![],
                    test_endpoint: None,
                }),
            },
        )?;
        kronn::db::mcps::insert_config(
            conn,
            &McpConfig {
                id: "cfg-notes".into(),
                server_id: "custom-notes".into(),
                label: "Notes".into(),
                env_keys: vec!["NOTES_TOKEN".into()],
                env_encrypted: encrypted,
                args_override: None,
                is_global: true,
                include_general: true,
                config_hash: "kt1490".into(),
                project_ids: Vec::new(),
                host_sync: HostSyncMode::None,
            },
        )?;
        kronn::db::api_access_policies::set(
            conn,
            "custom-notes",
            &ApiAccessPolicy {
                access: ApiAccessRule::Agents {
                    agents: vec![ApiAccessSubject {
                        agent: AgentType::Codex,
                        model: None,
                    }],
                },
                endpoints: vec![],
            },
        )?;
        let qa: QuickApi = serde_json::from_value(json!({
            "id": "qa-notes", "name": "Me", "icon": "x", "description": "",
            "project_id": null, "api_plugin_slug": "custom-notes",
            "api_config_id": "cfg-notes", "api_endpoint_path": "/users/me",
            "api_method": "GET", "variables": [],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        kronn::db::quick_apis::insert_quick_api(conn, &qa)
    })
    .await
    .unwrap();
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some(OPERATOR.into());
    config.encryption_secret = Some(secret);
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    build_router_with_auth(state, true)
}

fn assert_policy_refusal(text: &str, context: &Value) {
    assert!(
        text.contains("Access policy") && text.contains("no Kronn-issued identity"),
        "a marked host CLI is an unnamed agent to the policy: {context}"
    );
}

/// A saved Quick API run by a marked host CLI meets the agent access policy: a
/// plugin reserved to a named agent refuses a CLI Kronn cannot name.
#[tokio::test]
async fn a_marked_request_runs_a_quick_api_as_an_agent_under_the_access_policy() {
    let app = policy_app().await;
    let marked = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/quick-apis/qa-notes/run",
        Some(json!({"variables": {}})),
    )
    .await;
    assert_policy_refusal(
        marked["data"]["error"].as_str().unwrap_or_default(),
        &marked,
    );
}

/// The same holds on the fan-out route: a marked request is not « the person
/// in the UI » there either.
#[tokio::test]
async fn a_marked_request_on_the_batch_route_meets_the_agent_access_policy() {
    let app = policy_app().await;
    let marked = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/quick-apis/qa-notes/batch",
        Some(json!({"items": ["x"]})),
    )
    .await;
    assert_policy_refusal(&marked.to_string(), &marked);
}

/// The wizard's test routes ran as a hardcoded human: a marked host CLI with
/// the operator bearer skipped the agent access policy there.
#[tokio::test]
async fn a_marked_request_on_test_collect_api_data_meets_the_agent_access_policy() {
    let app = policy_app().await;
    let marked = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/workflow-steps/test-collect-api-data",
        Some(json!({
            "step": {
                "name": "collect",
                "step_type": {"type": "CollectApiData"},
                "collect_api_data": {"sources": [{"alias": "me", "quick_api_id": "qa-notes"}]}
            },
            "project_id": null,
        })),
    )
    .await;
    assert_policy_refusal(&marked.to_string(), &marked);
}

#[tokio::test]
async fn a_marked_request_on_test_api_call_meets_the_agent_access_policy() {
    let app = policy_app().await;
    let marked = call(
        &app,
        &HOST_AGENT,
        "POST",
        "/api/workflow-steps/test-api-call",
        Some(json!({
            "step": {
                "name": "me",
                "step_type": {"type": "ApiCall"},
                "api_plugin_slug": "custom-notes",
                "api_config_id": "cfg-notes",
                "api_endpoint_path": "/users/me",
                "api_method": "GET",
            },
            "project_id": "p1",
        })),
    )
    .await;
    assert_policy_refusal(&marked.to_string(), &marked);
}
