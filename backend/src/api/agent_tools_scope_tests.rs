//! KT-1346 — an HTTP agent's Kronn tools obey the bridge token's rule: its
//! room's project, the room grants for the room's own agent, nothing for a
//! worker or a workflow step, and never another project's resource otherwise.
use super::*;
use std::sync::Arc;

struct World {
    state: AppState,
    task_a: String,
    task_b: String,
}

async fn two_projects() -> World {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    let ids = db.with_conn(|conn| {
        for project in ["a", "b"] {
            conn.execute("INSERT INTO projects (id,name,path,created_at,updated_at) VALUES (?1,?1,?1,'2026-10-17T00:00:00Z','2026-10-17T00:00:00Z')", [project])?;
        }
        for (id, project) in [("general", None), ("room-a", Some("a")), ("room-b", Some("b"))] {
            conn.execute("INSERT INTO discussions (id,title,project_id,created_at,updated_at) VALUES (?1,?1,?2,'2026-10-17T00:00:00Z','2026-10-17T00:00:00Z')", rusqlite::params![id, project])?;
        }
        let mut ids = Vec::new();
        for (title, project) in [("task-a", "a"), ("task-b", "b")] {
            let request: CreatePlanningTaskRequest = serde_json::from_value(json!({
                "title": title,
                "project_ids": [project],
            }))?;
            ids.push(crate::db::planning::create_task(conn, &request)?.summary.id);
        }
        Ok(ids)
    })
    .await
    .unwrap();
    let mut config = crate::core::config::default_config();
    config.encryption_secret = Some(crate::core::crypto::generate_secret());
    World {
        state: AppState::new_defaults(
            Arc::new(tokio::sync::RwLock::new(config)),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        ),
        task_a: ids[0].clone(),
        task_b: ids[1].clone(),
    }
}

async fn call(executor: &dyn ToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: name.into(),
            name: name.into(),
            arguments,
        })
        .await
}

fn room_agent(state: &AppState, room: &str) -> Arc<dyn ToolExecutor> {
    KronnToolExecutor::arc(
        state.clone(),
        Some(room.into()),
        AgentType::Ollama,
        None,
        None,
    )
}

async fn grant_write(state: &AppState, room: &str, project: &str) {
    let (room, project) = (room.to_owned(), project.to_owned());
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussion_scope::grant(
                conn,
                &room,
                Some(&project),
                crate::models::discussion_scope::ScopeAccess::Write,
                crate::models::discussion_scope::ScopeGrantSource::Link,
                "human",
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn grant_general(state: &AppState) {
    state
        .db
        .with_conn(|conn| {
            crate::db::discussion_scope::grant(
                conn,
                "general",
                None,
                crate::models::discussion_scope::ScopeAccess::Write,
                crate::models::discussion_scope::ScopeGrantSource::General,
                "human",
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

fn refused(outcome: &ToolOutcome) -> bool {
    !outcome.ok && outcome.content.to_string().contains("Kronn refused")
}

#[tokio::test]
async fn a_room_agent_reaches_only_its_project_planning() {
    let World {
        state,
        task_a,
        task_b,
    } = two_projects().await;
    let agent = room_agent(&state, "room-a");
    let cases = [
        ("task_create", json!({"title": "x", "project_ids": ["b"]})),
        ("task_get", json!({"task_id": &task_b})),
        ("task_update", json!({"task_id": &task_b, "title": "taken"})),
        (
            "task_update",
            json!({"task_id": &task_a, "project_ids": ["b"]}),
        ),
        (
            "task_update_dod",
            json!({"task_id": &task_b, "dod_id": "d", "completed": true}),
        ),
        (
            "task_add_blocker",
            json!({"task_id": &task_b, "blocker_task_id": &task_a}),
        ),
        (
            "task_add_blocker",
            json!({"task_id": &task_a, "blocker_task_id": &task_b}),
        ),
        ("task_link_discussion", json!({"task_id": &task_b})),
    ];
    for (tool, arguments) in cases {
        let outcome = call(agent.as_ref(), tool, arguments.clone()).await;
        assert!(refused(&outcome), "{tool} {arguments}: {}", outcome.content);
    }
    let created = call(agent.as_ref(), "task_create", json!({"title": "mine"})).await;
    assert!(created.ok, "{}", created.content);
    let own = call(agent.as_ref(), "task_get", json!({"task_id": &task_a})).await;
    assert!(own.ok, "{}", own.content);
    let title: String = state
        .db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT title FROM planning_tasks WHERE id = ?1",
                [&task_b],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(title, "task-b", "a refused update never reaches the task");
}

#[tokio::test]
async fn a_room_without_a_project_reads_no_project_task() {
    let World { state, task_b, .. } = two_projects().await;
    let agent = room_agent(&state, "general");
    let outcome = call(agent.as_ref(), "task_get", json!({"task_id": &task_b})).await;
    assert!(refused(&outcome), "{}", outcome.content);
    let created = call(
        agent.as_ref(),
        "task_create",
        json!({"title": "x", "project_ids": ["b"]}),
    )
    .await;
    assert!(refused(&created), "{}", created.content);
}

#[tokio::test]
async fn a_room_write_grant_opens_planning_and_nothing_else() {
    let World { state, task_b, .. } = two_projects().await;
    grant_write(&state, "room-a", "b").await;
    let agent = room_agent(&state, "room-a");
    let created = call(
        agent.as_ref(),
        "task_create",
        json!({"title": "x", "project_ids": ["b"]}),
    )
    .await;
    assert!(created.ok, "{}", created.content);
    let updated = call(
        agent.as_ref(),
        "task_update",
        json!({"task_id": &task_b, "title": "granted"}),
    )
    .await;
    assert!(updated.ok, "{}", updated.content);
    // A planning grant never opens another project's saved calls.
    let draft = call(
        agent.as_ref(),
        "qa_create_draft",
        json!({"name": "x", "api_plugin_slug": "s", "api_config_id": "c", "api_endpoint_path": "/", "project_id": "b"}),
    )
    .await;
    assert!(refused(&draft), "{}", draft.content);
}

#[tokio::test]
async fn a_worker_or_a_step_never_takes_the_rooms_grants() {
    let World {
        state,
        task_a,
        task_b,
    } = two_projects().await;
    grant_write(&state, "room-a", "b").await;
    let worker = KronnToolExecutor::arc_for_worker_room(
        state.clone(),
        Some("room-a".into()),
        AgentType::Ollama,
        None,
        None,
        None,
    );
    let step = KronnToolExecutor::workflow_arc(
        state.clone(),
        Some("a".into()),
        TEST_STEP_RUN_ID.into(),
        "step".into(),
    );
    for executor in [worker, step] {
        let outcome = call(executor.as_ref(), "task_get", json!({"task_id": &task_b})).await;
        assert!(refused(&outcome), "{}", outcome.content);
        let own = call(executor.as_ref(), "task_get", json!({"task_id": &task_a})).await;
        assert!(own.ok, "{}", own.content);
    }
}

#[tokio::test]
async fn a_saved_call_cannot_be_moved_or_created_in_another_project() {
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "room-a");
    for tool in ["qa_create_draft", "qe_create_draft", "qp_create_draft"] {
        let outcome = call(
            agent.as_ref(),
            tool,
            json!({"name": "x", "project_id": "b"}),
        )
        .await;
        assert!(refused(&outcome), "{tool}: {}", outcome.content);
    }
    let created = call(
        agent.as_ref(),
        "qe_create_draft",
        json!({"name": "x", "command": "ls", "output_format": "text"}),
    )
    .await;
    assert!(created.ok, "{}", created.content);
    assert_eq!(
        created.content["project_id"], "a",
        "an unnamed project is the room's"
    );
    let id = created.content["id"].as_str().unwrap().to_owned();
    for project in [json!("b"), Value::Null] {
        let moved = call(
            agent.as_ref(),
            "qe_update",
            json!({"quick_exec_id": &id, "project_id": project}),
        )
        .await;
        assert!(refused(&moved), "{project}: {}", moved.content);
    }
}

#[tokio::test]
async fn a_shared_workflow_or_page_is_read_only_to_a_project_room() {
    let World { state, .. } = two_projects().await;
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflows(id, name, trigger_json, steps_json, created_at, updated_at) \
                 VALUES ('shared-wf', 'shared', '{\"type\":\"Manual\"}', '[]', '2026-10-18T00:00:00Z', '2026-10-18T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    grant_general(&state).await;
    let general = room_agent(&state, "general");
    let page = call(
        general.as_ref(),
        "page_create",
        json!({"title": "Shared", "slug": "shared", "html": "<h1>x</h1>", "datasets": []}),
    )
    .await;
    assert!(page.ok, "{}", page.content);
    assert!(page.content["project_id"].is_null(), "{}", page.content);
    let agent = room_agent(&state, "room-a");
    let cases = [
        (
            "workflow_update",
            json!({"workflow_id": "shared-wf", "name": "taken"}),
        ),
        (
            "page_update_html",
            json!({"page_id": "shared", "html": "<h1>taken</h1>"}),
        ),
        (
            "page_add_dataset",
            json!({"page_id": "shared", "name": "x", "kind": "snapshot"}),
        ),
    ];
    for (tool, arguments) in cases {
        let outcome = call(agent.as_ref(), tool, arguments.clone()).await;
        assert!(refused(&outcome), "{tool} {arguments}: {}", outcome.content);
    }
    let read = call(agent.as_ref(), "page_get", json!({"page_id": "shared"})).await;
    assert!(read.ok, "{}", read.content);
}

#[tokio::test]
async fn a_tool_without_an_access_rule_is_refused() {
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "room-a");
    let outcome = call(agent.as_ref(), "task_unknown_write", json!({})).await;
    assert!(refused(&outcome), "{}", outcome.content);
}

#[tokio::test]
async fn a_task_execution_never_reads_another_projects_task() {
    let World { state, task_b, .. } = two_projects().await;
    let reference: String = state
        .db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT 'KT-' || task_number FROM planning_tasks WHERE id = ?1",
                [&task_b],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    for room in ["room-a", "general"] {
        let agent = room_agent(&state, room);
        for tool in ["task_exec_prepare", "task_exec_launch"] {
            let outcome = call(
                agent.as_ref(),
                tool,
                json!({
                    "task_reference": &reference,
                    "worker_scope_intent": "generic",
                    "worker": {"kind": "agent", "agent_type": "ClaudeCode"},
                }),
            )
            .await;
            assert!(refused(&outcome), "{room} {tool}: {}", outcome.content);
            assert!(
                !outcome.content.to_string().contains("task-b"),
                "{}",
                outcome.content
            );
        }
    }
}

#[tokio::test]
async fn a_room_without_a_project_writes_the_general_resources_only_once_a_human_allowed_it() {
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "general");
    let page = json!({"title": "G", "slug": "g", "html": "<h1>x</h1>", "datasets": []});
    let refused_page = call(agent.as_ref(), "page_create", page.clone()).await;
    assert!(refused(&refused_page), "{}", refused_page.content);
    assert!(refused_page.content.to_string().contains("General · write"));
    let refused_qp = call(
        agent.as_ref(),
        "qp_create_draft",
        json!({"name": "x", "prompt_template": "Hi"}),
    )
    .await;
    assert!(refused(&refused_qp), "{}", refused_qp.content);
    grant_general(&state).await;
    // The grant is read on every call: the same run may write now.
    let created = call(agent.as_ref(), "page_create", page).await;
    assert!(created.ok, "{}", created.content);
    let edited = call(
        agent.as_ref(),
        "page_update_html",
        json!({"page_id": "g", "html": "<h1>y</h1>"}),
    )
    .await;
    assert!(edited.ok, "{}", edited.content);
    // A general resource the room did not create stays the human's.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO quick_prompts(id, name, prompt_template, project_id, created_at, updated_at) \
                 VALUES ('human-qp', 'human', 'x', NULL, '2026-10-18T00:00:00Z', '2026-10-18T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let foreign = call(
        agent.as_ref(),
        "qp_update",
        json!({"qp_id": "human-qp", "name": "taken"}),
    )
    .await;
    assert!(refused(&foreign), "{}", foreign.content);
    // A project room never gets the general grant.
    let denied = state
        .db
        .with_conn(|conn| {
            Ok(crate::db::discussion_scope::grant(
                conn,
                "room-a",
                None,
                crate::models::discussion_scope::ScopeAccess::Write,
                crate::models::discussion_scope::ScopeGrantSource::General,
                "human",
            )
            .is_err())
        })
        .await
        .unwrap();
    assert!(
        denied,
        "a project room writes its project, not the general resources"
    );
}

#[tokio::test]
async fn the_room_s_own_task_still_reaches_task_execution() {
    let World { state, task_a, .. } = two_projects().await;
    let reference: String = state
        .db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT 'KT-' || task_number FROM planning_tasks WHERE id = ?1",
                [&task_a],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    let agent = room_agent(&state, "room-a");
    let outcome = call(
        agent.as_ref(),
        "task_exec_prepare",
        json!({
            "task_reference": &reference,
            "worker_scope_intent": "generic",
            "worker": {"kind": "agent", "agent_type": "ClaudeCode"},
        }),
    )
    .await;
    assert!(!refused(&outcome), "{}", outcome.content);
    assert_eq!(outcome.content["project_id"], "a", "{}", outcome.content);
}

#[tokio::test]
async fn the_general_grant_edits_only_the_rooms_own_pages_workflows_and_saved_calls() {
    let World { state, .. } = two_projects().await;
    grant_general(&state).await;
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflows(id, name, trigger_json, steps_json, created_at, updated_at) \
                 VALUES ('human-wf', 'human', '{\"type\":\"Manual\"}', '[]', '2026-10-18T00:00:00Z', '2026-10-18T00:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO quick_execs(id, name, project_id, command, created_at, updated_at) \
                 VALUES ('human-qe', 'human', NULL, 'echo', '2026-10-18T00:00:00Z', '2026-10-18T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let agent = room_agent(&state, "general");
    let earlier_launch = room_agent(&state, "general");
    // A Page another launch of the room created is the room's; one written in
    // the database by a human is not.
    let page = call(
        earlier_launch.as_ref(),
        "page_create",
        json!({"title": "Room", "slug": "room-page", "html": "<h1>x</h1>", "datasets": []}),
    )
    .await;
    assert!(page.ok, "{}", page.content);
    let edited = call(
        agent.as_ref(),
        "page_update_html",
        json!({"page_id": "room-page", "html": "<h1>y</h1>"}),
    )
    .await;
    assert!(edited.ok, "the room's own Page: {}", edited.content);

    let workflow = call(
        agent.as_ref(),
        "workflow_create_draft",
        json!({"name": "Mine", "trigger": {"type": "Manual"}, "steps": [{"name": "data", "step_type": {"type": "JsonData"}, "json_data_payload": {"items": [1]}}]}),
    )
    .await;
    assert!(workflow.ok, "{}", workflow.content);
    assert!(
        workflow.content["project_id"].is_null(),
        "{}",
        workflow.content
    );
    let id = workflow.content["id"].as_str().unwrap().to_owned();
    let own = call(
        agent.as_ref(),
        "workflow_update",
        json!({"workflow_id": id, "name": "Renamed"}),
    )
    .await;
    assert!(own.ok, "the room's own workflow: {}", own.content);
    let foreign = call(
        agent.as_ref(),
        "workflow_update",
        json!({"workflow_id": "human-wf", "name": "Taken"}),
    )
    .await;
    assert!(refused(&foreign), "{}", foreign.content);

    // A Quick Exec is never general from a room without a project.
    let qe = call(
        agent.as_ref(),
        "qe_update",
        json!({"quick_exec_id": "human-qe", "name": "Taken"}),
    )
    .await;
    assert!(!qe.ok, "{}", qe.content);
}

#[tokio::test]
async fn a_room_agent_reads_its_reach_and_asks_a_human_with_room_scope() {
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "room-a");
    let reach = call(agent.as_ref(), "room_scope", json!({})).await;
    assert!(reach.ok, "{}", reach.content);
    assert_eq!(
        reach.content["primary_project_id"], "a",
        "{}",
        reach.content
    );
    let asked = call(
        agent.as_ref(),
        "room_scope",
        json!({"project": "b", "access": "write", "reason": "the parent task spans b"}),
    )
    .await;
    assert!(asked.ok, "{}", asked.content);
    let pending: i64 = state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM discussion_scope_requests WHERE discussion_id = 'room-a' AND status = 'pending'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        pending, 1,
        "the request waits for a human; nothing is granted"
    );
    // The refusal on another project names the way out for a room agent.
    let refused_task = call(
        agent.as_ref(),
        "task_create",
        json!({"title": "x", "project_ids": ["b"]}),
    )
    .await;
    assert!(
        refused_task.content.to_string().contains("room_scope"),
        "{}",
        refused_task.content
    );
}

#[tokio::test]
async fn a_worker_or_a_step_is_told_room_scope_is_not_theirs() {
    let World { state, .. } = two_projects().await;
    let worker = KronnToolExecutor::arc_for_worker_room(
        state.clone(),
        Some("room-a".into()),
        AgentType::Ollama,
        None,
        None,
        None,
    );
    assert!(!worker
        .catalogue()
        .iter()
        .any(|tool| tool["function"]["name"] == "room_scope"));
    let step = KronnToolExecutor::workflow_arc(
        state.clone(),
        Some("a".into()),
        TEST_STEP_RUN_ID.into(),
        "step".into(),
    );
    assert!(!step
        .catalogue()
        .iter()
        .any(|tool| tool["function"]["name"] == "room_scope"));
    let direct = call(worker.as_ref(), "room_scope", json!({"project": "b"})).await;
    assert!(!direct.ok, "{}", direct.content);
}

#[test]
fn a_project_refusal_names_the_way_out_for_its_caller() {
    use crate::core::bridge_token::{project_refusal, BridgeScope};
    let room = BridgeScope {
        discussion_ids: vec!["room".into()],
        room_grant_discussions: vec!["room".into()],
        ..Default::default()
    };
    let step = BridgeScope {
        discussion_ids: vec!["room".into()],
        workflow_run_id: Some("run".into()),
        ..Default::default()
    };
    let worker = BridgeScope {
        discussion_ids: vec!["child".into()],
        task_execution_id: Some("exec".into()),
        ..Default::default()
    };
    assert!(project_refusal(&room, false).contains("room_scope"));
    assert!(project_refusal(&room, true).contains("write"));
    let for_step = project_refusal(&step, false);
    assert!(
        for_step.contains("workflow's scope") && for_step.contains("Projects accessible"),
        "{for_step}"
    );
    assert!(!for_step.contains("room_scope"), "{for_step}");
    let for_worker = project_refusal(&worker, false);
    assert!(!for_worker.contains("room_scope"), "{for_worker}");
}
