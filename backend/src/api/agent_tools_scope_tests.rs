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
