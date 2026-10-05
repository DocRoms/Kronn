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

fn sha256_hex(raw: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(raw.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

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
        // A synthetic API plugin config linked to p2 only.
        conn.execute(
            "INSERT INTO mcp_servers(id, name, transport) VALUES ('synthetic-api', 'Synthetic', 'stdio')",
            [],
        )?;
        conn.execute(
            "INSERT INTO mcp_configs(id, server_id, label, include_general) VALUES ('cfg-b', 'synthetic-api', 'cfg-b', 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO mcp_config_projects(config_id, project_id) VALUES ('cfg-b', 'p2')",
            [],
        )?;
        // Layer B review fixtures: shared resources, a General room, a p2
        // session, invite and offer, a cross-project blocker, a replay binding.
        conn.execute(
            "INSERT INTO discussions(id, title, project_id, created_at, updated_at) \
             VALUES ('room-g', 'room-g', NULL, ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO quick_prompts(id, name, prompt_template, project_id, created_at, updated_at) \
             VALUES ('qp-global', 'qp-global', 'x', NULL, ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO quick_execs(id, name, project_id, command, created_at, updated_at) \
             VALUES ('qe-global', 'qe-global', NULL, 'echo', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO quick_apis(id, name, project_id, api_plugin_slug, api_config_id, \
             api_endpoint_path, created_at, updated_at) \
             VALUES ('qa-global', 'qa-global', NULL, 'synthetic-api', 'cfg-b', '/', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO planning_tasks(id, task_number, title, created_at, updated_at) \
             VALUES ('task-global', 9003, 'task-global', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO workflows(id, name, project_id, project_scope_json, trigger_json, steps_json, \
             created_at, updated_at) VALUES ('wf-all', 'wf-all', NULL, '{\"type\":\"All\"}', \
             '{\"type\":\"Manual\"}', '[]', ?1, ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO discussion_sessions(id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (901, 'room-b', 'Codex', 'sess-b', 'peer', 'active', ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO discussion_invite_tokens(token_hash, disc_id, created_at, expires_at) \
             VALUES (?1, 'room-b', ?2, '2999-01-01T00:00:00Z')",
            rusqlite::params![sha256_hex("kr-join-room-b"), now],
        )?;
        for (suffix, number) in [("1", 1), ("2", 2), ("3", 3)] {
            conn.execute(
                "INSERT INTO discussions(id, title, project_id, created_at, updated_at) \
                 VALUES (?1, ?1, 'p2', ?2, ?2)",
                rusqlite::params![
                    format!("p2-newer-{suffix}"),
                    format!("2999-01-0{number}T00:00:00Z")
                ],
            )?;
        }
        kronn::db::disc_source::bind_to_source(conn, "room-b", "Codex", "sess-b-bind")?;
        // The global config the fixture's quick APIs use.
        conn.execute(
            "INSERT INTO mcp_configs(id, server_id, label, is_global) VALUES ('c', 'synthetic-api', 'c', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO discussions(id, title, project_id, created_at, updated_at) \
             VALUES ('room-del', 'room-del', 'p1', ?1, ?1)",
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
        conn.execute(
            "INSERT INTO planning_task_blockers(task_id, blocker_task_id, created_at) \
             VALUES ('task-a', 'task-b', ?1)",
            [&now],
        )?;
        conn.execute(
            "INSERT INTO task_execution_worker_offers(id, task_execution_id, target_cli_session_id, \
             origin_discussion_id, child_discussion_id, created_at, updated_at) \
             VALUES ('offer-b', 'exec-b', 901, 'room-b', 'room-b', ?1, ?1)",
            [&now],
        )?;
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

fn bridge_for(room: &str) -> kronn::core::bridge_token::BridgeTokenGuard {
    mint(BridgeScope {
        discussion_ids: vec![room.into()],
        ..Default::default()
    })
    .unwrap()
}

/// `KT-9002` is project B's task by reference: refused like its id, in the
/// path and in the body; a reference that names nothing is refused too.
#[tokio::test]
async fn task_references_are_resolved_before_the_scope_check() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for (method, path, body) in [
        ("GET", "/api/planning/tasks/KT-9002", None),
        (
            "PATCH",
            "/api/planning/tasks/KT-9002",
            Some(json!({"title": "x"})),
        ),
        ("GET", "/api/planning/tasks/kt-9002", None),
        (
            "POST",
            "/api/planning/tasks/task-a/blockers",
            Some(json!({"blocker_task_id": "KT-9002"})),
        ),
        (
            "POST",
            "/api/orchestration/tool/executions/KT-9002/status",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/orchestration/tool/launch",
            Some(json!({"task_id": "KT-9002"})),
        ),
        ("GET", "/api/planning/tasks/KT-99999", None),
        ("GET", "/api/workflows/no-such-workflow", None),
    ] {
        let (status, response) = call(&app, method, path, Some(&token), body).await;
        assert_eq!(status, 403, "{method} {path}: {response}");
    }
    let (status, response) = call(
        &app,
        "GET",
        "/api/planning/tasks/KT-9001",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "own task by reference: {response}");
}

/// A bridge on p1 naming p2's API config, with no discussion, project or
/// quick API id, is refused before any request leaves.
#[tokio::test]
async fn api_call_runs_for_the_token_s_project_never_the_config_s() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(guard.value()),
        Some(json!({
            "api_plugin_slug": "synthetic-api",
            "api_config_id": "cfg-b",
            "endpoint_path": "/anything",
            "method": "GET"
        })),
    )
    .await;
    assert_eq!(status, 403, "{response}");
    // Named by kind, never by the other project's id (B3-05).
    let error = response["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("McpConfig") && !error.contains("cfg-b"),
        "{response}"
    );
}

/// Lists, searches and lookups show a p1 token none of p2's resources.
#[tokio::test]
async fn lists_and_lookups_only_show_the_token_s_project() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for (path, own, foreign) in [
        ("/api/quick-apis", "qa-a", "qa-b"),
        ("/api/quick-prompts", "qp-a", "qp-b"),
        ("/api/quick-execs", "qe-a", "qe-b"),
        ("/api/workflows", "wf-a", "wf-b"),
        ("/api/planning/tasks", "task-a", "task-b"),
        ("/api/discussions", "room-a2", "room-b"),
        ("/api/disc/search?q=room", "room-a", "room-b"),
    ] {
        let (status, response) = call(&app, "GET", path, Some(&token), None).await;
        assert_eq!(status, 200, "{path}: {response}");
        let text = response.to_string();
        assert!(
            text.contains(own),
            "{path} lost the token's own {own}: {text}"
        );
        assert!(
            !text.contains(foreign),
            "{path} shows p2's {foreign}: {text}"
        );
    }
    // p2's API config: listed for the operator, hidden from the p1 token.
    let (_, operator_view) = call(&app, "GET", "/api/mcps", None, None).await;
    assert!(
        operator_view.to_string().contains("cfg-b"),
        "{operator_view}"
    );
    let (status, token_view) = call(&app, "GET", "/api/mcps", Some(&token), None).await;
    assert_eq!(status, 200);
    assert!(!token_view.to_string().contains("cfg-b"), "{token_view}");
    for id in ["room-b", "task-b", "wf-b", "qp-b"] {
        let (status, response) = call(
            &app,
            "GET",
            &format!("/api/resolve/{id}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, 403, "resolve {id}: {response}");
    }
    let (status, _) = call(&app, "GET", "/api/resolve/task-a", Some(&token), None).await;
    assert_eq!(status, 200);
}

/// Deleting the token's room kills the token, even after a call resolved it.
#[tokio::test]
async fn deleting_the_room_revokes_its_token() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-del");
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-del/meta",
        Some(guard.value()),
        None,
    )
    .await;
    assert_eq!(status, 200);
    db.with_conn(|conn| {
        conn.execute("DELETE FROM discussions WHERE id = 'room-del'", [])?;
        Ok(())
    })
    .await
    .unwrap();
    let (status, _) = call(&app, "GET", "/api/discussions", Some(guard.value()), None).await;
    assert_eq!(status, 401, "the deleted room's token is dead");
}

/// The WebSocket upgrade: a bridge token never opens the event bus, a wrong
/// or expired credential is refused, no credential keeps loopback trust.
#[tokio::test]
async fn the_websocket_refuses_bridge_and_invalid_credentials() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let live = guard.value().to_owned();
    let (status, _) = call(&app, "GET", "/api/ws", Some(&live), None).await;
    assert_eq!(status, 403, "a live bridge token is refused on the bus");
    let (status, _) = call(&app, "GET", &format!("/api/ws?token={live}"), None, None).await;
    assert_eq!(status, 403, "also through the query");
    for bad in ["kbt_expired", "wrong"] {
        let (status, _) = call(&app, "GET", "/api/ws", Some(bad), None).await;
        assert_eq!(status, 401, "{bad}");
        let (status, _) = call(&app, "GET", &format!("/api/ws?token={bad}"), None, None).await;
        assert_eq!(status, 401, "{bad} in the query");
    }
    // No credential, or the operator's: the request reaches the upgrade
    // handler (which refuses a non-upgrade request on its own).
    for bearer in [None, Some("operator-bearer-never-sent-here")] {
        let (status, _) = call(&app, "GET", "/api/ws", bearer, None).await;
        assert!(status != 401 && status != 403, "{bearer:?}: {status}");
    }
}

/// A locked instance (stored auth token not decryptable) refuses bridge
/// tokens everywhere with 423, the recovery routes and the WebSocket included.
#[tokio::test]
async fn a_locked_instance_refuses_bridge_tokens_everywhere() {
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = None;
    config.server.auth_locked = true;
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let app = build_router_with_auth(state, true);
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for (method, path, body) in [
        ("GET", "/api/discussions", None),
        ("POST", "/api/disc/append", Some(append("room-a", "locked"))),
        ("GET", "/api/config/recovery/status", None),
        (
            "POST",
            "/api/config/recovery/restore",
            Some(json!({"passphrase": "x"})),
        ),
        ("GET", "/api/ws", None),
    ] {
        let (status, _) = call(&app, method, path, Some(&token), body).await;
        assert_eq!(status, 423, "{method} {path}");
    }
    let (status, _) = call(&app, "GET", &format!("/api/ws?token={token}"), None, None).await;
    assert_eq!(status, 423, "WS query credential while locked");
    // The local recovery status itself stays reachable without a token.
    let (status, _) = call(&app, "GET", "/api/config/recovery/status", None, None).await;
    assert!(status != 423 && status != 401, "{status}");
}

/// KT-1025 — `step_agents` passes the bridge to the handler without changing
/// what a token may reach: the workflow it names is still scope-checked.
#[tokio::test]
async fn step_agents_on_workflow_trigger_reach_the_handler_and_keep_the_scope() {
    let (app, _repos) = fixture().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        ..Default::default()
    })
    .unwrap();
    let step_agents = json!({"ghost": {"agent": "Codex", "model": "gpt-5"}});
    let (status, body) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(guard.value()),
        Some(json!({"workflow_id": "wf-b", "step_agents": step_agents})),
    )
    .await;
    assert_eq!(
        status, 403,
        "another project's workflow stays refused: {body}"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(guard.value()),
        Some(json!({"workflow_id": "wf-a", "step_agents": step_agents})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], false, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("no step `ghost`"),
        "the handler saw the field: {body}"
    );
}

/// KT-918 — the script status endpoint is for the step editor only, and an
/// agent's save never approves script content: an empty hash stays empty
/// until a human saves the workflow.
#[tokio::test]
async fn a_bridge_token_never_approves_a_repository_script() {
    let (app, repos) = fixture().await;
    let script = repos.path().join("p1/scripts/run.cjs");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "console.log('x');\n").unwrap();
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();

    let (status, body) = call(
        &app,
        "POST",
        "/api/workflows/exec-scripts/status",
        Some(&token),
        Some(json!({"project_id": "p1", "files": [{"path": "scripts/run.cjs", "sha256": ""}]})),
    )
    .await;
    assert_eq!(
        status, 403,
        "the status endpoint is out of bridge reach: {body}"
    );

    let workflow = json!({
        "name": "framing", "project_id": "p1", "trigger": {"type": "Manual"},
        "exec_allowlist": ["node"],
        "steps": [{"name": "run", "step_type": {"type": "Exec"}, "exec_command": "node",
            "exec_args": ["scripts/run.cjs"],
            "exec_script_files": [{"path": "scripts/run.cjs", "sha256": ""}]}]
    });
    let (status, agent) = call(
        &app,
        "POST",
        "/api/workflows",
        Some(&token),
        Some(workflow.clone()),
    )
    .await;
    assert_eq!(status, 200, "{agent}");
    assert_eq!(agent["success"], true, "{agent}");
    assert_eq!(
        agent["data"]["steps"][0]["exec_script_files"][0]["sha256"], "",
        "an agent's save leaves the hash empty"
    );

    let id = agent["data"]["id"].as_str().unwrap().to_owned();
    let (status, updated) = call(
        &app,
        "PUT",
        &format!("/api/workflows/{id}"),
        Some(&token),
        Some(json!({"steps": workflow["steps"]})),
    )
    .await;
    assert_eq!(status, 200, "{updated}");
    assert_eq!(
        updated["data"]["steps"][0]["exec_script_files"][0]["sha256"],
        ""
    );

    let (_, human) = call(&app, "POST", "/api/workflows", None, Some(workflow)).await;
    let pinned = human["data"]["steps"][0]["exec_script_files"][0]["sha256"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(pinned.len(), 64, "a human save pins the content: {human}");
}

// ─── Layer B completeness review (review-layer-b2) ──────────────────────────

async fn raw_call(
    app: &Router,
    method: &str,
    path: &str,
    bearer: &str,
    content_type: &str,
    body: &str,
) -> (u16, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", content_type)
        .body(Body::from(body.to_owned()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40404))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The handler refused the config for the token's scope (not some later,
/// unrelated failure).
fn refused_for_scope(response: &Value) -> bool {
    response["success"] == false
        && response["error"]
            .as_str()
            .is_some_and(|error| error.contains("not available to this agent's project"))
}

async fn query_one(
    db: &Arc<kronn::db::Database>,
    sql: &'static str,
    arg: String,
) -> Option<String> {
    db.with_conn(move |conn| {
        Ok(rusqlite::OptionalExtension::optional(
            conn.query_row(sql, [&arg], |row| row.get::<_, Option<String>>(0)),
        )?
        .flatten())
    })
    .await
    .unwrap()
}

/// B-01 / B-02 — an id nested in a workflow body, a step or `on_failure`,
/// naming another project's resource, is refused on create and update.
#[tokio::test]
async fn nested_step_references_to_another_project_are_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for step in [
        json!({"sub_workflow_id": "wf-b"}),
        json!({"room_id": "room-b"}),
        json!({"quick_prompt_id": "qp-b"}),
        json!({"batch_quick_prompt_id": "qp-b"}),
        json!({"quick_api_id": "qa-b"}),
        json!({"api_config_id": "cfg-b"}),
        json!({"name": "s", "on_failure": [{"quick_prompt_id": "qp-b"}]}),
    ] {
        let body = json!({"name": "x", "steps": [step.clone()]});
        let (status, response) = call(
            &app,
            "PUT",
            "/api/workflows/wf-a",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, 403, "update with {step}: {response}");
        let (status, response) =
            call(&app, "POST", "/api/workflows", Some(&token), Some(body)).await;
        assert_eq!(status, 403, "create with {step}: {response}");
    }
}

/// B-16 — the workflow an import carries is walked too.
#[tokio::test]
async fn an_import_naming_another_project_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    for workflow in [
        json!({"id": "w", "steps": [{"sub_workflow_id": "wf-b"}]}),
        json!({"id": "w", "steps": [], "project_scope": {"type": "Projects", "project_ids": ["p2"]}}),
        json!({"id": "w", "steps": [], "project_scope": {"type": "All"}}),
    ] {
        let content = json!({"kind": "kronn.workflow", "workflow": workflow}).to_string();
        let (status, response) = call(
            &app,
            "POST",
            "/api/workflows/import",
            Some(guard.value()),
            Some(json!({"content": content})),
        )
        .await;
        assert_eq!(status, 403, "{response}");
    }
}

/// B-03 — a workflow scope may name the token's project only.
#[tokio::test]
async fn a_workflow_scope_beyond_the_token_s_project_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    for scope in [
        json!({"type": "All"}),
        json!({"type": "Projects", "project_ids": ["p2"]}),
        json!({"type": "Projects", "project_ids": ["p1", "p2"]}),
    ] {
        let (status, response) = call(
            &app,
            "PUT",
            "/api/workflows/wf-a",
            Some(guard.value()),
            Some(json!({"project_scope": scope})),
        )
        .await;
        assert_eq!(status, 403, "{response}");
    }
    let (status, response) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        Some(guard.value()),
        Some(json!({"project_scope": {"type": "Projects", "project_ids": ["p1"]}})),
    )
    .await;
    assert_ne!(status, 403, "its own project: {response}");
}

/// B-04 / F-02 — shared resources are read, never written.
#[tokio::test]
async fn shared_resources_are_read_but_never_written() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for (method, path, body) in [
        (
            "PUT",
            "/api/quick-prompts/qp-global",
            Some(json!({"name": "x"})),
        ),
        ("DELETE", "/api/quick-prompts/qp-global", None),
        (
            "PUT",
            "/api/quick-apis/qa-global",
            Some(json!({"name": "x"})),
        ),
        (
            "PUT",
            "/api/quick-execs/qe-global",
            Some(json!({"name": "x"})),
        ),
        (
            "PATCH",
            "/api/planning/tasks/task-global",
            Some(json!({"title": "x"})),
        ),
        ("PUT", "/api/workflows/wf-all", Some(json!({"name": "x"}))),
        (
            "PUT",
            "/api/workflows/wf-global",
            Some(json!({"name": "x"})),
        ),
    ] {
        let (status, response) = call(&app, method, path, Some(&token), body).await;
        assert_eq!(status, 403, "{method} {path}: {response}");
    }
    for path in [
        "/api/workflows/wf-all",
        "/api/workflows/wf-global",
        "/api/planning/tasks/task-global",
    ] {
        let (status, response) = call(&app, "GET", path, Some(&token), None).await;
        assert_eq!(status, 200, "GET {path}: {response}");
    }
}

/// B-05 — a shared Quick API runs for the token's project, so a config only
/// another project sees is refused; a shared Quick Exec runs for the token's
/// project, whatever the body says.
#[tokio::test]
async fn shared_quick_runs_happen_for_the_token_s_project_only() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/quick-apis/qa-global/run",
        Some(guard.value()),
        Some(json!({"project_id": "p1", "variables": {}})),
    )
    .await;
    assert!(
        status == 403 || refused_for_scope(&response),
        "a config only p2 sees must not run: {status} {response}"
    );
    let (status, response) = call(
        &app,
        "POST",
        "/api/quick-execs/qe-global/run",
        Some(guard.value()),
        Some(json!({"variables": {}})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let run_id = response["data"]["run_id"].as_str().unwrap().to_owned();
    let project = query_one(
        &db,
        "SELECT project_id FROM shared_runs WHERE id = ?1",
        run_id,
    )
    .await;
    assert_eq!(
        project.as_deref(),
        Some("p1"),
        "the run belongs to the token's project"
    );
}

/// B-06 / F-03 — a project-less token (a General discussion's).
#[tokio::test]
async fn a_project_less_token_cannot_reach_project_resources_through_shared_ones() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-g");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "POST",
        "/api/quick-apis/qa-global/run",
        Some(&token),
        Some(json!({"variables": {}})),
    )
    .await;
    assert!(
        status == 403 || refused_for_scope(&response),
        "a project-only config: {status} {response}"
    );
    let (status, _) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(&token),
        Some(json!({"workflow_id": "wf-all"})),
    )
    .await;
    assert_eq!(status, 403, "an every-project workflow");
    let (status, _) = call(
        &app,
        "PUT",
        "/api/workflows/wf-all",
        Some(&token),
        Some(json!({"name": "x"})),
    )
    .await;
    assert_eq!(status, 403);
}

/// B-07 — created resources land in the token's project; a move out of it is
/// refused.
#[tokio::test]
async fn created_resources_land_in_the_token_s_project() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "POST",
        "/api/quick-prompts",
        Some(&token),
        Some(json!({"name": "created", "prompt_template": "x"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let id = response["data"]["id"].as_str().unwrap().to_owned();
    let project = query_one(
        &db,
        "SELECT project_id FROM quick_prompts WHERE id = ?1",
        id,
    )
    .await;
    assert_eq!(project.as_deref(), Some("p1"));
    let (status, response) = call(
        &app,
        "POST",
        "/api/planning/tasks",
        Some(&token),
        Some(json!({"title": "created task"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let id = response["data"]["id"].as_str().unwrap().to_owned();
    let project = query_one(
        &db,
        "SELECT project_id FROM planning_task_projects WHERE task_id = ?1",
        id,
    )
    .await;
    assert_eq!(project.as_deref(), Some("p1"));
    let (status, _) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        Some(&token),
        Some(json!({"project_id": null})),
    )
    .await;
    assert_eq!(status, 403);
}

/// B-08 — a task cannot be attached under another project's task.
#[tokio::test]
async fn a_task_parent_in_another_project_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, _) = call(
        &app,
        "POST",
        "/api/planning/tasks",
        Some(&token),
        Some(json!({"title": "t", "parent_id": "task-b"})),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-a",
        Some(&token),
        Some(json!({"parent_id": "task-b"})),
    )
    .await;
    assert_eq!(status, 403);
}

/// B-09 — the shared agent library is read only for a token.
#[tokio::test]
async fn the_agent_library_is_read_only_for_a_token() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for (method, path) in [
        ("PUT", "/api/skills/any-skill"),
        ("DELETE", "/api/skills/any-skill"),
        ("POST", "/api/profiles"),
        ("DELETE", "/api/directives/any-directive"),
    ] {
        let (status, _) = call(&app, method, path, Some(&token), Some(json!({}))).await;
        assert_eq!(status, 403, "{method} {path}");
    }
    let (status, _) = call(&app, "GET", "/api/skills", Some(&token), None).await;
    assert_eq!(status, 200);
}

/// B-10 — a session or invite of another project's room is refused (a
/// deliberate rule, design §9).
#[tokio::test]
async fn sessions_and_invites_cannot_cross_projects() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "POST",
        "/api/discussions/peer-leave",
        Some(&token),
        Some(json!({"agent_type": "Codex", "session_id": "sess-b"})),
    )
    .await;
    assert_eq!(status, 403, "{response}");
    let (status, response) = call(
        &app,
        "POST",
        "/api/discussions/peer-join",
        Some(&token),
        Some(json!({"token": "kr-join-room-b", "agent_type": "Codex", "session_id": "s"})),
    )
    .await;
    assert_eq!(status, 403, "{response}");
    let (status, _) = call(
        &app,
        "POST",
        "/api/discussions/peer-resume",
        Some(&token),
        Some(json!({"agent_type": "Codex", "session_id": "s", "resume_token": "unknown"})),
    )
    .await;
    assert_eq!(status, 403, "an unresolvable credential");
}

/// B-11 — accepting an offer of another project's execution is refused.
#[tokio::test]
async fn an_offer_of_another_project_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/orchestration/accept-offer",
        Some(guard.value()),
        Some(json!({"offer_id": "offer-b", "source_agent": "Codex",
            "source_session_id": "s", "source_binding_session_id": "s"})),
    )
    .await;
    assert_eq!(status, 403, "{response}");
}

/// B-12 / F-04 — non-GET responses are scoped: a replay returning another
/// project's discussion is refused, another project's blocker is dropped.
#[tokio::test]
async fn write_responses_are_scoped_too() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "POST",
        "/api/disc/create",
        Some(&token),
        Some(
            json!({"title": "x", "agent": "Codex", "source_agent": "Codex",
            "source_session_id": "sess-b-bind"}),
        ),
    )
    .await;
    assert_eq!(status, 403, "replay of room-b: {response}");
    let (status, response) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-a",
        Some(&token),
        Some(json!({"title": "renamed"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert!(
        !response.to_string().contains("task-b"),
        "p2's blocker leaked: {response}"
    );
}

/// B-14 — resolving another project's config is refused.
#[tokio::test]
async fn resolving_another_project_s_config_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, _) = call(&app, "GET", "/api/resolve/cfg-b", Some(guard.value()), None).await;
    assert_eq!(status, 403);
}

/// B-15 — the project filter is in the list query: newer rows of another
/// project do not push the token's own off the first page.
#[tokio::test]
async fn a_bridge_list_page_is_filtered_in_the_query() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "GET",
        "/api/discussions?page=1&per_page=2",
        Some(guard.value()),
        None,
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let page = response["data"].as_array().unwrap();
    assert_eq!(page.len(), 2, "a full page of in-scope rows: {response}");
    assert!(
        page.iter().all(|item| item["project_id"] == "p1"),
        "{response}"
    );
}

/// B-17 — a token cannot move its own room out of its project.
#[tokio::test]
async fn a_token_cannot_move_its_room_out_of_its_project() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/discussions/room-a",
        Some(&token),
        Some(json!({"project_id": null})),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a2/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "still bound to p1");
}

/// B-18 — a task reference of another project is refused before prepare.
#[tokio::test]
async fn a_task_reference_of_another_project_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, _) = call(
        &app,
        "POST",
        "/api/orchestration/tool/prepare",
        Some(guard.value()),
        Some(json!({"task_reference": "KT-9002"})),
    )
    .await;
    assert_eq!(status, 403);
}

/// B-19 — a declared path parameter that cannot be decoded is refused.
#[tokio::test]
async fn an_undecodable_path_parameter_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "GET",
        "/api/workflows/%FF%FE",
        Some(guard.value()),
        None,
    )
    .await;
    assert_eq!(status, 400, "{response}");
    assert_eq!(
        response["error"], "invalid path parameters",
        "refused by the gate, before any handler"
    );
}

/// C-01 — a General discussion is private to its own launches.
#[tokio::test]
async fn a_general_discussion_is_private_to_its_launches() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for path in [
        "/api/discussions/room-g/meta",
        "/api/disc/load_other?discussion_id=room-g",
    ] {
        let (status, _) = call(&app, "GET", path, Some(&token), None).await;
        assert_eq!(status, 403, "{path}");
    }
    let (_, list) = call(&app, "GET", "/api/discussions", Some(&token), None).await;
    assert!(!list.to_string().contains("room-g"), "{list}");
    let own = bridge_for("room-g");
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-g/meta",
        Some(own.value()),
        None,
    )
    .await;
    assert_eq!(status, 200, "its own launch reads it");
}

/// E-01 — a launch that owns nothing reads catalogues only.
#[tokio::test]
async fn a_token_owning_nothing_reads_catalogues_only() {
    let (app, _repos) = fixture().await;
    let guard = mint(BridgeScope::default()).unwrap();
    let (status, _) = call(&app, "GET", "/api/discussions", Some(guard.value()), None).await;
    assert_eq!(status, 403);
    let (status, _) = call(&app, "GET", "/api/skills", Some(guard.value()), None).await;
    assert_eq!(status, 200);
}

/// E-04 — a scope spanning two projects has no binding.
#[tokio::test]
async fn a_scope_spanning_projects_is_dead() {
    let (app, _repos) = fixture().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into(), "room-b".into()],
        ..Default::default()
    })
    .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        Some(guard.value()),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

/// F-07 — a non-JSON body is not a way around the body checks.
#[tokio::test]
async fn a_non_json_body_is_refused() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let body = json!({"disc_id": "room-b", "messages": []}).to_string();
    let (status, response) = raw_call(
        &app,
        "POST",
        "/api/disc/append",
        guard.value(),
        "text/plain",
        &body,
    )
    .await;
    assert_eq!(status, 415, "{response}");
    assert!(
        response.contains("a bridge-token request body must be JSON"),
        "refused by the gate, before any handler: {response}"
    );
}
