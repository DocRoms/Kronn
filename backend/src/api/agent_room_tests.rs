//! KT-1244 — an HTTP Agent step with a `room_id` is a member of that room and
//! of no other, for as long as the step runs.
use super::*;
use crate::agents::tools::ToolExecutor;
use crate::workflows::step_room::{activate, StepRoomActivation};
use crate::workflows::template::TemplateContext;
use std::sync::Arc;

const NOW: &str = "2026-01-01T00:00:00Z";

async fn state() -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        for room in ["room-a", "room-b"] {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) VALUES (?1, ?1, ?2, ?2)",
                rusqlite::params![room, NOW],
            )?;
        }
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES ('workflow-1', 'Framing', '{}', '[]', ?1, ?1)",
            [NOW],
        )?;
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, started_at)
             VALUES ('run-1', 'workflow-1', 'Running', ?1)",
            [NOW],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

fn step(agent: AgentType, room_id: Option<&str>) -> crate::models::WorkflowStep {
    crate::models::WorkflowStep {
        name: "analyse".into(),
        step_type: crate::models::StepType::Agent,
        prompt_template: "go".into(),
        agent,
        room_id: room_id.map(str::to_owned),
        ..Default::default()
    }
}

async fn activation(state: &AppState, agent: AgentType) -> StepRoomActivation {
    let mut ctx = TemplateContext::new();
    ctx.set("room", "room-a");
    activate(
        &state.workflow_step_rooms,
        &state.db,
        "run-1",
        None,
        &step(agent, Some("{{room}}")),
        &ctx,
    )
    .await
    .unwrap()
    .expect("a step with a room is activated")
}

fn executor(state: &AppState, activation: &StepRoomActivation) -> Arc<dyn ToolExecutor> {
    KronnToolExecutor::workflow_room_arc(
        state.clone(),
        None,
        "run-1".into(),
        "analyse".into(),
        activation.member().cloned(),
    )
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

fn names(executor: &dyn ToolExecutor) -> Vec<String> {
    executor
        .catalogue()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

fn question(key: &str) -> String {
    format!(
        "Analysis done.\n\n```kronn-question\n{{\"version\":1,\"key\":\"{key}\",\
         \"question\":\"Which scope?\",\"options\":[{{\"id\":\"a\",\"label\":\"A\"}},\
         {{\"id\":\"b\",\"label\":\"B\"}}]}}\n```"
    )
}

async fn messages(state: &AppState, room: &'static str) -> Vec<crate::models::DiscussionMessage> {
    state
        .db
        .with_read_conn(move |conn| crate::db::discussions::list_messages(conn, room))
        .await
        .unwrap()
}

/// DoD 1 + 2 — LiteLLM and a custom provider (OpenRouter) are accepted; the
/// step posts its analysis and a question, the room's own API reads them back,
/// and the step's agent is their author.
#[tokio::test]
async fn an_http_step_posts_and_asks_in_its_room_as_its_agent() {
    for agent in [AgentType::LiteLlm, AgentType::Custom] {
        let state = state().await;
        let active = activation(&state, agent.clone()).await;
        let tools = executor(&state, &active);
        let offered = names(tools.as_ref());
        for tool in NAMES {
            assert!(offered.iter().any(|name| name == tool), "{agent:?}: {tool}");
        }

        let posted = call(
            tools.as_ref(),
            "disc_append",
            json!({"content": question("scope")}),
        )
        .await;
        assert!(posted.ok, "{}", posted.content);
        assert_eq!(posted.content["appended"], 1);
        assert!(
            posted.content.get("invalid_cards").is_none(),
            "{}",
            posted.content
        );

        // What the room's API says, not what the agent was told.
        let (status, axum::Json(listed)) = crate::api::discussion_questions::list(
            axum::extract::State(state.clone()),
            axum::extract::Path("room-a".into()),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let listed = listed.data.unwrap();
        assert_eq!(listed.pending_count, 1);
        assert_eq!(listed.questions[0].key, "scope");

        let read_back = call(
            tools.as_ref(),
            "disc_question_list",
            json!({"key": "scope"}),
        )
        .await;
        assert!(read_back.ok, "{}", read_back.content);
        assert_eq!(read_back.content["matching_count"], 1);
        assert_eq!(read_back.content["questions"][0]["state"], "pending");
        let pending = call(tools.as_ref(), "disc_question_list", json!({})).await;
        assert_eq!(pending.content["questions"][0]["key"], "scope");
        let absent = call(tools.as_ref(), "disc_question_list", json!({"key": "nope"})).await;
        assert_eq!(absent.content["matching_count"], 0);

        let thread = call(tools.as_ref(), "disc_read", json!({"limit": 5})).await;
        assert!(thread.ok, "{}", thread.content);
        assert_eq!(thread.content["total_messages"], 1);
        assert_eq!(thread.content["messages"][0]["role"], "Agent");
        assert_eq!(
            thread.content["messages"][0]["agent_type"],
            format!("{agent:?}")
        );

        let stored = messages(&state, "room-a").await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].agent_type, Some(agent.clone()));
        let (identity, running) = state
            .db
            .with_read_conn({
                let id = stored[0].id.clone();
                move |conn| crate::db::workflow_step_rooms::message_author(conn, "room-a", &id)
            })
            .await
            .unwrap()
            .expect("the message is the step's");
        assert_eq!(identity.step_name, "analyse");
        assert!(running);
        // KT-883 — the room shows the step working.
        let working = state
            .db
            .with_read_conn(|conn| {
                crate::db::workflow_step_rooms::list_active_for_discussion(conn, "room-a")
            })
            .await
            .unwrap();
        assert_eq!(working.len(), 1);
        assert_eq!(working[0].agent_type, agent);
        active.finish().await;
    }
}

/// DoD 3 — nothing of another discussion is read, listed or written, and the
/// refusal says so rather than quietly acting on the step's room.
#[tokio::test]
async fn the_room_tools_refuse_every_other_discussion() {
    let state = state().await;
    let active = activation(&state, AgentType::LiteLlm).await;
    let tools = executor(&state, &active);
    for field in ["discussion_id", "disc_id"] {
        for (tool, arguments) in [
            ("disc_read", json!({field: "room-b"})),
            ("disc_append", json!({field: "room-b", "content": "hello"})),
            ("disc_question_list", json!({field: "room-b"})),
            ("disc_append", json!({field: 7, "content": "hello"})),
        ] {
            let outcome = call(tools.as_ref(), tool, arguments).await;
            assert!(!outcome.ok, "{tool} {field}: {}", outcome.content);
            assert!(
                outcome.content["error"]
                    .as_str()
                    .unwrap()
                    .contains("not this step's room"),
                "{}",
                outcome.content
            );
        }
    }
    assert!(messages(&state, "room-b").await.is_empty());
    assert!(messages(&state, "room-a").await.is_empty());
    // Naming its own room, or nothing, is the same call.
    for named in [json!("room-a"), json!(""), Value::Null] {
        let outcome = call(
            tools.as_ref(),
            "disc_append",
            json!({"discussion_id": named, "content": "hello"}),
        )
        .await;
        assert!(outcome.ok, "{}", outcome.content);
    }
    assert_eq!(messages(&state, "room-a").await.len(), 3);
    assert!(messages(&state, "room-b").await.is_empty());
    active.finish().await;
}

/// DoD 4 — without a `room_id` the catalogue is the one a workflow step always
/// had, and the room tool names are not even callable.
#[tokio::test]
async fn a_step_without_a_room_keeps_its_catalogue() {
    let state = state().await;
    assert!(activate(
        &state.workflow_step_rooms,
        &state.db,
        "run-1",
        None,
        &step(AgentType::LiteLlm, None),
        &TemplateContext::new(),
    )
    .await
    .unwrap()
    .is_none());
    for project in [None, Some("project".to_string())] {
        let plain = KronnToolExecutor::workflow_arc(
            state.clone(),
            project.clone(),
            "run-1".into(),
            "analyse".into(),
        );
        let expected: Vec<String> = workflow_tool_catalogue(project.is_some())
            .into_iter()
            .chain(
                project
                    .is_some()
                    .then(workflow_workspace_tool_catalogue)
                    .unwrap_or_default(),
            )
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(names(plain.as_ref()), expected);
        assert!(!expected.iter().any(|name| NAMES.contains(&name.as_str())));
        let refused = call(plain.as_ref(), "disc_append", json!({"content": "hi"})).await;
        assert!(!refused.ok);
        assert!(refused.content["error"]
            .as_str()
            .unwrap()
            .contains("not allowed in workflow Agent steps"));
    }
    assert!(messages(&state, "room-a").await.is_empty());

    // With a room, the catalogue is that one plus exactly the room tools.
    let active = activation(&state, AgentType::LiteLlm).await;
    let with_room = names(executor(&state, &active).as_ref());
    let plain = names(
        KronnToolExecutor::workflow_arc(state.clone(), None, "run-1".into(), "analyse".into())
            .as_ref(),
    );
    let extra: Vec<&String> = with_room.iter().filter(|n| !plain.contains(n)).collect();
    assert_eq!(extra.len(), NAMES.len(), "{extra:?}");
    active.finish().await;
}

/// DoD 5 — the tools stop with the step, and with a backend restart.
#[tokio::test]
async fn the_room_tools_stop_with_the_step_and_at_boot() {
    let state = state().await;
    let active = activation(&state, AgentType::LiteLlm).await;
    let tools = executor(&state, &active);
    assert!(
        call(tools.as_ref(), "disc_append", json!({"content": "one"}))
            .await
            .ok
    );

    // A restart revokes the membership while nothing else knows the step died.
    state
        .db
        .with_conn(crate::db::workflow_step_rooms::revoke_all_after_restart)
        .await
        .unwrap();
    for (tool, arguments) in [
        ("disc_append", json!({"content": "two"})),
        ("disc_read", json!({})),
        ("disc_question_list", json!({})),
    ] {
        let outcome = call(tools.as_ref(), tool, arguments).await;
        assert!(!outcome.ok, "{tool}: {}", outcome.content);
        assert!(outcome.content["error"]
            .as_str()
            .unwrap()
            .contains("membership has ended"));
    }
    assert_eq!(messages(&state, "room-a").await.len(), 1);

    // The end of the step ends the membership, and the tools with it.
    let state = self::state().await;
    let active = activation(&state, AgentType::LiteLlm).await;
    let tools = executor(&state, &active);
    let session = active.member().unwrap().session_id.clone();
    let status = |state: &AppState, session: String| {
        let db = state.db.clone();
        async move {
            db.with_read_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT status FROM discussion_sessions WHERE session_id = ?1",
                    [session],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(status(&state, session.clone()).await, "active");
    active.finish().await;
    assert_eq!(status(&state, session).await, "left");
    let after = call(tools.as_ref(), "disc_append", json!({"content": "late"})).await;
    assert!(!after.ok, "{}", after.content);
    assert!(messages(&state, "room-a").await.is_empty());

    // A step dropped without a verdict (cancellation) ends it as well.
    let active = activation(&state, AgentType::Ollama).await;
    let tools = executor(&state, &active);
    drop(active);
    let mut refused = false;
    for _ in 0..100 {
        let outcome = call(tools.as_ref(), "disc_read", json!({})).await;
        if !outcome.ok {
            refused = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(refused, "a dropped step keeps no tools");
}
