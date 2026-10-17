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
                "INSERT INTO workflow_runs(id, workflow_id, project_id, started_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![format!("run-{suffix}"), format!("wf-{suffix}"), project, now],
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
        ("GET", "/api/disc/load_other?disc_id=room-b", None),
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

/// KT-1107 — the readiness probe spawns agents: a launched agent's bridge
/// token cannot call it, even for its own project; the operator still can.
#[tokio::test]
async fn a_bridge_token_cannot_spawn_agents_through_the_readiness_probe() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let body = json!({ "project_id": "p1", "agents": ["Ollama"] });
    let (status, json) = call(
        &app,
        "POST",
        "/api/agents/readiness",
        Some(guard.value()),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 403, "{json}");
    let (status, json) = call(&app, "POST", "/api/agents/readiness", None, Some(body)).await;
    assert_eq!(status, 200, "{json}");
    assert_eq!(json["data"][0]["reason"], "not_probed");
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

/// The token of a room's own agent: the room's grants (KT-1270) reach it.
fn bridge_for(room: &str) -> kronn::core::bridge_token::BridgeTokenGuard {
    mint(BridgeScope {
        discussion_ids: vec![room.into()],
        room_grant_discussions: vec![room.into()],
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
        "/api/disc/load_other?disc_id=room-g",
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
    let (status, _) = call(
        &app,
        "GET",
        "/api/disc/load_other?disc_id=room-g",
        Some(own.value()),
        None,
    )
    .await;
    assert_eq!(status, 200, "the key the handler reads is the one checked");
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

/// The bridge sends `Content-Type: application/json` on every request, a
/// body-less GET included (`_http` in disc-introspection-mcp.py).
#[tokio::test]
async fn a_body_less_get_labelled_json_reaches_its_handler() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let (status, response) = raw_call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        guard.value(),
        "application/json",
        "",
    )
    .await;
    assert_eq!(status, 200, "{response}");
    // A labelled body that is not JSON is still refused.
    let (status, response) = raw_call(
        &app,
        "POST",
        "/api/disc/append",
        guard.value(),
        "application/json",
        "",
    )
    .await;
    assert_eq!(status, 400, "{response}");
}

// ─── Layer B round 3 (review-layer-b3) ──────────────────────────────────────

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<&str>,
) -> (u16, String) {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_owned())))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40404))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Default deny — an invented id field naming another project's resource is
/// refused, in the body at any depth and in the query.
#[tokio::test]
async fn an_unknown_id_field_is_refused_for_a_token() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for body in [
        json!({"name": "x", "foo_id": "wf-b"}),
        json!({"name": "x", "steps": [{"name": "s", "widget": {"foo_id": "room-b"}}]}),
    ] {
        let (status, response) =
            call(&app, "PUT", "/api/workflows/wf-a", Some(&token), Some(body)).await;
        assert_eq!(status, 403, "{response}");
        assert!(!response.to_string().contains("room-b"), "{response}");
    }
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions?foo_id=room-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403);
}

/// B3-01 — an import step's own `id`, or an object's `id`, equal to another
/// project's resource never hides the reference that names it.
#[tokio::test]
async fn an_import_cannot_hide_a_reference_behind_an_internal_id() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    for workflow in [
        json!({"id": "w", "steps": [{"id": "wf-b", "name": "s", "sub_workflow_id": "wf-b"}]}),
        json!({"id": "w", "steps": [{"id": "s", "name": "s", "room_id": "room-b",
            "options": [{"id": "room-b"}]}]}),
    ] {
        let content = json!({"kind": "kronn.workflow", "version": 2, "workflow": workflow,
            "referenced_workflows": [], "referenced_quick_prompts": []})
        .to_string();
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

fn collect_step(quick_exec_id: &str) -> Value {
    json!({"name": "collect", "step_type": {"type": "CollectApiData"},
        "collect_api_data": {"sources": [{"alias": "x", "quick_exec_id": quick_exec_id}]}})
}

/// B3-02 — a saved Quick Exec of another project, named by a token on update
/// or import, is refused at the gate; an import by the operator is refused by
/// the handler.
#[tokio::test]
async fn another_project_s_quick_exec_is_refused_in_a_workflow() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let body = json!({"name": "x", "exec_allowlist": ["echo"], "steps": [collect_step("qe-b")]});
    let (status, response) =
        call(&app, "PUT", "/api/workflows/wf-a", Some(&token), Some(body)).await;
    assert_eq!(status, 403, "{response}");
    // A real export, rewritten to name p2's saved Quick Exec.
    let (status, mut envelope) = call(&app, "GET", "/api/workflows/wf-a/export", None, None).await;
    assert_eq!(status, 200, "{envelope}");
    // A pre-0.10 bundle keeps refs it does not carry: the legacy path.
    envelope["version"] = json!(2);
    envelope["workflow"]["exec_allowlist"] = json!(["echo"]);
    envelope["workflow"]["steps"] = json!([collect_step("qe-b")]);
    let content = envelope.to_string();
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows/import",
        Some(&token),
        Some(json!({"content": content.clone()})),
    )
    .await;
    assert_eq!(status, 403, "{response}");
    // No token: the handler's own rule, the same as on create.
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows/import",
        None,
        Some(json!({"content": content, "project_id": "p1"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["success"], false, "{response}");
    assert!(
        response["error"]
            .as_str()
            .is_some_and(|error| error.contains("autre projet")),
        "{response}"
    );
}

/// B3-03 — a token's media generation without a discussion lands in a new
/// discussion of the token's project, which the token then owns.
#[tokio::test]
async fn a_token_s_media_discussion_lands_in_its_project() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        kronn::db::external_api_connections::insert(
            conn,
            &kronn::models::ExternalApiConnection {
                id: "media-conn".into(),
                display_name: "Media".into(),
                mention_alias: "media".into(),
                endpoint: Some("http://127.0.0.1:1".into()),
                credential_slug: "media".into(),
                origin_preset: kronn::models::ExternalApiConnectionPreset::OpenRouter,
                economy_model: None,
                default_model: None,
                reasoning_model: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                image_model: Some("stub/image".into()),
                video_model: None,
                media_endpoint: None,
            },
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "POST",
        "/api/media/generate",
        Some(&token),
        Some(json!({"modality": "image", "prompt": "a lighthouse"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let created = response["data"]["discussion_id"]
        .as_str()
        .expect("a discussion")
        .to_owned();
    let project = query_one(
        &db,
        "SELECT project_id FROM discussions WHERE id = ?1",
        created.clone(),
    )
    .await;
    assert_eq!(project.as_deref(), Some("p1"));
    let job_project = query_one(
        &db,
        "SELECT project_id FROM media_jobs WHERE discussion_id = ?1",
        created.clone(),
    )
    .await;
    assert_eq!(job_project.as_deref(), Some("p1"));
    let (status, meta) = call(
        &app,
        "GET",
        &format!("/api/discussions/{created}/meta"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "{meta}");
}

/// B3-04 / B3-05 / B-10 — every route reaching a discussion through a
/// caller-supplied session resolves it first: a p2 session is refused, nothing
/// is written, and the refusal never names the p2 room.
#[tokio::test]
async fn session_keyed_routes_refuse_another_project_s_session() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let joined = json!({"source_agent": "Codex", "source_session_id": "sess-b"});
    let bound = json!({"source_agent": "Codex", "source_session_id": "sess-b-bind"});
    let merge = |base: &Value, extra: Value| {
        let mut merged = base.clone();
        merged
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        merged
    };
    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        (
            "POST",
            "/api/discussions/peer-join",
            Some(json!({"token": "kr-join-room-b", "agent_type": "Codex", "session_id": "x"})),
        ),
        (
            "POST",
            "/api/discussions/peer-leave",
            Some(json!({"agent_type": "Codex", "session_id": "sess-b"})),
        ),
        (
            "POST",
            "/api/discussions/peer-resume",
            Some(json!({"agent_type": "Codex", "session_id": "sess-b", "resume_token": "x"})),
        ),
        (
            "POST",
            "/api/discussions/orchestrator-return-resume",
            Some(json!({"agent_type": "Codex", "session_id": "sess-b", "resume_token": "x"})),
        ),
        (
            "POST",
            "/api/disc/workspace",
            Some(merge(&joined, json!({"workspace_path": "/tmp/x"}))),
        ),
        (
            "GET",
            "/api/disc/workspace?source_agent=Codex&source_session_id=sess-b",
            None,
        ),
        (
            "POST",
            "/api/disc/workspace/history-lease",
            Some(merge(&joined, json!({"action": "acquire"}))),
        ),
        (
            "POST",
            "/api/disc/link",
            Some(merge(&bound, json!({"disc_id": "room-a"}))),
        ),
        (
            "POST",
            "/api/disc/link",
            Some(merge(
                &bound,
                json!({"disc_id": "room-a", "force_reassign": true}),
            )),
        ),
        (
            "POST",
            "/api/disc/unlink",
            Some(merge(&bound, json!({"disc_id": "room-a"}))),
        ),
        (
            "POST",
            "/api/disc/transfer-session",
            Some(merge(
                &bound,
                json!({"from_disc_id": "room-a", "to_disc_id": "room-a2", "confirm_transfer": true}),
            )),
        ),
        (
            "POST",
            "/api/orchestration/accept-offer",
            Some(merge(&joined, json!({"offer_id": "offer-b"}))),
        ),
        (
            "GET",
            "/api/disc/find_by_session?source_agent=Codex&source_session_id=sess-b-bind",
            None,
        ),
        (
            "GET",
            "/api/disc/session-status?source_agent=Codex&source_session_id=sess-b-bind",
            None,
        ),
    ];
    let mut covered = std::collections::HashSet::new();
    for (method, path, body) in cases {
        let (status, response) = call(&app, method, path, Some(&token), body.clone()).await;
        assert_eq!(status, 403, "{method} {path} {body:?}: {response}");
        assert!(
            !response.to_string().contains("room-b"),
            "{path} names the other room: {response}"
        );
        covered.insert((method, path.split('?').next().unwrap()));
    }
    for (method, pattern) in kronn::core::bridge_token::SESSION_KEYED_ROUTES {
        assert!(
            covered.contains(&(*method, *pattern)),
            "{method} {pattern} has no p2-session case"
        );
    }
    // An unknown session reaches no room: refused on a write.
    let (status, _) = call(
        &app,
        "POST",
        "/api/disc/workspace",
        Some(&token),
        Some(
            json!({"source_agent": "Codex", "source_session_id": "nobody",
            "workspace_path": "/tmp/x"}),
        ),
    )
    .await;
    assert_eq!(status, 403);
    let written = query_one(
        &db,
        "SELECT CAST(COUNT(*) AS TEXT) FROM discussion_workspaces WHERE disc_id = ?1",
        "room-b".into(),
    )
    .await;
    assert_eq!(written.as_deref(), Some("0"));
    let binding = query_one(
        &db,
        "SELECT disc_id FROM disc_source_history \
         WHERE source_session_id = ?1 AND unlinked_at IS NULL",
        "sess-b-bind".into(),
    )
    .await;
    assert_eq!(binding.as_deref(), Some("room-b"), "binding unchanged");
}

/// B3-04 — a task the workspace is set for must be the token's.
#[tokio::test]
async fn a_workspace_task_ref_of_another_project_is_refused() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussion_sessions(id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (902, 'room-a', 'Codex', 'sess-a', 'peer', 'active', '2026-01-01T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    for task in ["task-b", "KT-9002"] {
        let (status, response) = call(
            &app,
            "POST",
            "/api/disc/workspace",
            Some(guard.value()),
            Some(
                json!({"source_agent": "Codex", "source_session_id": "sess-a",
                "workspace_path": "/tmp/x", "task_ref": task}),
            ),
        )
        .await;
        assert_eq!(status, 403, "{task}: {response}");
    }
}

/// B3-06 — a body that is not an `ApiResponse` envelope is scoped too, and an
/// export bundling another project's dependency is refused whole.
#[tokio::test]
async fn an_export_bundling_another_project_s_dependency_is_refused() {
    let (app, _repos, db) = fixture_with_db().await;
    let token_guard = bridge_for("room-a");
    let token = token_guard.value().to_owned();
    let (status, body) = send(
        &app,
        "GET",
        "/api/workflows/wf-a/export",
        &[("authorization", format!("Bearer {token}"))],
        None,
    )
    .await;
    assert_eq!(status, 200, "a self-contained export: {body}");
    for steps in [
        json!([{"name": "child", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "wf-b"}]),
        json!([collect_step("qe-b")]),
        json!([{"name": "qp", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-b"}]),
    ] {
        let steps_json = steps.to_string();
        db.with_conn(move |conn| {
            conn.execute(
                "UPDATE workflows SET steps_json = ?1 WHERE id = 'wf-a'",
                [&steps_json],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let (status, body) = send(
            &app,
            "GET",
            "/api/workflows/wf-a/export",
            &[("authorization", format!("Bearer {token}"))],
            None,
        )
        .await;
        assert_eq!(status, 403, "{steps}: {body}");
        assert!(!body.contains("qe-b") && !body.contains("qp-b"), "{body}");
    }
}

/// B3-07 / B3-08 — a bridge token is taken by the gate whatever the scheme's
/// case, and never reaches a route answered before it.
#[tokio::test]
async fn a_bridge_token_never_bypasses_the_gate() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for scheme in ["bearer", "BEARER", "Bearer"] {
        let (status, _) = send(
            &app,
            "GET",
            "/api/config/export",
            &[("authorization", format!("{scheme} {token}"))],
            None,
        )
        .await;
        assert_eq!(status, 403, "{scheme}: an off-list route");
        let (status, _) = send(
            &app,
            "GET",
            "/api/discussions/room-a/meta",
            &[("authorization", format!("{scheme} {token}"))],
            None,
        )
        .await;
        assert_eq!(status, 200, "{scheme}: its own room");
    }
    for header in [
        format!("Basic {token}"),
        format!("Token {token}"),
        token.clone(),
    ] {
        let (status, _) = send(
            &app,
            "GET",
            "/api/discussions/room-a/meta",
            &[("authorization", header.clone())],
            None,
        )
        .await;
        assert_eq!(status, 403, "{header}");
    }
    for path in ["/api/disc/claim-by-token", "/api/disc/fetch-file"] {
        let (status, _) = send(
            &app,
            "POST",
            path,
            &[
                ("authorization", format!("bearer {token}")),
                ("content-type", "application/json".into()),
            ],
            Some("{}"),
        )
        .await;
        assert_eq!(status, 403, "{path}");
    }
}

/// B3-14 — a token's learning proposal is its project's; a preference (every
/// project) is refused.
#[tokio::test]
async fn a_learning_proposal_is_forced_into_the_token_s_project() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let proposal = |kind: &str, project: Option<&str>| {
        let mut body = json!({"claim": "c", "kind": kind,
            "evidence": [{"kind": "user", "ref": "2026-10-05 the user said so"}]});
        if let Some(project) = project {
            body["project_id"] = json!(project);
        }
        body
    };
    let (status, _) = call(
        &app,
        "POST",
        "/api/learnings/propose",
        Some(&token),
        Some(proposal("preference", None)),
    )
    .await;
    assert_eq!(status, 403, "a preference");
    let (status, _) = call(
        &app,
        "POST",
        "/api/learnings/propose",
        Some(&token),
        Some(proposal("fact", Some("p2"))),
    )
    .await;
    assert_eq!(status, 403, "another project");
    let (status, response) = call(
        &app,
        "POST",
        "/api/learnings/propose",
        Some(&token),
        Some(proposal("fact", None)),
    )
    .await;
    assert_eq!(status, 200, "forced into p1: {response}");
}

/// B3-18 — the bound project is frozen at first use: moving the room to
/// another project kills the token.
#[tokio::test]
async fn a_room_moved_to_another_project_kills_its_token() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE discussions SET project_id = 'p2' WHERE id = 'room-a'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let (status, response) = call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 401, "{response}");
    assert!(
        response.to_string().contains("project changed"),
        "{response}"
    );
}

/// B3-19 — a write without a Content-Type is refused, never forwarded unread.
#[tokio::test]
async fn a_write_without_a_content_type_is_refused() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let body = append("room-b", "no-content-type").to_string();
    let (status, response) = send(
        &app,
        "POST",
        "/api/disc/append",
        &[("authorization", format!("Bearer {}", guard.value()))],
        Some(&body),
    )
    .await;
    // Refused by the gate itself, not left to whatever the handler extracts.
    assert_eq!(status, 415, "{response}");
    assert!(
        response.contains("a bridge-token request body must be JSON"),
        "{response}"
    );
    let appended = query_one(
        &db,
        "SELECT CAST(COUNT(*) AS TEXT) FROM messages WHERE discussion_id = ?1",
        "room-b".into(),
    )
    .await;
    assert_eq!(appended.as_deref(), Some("0"));
}

// ─── Layer B round 4 (review-layer-b4) ──────────────────────────────────────

/// B4-02 — the plugin overview shows a token only what its project may use.
#[tokio::test]
async fn the_plugin_overview_is_scoped_to_the_token_s_project() {
    let (app, repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        for (id, general) in [("cfg-general", 1), ("cfg-unlinked", 0), ("cfg-a", 0)] {
            conn.execute(
                "INSERT INTO mcp_configs(id, server_id, label, include_general) \
                 VALUES (?1, 'synthetic-api', ?1, ?2)",
                rusqlite::params![id, general],
            )?;
        }
        conn.execute(
            "INSERT INTO mcp_config_projects(config_id, project_id) VALUES ('cfg-a', 'p1')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    for (project, slug) in [("p2", "cfg-b"), ("p2", "c"), ("p1", "c")] {
        let path = repos.path().join(project);
        kronn::core::mcp_scanner::write_mcp_context(
            path.to_str().unwrap(),
            slug,
            "A rule written for this project.",
        )
        .unwrap();
    }
    let guard = bridge_for("room-a");
    let (status, response) = call(&app, "GET", "/api/mcps", Some(guard.value()), None).await;
    assert_eq!(status, 200, "{response}");
    let mut ids: Vec<&str> = response["data"]["configs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|config| config["id"].as_str().unwrap())
        .collect();
    ids.sort();
    assert_eq!(ids, ["c", "cfg-a"], "{response}");
    let contexts = response["data"]["customized_contexts"].to_string();
    assert!(!contexts.contains(":p2"), "{contexts}");
    assert!(contexts.contains("c:p1"), "{contexts}");
    assert!(!response.to_string().contains("\"p2\""), "{response}");
}

/// B4-03 — the workflows feeding a page are scoped like any workflow list.
#[tokio::test]
async fn a_page_s_feeding_workflows_are_scoped() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_pages(id, project_id, title, slug, current_revision_id, \
             created_at, updated_at) VALUES ('page-shared', NULL, 'shared', 'page-shared', \
             'rev-shared', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )?;
        conn.execute(
            "INSERT INTO live_page_revisions(id, page_id, revision, html, created_by_agent, \
             created_at) VALUES ('rev-shared', 'page-shared', 1, '<p></p>', NULL, \
             '2026-01-01T00:00:00Z')",
            [],
        )?;
        let steps = json!([{"name": "publish", "step_type": {"type": "PublishPageData"},
            "page_publish": {"page_id": "page-shared", "writes": []}}])
        .to_string();
        conn.execute(
            "UPDATE workflows SET steps_json = ?1 WHERE id IN ('wf-a', 'wf-b')",
            [&steps],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let (_, all) = call(&app, "GET", "/api/pages/page-shared/workflows", None, None).await;
    assert_eq!(all["data"].as_array().map(Vec::len), Some(2), "{all}");
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "GET",
        "/api/pages/page-shared/workflows",
        Some(guard.value()),
        None,
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let ids: Vec<&str> = response["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workflow| workflow["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["wf-a"], "{response}");
}

/// B4-06 — a discussion the token created and a human moved kills the token.
#[tokio::test]
async fn an_adopted_discussion_moved_to_another_project_kills_the_token() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, created) = call(
        &app,
        "POST",
        "/api/disc/create",
        Some(&token),
        Some(json!({"title": "child", "agent": "ClaudeCode"})),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let child = created["data"]["disc_id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &app,
        "POST",
        "/api/disc/append",
        Some(&token),
        Some(append(&child, "one")),
    )
    .await;
    assert_eq!(status, 200);
    let moved = child.clone();
    db.with_conn(move |conn| {
        conn.execute(
            "UPDATE discussions SET project_id = 'p2' WHERE id = ?1",
            [&moved],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let (status, response) = call(
        &app,
        "POST",
        "/api/disc/append",
        Some(&token),
        Some(append(&child, "two")),
    )
    .await;
    assert_eq!(status, 401, "{response}");
}

/// B4-06 — with several scope discussions, deleting any one kills the token.
#[tokio::test]
async fn deleting_any_scope_discussion_kills_the_token() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into(), "room-a2".into()],
        ..Default::default()
    })
    .unwrap();
    let token = guard.value().to_owned();
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    db.with_conn(|conn| {
        conn.execute("DELETE FROM discussions WHERE id = 'room-a2'", [])?;
        Ok(())
    })
    .await
    .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-a/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

/// B4-07 — a token's planning write is recorded as an agent's.
#[tokio::test]
async fn a_token_s_planning_write_is_an_agent_s() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-a",
        Some(guard.value()),
        Some(json!({"title": "renamed", "actor": {"kind": "human", "id": "romu"}})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["success"], true, "{response}");
    let kind = query_one(
        &db,
        "SELECT actor_kind FROM planning_task_events WHERE task_id = ?1 \
         ORDER BY created_at DESC LIMIT 1",
        "task-a".into(),
    )
    .await;
    assert_eq!(kind.as_deref(), Some("agent"));
}

/// B4-10 — keys of a caller's own maps are names, not Kronn fields.
#[tokio::test]
async fn keys_of_a_caller_s_own_maps_are_not_id_fields() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(&token),
        Some(
            json!({"api_plugin_slug": "synthetic-api", "api_config_id": "c",
            "endpoint_path": "/projects/{project_id}", "method": "GET",
            "path_params": {"project_id": "123"}, "query": {"issue_ref": "x"}}),
        ),
    )
    .await;
    assert_ne!(status, 403, "{response}");
    let (status, response) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(&token),
        Some(json!({"workflow_id": "wf-a", "variables": {"ticket_id": "X"}})),
    )
    .await;
    assert_ne!(status, 403, "{response}");
}

// ─── Layer B round 5 (review-layer-b5) ──────────────────────────────────────

/// B5-01 — joining ends the named session elsewhere: a token cannot evict a
/// session active in another project's room.
#[tokio::test]
async fn peer_join_cannot_evict_a_session_from_another_project() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussion_invite_tokens(token_hash, disc_id, created_at, expires_at) \
             VALUES (?1, 'room-a', '2026-01-01T00:00:00Z', '2999-01-01T00:00:00Z')",
            [sha256_hex("kr-join-room-a")],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/discussions/peer-join",
        Some(guard.value()),
        Some(json!({"token": "kr-join-room-a", "agent_type": "Codex", "session_id": "sess-b"})),
    )
    .await;
    assert_eq!(status, 403, "{response}");
    let state = query_one(
        &db,
        "SELECT status FROM discussion_sessions WHERE id = CAST(?1 AS INTEGER)",
        "901".into(),
    )
    .await;
    assert_eq!(state.as_deref(), Some("active"));
}

/// B5-02 — a page a token's workflow will publish into must be its project's.
#[tokio::test]
async fn a_token_s_workflow_publishes_only_into_its_project_s_pages() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_pages(id, project_id, title, slug, created_at, updated_at) \
             VALUES ('page-shared', NULL, 'shared', 'page-shared', '2026-01-01T00:00:00Z', \
             '2026-01-01T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let workflow = |page: &str| {
        json!({"name": "publish", "trigger": {"type": "Manual"},
            "steps": [{"name": "publish", "step_type": {"type": "PublishPageData"},
                "page_publish": {"page_id": page, "writes": []}}]})
    };
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows",
        Some(guard.value()),
        Some(workflow("page-shared")),
    )
    .await;
    assert_eq!(status, 403, "a shared page: {response}");
    let (status, response) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        Some(guard.value()),
        Some(workflow("page-shared")),
    )
    .await;
    assert_eq!(status, 403, "a shared page on update: {response}");
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows",
        Some(guard.value()),
        Some(workflow("page-a")),
    )
    .await;
    assert_eq!(status, 200, "its own project's page: {response}");
}

/// B5-03 — a project-less run is private to its launch, like a General
/// discussion.
#[tokio::test]
async fn a_project_less_run_is_private_to_its_launch() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO workflow_runs(id, workflow_id, project_id, started_at) VALUES \
             ('run-general', 'wf-global', NULL, '2026-01-01T00:00:00Z'), \
             ('run-global-p1', 'wf-global', 'p1', '2026-01-01T00:00:01Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, response) = call(
        &app,
        "GET",
        "/api/workflows/wf-global/runs",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let listed = response["data"].to_string();
    assert!(listed.contains("run-global-p1"), "{response}");
    assert!(!listed.contains("run-general"), "{response}");
    for path in [
        "/api/workflows/wf-global/runs/run-general",
        "/api/mcp/workflow-run-status/run-general",
    ] {
        let (status, _) = call(&app, "GET", path, Some(&token), None).await;
        assert_eq!(status, 403, "{path}");
    }
    let own = mint(BridgeScope {
        workflow_run_id: Some("run-general".into()),
        ..Default::default()
    })
    .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        "/api/workflows/wf-global/runs/run-general",
        Some(own.value()),
        None,
    )
    .await;
    assert_eq!(status, 200, "its own launch reads it");
}

/// B5-09 — a config linked to a project and opted into General is usable by
/// a project-less token, as the overview says, and not by another project.
#[tokio::test]
async fn a_config_opted_into_general_follows_one_rule_everywhere() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO mcp_configs(id, server_id, label, include_general) \
             VALUES ('cfg-b-general', 'synthetic-api', 'cfg-b-general', 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO mcp_config_projects(config_id, project_id) VALUES ('cfg-b-general', 'p2')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let call_body = json!({"api_plugin_slug": "synthetic-api", "api_config_id": "cfg-b-general",
        "endpoint_path": "/anything", "method": "GET"});
    let general = bridge_for("room-g");
    let (_, overview) = call(&app, "GET", "/api/mcps", Some(general.value()), None).await;
    assert!(overview.to_string().contains("cfg-b-general"), "{overview}");
    let (status, response) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(general.value()),
        Some(call_body.clone()),
    )
    .await;
    assert_ne!(status, 403, "{response}");
    let p1 = bridge_for("room-a");
    let (_, overview) = call(&app, "GET", "/api/mcps", Some(p1.value()), None).await;
    assert!(
        !overview.to_string().contains("cfg-b-general"),
        "{overview}"
    );
    let (status, _) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(p1.value()),
        Some(call_body),
    )
    .await;
    assert_eq!(status, 403);
}

/// B5-10 — a token learns that a slug is taken, never that another project
/// holds it.
#[tokio::test]
async fn a_slug_conflict_names_no_other_project() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_pages(id, project_id, title, slug, created_at, updated_at) \
             VALUES ('page-taken', 'p2', 'taken', 'taken', '2026-01-01T00:00:00Z', \
             '2026-01-01T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/pages",
        Some(guard.value()),
        Some(json!({"title": "Mine", "slug": "taken", "html": "<p>x</p>"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let error = response["error"].as_str().unwrap_or_default();
    assert_eq!(
        error, "This Page slug is not available; choose another",
        "{response}"
    );
    let (_, uuid_slug) = call(
        &app,
        "POST",
        "/api/pages",
        Some(guard.value()),
        Some(
            json!({"title": "Mine", "slug": "0b8f5c2e-3a7d-4f1e-9c6b-2d4e8a1f7c3b",
            "html": "<p>x</p>"}),
        ),
    )
    .await;
    assert_eq!(uuid_slug["success"], false, "{uuid_slug}");
}

// ─── Layer B round 6 (review-layer-b6) ──────────────────────────────────────

/// A project-less workflow with an older p1 run and a newer p2 run, both
/// carrying `state.k = "v"`.
async fn shared_workflow_runs(db: &Arc<kronn::db::Database>) {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO workflow_runs(id, workflow_id, project_id, status, state, started_at) VALUES \
             ('run-old-p1', 'wf-global', 'p1', 'Success', '{\"k\":\"v\"}', '2026-01-01T00:00:00Z'), \
             ('run-new-p2', 'wf-global', 'p2', 'Success', '{\"k\":\"v\"}', '2026-01-02T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// B6-01 — a token's `last_run` is one it may see.
#[tokio::test]
async fn a_workflow_s_last_run_is_one_the_token_may_see() {
    let (app, _repos, db) = fixture_with_db().await;
    shared_workflow_runs(&db).await;
    let guard = bridge_for("room-a");
    let (status, response) = call(&app, "GET", "/api/workflows", Some(guard.value()), None).await;
    assert_eq!(status, 200, "{response}");
    let shared = response["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workflow| workflow["id"] == "wf-global")
        .expect("the shared workflow is listed");
    assert_eq!(shared["last_run"]["id"], "run-old-p1", "{shared}");
    assert!(!response.to_string().contains("run-new-p2"), "{response}");
}

/// B6-02 — hidden runs neither answer a state filter nor take a page's place.
#[tokio::test]
async fn hidden_runs_never_answer_a_run_list_query() {
    let (app, _repos, db) = fixture_with_db().await;
    shared_workflow_runs(&db).await;
    let guard = bridge_for("room-a");
    for path in [
        "/api/workflows/wf-global/runs?state_key=k&state_value=v&limit=1",
        "/api/workflows/wf-global/runs?limit=1",
        "/api/workflows/wf-global/runs?limit=1&complete_group=true",
    ] {
        let (status, response) = call(&app, "GET", path, Some(guard.value()), None).await;
        assert_eq!(status, 200, "{path}: {response}");
        let ids: Vec<&str> = response["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| run["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["run-old-p1"], "{path}");
    }
}

/// B6-03 — a run without a project stays private after its workflow moves.
#[tokio::test]
async fn a_project_less_run_stays_private_after_its_workflow_moves() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO workflow_runs(id, workflow_id, project_id, started_at) \
             VALUES ('run-before-move', 'wf-global', NULL, '2026-01-01T00:00:00Z')",
            [],
        )?;
        conn.execute(
            "UPDATE workflows SET project_id = 'p1' WHERE id = 'wf-global'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let (status, _) = call(
        &app,
        "GET",
        "/api/workflows/wf-global/runs/run-before-move",
        Some(guard.value()),
        None,
    )
    .await;
    assert_eq!(status, 403);
    let (_, list) = call(
        &app,
        "GET",
        "/api/workflows/wf-global/runs",
        Some(guard.value()),
        None,
    )
    .await;
    assert!(!list.to_string().contains("run-before-move"), "{list}");
}

/// B6-06 — an import's unbundled literal page and a save's foreign room are
/// write targets; a bundled page lands in the token's project.
#[tokio::test]
async fn imported_and_saved_targets_stay_in_the_token_s_project() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_pages(id, project_id, title, slug, created_at, updated_at) \
             VALUES ('page-shared', NULL, 'shared', 'page-shared', '2026-01-01T00:00:00Z', \
             '2026-01-01T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, mut envelope) = call(&app, "GET", "/api/workflows/wf-a/export", None, None).await;
    assert_eq!(status, 200, "{envelope}");
    envelope["version"] = json!(2);
    envelope["workflow"]["steps"] = json!([
        {"name": "seed", "step_type": {"type": "JsonData"}, "json_data_payload": {"v": 1}},
        {"name": "publish", "step_type": {"type": "PublishPageData"},
         "page_publish": {"page_id": "page-shared", "writes": [
             {"dataset": "summary", "operation": "replace", "value_from": "steps.seed.data"}]}}]);
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows/import",
        Some(&token),
        Some(json!({"content": envelope.to_string()})),
    )
    .await;
    assert_eq!(status, 403, "an unbundled shared page: {response}");
    envelope["referenced_pages"] = json!([{"id": "page-shared", "slug": "bundled-page",
        "title": "Bundled", "html": "<p>x</p>", "created_by_agent": null,
        "datasets": [{"name": "summary", "kind": "snapshot", "schema": null,
            "max_points": 100, "max_age_days": null}]}]);
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows/import",
        Some(&token),
        Some(json!({"content": envelope.to_string()})),
    )
    .await;
    assert_eq!(status, 200, "a bundled page: {response}");
    let project = query_one(
        &db,
        "SELECT project_id FROM live_pages WHERE slug = ?1",
        "bundled-page".into(),
    )
    .await;
    assert_eq!(project.as_deref(), Some("p1"), "{response}");
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows",
        Some(&token),
        Some(json!({"name": "room", "trigger": {"type": "Manual"},
            "steps": [{"name": "agent", "step_type": {"type": "Agent"},
                "prompt_template": "go", "room_id": "room-b"}]})),
    )
    .await;
    assert_eq!(status, 403, "another project's room: {response}");
}

// ─── Layer B round 7 (review-layer-b7) ──────────────────────────────────────

/// B7-05 — a token's duration estimate uses only runs it may see.
#[tokio::test]
async fn workflow_duration_estimates_use_only_visible_runs() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO workflow_runs(id, workflow_id, project_id, status, started_at, finished_at) VALUES \
             ('run-p2-a', 'wf-global', 'p2', 'Success', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z'), \
             ('run-p2-b', 'wf-global', 'p2', 'Success', '2026-01-02T00:00:00Z', '2026-01-02T00:01:00Z'), \
             ('run-p1-live', 'wf-global', 'p1', 'Running', '2026-01-03T00:00:00Z', NULL)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, human) = call(
        &app,
        "GET",
        "/api/mcp/workflow-run-status/run-p1-live",
        None,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(human["data"]["samples"], 2, "{human}");
    let (status, response) = call(
        &app,
        "GET",
        "/api/mcp/workflow-run-status/run-p1-live",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["data"]["samples"], 0, "{response}");
    assert!(
        response["data"]["expected_duration_ms"].is_null(),
        "{response}"
    );
    let (status, response) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(&token),
        Some(json!({"workflow_id": "wf-global"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["data"]["samples"], 0, "{response}");
    assert!(
        response["data"]["expected_duration_ms"].is_null(),
        "{response}"
    );
}

/// B7-06 — a token's Quick Prompt estimate counts only its project's launches.
#[tokio::test]
async fn quick_prompt_estimates_count_only_the_token_s_launches() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        for n in 0..3 {
            let disc = format!("qp-launch-{n}");
            conn.execute(
                "INSERT INTO discussions(id, title, project_id, originating_qp_id, \
                 originating_qp_version, created_at, updated_at) \
                 VALUES (?1, ?1, 'p2', 'qp-global', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [&disc],
            )?;
            conn.execute(
                "INSERT INTO messages(id, discussion_id, role, content, timestamp, sort_order, \
                 duration_ms) VALUES (?1, ?2, 'Agent', 'done', '2026-01-01T00:00:00Z', 0, 60000)",
                rusqlite::params![format!("{disc}-m"), disc],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/mcp/qp-run",
        Some(guard.value()),
        Some(json!({"qp_id": "qp-global"})),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(response["data"]["samples"], 0, "{response}");
}

/// KT-1098 — a renamed page stays in its project's scope under its old slug,
/// and another project's token can neither open, rename nor squat it.
#[tokio::test]
async fn a_renamed_page_stays_in_scope_and_its_old_slug_stays_its_own() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_pages(id, project_id, title, slug, current_revision_id, \
             created_at, updated_at) VALUES ('page-c', 'p1', 'c', 'c-old', 'rev-c', \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )?;
        conn.execute(
            "INSERT INTO live_page_revisions(id, page_id, revision, html, created_by_agent, \
             created_at) VALUES ('rev-c', 'page-c', 1, '<p></p>', NULL, '2026-01-01T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let own = bridge_for("room-a");
    let foreign = bridge_for("room-b");

    let (status, renamed) = call(
        &app,
        "PATCH",
        "/api/pages/c-old",
        Some(own.value()),
        Some(json!({"slug": "c-new"})),
    )
    .await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["data"]["slug"], "c-new", "{renamed}");
    let (status, opened) = call(&app, "GET", "/api/pages/c-old", Some(own.value()), None).await;
    assert_eq!(status, 200, "{opened}");
    assert_eq!(opened["data"]["id"], "page-c", "{opened}");

    for (method, path, body) in [
        ("GET", "/api/pages/c-old", None),
        ("GET", "/api/pages/c-new", None),
        ("PATCH", "/api/pages/c-old", Some(json!({"slug": "c-mine"}))),
        (
            "PATCH",
            "/api/pages/page-c",
            Some(json!({"slug": "c-mine"})),
        ),
    ] {
        let (status, response) = call(&app, method, path, Some(foreign.value()), body).await;
        assert_eq!(status, 403, "{method} {path} must be refused: {response}");
    }
    // Taking the old slug is refused without naming the page that keeps it.
    for (method, path, body) in [
        ("PATCH", "/api/pages/page-b", json!({"slug": "c-old"})),
        (
            "POST",
            "/api/pages",
            json!({"title": "squat", "slug": "c-old", "html": "<p>x</p>"}),
        ),
    ] {
        let (_, response) = call(&app, method, path, Some(foreign.value()), Some(body)).await;
        assert_eq!(
            response["error"], "This Page slug is not available; choose another",
            "{method} {path}: {response}"
        );
    }
    let (_, still) = call(&app, "GET", "/api/pages/c-old", Some(own.value()), None).await;
    assert_eq!(still["data"]["id"], "page-c", "{still}");
}

/// KT-1109 — attaching handoff recipients is the human's orchestration
/// choice: an agent's token cannot widen them, even on its own room.
#[tokio::test]
async fn a_bridge_token_cannot_attach_agents_to_its_own_discussion() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        ..Default::default()
    })
    .unwrap();
    let token = guard.value().to_owned();
    let participants = |disc: &'static str| {
        let db = db.clone();
        async move {
            db.with_read_conn(move |conn| {
                Ok(kronn::db::discussions::get_discussion(conn, disc)?
                    .expect("discussion")
                    .participants)
            })
            .await
            .unwrap()
        }
    };
    let attach = json!({"attach_agents": ["Codex", "OpenCode"]});

    let (status, body) = call(
        &app,
        "PATCH",
        "/api/discussions/room-a",
        Some(&token),
        Some(attach.clone()),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(participants("room-a").await.is_empty(), "nothing attached");

    // Any other field the token may already change still goes through.
    let (status, body) = call(
        &app,
        "PATCH",
        "/api/discussions/room-a",
        Some(&token),
        Some(json!({"title": "renamed by the agent"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = call(
        &app,
        "PATCH",
        "/api/discussions/room-b",
        Some(&token),
        Some(attach.clone()),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(participants("room-b").await.is_empty());

    // The human composer path attaches them.
    let (status, body) = call(&app, "PATCH", "/api/discussions/room-a", None, Some(attach)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], true, "{body}");
    let attached: Vec<String> = participants("room-a")
        .await
        .iter()
        .map(|agent| format!("{agent:?}"))
        .collect();
    assert_eq!(attached, vec!["Codex", "OpenCode"]);
}

/// KT-1043 — the approval a workflow's Security settings require before a run
/// is a human decision: `/decide` is outside the bridge token's list.
#[tokio::test]
async fn a_bridge_token_cannot_approve_the_security_pause() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE workflows SET safety_json = '{\"sandbox\":false,\"require_approval\":true}' \
             WHERE id = 'wf-a'",
            [],
        )?;
        let mut run = kronn::db::workflows::get_run(conn, "run-a")?.expect("run-a");
        run.status = kronn::models::RunStatus::WaitingApproval;
        run.step_results = vec![kronn::workflows::safety::approval_result()];
        kronn::db::workflows::update_run_progress(
            conn,
            kronn::db::workflows::RunProgressSnapshot::from_run(&run),
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let (status, response) = call(
        &app,
        "POST",
        "/api/workflows/wf-a/runs/run-a/decide",
        Some(guard.value()),
        Some(json!({"decision": "approve"})),
    )
    .await;
    assert_eq!(status, 403, "{response}");
    let run = db
        .with_conn(|conn| kronn::db::workflows::get_run(conn, "run-a"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, kronn::models::RunStatus::WaitingApproval);
}

/// Every `discussion_ids` list in a response, at any depth.
fn discussion_id_lists(value: &Value, out: &mut Vec<Vec<String>>) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                if key == "discussion_ids" {
                    if let Some(items) = child.as_array() {
                        out.push(
                            items
                                .iter()
                                .filter_map(|id| id.as_str().map(str::to_owned))
                                .collect(),
                        );
                    }
                } else {
                    discussion_id_lists(child, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| discussion_id_lists(item, out)),
        _ => {}
    }
}

/// GitHub #248: one parent task shared by a room of each project stays
/// visible from both rooms, each seeing only its own room among the links.
#[tokio::test]
async fn a_task_shared_by_rooms_of_two_projects_is_visible_from_each() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO planning_tasks(id, task_number, title, created_at, updated_at) \
             VALUES ('task-shared', 9010, 'task-shared', ?1, ?1)",
            [&now],
        )?;
        for project in ["p1", "p2"] {
            conn.execute(
                "INSERT INTO planning_task_projects(task_id, project_id) VALUES ('task-shared', ?1)",
                [project],
            )?;
        }
        for room in ["room-a", "room-b"] {
            conn.execute(
                "INSERT INTO planning_task_discussions(task_id, discussion_id, placement, is_primary, created_at) \
                 VALUES ('task-shared', ?1, 'active', 1, ?2)",
                rusqlite::params![room, now],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    for (room, other) in [("room-a", "room-b"), ("room-b", "room-a")] {
        let guard = bridge_for(room);
        for path in [
            "/api/planning/tasks/task-shared".to_owned(),
            format!("/api/discussions/{room}/plan"),
        ] {
            let (status, response) = call(&app, "GET", &path, Some(guard.value()), None).await;
            assert_eq!(status, 200, "{room} {path}: {response}");
            let text = response.to_string();
            assert!(text.contains("task-shared"), "{room} {path}: {text}");
            assert!(
                !text.contains(other),
                "{room} {path} names the other project's room: {text}"
            );
            let mut lists = Vec::new();
            discussion_id_lists(&response, &mut lists);
            assert!(
                !lists.is_empty(),
                "{room} {path}: no discussion_ids in {text}"
            );
            for list in lists {
                assert_eq!(list, vec![room.to_owned()], "{room} {path}: {text}");
            }
        }
    }
    // The operator still sees both links.
    let (_, operator) = call(&app, "GET", "/api/planning/tasks/task-shared", None, None).await;
    let text = operator.to_string();
    assert!(text.contains("room-a") && text.contains("room-b"), "{text}");
}

/// KT-1270 — a human opens another project to a room; the room's token
/// follows on its next call, and only a human can open or close it.
async fn grant_scope(app: &Router, room: &str, body: Value) -> (u16, Value) {
    call(
        app,
        "POST",
        &format!("/api/discussions/{room}/scope/grants"),
        None,
        Some(body),
    )
    .await
}

#[tokio::test]
async fn a_read_grant_reads_another_project_and_changes_nothing_there() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403, "no grant yet");

    let (status, body) = grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "read"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "read grant, same token, next call: {body}");
    let (status, body) = call(
        &app,
        "GET",
        "/api/planning/tasks?project_id=p2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body.to_string().contains("task-b"), "{body}");
    let (status, body) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "changed"})),
    )
    .await;
    assert_eq!(status, 403, "a read grant never writes: {body}");
    let (status, _) = call(
        &app,
        "GET",
        "/api/discussions/room-b/meta",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "the granted project's rooms read too");
    let (status, _) = call(
        &app,
        "POST",
        "/api/disc/append",
        Some(&token),
        Some(append("room-b", "g1")),
    )
    .await;
    assert_eq!(status, 403, "but are never written to");
}

#[tokio::test]
async fn a_write_grant_opens_planning_only_and_never_credentials() {
    let (app, _repos, db) = fixture_with_db().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, body) = grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "write"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "changed by the room"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        &app,
        "POST",
        "/api/planning/tasks",
        Some(&token),
        Some(json!({"title": "api docs", "project_ids": ["p2"]})),
    )
    .await;
    assert_eq!(status, 200, "a task created in the granted project: {body}");
    let (status, body) = call(
        &app,
        "POST",
        "/api/planning/tasks",
        Some(&token),
        Some(json!({"title": "unnamed"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (projects_api, projects_unnamed) = db
        .with_read_conn(|conn| {
            let of = |title: &str| -> rusqlite::Result<String> {
                conn.query_row(
                    "SELECT GROUP_CONCAT(p.project_id) FROM planning_task_projects p \
                     JOIN planning_tasks t ON t.id = p.task_id WHERE t.title = ?1",
                    [title],
                    |row| row.get(0),
                )
            };
            Ok((of("api docs")?, of("unnamed")?))
        })
        .await
        .unwrap();
    assert_eq!(projects_api, "p2");
    assert_eq!(
        projects_unnamed, "p1",
        "a creation left unnamed lands in the room's project"
    );

    // Planning only: another project's workflow, page or room stays read-only.
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-b",
        Some(&token),
        Some(json!({"name": "taken"})),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    // Credentials never follow a grant.
    let (status, body) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(&token),
        Some(json!({
            "api_plugin_slug": "synthetic-api",
            "api_config_id": "cfg-b",
            "endpoint_path": "/anything",
        })),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    let (status, view) = call(&app, "GET", "/api/mcps", Some(&token), None).await;
    assert_eq!(status, 200);
    assert!(!view.to_string().contains("cfg-b"), "{view}");
    // A route that changes the project itself is not planning.
    let (status, body) = call(
        &app,
        "POST",
        "/api/projects/p2/install-template",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403, "{body}");
    // Effects stay on the room's project.
    let (status, body) = call(
        &app,
        "POST",
        "/api/mcp/workflow-trigger",
        Some(&token),
        Some(json!({"workflow_id": "wf-b"})),
    )
    .await;
    assert_eq!(status, 403, "{body}");
}

#[tokio::test]
async fn a_revoke_closes_the_project_on_the_next_call() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (_, granted) = grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "write"}),
    )
    .await;
    let grant_id = granted["data"]["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/discussions/room-a/scope/grants/{grant_id}/revoke"),
        None,
        Some(json!({"reason": "done"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403, "revoked, without restarting anything");
    let (_, scope) = call(&app, "GET", "/api/discussions/room-a/scope", None, None).await;
    assert_eq!(
        scope["data"]["revoked"][0]["revoked_reason"], "done",
        "{scope}"
    );
}

#[tokio::test]
async fn every_project_in_write_needs_a_typed_confirmation_and_the_seen_scope() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let (status, body) = grant_scope(&app, "room-a", json!({"all": true, "access": "read"})).await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "every project reads");
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "x"})),
    )
    .await;
    assert_eq!(
        status, 403,
        "and changes nothing outside the room's project"
    );

    let (_, scope) = call(&app, "GET", "/api/discussions/room-a/scope", None, None).await;
    let seen = scope["data"]["fingerprint"].as_str().unwrap().to_owned();
    for (body, expected) in [
        (json!({"all": true, "access": "write"}), 400),
        (
            json!({"all": true, "access": "write", "confirm": "yes"}),
            400,
        ),
        (
            json!({"all": true, "access": "write", "confirm": "ALL-PROJECTS-WRITE"}),
            400,
        ),
        (
            json!({"all": true, "access": "write", "confirm": "ALL-PROJECTS-WRITE", "expected_fingerprint": "stale"}),
            409,
        ),
    ] {
        let (status, response) = grant_scope(&app, "room-a", body.clone()).await;
        assert_eq!(status, expected, "{body}: {response}");
    }
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "x"})),
    )
    .await;
    assert_eq!(status, 403, "no refused attempt widened anything");
    let (status, body) = grant_scope(
        &app,
        "room-a",
        json!({"all": true, "access": "write", "confirm": "ALL-PROJECTS-WRITE", "expected_fingerprint": seen}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "x"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn only_a_human_opens_or_closes_a_room_s_scope() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    for (path, body) in [
        (
            "/api/discussions/room-a/scope/grants",
            json!({"project_id": "p2", "access": "write"}),
        ),
        (
            "/api/discussions/room-a/scope/grants",
            json!({"all": true, "access": "read"}),
        ),
        ("/api/discussions/room-a/scope/grants/any/revoke", json!({})),
        (
            "/api/discussions/room-a/scope/requests/any/approve",
            json!({"access": "write"}),
        ),
        (
            "/api/discussions/room-a/scope/requests/any/decline",
            json!({}),
        ),
    ] {
        let (status, response) = call(&app, "POST", path, Some(&token), Some(body)).await;
        assert_eq!(status, 403, "{path}: {response}");
    }
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn an_agent_request_grants_nothing_until_a_human_approves_it() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let request =
        json!({"project": "p2", "access": "write", "reason": "create the API doc sub-task"});
    let (status, filed) = call(
        &app,
        "POST",
        "/api/discussions/room-a/scope/requests",
        Some(&token),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, 200, "{filed}");
    assert_eq!(filed["data"]["status"], "pending", "{filed}");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403, "a request is not a grant");
    let (_, again) = call(
        &app,
        "POST",
        "/api/discussions/room-a/scope/requests",
        Some(&token),
        Some(request),
    )
    .await;
    assert_eq!(again["data"]["status"], "already_pending", "{again}");
    let (status, other) = call(
        &app,
        "POST",
        "/api/discussions/room-a2/scope/requests",
        Some(&token),
        Some(json!({"project": "p2", "access": "read", "reason": "x"})),
    )
    .await;
    assert_eq!(status, 403, "only for its own room: {other}");

    let request_id = filed["data"]["request_ref"].as_str().unwrap();
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/discussions/room-a/scope/requests/{request_id}/approve"),
        None,
        Some(json!({"access": "read"})),
    )
    .await;
    assert_eq!(status, 200, "the human narrows it to read: {body}");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "x"})),
    )
    .await;
    assert_eq!(status, 403);
    let (status, scope) = call(
        &app,
        "GET",
        "/api/discussions/room-a/scope",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "an agent sees its room's reach: {scope}");
}

#[tokio::test]
async fn a_grant_belongs_to_its_room_and_never_spreads() {
    let (app, _repos) = fixture().await;
    grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "write"}),
    )
    .await;
    let sibling = bridge_for("room-a2");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(sibling.value()),
        None,
    )
    .await;
    assert_eq!(status, 403, "a room of the same project gets nothing");
    let (status, body) = grant_scope(
        &app,
        "room-g",
        json!({"project_id": "p2", "access": "read"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a room without a project reaches no other: {body}"
    );
    let (status, body) = grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p1", "access": "read"}),
    )
    .await;
    assert_eq!(status, 400, "its own project needs no grant: {body}");
}

#[tokio::test]
async fn a_task_shared_with_another_project_needs_write_on_both() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO planning_tasks(id, task_number, title, created_at, updated_at) \
             VALUES ('task-ab', 9011, 'task-ab', ?1, ?1)",
            [&now],
        )?;
        for project in ["p1", "p2"] {
            conn.execute(
                "INSERT INTO planning_task_projects(task_id, project_id) VALUES ('task-ab', ?1)",
                [project],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let patch = || json!({"title": "edited"});
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-ab",
        Some(&token),
        Some(patch()),
    )
    .await;
    assert_eq!(
        status, 403,
        "shared with p2, unchanged rule without a grant"
    );
    grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "read"}),
    )
    .await;
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-ab",
        Some(&token),
        Some(patch()),
    )
    .await;
    assert_eq!(status, 403, "read on p2 is not enough");
    grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "write"}),
    )
    .await;
    let (status, body) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-ab",
        Some(&token),
        Some(patch()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

/// KT-1270 review B2 — a workflow step posting in a room is not that room's
/// agent: the room's grants never reach its token, so pointing a step's
/// `room_id` at a granted room opens nothing.
#[tokio::test]
async fn a_step_posting_in_a_granted_room_gets_none_of_its_grants() {
    let (app, _repos) = fixture().await;
    grant_scope(
        &app,
        "room-a",
        json!({"project_id": "p2", "access": "write"}),
    )
    .await;
    let step = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        workflow_run_id: Some("run-a".into()),
        ..Default::default()
    })
    .unwrap();
    let token = step.value().to_owned();
    let (status, body) = call(
        &app,
        "POST",
        "/api/disc/append",
        Some(&token),
        Some(append("room-a", "step-b2")),
    )
    .await;
    assert_eq!(status, 200, "the step still posts in its room: {body}");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 403, "no read through the room's grant");
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(&token),
        Some(json!({"title": "taken"})),
    )
    .await;
    assert_eq!(status, 403, "no write through the room's grant");
    let (status, _) = call(
        &app,
        "POST",
        "/api/planning/tasks",
        Some(&token),
        Some(json!({"title": "x", "project_ids": ["p2"]})),
    )
    .await;
    assert_eq!(status, 403, "no creation through the room's grant");
    // A task worker is not the room's agent either.
    let worker = mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        task_execution_id: Some("exec-a".into()),
        ..Default::default()
    })
    .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(worker.value()),
        None,
    )
    .await;
    assert_eq!(status, 403);
    // The room's own agent does reach it.
    let agent = bridge_for("room-a");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(agent.value()),
        None,
    )
    .await;
    assert_eq!(status, 200);
}

/// Start `run_id` for `workflow_id` and pin it with the production pin, as a
/// launch does: the definitions it pins are the ones it executes.
async fn pinned_run(db: &Arc<kronn::db::Database>, workflow_id: &str, run_id: &str) {
    let (workflow_id, run_id) = (workflow_id.to_owned(), run_id.to_owned());
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO workflow_runs(id, workflow_id, project_id, started_at) VALUES (?1, ?2, 'p1', ?3)",
            rusqlite::params![run_id, workflow_id, chrono::Utc::now().to_rfc3339()],
        )?;
        let workflow = kronn::db::workflows::get_workflow(conn, &workflow_id)?.unwrap();
        let run = kronn::db::workflows::get_run(conn, &run_id)?.unwrap();
        kronn::workflows::run_pins::pin_or_load(conn, &workflow, &run)?.expect("pin");
        Ok(())
    })
    .await
    .unwrap();
}

fn step_of(run_id: &str) -> kronn::core::bridge_token::BridgeTokenGuard {
    mint(BridgeScope {
        discussion_ids: vec!["room-a".into()],
        workflow_run_id: Some(run_id.into()),
        ..Default::default()
    })
    .unwrap()
}

async fn edits_task_b(app: &Router, token: &str) -> u16 {
    call(
        app,
        "PATCH",
        "/api/planning/tasks/task-b",
        Some(token),
        Some(json!({"title": "edited by a run"})),
    )
    .await
    .0
}

/// KT-1270 DoD — a workflow's scope, validated once by a human, reaches the
/// runs that pinned the validated revision, and only them.
#[tokio::test]
async fn a_workflow_scope_reaches_only_runs_that_pinned_the_validated_revision() {
    let (app, _repos, db) = fixture_with_db().await;
    let scope = json!({"entries": [{"project_id": "p2", "access": "write"}]});
    let agent = bridge_for("room-a");
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        Some(agent.value()),
        Some(scope.clone()),
    )
    .await;
    assert_eq!(
        status, 403,
        "a bridge token cannot set a workflow's scope: {body}"
    );
    let (status, body) = call(&app, "PUT", "/api/workflows/wf-a/scope", None, Some(scope)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["valid"], true, "{body}");

    pinned_run(&db, "wf-a", "run-validated").await;
    let validated = step_of("run-validated");
    assert_eq!(
        edits_task_b(&app, validated.value()).await,
        200,
        "the validated revision reaches p2"
    );
    let (status, body) = call(
        &app,
        "POST",
        "/api/planning/tasks",
        Some(validated.value()),
        Some(json!({"title": "api docs", "project_ids": ["p2"]})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(validated.value()),
        Some(json!({"api_plugin_slug": "synthetic-api", "api_config_id": "cfg-b", "endpoint_path": "/x"})),
    )
    .await;
    assert_eq!(status, 403, "never another project's credentials");

    // An agent changes the workflow: the scope is suspended and a run that pins
    // the change reaches nothing...
    let changed = json!({"steps": [{"name": "changed", "step_type": {"type": "Agent"}, "prompt_template": "UNVALIDATED"}]});
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        Some(agent.value()),
        Some(changed),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (_, scope) = call(&app, "GET", "/api/workflows/wf-a/scope", None, None).await;
    assert_eq!(scope["data"]["valid"], false, "{scope}");
    pinned_run(&db, "wf-a", "run-changed").await;
    let changed_run = step_of("run-changed");
    assert_eq!(edits_task_b(&app, changed_run.value()).await, 403);
    // ...and restoring the validated definition never re-opens it: the run is
    // judged on what it pinned (review P1-A).
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        Some(agent.value()),
        Some(json!({"steps": []})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (_, scope) = call(&app, "GET", "/api/workflows/wf-a/scope", None, None).await;
    assert_eq!(
        scope["data"]["valid"], true,
        "the live definition is the validated one again: {scope}"
    );
    assert_eq!(
        edits_task_b(&app, changed_run.value()).await,
        403,
        "the run that pinned the change stays closed"
    );
    assert_eq!(
        edits_task_b(&app, validated.value()).await,
        200,
        "the run that pinned the validated revision keeps it"
    );

    // Another workflow's runs never reach it.
    let other = mint(BridgeScope {
        discussion_ids: vec!["room-b".into()],
        workflow_run_id: Some("run-b".into()),
        ..Default::default()
    })
    .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-a",
        Some(other.value()),
        None,
    )
    .await;
    assert_eq!(status, 403);
}

/// KT-1270 review P1-B — what a workflow executes is part of what a human
/// validated: an edited Quick Prompt suspends the scope, and a run that pinned
/// the edited prompt reaches nothing even after the prompt is put back.
#[tokio::test]
async fn an_edited_dependency_suspends_a_workflow_scope() {
    let (app, _repos, db) = fixture_with_db().await;
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        None,
        Some(json!({"steps": [{"name": "prompt-step", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-a"}]})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        None,
        Some(json!({"entries": [{"project_id": "p2", "access": "write"}]})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let agent = bridge_for("room-a");
    let (status, body) = call(
        &app,
        "PUT",
        "/api/quick-prompts/qp-a",
        Some(agent.value()),
        Some(json!({"name": "qp-a", "prompt_template": "UNVALIDATED_DEPENDENCY"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (_, scope) = call(&app, "GET", "/api/workflows/wf-a/scope", None, None).await;
    assert_eq!(scope["data"]["valid"], false, "{scope}");
    pinned_run(&db, "wf-a", "run-edited-prompt").await;
    let edited = step_of("run-edited-prompt");
    assert_eq!(edits_task_b(&app, edited.value()).await, 403);
    let (status, body) = call(
        &app,
        "PUT",
        "/api/quick-prompts/qp-a",
        Some(agent.value()),
        Some(json!({"name": "qp-a", "prompt_template": "x"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        edits_task_b(&app, edited.value()).await,
        403,
        "put back after the pin: still closed"
    );
}

/// KT-1270 review — an agent's request never says whether a project exists.
#[tokio::test]
async fn an_agent_request_answers_the_same_for_an_unknown_project() {
    let (app, _repos) = fixture().await;
    let guard = bridge_for("room-a");
    let token = guard.value().to_owned();
    let ask = |project: &str| json!({"project": project, "access": "read", "reason": "look"});
    let (status, known) = call(
        &app,
        "POST",
        "/api/discussions/room-a/scope/requests",
        Some(&token),
        Some(ask("p2")),
    )
    .await;
    assert_eq!(status, 200, "{known}");
    let (status, unknown) = call(
        &app,
        "POST",
        "/api/discussions/room-a/scope/requests",
        Some(&token),
        Some(ask("no-such-project")),
    )
    .await;
    assert_eq!(status, 200, "{unknown}");
    assert_eq!(known["data"]["status"], unknown["data"]["status"]);
    let shape = |value: &Value| {
        let mut keys: Vec<String> = value["data"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(shape(&known), shape(&unknown));
}

/// KT-1270 review — a project's token override never reaches a bridge token.
#[tokio::test]
async fn a_project_token_override_never_reaches_a_bridge_token() {
    let (app, _repos, db) = fixture_with_db().await;
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE projects SET token_override_json = '{\"provider\":\"github\",\"token\":\"ghp-secret\"}' WHERE id = 'p1'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let guard = bridge_for("room-a");
    let (status, body) = call(&app, "GET", "/api/projects/p1", Some(guard.value()), None).await;
    assert_eq!(status, 200, "{body}");
    assert!(!body.to_string().contains("ghp-secret"), "{body}");
    assert!(body["data"].get("token_override").is_none(), "{body}");
}

/// KT-1270 review P2 — a global or multi-project workflow resolves another
/// revision per launch project: it cannot carry a scope, and says so rather
/// than showing a validation that would never apply.
#[tokio::test]
async fn a_global_or_multi_project_workflow_cannot_carry_a_scope() {
    let (app, _repos) = fixture().await;
    for workflow in ["wf-global", "wf-all"] {
        let (status, body) = call(
            &app,
            "PUT",
            &format!("/api/workflows/{workflow}/scope"),
            None,
            Some(json!({"entries": [{"project_id": "p2", "access": "write"}]})),
        )
        .await;
        assert_eq!(status, 400, "{workflow}: {body}");
        let (status, scope) = call(
            &app,
            "GET",
            &format!("/api/workflows/{workflow}/scope"),
            None,
            None,
        )
        .await;
        assert_eq!(status, 200, "{scope}");
        assert_eq!(scope["data"]["supported"], false, "{workflow}: {scope}");
        assert_eq!(scope["data"]["valid"], false, "{workflow}: {scope}");
    }
    let (_, scope) = call(&app, "GET", "/api/workflows/wf-a/scope", None, None).await;
    assert_eq!(scope["data"]["supported"], true, "{scope}");
}

/// KT-1270 review — a human can always withdraw a workflow's scope, even once
/// the workflow became global: the run that pinned the validated revision loses
/// it on its next call.
#[tokio::test]
async fn a_workflow_scope_stays_revocable_after_the_workflow_becomes_global() {
    let (app, _repos, db) = fixture_with_db().await;
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        None,
        Some(json!({"entries": [{"project_id": "p2", "access": "write"}]})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    pinned_run(&db, "wf-a", "run-before-global").await;
    let step = step_of("run-before-global");
    assert_eq!(edits_task_b(&app, step.value()).await, 200);

    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a",
        None,
        Some(json!({"project_scope": {"type": "All"}})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (_, scope) = call(&app, "GET", "/api/workflows/wf-a/scope", None, None).await;
    assert_eq!(scope["data"]["supported"], false, "{scope}");
    assert_eq!(
        scope["data"]["entries"].as_array().map(Vec::len),
        Some(1),
        "still listed: {scope}"
    );
    let (status, _) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        None,
        Some(json!({"entries": [{"project_id": "p2", "access": "write"}]})),
    )
    .await;
    assert_eq!(status, 400, "no new validation on an unsupported workflow");

    let agent = bridge_for("room-a");
    let (status, _) = call(
        &app,
        "POST",
        "/api/workflows/wf-a/scope/revoke",
        Some(agent.value()),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 403, "a bridge token cannot revoke either");
    let (status, body) = call(
        &app,
        "POST",
        "/api/workflows/wf-a/scope/revoke",
        None,
        Some(json!({"project_id": "p2"})),
    )
    .await;
    assert_eq!(status, 200, "the human withdraws it: {body}");
    assert_eq!(
        edits_task_b(&app, step.value()).await,
        403,
        "the pinned run lost it on its next call"
    );
}

/// KT-1336 — every project for a workflow's runs: read like any validated
/// entry, write only behind the typed confirmation and the scope seen, and
/// withdrawn on its own.
#[tokio::test]
async fn a_workflow_scope_may_reach_every_project() {
    let (app, _repos, db) = fixture_with_db().await;
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        None,
        Some(json!({"entries": [], "all": "read"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["all"], "read", "{body}");
    assert_eq!(body["data"]["valid"], true, "{body}");
    pinned_run(&db, "wf-a", "run-all").await;
    let step = step_of("run-all");
    let (status, _) = call(
        &app,
        "GET",
        "/api/planning/tasks/task-b",
        Some(step.value()),
        None,
    )
    .await;
    assert_eq!(status, 200, "every project reads");
    assert_eq!(
        edits_task_b(&app, step.value()).await,
        403,
        "read is not write"
    );

    let seen = body["data"]["fingerprint"].as_str().unwrap().to_owned();
    for (request, expected) in [
        (json!({"entries": [], "all": "write"}), 400),
        (
            json!({"entries": [], "all": "write", "confirm": "yes"}),
            400,
        ),
        (
            json!({"entries": [], "all": "write", "confirm": "ALL-PROJECTS-WRITE"}),
            400,
        ),
        (
            json!({"entries": [], "all": "write", "confirm": "ALL-PROJECTS-WRITE", "expected_fingerprint": "stale"}),
            409,
        ),
    ] {
        let (status, response) = call(
            &app,
            "PUT",
            "/api/workflows/wf-a/scope",
            None,
            Some(request.clone()),
        )
        .await;
        assert_eq!(status, expected, "{request}: {response}");
    }
    assert_eq!(
        edits_task_b(&app, step.value()).await,
        403,
        "no refused attempt opened anything"
    );
    let (status, body) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        None,
        Some(json!({"entries": [], "all": "write", "confirm": "ALL-PROJECTS-WRITE", "expected_fingerprint": seen})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        edits_task_b(&app, step.value()).await,
        200,
        "every project, planning write"
    );
    let (status, _) = call(
        &app,
        "POST",
        "/api/agent-api/call",
        Some(step.value()),
        Some(json!({"api_plugin_slug": "synthetic-api", "api_config_id": "cfg-b", "endpoint_path": "/x"})),
    )
    .await;
    assert_eq!(status, 403, "never another project's credentials");

    let agent = bridge_for("room-a");
    let (status, _) = call(
        &app,
        "PUT",
        "/api/workflows/wf-a/scope",
        Some(agent.value()),
        Some(json!({"entries": [], "all": "read"})),
    )
    .await;
    assert_eq!(status, 403, "a bridge token never sets it");
    let (status, body) = call(
        &app,
        "POST",
        "/api/workflows/wf-a/scope/revoke",
        None,
        Some(json!({"all": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["data"]["all"].is_null(), "{body}");
    assert_eq!(
        edits_task_b(&app, step.value()).await,
        403,
        "withdrawn on the next call"
    );
}
