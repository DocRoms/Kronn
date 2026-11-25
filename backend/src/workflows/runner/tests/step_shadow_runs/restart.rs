//! KT-2029 — a restart of Kronn never rewrites the main execution of a step
//! with shadows: its row is identity only, the step result carries what it was
//! and produced, finished or not.

use super::*;
use crate::workflows::shadow_artifact::ExecutionHtml;

const PAGE: &str = "Here is the report:\n\n```html\n<!DOCTYPE html>\n<html><body><h1>Report</h1></body></html>\n```\n";

async fn comparisons_of(
    state: &crate::AppState,
    run_id: &str,
) -> Vec<shadow::StepShadowComparison> {
    let id = run_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            let run = crate::db::workflows::get_run(conn, &id)?.unwrap();
            shadow::comparisons(conn, &run)
        })
        .await
        .unwrap()
}

async fn restart(state: &crate::AppState) -> usize {
    state
        .db
        .with_conn(step_shadows::interrupt_running)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_restart_leaves_the_main_execution_of_a_finished_run_as_its_step_result() {
    let mut main = scripted(PAGE);
    main.usage = Some((12, 34));
    let mut fx = fixture("proj-shadow-restart", main).await;
    fx.model_for(AgentType::Codex, scripted("other answer"));
    let mut differing = shadow_of("other-model");
    differing.agent = on(AgentType::Codex, "gpt-x");
    let wf = fx.workflow(vec![shadow_of("same"), differing]);
    let run = fx.run(&wf, "run-restart-main").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    fx.settled("run-restart-main", 2).await;

    let before = comparisons_of(&fx.state, "run-restart-main").await;
    assert_eq!(before[0].main.status, ShadowStatus::Success);
    assert_eq!(before[0].main.html, ExecutionHtml::Complete);

    // The boot of the next process: nothing of this finished run is running.
    assert_eq!(restart(&fx.state).await, 0, "no shadow was left running");

    let after = comparisons_of(&fx.state, "run-restart-main").await;
    let (was, is) = (&before[0].main, &after[0].main);
    assert_eq!(is.status, ShadowStatus::Success, "{is:?}");
    assert_eq!(is.output, run.step_results[1].output);
    assert_eq!(is.output, was.output);
    assert_eq!(is.tokens_used, was.tokens_used);
    assert!(is.tokens_used.unwrap_or_default() > 0, "{is:?}");
    assert_eq!(is.duration_ms, Some(run.step_results[1].duration_ms));
    assert_eq!(is.html, ExecutionHtml::Complete, "an Artifact can be made");
    assert_eq!(is.stopped, None);
    let shadows = |steps: &[shadow::StepShadowComparison]| -> Vec<(ShadowStatus, String)> {
        steps[0]
            .shadows
            .iter()
            .map(|s| (s.status, s.output.clone()))
            .collect()
    };
    assert_eq!(
        shadows(&after),
        shadows(&before),
        "the shadows are unchanged"
    );
}

fn row(run_id: &str, shadow_id: &str) -> StepShadowRow {
    StepShadowRow {
        run_id: run_id.into(),
        result_index: 0,
        step_name: "review".into(),
        shadow_id: shadow_id.into(),
        label: shadow_id.into(),
        status: ShadowStatus::Running,
        identity: Default::default(),
        output: String::new(),
        tokens_used: None,
        duration_ms: None,
        cost_usd: None,
        model: None,
        stopped: None,
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
    }
}

#[tokio::test]
async fn a_run_interrupted_mid_step_shows_its_main_execution_unfinished() {
    let (state, _, _) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-restart-mid".into();
    let mut run = pending_run("run-restart-mid", &wf.id);
    run.status = RunStatus::Running;
    insert_wf_and_run(&state, &wf, &run).await;
    let (main, shadow) = (
        row("run-restart-mid", step_shadows::MAIN),
        row("run-restart-mid", "s"),
    );
    state
        .db
        .with_conn(move |conn| {
            step_shadows::insert(conn, &main)?;
            step_shadows::insert(conn, &shadow)
        })
        .await
        .unwrap();

    assert_eq!(restart(&state).await, 1, "only the shadow is marked");

    let steps = comparisons_of(&state, "run-restart-mid").await;
    let main = &steps[0].main;
    assert!(
        !matches!(main.status, ShadowStatus::Success | ShadowStatus::Failed),
        "the step never finished: {main:?}"
    );
    assert_eq!(main.output, "");
    assert_eq!(main.duration_ms, None);
    assert_eq!(steps[0].shadows[0].status, ShadowStatus::Interrupted);
    assert!(steps[0].shadows[0]
        .stopped
        .as_deref()
        .unwrap_or_default()
        .contains("restarted"));
}
