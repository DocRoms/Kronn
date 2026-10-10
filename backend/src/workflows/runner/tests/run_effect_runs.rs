//! KT-1100 — a finished run is classified through the real executors: an
//! identical publish ends it as no-op, a changed one never does.

use super::*;
use crate::models::{
    LivePageWriteOperation, PublishPageDataConfig, PublishPageDataWrite, WorkflowRunOutcome,
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
                    id: "page-o".into(),
                    project_id: None,
                    title: "État".into(),
                    slug: "etat".into(),
                    current_revision_id: "rev-o".into(),
                    data_revision: 0,
                    created_at: now,
                    updated_at: now,
                    last_published_at: None,
                    pinned: false,
                    archived: false,
                },
                &crate::models::LivePageRevision {
                    id: "rev-o".into(),
                    page_id: "page-o".into(),
                    revision: 1,
                    html: "<main></main>".into(),
                    created_by_agent: None,
                    created_at: now,
                },
                &[crate::models::CreateLivePageDataset {
                    name: "etat".into(),
                    kind: crate::models::LivePageDatasetKind::Snapshot,
                    initial: Some(serde_json::json!({ "v": "avant" })),
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

fn publish_step(value_from: &str) -> WorkflowStep {
    let mut step = fake_step("publie");
    step.step_type = StepType::PublishPageData;
    step.page_publish = Some(PublishPageDataConfig {
        page_id: "page-o".into(),
        writes: vec![PublishPageDataWrite {
            dataset: "etat".into(),
            operation: LivePageWriteOperation::Replace,
            value_from: value_from.into(),
            observed_at: None,
            dedupe_key: None,
            key_field: None,
        }],
    });
    step
}

fn page_workflow(id: &str, value: serde_json::Value) -> Workflow {
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = id.into();
    wf.steps = vec![json_data_step("lit", value), publish_step("steps.lit.data")];
    wf
}

async fn run_once(
    state: &crate::AppState,
    tokens: &TokensConfig,
    agents: &AgentsConfig,
    wf: &Workflow,
    run_id: &str,
) -> WorkflowRun {
    let mut run = pending_run(run_id, &wf.id);
    let (wf_db, run_db) = (wf.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            if crate::db::workflows::get_workflow(conn, &wf_db.id)?.is_none() {
                crate::db::workflows::insert_workflow(conn, &wf_db)?;
            }
            crate::db::workflows::insert_run(conn, &run_db)
        })
        .await
        .unwrap();
    execute_run(
        state.clone(),
        wf,
        &mut run,
        tokens,
        agents,
        None,
        None,
        None,
    )
    .await
    .expect("a deterministic run must not error");
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &id))
        .await
        .unwrap()
        .expect("run row")
}

#[tokio::test]
async fn an_identical_publish_ends_as_no_op_and_a_changed_one_never_does() {
    let (state, tokens, agents) = state_with_page().await;
    let wf = page_workflow("wf-outcome", serde_json::json!({ "v": "après" }));

    let first = run_once(&state, &tokens, &agents, &wf, "run-changed").await;
    assert_eq!(
        first.outcome,
        Some(WorkflowRunOutcome::Changed),
        "the dataset content changed"
    );

    let second = run_once(&state, &tokens, &agents, &wf, "run-same").await;
    assert_eq!(
        second.outcome,
        Some(WorkflowRunOutcome::NoOp),
        "the same value was published again"
    );
    // Every step succeeded; the stored outputs are only shortened.
    assert_eq!(second.step_results.len(), 2);
}

#[tokio::test]
async fn a_changed_publish_after_identical_ones_is_changed_again() {
    let (state, tokens, agents) = state_with_page().await;
    let same = page_workflow("wf-same", serde_json::json!({ "v": "avant" }));
    let unchanged = run_once(&state, &tokens, &agents, &same, "run-a").await;
    assert_eq!(unchanged.outcome, Some(WorkflowRunOutcome::NoOp));

    let mut changed = same.clone();
    changed.steps[0] = json_data_step("lit", serde_json::json!({ "v": "nouveau" }));
    state
        .db
        .with_conn({
            let changed = changed.clone();
            move |conn| crate::db::workflows::update_workflow(conn, &changed)
        })
        .await
        .unwrap();
    let after = run_once(&state, &tokens, &agents, &changed, "run-b").await;
    assert_eq!(after.outcome, Some(WorkflowRunOutcome::Changed));
}
