//! Workflow runner: orchestrates a full workflow run.
//!
//! Creates workspace → runs hooks → executes steps sequentially →
//! executes actions → cleans up workspace.

use std::sync::Arc;
use anyhow::Result;
use chrono::Utc;

use crate::db::Database;
use crate::models::*;

use super::template::TemplateContext;
use super::workspace::Workspace;
use super::steps::{execute_step, StepOutcome};

/// Events emitted during a workflow run for real-time SSE streaming.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", content = "data")]
pub enum RunEvent {
    /// A step is about to start executing.
    StepStart { step_name: String, step_index: usize, total_steps: usize },
    /// A step has finished executing.
    StepDone { step_result: StepResult },
    /// The entire run has finished.
    RunDone { status: RunStatus },
    /// An error occurred.
    #[allow(dead_code)]
    RunError { error: String },
}

/// Optional sender for real-time progress events.
pub type EventSender = tokio::sync::mpsc::Sender<RunEvent>;

/// Execute a complete workflow run.
pub async fn execute_run(
    db: Arc<Database>,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
) -> Result<()> {
    // Helper to send events (best-effort, ignore send errors)
    let emit = |evt: RunEvent| {
        let tx = events_tx.clone();
        async move {
            if let Some(tx) = tx {
                let _ = tx.send(evt).await;
            }
        }
    };

    // Update run status to Running
    run.status = RunStatus::Running;
    let r = run.clone();
    let db2 = db.clone();
    db2.with_conn(move |conn| crate::db::workflows::update_run(conn, &r)).await?;

    // Resolve project path
    let project_path = if let Some(ref pid) = workflow.project_id {
        let pid = pid.clone();
        let db3 = db.clone();
        let project = db3.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await?;
        project.map(|p| p.path).unwrap_or_default()
    } else {
        String::new()
    };

    // Create workspace (if we have a project path)
    let workspace = if !project_path.is_empty() {
        let repo_path = crate::core::scanner::resolve_host_path(&project_path);
        if repo_path.exists() {
            let hooks = workflow.workspace_config.as_ref().map(|c| c.hooks.clone());
            match Workspace::create(&repo_path, &workflow.name, &run.id, hooks).await {
                Ok(ws) => {
                    run.workspace_path = Some(ws.path.to_string_lossy().to_string());
                    Some(ws)
                }
                Err(e) => {
                    tracing::warn!("Failed to create worktree, running in main tree: {}", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Determine working directory
    let work_dir = workspace.as_ref()
        .map(|ws| ws.path.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            let resolved = crate::core::scanner::resolve_host_path(&project_path);
            if resolved.exists() {
                resolved.to_string_lossy().to_string()
            } else {
                project_path.clone()
            }
        });

    // Run before_run hook
    if let Some(ref ws) = workspace {
        let _ = ws.before_run().await;
    }

    // Build template context from trigger context
    let mut ctx = TemplateContext::new();
    if let Some(ref trigger_ctx) = run.trigger_context {
        inject_trigger_context(&mut ctx, trigger_ctx);
    }

    // Execute steps sequentially
    let mut all_success = true;
    let mut step_idx = 0;
    let total_steps = workflow.steps.len();

    while step_idx < workflow.steps.len() {
        let step = &workflow.steps[step_idx];
        tracing::info!("Executing step {}/{}: '{}'", step_idx + 1, total_steps, step.name);

        emit(RunEvent::StepStart {
            step_name: step.name.clone(),
            step_index: step_idx,
            total_steps,
        }).await;

        let full_access = agents_config.full_access_for(&step.agent);
        let outcome: StepOutcome = execute_step(
            step,
            &project_path,
            &work_dir,
            tokens_config,
            full_access,
            &ctx,
        ).await;

        // Record step output for template chaining
        ctx.set_step_output(&step.name, &outcome.result.output);

        // Accumulate tokens
        run.tokens_used += outcome.result.tokens_used;

        let step_failed = outcome.result.status == RunStatus::Failed;

        // Emit step done event
        emit(RunEvent::StepDone { step_result: outcome.result.clone() }).await;

        run.step_results.push(outcome.result);

        // Persist progress
        let r = run.clone();
        let db4 = db.clone();
        db4.with_conn(move |conn| crate::db::workflows::update_run(conn, &r)).await?;

        if step_failed {
            all_success = false;
            break;
        }

        // Delay after step (if configured)
        if let Some(delay_secs) = step.delay_after_secs {
            if delay_secs > 0 {
                tracing::info!("Step '{}' — waiting {}s before next step", step.name, delay_secs);
                tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            }
        }

        // Handle condition actions
        match outcome.condition_action {
            Some(ConditionAction::Stop) => {
                tracing::info!("Step '{}' triggered Stop condition", step.name);
                break;
            }
            Some(ConditionAction::Skip) => {
                tracing::info!("Step '{}' triggered Skip — skipping next step", step.name);
                step_idx += 2; // skip next step
                continue;
            }
            Some(ConditionAction::Goto { ref step_name }) => {
                if let Some(target) = workflow.steps.iter().position(|s| s.name == *step_name) {
                    tracing::info!("Step '{}' triggered Goto '{}' (index {})", step.name, step_name, target);
                    step_idx = target;
                    continue;
                } else {
                    tracing::warn!("Goto target '{}' not found, continuing normally", step_name);
                }
            }
            None => {}
        }

        step_idx += 1;
    }

    // Run after_run hook
    if let Some(ref ws) = workspace {
        let _ = ws.after_run().await;
    }

    // TODO: Execute post-step actions (CreatePr, CommentIssue, etc.)
    // This requires the tracker adapter implementation

    // Final status
    run.status = if all_success { RunStatus::Success } else { RunStatus::Failed };
    run.finished_at = Some(Utc::now());

    let r = run.clone();
    let db5 = db.clone();
    db5.with_conn(move |conn| crate::db::workflows::update_run(conn, &r)).await?;

    // Emit run done
    emit(RunEvent::RunDone { status: run.status.clone() }).await;

    // Cleanup workspace
    if let Some(ws) = workspace {
        let _ = ws.cleanup().await;
    }

    tracing::info!("Workflow run {} finished: {:?}", run.id, run.status);
    Ok(())
}

/// Inject trigger context JSON into template variables.
fn inject_trigger_context(ctx: &mut TemplateContext, trigger_json: &serde_json::Value) {
    if let Some(obj) = trigger_json.as_object() {
        // Issue fields from tracker trigger
        if let Some(title) = obj.get("issue_title").and_then(|v| v.as_str()) {
            ctx.set("issue.title", title);
        }
        if let Some(body) = obj.get("issue_body").and_then(|v| v.as_str()) {
            ctx.set("issue.body", body);
        }
        if let Some(number) = obj.get("issue_number").and_then(|v| v.as_str()) {
            ctx.set("issue.number", number);
        } else if let Some(number) = obj.get("issue_number").and_then(|v| v.as_u64()) {
            ctx.set("issue.number", number.to_string());
        }
        if let Some(url) = obj.get("issue_url").and_then(|v| v.as_str()) {
            ctx.set("issue.url", url);
        }
        if let Some(labels) = obj.get("issue_labels").and_then(|v| v.as_array()) {
            let label_strs: Vec<String> = labels.iter()
                .filter_map(|l| l.as_str().map(String::from))
                .collect();
            ctx.set("issue.labels", label_strs.join(", "));
        }

        // Generic fields — expose any string value from trigger context
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                ctx.set(key.clone(), s);
            }
        }
    }
}
