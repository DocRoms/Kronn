//! KT-1030 — the shipped « Ma Todo » workflows, run by the real runner: each
//! action publishes the new board itself and tells open Pages at once.

use super::*;

async fn run_with(
    state: &crate::AppState,
    tokens: &crate::models::TokensConfig,
    agents: &crate::models::AgentsConfig,
    workflow: &Workflow,
    values: &[(&str, &str)],
) -> WorkflowRun {
    let mut run = pending_run(&uuid::Uuid::new_v4().to_string(), &workflow.id);
    let run_db = run.clone();
    let secret = state.config.read().await.encryption_secret.clone().unwrap();
    let values: std::collections::HashMap<String, String> = values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_run(conn, &run_db)?;
            let key = crate::core::crypto::parse_secret(&secret).map_err(anyhow::Error::msg)?;
            crate::db::execution_variable_snapshots::insert(
                conn,
                crate::db::execution_variable_snapshots::NewSnapshot {
                    run_kind: "workflow",
                    run_id: &run_db.id,
                    project_id: None,
                    environment_ref: "project_mcp_configs",
                    resolved_at: chrono::Utc::now(),
                    retention_days: 30,
                    expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
                    values: &values,
                    provenance: &[],
                },
                &key,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    execute_run(
        state.clone(),
        workflow,
        &mut run,
        tokens,
        agents,
        None,
        None,
        None,
    )
    .await
    .expect("run");
    run
}

/// `(title, column)` of the published board, markers left out.
async fn published(state: &crate::AppState, page_id: &str) -> Vec<(String, String)> {
    let page_id = page_id.to_string();
    let detail = state
        .db
        .with_conn(move |conn| crate::db::live_pages::get_live_page(conn, &page_id))
        .await
        .unwrap()
        .unwrap();
    let dataset = detail
        .datasets
        .into_iter()
        .find(|d| d.dataset.name == "todo")
        .unwrap();
    dataset
        .dataset
        .current
        .unwrap_or_default()
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.get("sentinel").is_none())
        .map(|row| {
            (
                row["title"].as_str().unwrap().into(),
                row["column"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[tokio::test]
async fn add_move_and_done_work_end_to_end_on_the_shipped_definitions() {
    let (state, tokens, agents) = test_state_and_configs();
    let status = state
        .db
        .with_conn(|conn| {
            crate::core::default_todo::ensure_installed(conn, "en")?;
            crate::core::default_todo::status(conn)
        })
        .await
        .unwrap();
    let page_id = status.page_id.clone().unwrap();
    let ids = status.workflow_ids.clone();
    let workflows = state
        .db
        .with_conn(move |conn| {
            ids.iter()
                .map(|id| {
                    crate::db::workflows::get_workflow(conn, id)?
                        .ok_or_else(|| anyhow::anyhow!("workflow {id} is missing"))
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    let by_name = |suffix: &str| {
        workflows
            .iter()
            .find(|w| w.name.ends_with(suffix))
            .unwrap_or_else(|| panic!("no workflow {suffix}"))
            .clone()
    };
    let (add, mv, toggle) = (by_name("— add"), by_name("— move"), by_name("— done"));
    let mut events = state.ws_broadcast.subscribe();

    let first = run_with(
        &state,
        &tokens,
        &agents,
        &add,
        &[
            ("title", "Write the doc"),
            ("description", "## Why\n- users"),
        ],
    )
    .await;
    assert_eq!(first.status, RunStatus::Success, "{:?}", first.step_results);
    let second = run_with(
        &state,
        &tokens,
        &agents,
        &add,
        &[("title", "Ship it"), ("tags", "release")],
    )
    .await;
    assert_eq!(
        second.status,
        RunStatus::Success,
        "{:?}",
        second.step_results
    );
    assert_eq!(
        published(&state, &page_id).await,
        vec![
            ("Ship it".into(), "todo".into()),
            ("Write the doc".into(), "todo".into())
        ]
    );
    let mut pushes = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, crate::models::WsMessage::LivePageDataChanged { page_id: ref id, .. } if *id == page_id)
        {
            pushes += 1;
        }
    }
    assert_eq!(pushes, 2, "each publish tells open pages at once");

    let rows = state
        .db
        .with_conn(|conn| crate::workflows::task_board_step::board_rows(conn, "todo", 15))
        .await
        .unwrap();
    let doc = rows.iter().find(|r| r["title"] == "Write the doc").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let moved = run_with(
        &state,
        &tokens,
        &agents,
        &mv,
        &[
            ("task", &doc),
            ("before", "__col_in_progress__"),
            ("column", "__col_in_progress__"),
        ],
    )
    .await;
    assert_eq!(moved.status, RunStatus::Success, "{:?}", moved.step_results);
    assert_eq!(
        published(&state, &page_id).await,
        vec![
            ("Ship it".into(), "todo".into()),
            ("Write the doc".into(), "in_progress".into())
        ]
    );

    let done = run_with(&state, &tokens, &agents, &toggle, &[("task", &doc)]).await;
    assert_eq!(done.status, RunStatus::Success, "{:?}", done.step_results);
    assert_eq!(
        published(&state, &page_id).await,
        vec![
            ("Ship it".into(), "todo".into()),
            ("Write the doc".into(), "done".into())
        ]
    );

    // A refused move publishes nothing: the page keeps the board it had.
    let before = published(&state, &page_id).await;
    let refused = run_with(
        &state,
        &tokens,
        &agents,
        &mv,
        &[
            ("task", "KT-999999"),
            ("before", "__col_done__"),
            ("column", "__col_done__"),
        ],
    )
    .await;
    assert_eq!(refused.status, RunStatus::Failed);
    assert_eq!(published(&state, &page_id).await, before);
}

#[tokio::test]
async fn no_scheduled_refresh_is_installed() {
    let (state, _tokens, _agents) = test_state_and_configs();
    let triggers = state
        .db
        .with_conn(|conn| {
            crate::core::default_todo::ensure_installed(conn, "fr")?;
            let ids = crate::core::default_todo::status(conn)?.workflow_ids;
            ids.iter()
                .map(|id| {
                    Ok(crate::db::workflows::get_workflow(conn, id)?
                        .ok_or_else(|| anyhow::anyhow!("workflow {id} is missing"))?
                        .trigger)
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    assert_eq!(triggers.len(), 6);
    assert!(triggers
        .iter()
        .all(|t| matches!(t, WorkflowTrigger::Manual)));
}

#[tokio::test]
async fn a_title_only_edit_keeps_a_long_unicode_description_byte_for_byte() {
    let (state, tokens, agents) = test_state_and_configs();
    let description = format!("## Contexte\n{}\nfin 🦀👩‍💻\n", "é漢字🙂 ".repeat(500));
    assert!(description.chars().count() > 2000);
    let wanted = description.clone();
    let (status, task_id) = state
        .db
        .with_conn(move |conn| {
            let task = crate::db::planning::create_task(
                conn,
                &crate::models::CreatePlanningTaskRequest {
                    title: "Long one".into(),
                    discussion_id: None,
                    idempotency_key: None,
                    description: wanted,
                    status: crate::models::PlanningTaskStatus::Todo,
                    priority: Default::default(),
                    parent_id: None,
                    project_ids: vec![],
                    tags: vec!["todo".into()],
                    definition_of_done: vec![],
                    links: vec![],
                    actor: Default::default(),
                },
            )?;
            crate::core::default_todo::ensure_installed(conn, "en")?;
            Ok((crate::core::default_todo::status(conn)?, task.summary.id))
        })
        .await
        .unwrap();
    let page_id = status.page_id.clone().unwrap();
    let (edit, refresh) = {
        let ids = status.workflow_ids.clone();
        state
            .db
            .with_conn(move |conn| {
                let all = ids
                    .iter()
                    .map(|id| {
                        crate::db::workflows::get_workflow(conn, id)?
                            .ok_or_else(|| anyhow::anyhow!("workflow {id} is missing"))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let pick = |suffix: &str| {
                    all.iter()
                        .find(|w| w.name.ends_with(suffix))
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("no workflow ends with {suffix}"))
                };
                Ok((pick("— edit")?, pick("— refresh")?))
            })
            .await
            .unwrap()
    };
    let refreshed = run_with(&state, &tokens, &agents, &refresh, &[]).await;
    assert_eq!(
        refreshed.status,
        RunStatus::Success,
        "{:?}",
        refreshed.step_results
    );

    // The card is pre-filled exactly as a reader opening « ✎ » sees it.
    let (page, task) = (page_id.clone(), task_id.clone());
    let prefill = state
        .db
        .with_conn(move |conn| {
            let action = crate::db::live_page_actions::list_for_live_page(conn, &page)?
                .into_iter()
                .find(|a| a.action_ref == "todo-edit")
                .ok_or_else(|| anyhow::anyhow!("todo-edit is missing"))?;
            crate::db::live_page_actions::prefill_values(
                conn,
                &action.id,
                &std::collections::HashMap::from([("task".to_string(), task)]),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        prefill["description"], description,
        "the card shows the whole text"
    );

    let edited = run_with(
        &state,
        &tokens,
        &agents,
        &edit,
        &[
            ("task", &task_id),
            ("title", "Renamed"),
            ("description", &prefill["description"]),
        ],
    )
    .await;
    assert_eq!(
        edited.status,
        RunStatus::Success,
        "{:?}",
        edited.step_results
    );
    let stored = state
        .db
        .with_conn(move |conn| {
            crate::db::planning::get_task(conn, &task_id)?
                .ok_or_else(|| anyhow::anyhow!("the task is missing"))
        })
        .await
        .unwrap();
    assert_eq!(stored.summary.title, "Renamed");
    assert_eq!(stored.description.as_bytes(), description.as_bytes());
}
