//! KT-1449 — a normal run starts its shadow once; the shadow runs the same
//! steps on the same inputs and skips what acts outside Kronn.

use super::*;
use crate::models::{
    LivePageWriteOperation, PublishPageDataConfig, PublishPageDataWrite, WorkflowShadow,
};

async fn state_with_page() -> (crate::AppState, TokensConfig, AgentsConfig) {
    let (state, tokens, agents) = test_state_and_configs();
    state
        .db
        .with_conn(|conn| {
            let now = chrono::Utc::now();
            crate::db::live_pages::create_live_page(
                conn,
                &crate::models::LivePage {
                    id: "page-s".into(),
                    project_id: None,
                    title: "Shadow".into(),
                    slug: "shadow".into(),
                    current_revision_id: "rev-s".into(),
                    data_revision: 0,
                    created_at: now,
                    updated_at: now,
                    last_published_at: None,
                    pinned: false,
                    archived: false,
                },
                &crate::models::LivePageRevision {
                    id: "rev-s".into(),
                    page_id: "page-s".into(),
                    revision: 1,
                    html: "<main></main>".into(),
                    created_by_agent: None,
                    created_at: now,
                },
                &[crate::models::CreateLivePageDataset {
                    name: "etat".into(),
                    kind: crate::models::LivePageDatasetKind::Snapshot,
                    initial: Some(serde_json::json!({ "v": 0 })),
                    schema: None,
                    max_points: None,
                    max_age_days: None,
                }],
                None,
            )
        })
        .await
        .unwrap();
    (state, tokens, agents)
}

fn shadowed_workflow() -> Workflow {
    let mut publish = fake_step("publie");
    publish.step_type = StepType::PublishPageData;
    publish.external_effect = true;
    publish.page_publish = Some(PublishPageDataConfig {
        page_id: "page-s".into(),
        writes: vec![PublishPageDataWrite {
            dataset: "etat".into(),
            operation: LivePageWriteOperation::Replace,
            value_from: "steps.lit.data".into(),
            observed_at: None,
            dedupe_key: None,
            key_field: None,
        }],
    });
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-shadow".into();
    wf.concurrency_limit = Some(1);
    wf.steps = vec![
        json_data_step("lit", serde_json::json!({ "v": 1 })),
        publish,
    ];
    wf.shadow = Some(WorkflowShadow {
        enabled: true,
        ..Default::default()
    });
    wf
}

async fn data_revision(state: &crate::AppState) -> u64 {
    state
        .db
        .with_read_conn(|conn| crate::db::live_pages::get_live_page(conn, "page-s"))
        .await
        .unwrap()
        .unwrap()
        .page
        .data_revision
}

async fn finished_shadow_of(state: &crate::AppState, normal: &str) -> WorkflowRun {
    for _ in 0..200 {
        let id = normal.to_string();
        let found = state
            .db
            .with_read_conn(move |conn| {
                let Some(shadow) = crate::db::workflows::shadow_run_of(conn, &id)? else {
                    return Ok(None);
                };
                crate::db::workflows::get_run(conn, &shadow)
            })
            .await
            .unwrap();
        if let Some(run) =
            found.filter(|run| !matches!(run.status, RunStatus::Pending | RunStatus::Running))
        {
            return run;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the shadow run never finished");
}

#[tokio::test]
async fn a_shadow_runs_alongside_and_skips_what_acts_outside_kronn() {
    let (state, tokens, agents) = state_with_page().await;
    let wf = shadowed_workflow();
    let mut run = pending_run("run-normal", &wf.id);
    let (wf_db, run_db) = (wf.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &wf_db)?;
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);

    let shadow = finished_shadow_of(&state, "run-normal").await;
    assert_eq!(
        shadow.status,
        RunStatus::Success,
        "{:?}",
        shadow.step_results
    );
    assert!(crate::workflows::shadow::is_shadow(&shadow));
    let skipped = shadow
        .step_results
        .iter()
        .find(|s| s.step_name == "publie")
        .unwrap();
    assert!(
        skipped.output.contains("SKIPPED_SHADOW"),
        "{}",
        skipped.output
    );
    // Only the normal run published, despite the concurrency limit of 1.
    assert_eq!(data_revision(&state).await, 1);

    // Running the normal run's definition again on the shadow never starts a
    // shadow of the shadow, and a normal run gets one shadow only.
    crate::workflows::shadow::spawn_if_wanted(&state, &wf, &run).await;
    crate::workflows::shadow::spawn_if_wanted(&state, &wf, &shadow).await;
    let shadows: u32 = state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM workflow_runs WHERE json_extract(trigger_context, '$.__variant__') = 'shadow'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(shadows, 1);
}

#[tokio::test]
async fn a_shadow_covers_only_the_projects_it_names() {
    let (state, _, _) = state_with_page().await;
    let mut wf = shadowed_workflow();
    wf.shadow.as_mut().unwrap().project_ids = vec!["p-other".into()];
    let mut run = pending_run("run-p1", &wf.id);
    run.project_id = Some("p1".into());
    crate::workflows::shadow::spawn_if_wanted(&state, &wf, &run).await;
    let found = state
        .db
        .with_read_conn(|conn| crate::db::workflows::shadow_run_of(conn, "run-p1"))
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn a_shadow_swaps_the_skills_its_variant_names() {
    let (state, _, _) = test_state_and_configs();
    let mut wf = shadowed_workflow();
    let mut agent = fake_step("revue");
    agent.skill_ids = vec!["custom-review".into(), "rust".into()];
    wf.steps.push(agent);
    wf.shadow.as_mut().unwrap().skills =
        [("custom-review".to_string(), "custom-review@2".to_string())].into();
    crate::workflows::shadow::apply_versions(&state, &mut wf)
        .await
        .unwrap();
    assert_eq!(wf.steps[2].skill_ids, vec!["custom-review@2", "rust"]);
}

#[tokio::test]
async fn a_shadow_swaps_the_skills_a_step_inherits_from_its_quick_prompt() {
    let (state, _, _) = test_state_and_configs();
    let prompt: crate::models::QuickPrompt = serde_json::from_value(serde_json::json!({
        "id": "qp-review", "name": "qp-review", "icon": "P", "prompt_template": "Review",
        "variables": [], "agent": "ClaudeCode", "skill_ids": ["custom-review", "rust"],
        "created_at": Utc::now(), "updated_at": Utc::now()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &prompt))
        .await
        .unwrap();
    let mut wf = shadowed_workflow();
    let mut agent = fake_step("revue");
    agent.quick_prompt_id = Some("qp-review".into());
    wf.steps.push(agent);
    wf.shadow.as_mut().unwrap().skills =
        [("custom-review".to_string(), "custom-review@2".to_string())].into();
    crate::workflows::shadow::apply_versions(&state, &mut wf)
        .await
        .unwrap();
    assert_eq!(wf.steps[2].skill_ids, vec!["custom-review@2", "rust"]);
}


#[test]
fn a_shadow_gets_its_own_worktree_unless_the_workflow_never_writes_the_checkout() {
    let mut wf = shadowed_workflow();
    wf.workspace_config = None;
    crate::workflows::shadow::isolate(&mut wf);
    assert!(workflow_requests_workspace(wf.workspace_config.as_ref()));

    let mut hooked = shadowed_workflow();
    hooked.workspace_config = Some(crate::models::WorkspaceConfig {
        hooks: Default::default(),
        require_isolation: false,
        main_tree_read_only: false,
        base_ref: Some("origin/main".into()),
    });
    crate::workflows::shadow::isolate(&mut hooked);
    let config = hooked.workspace_config.unwrap();
    assert!(config.require_isolation);
    assert_eq!(config.base_ref.as_deref(), Some("origin/main"));

    let mut reader = shadowed_workflow();
    reader.workspace_config = Some(crate::models::WorkspaceConfig {
        hooks: Default::default(),
        require_isolation: false,
        main_tree_read_only: true,
        base_ref: None,
    });
    crate::workflows::shadow::isolate(&mut reader);
    assert!(!workflow_requests_workspace(
        reader.workspace_config.as_ref()
    ));
}
