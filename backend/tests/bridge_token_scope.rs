//! KT-1006 — a launch's bridge token through the production router: it reaches
//! its own room, nothing outside its scope, and no secret-class route, while a
//! loopback request without any token keeps today's trust (human proof is
//! deferred to 0.15).

use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::core::bridge_token::{mint, BridgeScope};
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    // Loopback, exactly like an agent on this machine.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40404))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn fixture() -> (Router, tempfile::TempDir) {
    let (app, repos, _) = fixture_with_db().await;
    (app, repos)
}

async fn fixture_with_db() -> (Router, tempfile::TempDir, Arc<kronn::db::Database>) {
    let repos = tempfile::tempdir().unwrap();
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let paths: Vec<String> = ["p1", "p2"]
        .iter()
        .map(|name| {
            let path = repos.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect();
    db.with_conn(move |conn| {
        let now = chrono::Utc::now().to_rfc3339();
        for (id, path) in [("p1", &paths[0]), ("p2", &paths[1])] {
            conn.execute(
                "INSERT INTO projects(id, name, path, created_at, updated_at) VALUES (?1, ?1, ?2, ?3, ?3)",
                rusqlite::params![id, path, now],
            )?;
        }
        for (id, project) in [("room-a", "p1"), ("room-a2", "p1"), ("room-b", "p2")] {
            conn.execute(
                "INSERT INTO discussions(id, title, project_id, created_at, updated_at) VALUES (?1, ?1, ?2, ?3, ?3)",
                rusqlite::params![id, project, now],
            )?;
        }
        conn.execute(
            "INSERT INTO workflows(id, name, project_id, trigger_json, steps_json, created_at, updated_at) \
             VALUES ('wf-global', 'wf-global', NULL, '{\"type\":\"Manual\"}', '[]', ?1, ?1)",
            [&now],
        )?;
        for (suffix, project, number) in [("a", "p1", 9001), ("b", "p2", 9002)] {
            conn.execute(
                "INSERT INTO workflows(id, name, project_id, trigger_json, steps_json, created_at, updated_at) \
                 VALUES (?1, ?1, ?2, '{\"type\":\"Manual\"}', '[]', ?3, ?3)",
                rusqlite::params![format!("wf-{suffix}"), project, now],
            )?;
            conn.execute(
                "INSERT INTO workflow_runs(id, workflow_id, started_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![format!("run-{suffix}"), format!("wf-{suffix}"), now],
            )?;
            conn.execute(
                "INSERT INTO planning_tasks(id, task_number, title, created_at, updated_at) \
                 VALUES (?1, ?2, ?1, ?3, ?3)",
                rusqlite::params![format!("task-{suffix}"), number, now],
            )?;
            conn.execute(
                "INSERT INTO planning_task_projects(task_id, project_id) VALUES (?1, ?2)",
                rusqlite::params![format!("task-{suffix}"), project],
            )?;
            conn.execute(
                "INSERT INTO quick_prompts(id, name, prompt_template, project_id, created_at, updated_at) \
                 VALUES (?1, ?1, 'x', ?2, ?3, ?3)",
                rusqlite::params![format!("qp-{suffix}"), project, now],
            )?;
            conn.execute(
                "INSERT INTO quick_execs(id, name, project_id, command, created_at, updated_at) \
                 VALUES (?1, ?1, ?2, 'echo', ?3, ?3)",
                rusqlite::params![format!("qe-{suffix}"), project, now],
            )?;
            conn.execute(
                "INSERT INTO quick_apis(id, name, project_id, api_plugin_slug, api_config_id, \
                 api_endpoint_path, created_at, updated_at) VALUES (?1, ?1, ?2, 'x', 'c', '/', ?3, ?3)",
                rusqlite::params![format!("qa-{suffix}"), project, now],
            )?;
            conn.execute(
                "INSERT INTO live_pages(id, project_id, title, slug, created_at, updated_at) \
                 VALUES (?1, ?2, ?1, ?1, ?3, ?3)",
                rusqlite::params![format!("page-{suffix}"), project, now],
            )?;
            let room = if suffix == "a" { "room-a" } else { "room-b" };
            conn.execute(
                "INSERT INTO orchestration_runs(id, discussion_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?3)",
                rusqlite::params![format!("orch-{suffix}"), room, now],
            )?;
            conn.execute(
                "INSERT INTO task_executions(id, orchestration_run_id, task_id, parent_discussion_id, \
                 created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                rusqlite::params![
                    format!("exec-{suffix}"),
                    format!("orch-{suffix}"),
                    format!("task-{suffix}"),
                    room,
                    now
                ],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some("operator-bearer-never-sent-here".into());
    let db_handle = db.clone();
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (build_router_with_auth(state, true), repos, db_handle)
}

fn append(disc_id: &str, source: &str) -> Value {
    json!({
        "disc_id": disc_id,
        "messages": [{
            "source_msg_id": source,
            "role": "Agent",
            "content": "progress",
            "agent_type": "ClaudeCode"
        }]
    })
}

#[tokio::test]
async fn a_discussion_bridge_token_reaches_its_room_and_nothing_else() {
    let (app, _repos) = fixture().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        ..Default::default()
    })
    .unwrap();
    let token = guard.value().to_owned();

    // Its own room: appended.
    let (status, body) = call(
        &app,
        "POST",
        "/api/disc/append",
        Some(&token),
        Some(append("room-a", "m1")),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], true, "{body}");

    // Another room, same project or not: refused before the handler.
    for other in ["room-a2", "room-b"] {
        let (status, body) = call(
            &app,
            "POST",
            "/api/disc/append",
            Some(&token),
            Some(append(other, "m2")),
        )
        .await;
        assert_eq!(status, 403, "append to {other}: {body}");
    }

    // Path ids: a same-project read passes, a cross-project read does not.
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a2/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-b/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403);

    // Query and body ids naming another project.
    let (status, _) = call(
        &app,
        "GET",
        "/api/disc/search?q=x&project_id=p2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403);
    let (status, body) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(&token),
        Some(json!({"workflow_id": "wf-b"})),
    )
    .await;
    assert_eq!(status, 403, "cross-project effect: {body}");

    // Secret-class and unlisted routes refuse the token even from loopback.
    for (method, path, body) in [
        ("GET", "/api/config/export", None),
        (
            "POST",
            "/api/config/recovery/set",
            Some(json!({"passphrase": "aaaaaaaaaaaa"})),
        ),
        ("POST", "/api/config/sync-agent-tokens", Some(json!({}))),
        ("POST", "/api/config/discover-keys", Some(json!({}))),
        ("POST", "/api/config/auth-token/regenerate", Some(json!({}))),
        ("POST", "/api/mcps/configs/c1/reveal", Some(json!({}))),
        (
            "POST",
            "/api/external-api/connections/c1/reveal",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/mcps/bundles/export",
            Some(json!({"include_values": true})),
        ),
        (
            "POST",
            "/api/projects/p1/exec",
            Some(json!({"command": "env"})),
        ),
        (
            "POST",
            "/api/discussions/room-a/exec",
            Some(json!({"command": "env"})),
        ),
        ("GET", "/api/projects", None),
    ] {
        let (status, _) = call(&app, method, path, Some(&token), body).await;
        assert_eq!(status, 403, "{method} {path} must refuse a bridge token");
    }

    // Without a token, loopback keeps today's trust (human proof deferred).
    let (status, body) = call(
        &app,
        "POST",
        "/api/disc/append",
        None,
        Some(append("room-a2", "m3")),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // Dead after the launch: refused, never downgraded to loopback trust.
    drop(guard);
    let (status, _) = call(
        &app,
        "POST",
        "/api/disc/append",
        Some(&token),
        Some(append("room-a", "m4")),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        Some("kbt_0000000000000000000000000000000000000000000000000000000000000000"),
        None,
    )
    .await;
    assert_eq!(status, 401, "an unknown bridge token is refused");
}

#[tokio::test]
async fn a_token_whose_room_was_deleted_is_dead() {
    let (app, _repos) = fixture().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["deleted-room".into()],
        ..Default::default()
    })
    .unwrap();
    let (status, _) = call(&app, "GET", "/api/discussions", Some(guard.value()), None).await;
    assert_eq!(status, 401);
}

/// Each route family, scoped to room A / project A, refuses project B's
/// resource named in the path, the query or the body; same-project reads pass.
#[tokio::test]
async fn every_route_family_refuses_another_project_s_resource() {
    let (app, _repos) = fixture().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        ..Default::default()
    })
    .unwrap();
    let token = guard.value().to_owned();
    let refused: &[(&str, &str, Option<Value>)] = &[
        (
            "PATCH",
            "/api/discussions/room-a2",
            Some(json!({"title": "x"})),
        ),
        ("GET", "/api/discussions/room-b/participants", None),
        ("GET", "/api/disc/load_other?discussion_id=room-b", None),
        ("GET", "/api/planning/tasks/task-b", None),
        (
            "POST",
            "/api/planning/tasks/task-a/blockers",
            Some(json!({"blocker_task_id": "task-b"})),
        ),
        (
            "POST",
            "/api/planning/tasks",
            Some(json!({"title": "t", "project_ids": ["p2"]})),
        ),
        ("GET", "/api/workflows/wf-b", None),
        ("GET", "/api/workflows/wf-a/runs/run-b", None),
        (
            "POST",
            "/api/mcp/workflow-trigger",
            Some(json!({"workflow_id": "wf-b"})),
        ),
        ("GET", "/api/mcp/workflow-run-status/run-b", None),
        (
            "POST",
            "/api/mcp/workflow-wait-for-completion",
            Some(json!({"run_id": "run-b"})),
        ),
        ("POST", "/api/workflow-runs/run-b/resume", Some(json!({}))),
        (
            "POST",
            "/api/orchestration/tool/executions/exec-b/status",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/orchestration/tool/launch",
            Some(json!({"task_id": "task-b"})),
        ),
        (
            "POST",
            "/api/orchestration/deliver",
            Some(json!({"task_execution_id": "exec-b"})),
        ),
        ("PUT", "/api/quick-prompts/qp-b", Some(json!({"name": "x"}))),
        ("POST", "/api/mcp/qp-run", Some(json!({"qp_id": "qp-b"}))),
        (
            "POST",
            "/api/mcp/qp-run",
            Some(json!({"qp_id": "qp-a", "project_id": "p2"})),
        ),
        ("POST", "/api/quick-execs/qe-b/run", Some(json!({}))),
        ("POST", "/api/quick-apis/qa-b/run", Some(json!({}))),
        ("GET", "/api/pages/page-b", None),
        (
            "POST",
            "/api/pages",
            Some(json!({"title": "x", "project_id": "p2"})),
        ),
        ("GET", "/api/projects/p2", None),
        ("POST", "/api/projects/p2/full-audit", Some(json!({}))),
        (
            "POST",
            "/api/agent-api/call",
            Some(json!({"project_id": "p2"})),
        ),
        (
            "POST",
            "/api/media/generate",
            Some(json!({"project_id": "p2"})),
        ),
        (
            "POST",
            "/api/disc/create",
            Some(json!({"project_id": "p2", "title": "x"})),
        ),
    ];
    for (method, path, body) in refused {
        let (status, response) = call(&app, method, path, Some(&token), body.clone()).await;
        assert_eq!(status, 403, "{method} {path} must be refused: {response}");
    }
    // The same token reads its own project's resources.
    for path in [
        "/api/planning/tasks/task-a",
        "/api/workflows/wf-a",
        "/api/projects/p1",
        "/api/pages/page-a",
        "/api/mcp/workflow-run-status/run-a",
    ] {
        let (status, response) = call(&app, "GET", path, Some(&token), None).await;
        assert!(
            status != 403 && status != 401,
            "GET {path} is in scope: {status} {response}"
        );
    }
}

/// Any bearer that matches neither the operator token nor a live bridge token
/// is refused, even from loopback where no token at all would pass.
#[tokio::test]
async fn an_invalid_bearer_never_falls_back_to_loopback_trust() {
    let (app, _repos) = fixture().await;
    for bearer in ["wrong", "kbt_expired", "operator-bearer-never-sent-her"] {
        let (status, _) = call(&app, "GET", "/api/discussions", Some(bearer), None).await;
        assert_eq!(status, 401, "bearer {bearer:?}");
    }
    let (status, _) = call(&app, "GET", "/api/discussions", None, None).await;
    assert_eq!(status, 200, "no bearer: loopback trust is unchanged");
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions",
        Some("operator-bearer-never-sent-here"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the operator token still works");
}

/// A shared workflow triggered from a project-bound room runs for that
/// project; one that does not serve it stays refused.
#[tokio::test]
async fn a_shared_workflow_triggered_from_a_room_runs_for_the_room_s_project() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        ..Default::default()
    })
    .unwrap();
    let (status, body) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(guard.value()),
        Some(json!({"workflow_id": "wf-global"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], true, "{body}");
    let run_id = body["data"]["run_id"].as_str().unwrap().to_owned();
    let project: Option<String> = db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT project_id FROM workflow_runs WHERE id = ?1",
                [&run_id],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        project.as_deref(),
        Some("p1"),
        "resolved to the bound project"
    );

    let (status, _) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(guard.value()),
        Some(json!({"workflow_id": "wf-b"})),
    )
    .await;
    assert_eq!(
        status, 403,
        "a workflow that does not serve p1 stays refused"
    );
}
