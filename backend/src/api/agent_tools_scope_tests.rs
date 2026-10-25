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
    // The prompt names no ungranted project (KT-1417): this is where the agent
    // learns the requestable ones, by name, on demand.
    assert_eq!(
        reach.content["requestable"],
        json!(["b"]),
        "{}",
        reach.content
    );
    let general_agent = room_agent(&state, "general");
    let general = call(general_agent.as_ref(), "room_scope", json!({})).await;
    assert!(general.ok, "{}", general.content);
    assert_eq!(
        general.content["requestable"],
        json!([]),
        "{}",
        general.content
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
async fn a_granted_project_leaves_the_requestable_list() {
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "room-a");
    grant_write(&state, "room-a", "b").await;
    let reach = call(agent.as_ref(), "room_scope", json!({})).await;
    assert!(reach.ok, "{}", reach.content);
    assert_eq!(reach.content["requestable"], json!([]), "{}", reach.content);
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
    // The editor's « Test step »: a project, no room, run or task.
    let test_step = BridgeScope {
        project_id: Some("a".into()),
        ..Default::default()
    };
    let for_test = project_refusal(&test_step, false);
    assert!(
        for_test.contains("its project only") && for_test.contains("Projects accessible"),
        "{for_test}"
    );
    assert!(
        !for_test.contains("principal") && !for_test.contains("room_scope"),
        "{for_test}"
    );
    let for_worker = project_refusal(&worker, false);
    assert!(!for_worker.contains("room_scope"), "{for_worker}");
}

#[tokio::test]
async fn the_step_test_button_is_told_about_the_workflow_scope() {
    let World { state, .. } = two_projects().await;
    let step = KronnToolExecutor::workflow_arc(
        state.clone(),
        Some("a".into()),
        TEST_STEP_RUN_ID.into(),
        "step".into(),
    );
    // A step test may not create tasks; the gate's refusal on another project
    // is checked on a read it may do.
    let refused = call(step.as_ref(), "task_list", json!({"project_id": "b"})).await;
    let text = refused.content.to_string();
    assert!(!refused.ok, "{text}");
    assert!(
        text.contains("Projects accessible") && !text.contains("principal"),
        "{text}"
    );
}

/// KT-1434 — an HTTP agent reads versions as a CLI agent does: its project's
/// skill or Quick Prompt and the shared ones, never another project's.
#[tokio::test]
#[serial_test::serial]
async fn versions_are_read_within_the_room_s_reach() {
    let _data = crate::core::config::TestDataDir::new();
    let World { state, .. } = two_projects().await;
    let skill = |name: &str, project: Option<&str>, content: &str| {
        crate::core::skills::save_custom_skill(
            name,
            "desc",
            "🔎",
            &crate::models::SkillCategory::Domain,
            content,
            None,
            None,
            project,
        )
        .unwrap()
    };
    let own = skill("Own review", Some("a"), "rule one");
    crate::core::skills::update_custom_skill(
        &own,
        "Own review",
        "desc",
        "🔎",
        &crate::models::SkillCategory::Domain,
        "rule two",
        None,
        None,
        None,
    )
    .unwrap();
    let other = skill("Other review", Some("b"), "secret rule");
    state
        .db
        .with_conn(|conn| {
            for (id, project) in [("qp-own", Some("a")), ("qp-other", Some("b"))] {
                conn.execute(
                    "INSERT INTO quick_prompts(id, name, prompt_template, project_id, created_at, updated_at) \
                     VALUES (?1, ?1, 'v1', ?2, '2026-10-25T00:00:00Z', '2026-10-25T00:00:00Z')",
                    rusqlite::params![id, project],
                )?;
                let saved = crate::db::quick_prompts::get_quick_prompt(conn, id)?
                    .ok_or_else(|| anyhow::anyhow!("no {id}"))?;
                crate::db::quick_prompts::snapshot_quick_prompt_version(conn, &saved)?;
                conn.execute(
                    "UPDATE quick_prompts SET prompt_template = 'v2' WHERE id = ?1",
                    [id],
                )?;
                let saved = crate::db::quick_prompts::get_quick_prompt(conn, id)?
                    .ok_or_else(|| anyhow::anyhow!("no {id}"))?;
                crate::db::quick_prompts::snapshot_quick_prompt_version(conn, &saved)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let agent = room_agent(&state, "room-a");

    let read = call(agent.as_ref(), "skill_get", json!({"skill_id": own})).await;
    assert!(read.ok, "{}", read.content);
    assert_eq!(read.content["content"], "rule two");
    let versions = read.content["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2, "{}", read.content);
    assert_eq!(versions[0]["current"], true);
    assert!(
        versions[0].get("content").is_none(),
        "a list carries no body"
    );
    let first = call(
        agent.as_ref(),
        "skill_get",
        json!({"skill_id": format!("{own}@1")}),
    )
    .await;
    assert!(first.ok, "{}", first.content);
    assert_eq!(first.content["content"], "rule one");

    let read = call(agent.as_ref(), "qp_get", json!({"qp_id": "qp-own"})).await;
    assert!(read.ok, "{}", read.content);
    assert_eq!(read.content["prompt_template"], "v2");
    assert_eq!(read.content["versions"].as_array().unwrap().len(), 2);
    let first = call(
        agent.as_ref(),
        "qp_get",
        json!({"qp_id": "qp-own", "version": 1}),
    )
    .await;
    assert!(first.ok, "{}", first.content);
    assert_eq!(first.content["prompt_template"], "v1");
    let missing = call(
        agent.as_ref(),
        "qp_get",
        json!({"qp_id": "qp-own", "version": 9}),
    )
    .await;
    assert!(!missing.ok, "{}", missing.content);

    for (tool, arguments) in [
        ("skill_get", json!({"skill_id": other.clone()})),
        ("skill_get", json!({"skill_id": format!("{other}@1")})),
        ("qp_get", json!({"qp_id": "qp-other"})),
        ("qp_get", json!({"qp_id": "qp-other", "version": 1})),
    ] {
        let outcome = call(agent.as_ref(), tool, arguments.clone()).await;
        assert!(refused(&outcome), "{tool} {arguments}: {}", outcome.content);
        assert!(!outcome.content.to_string().contains("secret rule"));
    }
}

fn workflow_loading(id: &str, skill: &str) -> crate::models::Workflow {
    use crate::models::*;
    Workflow {
        refs: Default::default(),
        retention: None,
        shadow: None,
        project_scope: None,
        pinned: false,
        id: id.into(),
        name: id.into(),
        project_id: Some("a".into()),
        trigger: WorkflowTrigger::Manual,
        steps: vec![WorkflowStep {
            name: "agent".into(),
            skill_ids: vec![skill.into()],
            ..WorkflowStep::default()
        }],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        concurrency_key: None,
        guards: None,
        artifacts: Default::default(),
        on_failure: vec![],
        exec_allowlist: vec![],
        variables: vec![],
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// An HTTP agent writes skills as a CLI agent does: in its project, a new
/// version per change, never a builtin, another project's or a human's
/// `allowed_tools`; an unpinned workflow user is disabled, a pinned one is not.
#[tokio::test]
#[serial_test::serial]
async fn skills_are_written_like_quick_prompts() {
    let _data = crate::core::config::TestDataDir::new();
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "room-a");
    let fields = json!({
        "name": "Agent review", "description": "d", "icon": "🔎",
        "category": "Domain", "content": "rule one", "allowed_tools": "Bash",
    });
    let created = call(agent.as_ref(), "skill_create", fields.clone()).await;
    assert!(created.ok, "{}", created.content);
    assert_eq!(created.content["project_id"], "a", "{}", created.content);
    assert!(created.content.get("allowed_tools").is_none());
    let own = created.content["id"].as_str().unwrap().to_owned();
    let mut elsewhere = fields.clone();
    elsewhere["project_id"] = json!("b");
    let refused_create = call(agent.as_ref(), "skill_create", elsewhere).await;
    assert!(refused(&refused_create), "{}", refused_create.content);

    let human = crate::core::skills::save_custom_skill(
        "Human tools",
        "d",
        "🔎",
        &crate::models::SkillCategory::Domain,
        "body",
        None,
        Some("Read"),
        Some("a"),
    )
    .unwrap();
    let other = crate::core::skills::save_custom_skill(
        "Other review",
        "d",
        "🔎",
        &crate::models::SkillCategory::Domain,
        "secret",
        None,
        None,
        Some("b"),
    )
    .unwrap();
    let shared = crate::core::skills::save_custom_skill(
        "Shared review",
        "d",
        "🔎",
        &crate::models::SkillCategory::Domain,
        "shared",
        None,
        None,
        None,
    )
    .unwrap();
    let workflows = [
        ("wf-follows", own.clone()),
        ("wf-pinned", format!("{own}@1")),
    ];
    state
        .db
        .with_conn(move |conn| {
            for (id, skill) in &workflows {
                crate::db::workflows::insert_workflow(conn, &workflow_loading(id, skill))?;
            }
            Ok(())
        })
        .await
        .unwrap();

    let edit = json!({"skill_id": own, "content": "rule two"});
    let edited = call(agent.as_ref(), "skill_update", edit.clone()).await;
    assert!(edited.ok, "{}", edited.content);
    assert_eq!(edited.content["content"], "rule two");
    assert_eq!(
        edited.content["name"], "Agent review",
        "untouched fields stay"
    );
    let again = call(agent.as_ref(), "skill_update", edit).await;
    assert!(again.ok, "{}", again.content);
    let versions = crate::core::skill_versions::history(&own).unwrap();
    assert_eq!(versions.len(), 2, "an unchanged save adds no version");
    let enabled = state
        .db
        .with_conn(|conn| {
            Ok([
                crate::db::workflows::get_workflow(conn, "wf-follows")?,
                crate::db::workflows::get_workflow(conn, "wf-pinned")?,
            ])
        })
        .await
        .unwrap()
        .map(|workflow| workflow.is_some_and(|workflow| workflow.enabled));
    assert!(!enabled[0], "the bare id follows the agent's edit");
    assert!(enabled[1], "a pinned version is not affected");

    let kept = call(
        agent.as_ref(),
        "skill_update",
        json!({"skill_id": human, "content": "new body", "allowed_tools": "Bash(*)"}),
    )
    .await;
    assert!(kept.ok, "{}", kept.content);
    assert_eq!(kept.content["allowed_tools"], "Read", "{}", kept.content);

    for target in [other.as_str(), shared.as_str(), "rust", &format!("{own}@1")] {
        let outcome = call(
            agent.as_ref(),
            "skill_update",
            json!({"skill_id": target, "content": "taken"}),
        )
        .await;
        assert!(refused(&outcome), "{target}: {}", outcome.content);
    }
    assert_eq!(
        crate::core::skills::get_skill(&other).unwrap().content,
        "secret"
    );

    // A worker is not offered skill writes, like Quick Prompt writes.
    let worker = KronnToolExecutor::arc_for_worker_room(
        state.clone(),
        Some("room-a".into()),
        AgentType::Ollama,
        None,
        None,
        None,
    );
    let offered: Vec<String> = worker
        .catalogue()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect();
    assert!(!offered
        .iter()
        .any(|name| name == "skill_create" || name == "skill_update"));
    assert!(!offered.iter().any(|name| name == "qp_update"));
}

/// A room without a project creates a global skill only once a human granted
/// « General · write », then edits only the ones it created.
#[tokio::test]
#[serial_test::serial]
async fn a_room_without_a_project_writes_only_its_own_global_skills() {
    let _data = crate::core::config::TestDataDir::new();
    let World { state, .. } = two_projects().await;
    let agent = room_agent(&state, "general");
    let fields = json!({
        "name": "General skill", "description": "d", "icon": "🔎",
        "category": "Domain", "content": "body",
    });
    let before = call(agent.as_ref(), "skill_create", fields.clone()).await;
    assert!(refused(&before), "{}", before.content);
    grant_general(&state).await;
    let created = call(agent.as_ref(), "skill_create", fields).await;
    assert!(created.ok, "{}", created.content);
    assert!(
        created.content.get("project_id").is_none(),
        "{}",
        created.content
    );
    let id = created.content["id"].as_str().unwrap().to_owned();
    let edited = call(
        agent.as_ref(),
        "skill_update",
        json!({"skill_id": id, "content": "edited"}),
    )
    .await;
    assert!(edited.ok, "{}", edited.content);
    let human = crate::core::skills::save_custom_skill(
        "Human global",
        "d",
        "🔎",
        &crate::models::SkillCategory::Domain,
        "body",
        None,
        None,
        None,
    )
    .unwrap();
    let foreign = call(
        agent.as_ref(),
        "skill_update",
        json!({"skill_id": human, "content": "taken"}),
    )
    .await;
    assert!(refused(&foreign), "{}", foreign.content);
}
