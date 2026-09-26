//! Executor for `StepType::TriggerWorkflow`: launches another workflow as an
//! independent run, through the same path as a manual launch, and returns at
//! once. The parent keeps the child's id on its step result; the child keeps
//! the parent's id as `triggered_by_run_id`.

use std::time::Instant;

use serde_json::json;

use crate::models::{ConditionAction, RunStatus, StepResult, Workflow, WorkflowStep};

use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Longest chain of runs launching one another. Each round of a loop between
/// workflows adds generations; a loop that never settles stops here instead of
/// spending tokens forever.
pub(crate) const MAX_TRIGGER_CHAIN: usize = 20;

pub async fn execute_trigger_workflow_step(
    state: &crate::AppState,
    workflow: &Workflow,
    run_id: &str,
    step: &WorkflowStep,
    ctx: &TemplateContext,
) -> StepOutcome {
    let start = Instant::now();
    let Some(target) = step
        .sub_workflow_id
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty())
    else {
        return refused(
            step,
            start,
            None,
            "TriggerWorkflow step missing `sub_workflow_id`.".into(),
        );
    };
    if let Some(error) = super::sub_workflow_step::secret_mapping_error(step, &workflow.variables) {
        return refused(step, start, Some(target), error);
    }

    let lookup = target.to_string();
    let child = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &lookup))
        .await
    {
        Ok(Some(child)) => child,
        Ok(None) => {
            return refused(
                step,
                start,
                Some(target),
                format!("Workflow `{target}` not found (deleted? wrong id?)."),
            )
        }
        Err(error) => {
            return refused(
                step,
                start,
                Some(target),
                format!("DB error loading workflow `{target}`: {error}"),
            )
        }
    };
    if let Some(error) = super::sub_workflow_step::undeclared_mapping_error(step, &child) {
        return refused(step, start, Some(target), error);
    }
    let variables = match super::sub_workflow_step::render_child_variables(step, ctx) {
        Ok(variables) => variables,
        Err(error) => return refused(step, start, Some(target), error),
    };

    match chain_depth(state, run_id).await {
        Ok(depth) if depth >= MAX_TRIGGER_CHAIN => {
            return refused(
                step,
                start,
                Some(target),
                format!(
                    "{depth} runs already led to this one through triggers or sub-workflows; refusing to extend the chain beyond {MAX_TRIGGER_CHAIN} (a loop between workflows that never settles?)."
                ),
            )
        }
        Ok(_) => {}
        Err(error) => {
            return refused(
                step,
                start,
                Some(target),
                format!("Cannot read the runs that led to this one: {error}"),
            )
        }
    }

    let launch = crate::core::launch_context::LaunchContext {
        project_id: workflow.project_id.clone(),
        triggered_by_run_id: Some(run_id.to_string()),
        ..Default::default()
    };
    let mut names: Vec<String> = variables.keys().cloned().collect();
    names.sort();
    let (child_wf, child_run) = match crate::api::workflows::create_manual_run(
        state,
        target,
        variables,
        std::collections::HashMap::new(),
        launch,
    )
    .await
    {
        Ok(created) => created,
        Err(error) => return refused(step, start, Some(target), error),
    };
    let child_run_id = child_run.id.clone();
    let concurrency_key = child_run.concurrency_key.clone();
    let child_name = child_wf.name.clone();
    crate::api::workflows::spawn_manual_run(state, child_wf, child_run, None, true);
    tracing::info!(
        target: "kronn::trigger_workflow",
        parent_run = %run_id, child_run = %child_run_id, workflow = %target,
        "triggered workflow run"
    );

    // Variable values may hold anything the parent rendered; only names are kept.
    let data = json!({
        "child_run_id": child_run_id,
        "child_workflow_id": target,
        "child_workflow_name": child_name,
        "variables": names,
        "concurrency_key": concurrency_key,
    });
    let short_id: String = child_run_id.chars().take(8).collect();
    let summary = format!("Workflow « {child_name} » launched (run {short_id})");
    let output =
        super::step_output_format::format_step_output(data, "OK", &summary, None, &["TRIGGERED"]);
    outcome(step, start, RunStatus::Success, output, Some(child_run_id))
}

/// Generations above `run_id`, following the run that triggered each one or,
/// for a sub-workflow child, its parent.
async fn chain_depth(state: &crate::AppState, run_id: &str) -> anyhow::Result<usize> {
    let run_id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            use rusqlite::OptionalExtension;
            let mut seen = std::collections::HashSet::new();
            let mut depth = 0usize;
            let mut next = Some(run_id);
            while let Some(id) = next.take() {
                if !seen.insert(id.clone()) || depth > MAX_TRIGGER_CHAIN {
                    break;
                }
                next = conn
                    .query_row(
                        "SELECT COALESCE(triggered_by_run_id, parent_run_id)
                           FROM workflow_runs WHERE id = ?1",
                        [&id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                if next.is_some() {
                    depth += 1;
                }
            }
            Ok(depth)
        })
        .await
}

/// A refusal is an outcome the parent may branch on (`TRIGGER_REFUSED`), e.g.
/// when the child's concurrency key is already at its limit.
fn refused(
    step: &WorkflowStep,
    start: Instant,
    target: Option<&str>,
    reason: String,
) -> StepOutcome {
    let data = json!({ "child_workflow_id": target, "error": reason });
    let output = super::step_output_format::format_step_output(
        data,
        "TRIGGER_REFUSED",
        &reason,
        None,
        &["TRIGGER_REFUSED"],
    );
    outcome(step, start, RunStatus::Failed, output, None)
}

fn outcome(
    step: &WorkflowStep,
    start: Instant,
    status: RunStatus,
    output: String,
    child_run_id: Option<String>,
) -> StepOutcome {
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|action| match action {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status,
            output,
            tokens_used: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: Some(true),
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id,
            agent_provenance: None,
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
        },
        condition_action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow(id: &str, value: serde_json::Value) -> Workflow {
        let mut base = json!({
            "id": id, "name": id, "project_id": null,
            "trigger": {"type": "Manual"},
            "steps": [{"name": "review", "step_type": {"type": "Gate"}}],
            "actions": [],
            "safety": {"sandbox": false, "max_files": null, "max_lines": null, "require_approval": false},
            "workspace_config": null, "concurrency_limit": null,
            "enabled": true,
            "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now(),
        });
        for (key, field) in value.as_object().unwrap() {
            base[key] = field.clone();
        }
        serde_json::from_value(base).expect("workflow")
    }

    fn run(id: &str, workflow_id: &str, triggered_by: Option<&str>) -> crate::models::WorkflowRun {
        serde_json::from_value(json!({
            "id": id, "workflow_id": workflow_id, "status": "Success",
            "trigger_context": null, "step_results": [], "tokens_used": 0,
            "workspace_path": null, "started_at": chrono::Utc::now(), "finished_at": null,
            "triggered_by_run_id": triggered_by,
        }))
        .expect("run")
    }

    async fn state_with(
        workflows: Vec<Workflow>,
        runs: Vec<crate::models::WorkflowRun>,
    ) -> crate::AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        state
            .db
            .with_conn(move |conn| {
                for workflow in &workflows {
                    crate::db::workflows::insert_workflow(conn, workflow)?;
                }
                for run in &runs {
                    crate::db::workflows::insert_run(conn, run)?;
                }
                Ok(())
            })
            .await
            .unwrap();
        state
    }

    fn trigger_step(on_result: serde_json::Value) -> WorkflowStep {
        serde_json::from_value(json!({
            "name": "launch", "step_type": {"type": "TriggerWorkflow"},
            "sub_workflow_id": "child",
            "sub_workflow_variables": {"ticketKey": "{{ticket}}"},
            "on_result": on_result,
        }))
        .expect("step")
    }

    #[tokio::test]
    async fn a_refused_launch_is_a_branchable_outcome_and_starts_nothing() {
        let child = workflow(
            "child",
            json!({
                "concurrency_limit": 1,
                "concurrency_key": "{{ticketKey}}",
                "variables": [{"name": "ticketKey", "label": "Ticket", "placeholder": ""}],
            }),
        );
        let parent = workflow("parent", json!({}));
        let mut active = run("active", "child", None);
        active.status = RunStatus::Running;
        active.concurrency_key = Some("EW-1".into());
        let state = state_with(
            vec![child, parent.clone()],
            vec![run("parent-run", "parent", None), active],
        )
        .await;
        let mut ctx = TemplateContext::new();
        ctx.set("ticket", "EW-1");
        let step =
            trigger_step(json!([{"contains": "TRIGGER_REFUSED", "action": {"type": "Stop"}}]));

        let outcome =
            execute_trigger_workflow_step(&state, &parent, "parent-run", &step, &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("[SIGNAL: TRIGGER_REFUSED]"));
        assert!(
            outcome
                .result
                .output
                .contains("Concurrency limit reached for key `EW-1`"),
            "{}",
            outcome.result.output
        );
        assert!(matches!(
            outcome.condition_action,
            Some(ConditionAction::Stop)
        ));
        assert!(outcome.result.child_run_id.is_none());
        let runs = state
            .db
            .with_conn(|conn| crate::db::workflows::count_runs(conn, "child"))
            .await
            .unwrap();
        assert_eq!(runs, 1, "only the run already active");
    }

    #[tokio::test]
    async fn a_chain_of_runs_launching_one_another_is_bounded() {
        let child = workflow(
            "child",
            json!({"variables": [{"name": "ticketKey", "label": "Ticket", "placeholder": ""}]}),
        );
        let parent = workflow("parent", json!({}));
        let mut runs = vec![run("gen-0", "parent", None)];
        for generation in 1..=MAX_TRIGGER_CHAIN {
            runs.push(run(
                &format!("gen-{generation}"),
                "parent",
                Some(&format!("gen-{}", generation - 1)),
            ));
        }
        let state = state_with(vec![child, parent.clone()], runs).await;
        let mut ctx = TemplateContext::new();
        ctx.set("ticket", "EW-1");
        let step = trigger_step(json!([]));

        let last = format!("gen-{MAX_TRIGGER_CHAIN}");
        let refused = execute_trigger_workflow_step(&state, &parent, &last, &step, &ctx).await;
        assert!(
            refused
                .result
                .output
                .contains("refusing to extend the chain"),
            "{}",
            refused.result.output
        );
        let below = format!("gen-{}", MAX_TRIGGER_CHAIN - 1);
        let launched = execute_trigger_workflow_step(&state, &parent, &below, &step, &ctx).await;
        assert_eq!(
            launched.result.status,
            RunStatus::Success,
            "{}",
            launched.result.output
        );
    }
}
