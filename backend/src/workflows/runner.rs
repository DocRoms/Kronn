//! Workflow runner: orchestrates a full workflow run.
//!
//! Creates workspace → runs hooks → executes steps sequentially →
//! executes actions → cleans up workspace.

use anyhow::Result;
use chrono::Utc;

use crate::models::*;
use crate::AppState;

use super::template::TemplateContext;
use super::workspace::Workspace;
use super::steps::{execute_step, StepOutcome};

/// Events emitted during a workflow run for real-time SSE streaming.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", content = "data")]
pub enum RunEvent {
    /// A step is about to start executing.
    StepStart { step_name: String, step_index: usize, total_steps: usize },
    /// Partial output from the agent (streamed in real-time).
    StepProgress { text: String },
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
    state: AppState,
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

    let db = state.db.clone();

    // Register a cancellation token keyed by the run id. The "⏹ Arrêter" UI
    // triggers this token via POST /api/workflows/.../runs/:run_id/cancel.
    // We check it between steps and during any long await — short-circuiting
    // to status = Cancelled. The CancelGuard auto-cleans on scope exit.
    let cancel_guard = crate::CancelGuard::insert(&state.cancel_registry, run.id.clone());
    let cancel_token = cancel_guard.token.clone();

    // Update run status to Running
    run.status = RunStatus::Running;
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let db2 = db.clone();
    db2.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;

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
    let mut cancelled_by_user = false;
    let mut step_idx = 0;
    let total_steps = workflow.steps.len();
    let max_total_iterations = max_iterations_for(total_steps); // safeguard against infinite Goto loops
    let mut iteration_count = 0;

    while step_idx < workflow.steps.len() {
        // Cancellation check — fires when the user clicked "⏹ Arrêter" and
        // the /cancel endpoint triggered our token. We break BEFORE executing
        // the next step so a runaway linear run doesn't keep burning tokens.
        // Note: a step already in flight won't stop here — agent steps have
        // their own disc-level token checked inside `make_agent_stream`.
        if cancel_token.is_cancelled() {
            tracing::info!("Workflow run {} cancelled by user before step {}", run.id, step_idx);
            cancelled_by_user = true;
            run.step_results.push(StepResult {
                step_name: "__cancelled_by_user__".to_string(),
                status: RunStatus::Cancelled,
                output: "Workflow run cancelled by user".to_string(),
                tokens_used: 0,
                duration_ms: 0,
                condition_result: None,
            });
            all_success = false;
            break;
        }
        iteration_count += 1;
        if iteration_count > max_total_iterations {
            tracing::error!("Workflow run exceeded {} iterations — aborting to prevent infinite loop", max_total_iterations);
            all_success = false;
            run.step_results.push(StepResult {
                step_name: "__safeguard_abort__".to_string(),
                status: RunStatus::Failed,
                output: format!("Workflow aborted: exceeded {} total step iterations (possible infinite Goto loop)", max_total_iterations),
                tokens_used: 0,
                duration_ms: 0,
                condition_result: None,
            });
            break;
        }
        let step = &workflow.steps[step_idx];
        tracing::info!("Executing step {}/{}: '{}'", step_idx + 1, total_steps, step.name);

        emit(RunEvent::StepStart {
            step_name: step.name.clone(),
            step_index: step_idx,
            total_steps,
        }).await;

        let outcome: StepOutcome = match step.step_type {
            StepType::BatchQuickPrompt => {
                // Phase 2 batch workflows — fan out a Quick Prompt over items
                // from a previous step's output, then optionally wait for all
                // children to finish before moving on.
                super::batch_step::execute_batch_quick_prompt_step(
                    step,
                    &run.id,
                    state.clone(),
                    &ctx,
                ).await
            }
            StepType::Notify => {
                // Direct HTTP webhook — zero agent tokens. Used as a workflow
                // finalizer or mechanical data step (post to Slack, create
                // ticket, etc.). Shipped 0.3.5.
                super::notify_step::execute_notify_step(step, &ctx).await
            }
            StepType::Agent | StepType::ApiCall => {
                let full_access = agents_config.full_access_for(&step.agent);
                execute_step(
                    step,
                    &project_path,
                    &work_dir,
                    tokens_config,
                    full_access,
                    &ctx,
                    None,
                ).await
            }
        };

        // Record step output for template chaining
        ctx.set_step_output(&step.name, &outcome.result.output);

        // Accumulate tokens
        run.tokens_used += outcome.result.tokens_used;

        let step_failed = outcome.result.status == RunStatus::Failed;

        // Emit step done event
        emit(RunEvent::StepDone { step_result: outcome.result.clone() }).await;

        run.step_results.push(outcome.result);

        // Persist progress
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        let db4 = db.clone();
        db4.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;

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

    // Post-step actions (CreatePr, CommentIssue, etc.) are handled by MCP tools
    // injected into agent prompts — no separate actions phase needed.

    // Final status: Cancelled takes precedence over Failed/Success when the
    // user explicitly stopped the run.
    run.status = if cancelled_by_user {
        RunStatus::Cancelled
    } else if all_success {
        RunStatus::Success
    } else {
        RunStatus::Failed
    };
    run.finished_at = Some(Utc::now());

    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let db5 = db.clone();
    db5.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;

    // Emit run done
    emit(RunEvent::RunDone { status: run.status.clone() }).await;

    // Cleanup workspace
    if let Some(ws) = workspace {
        let _ = ws.cleanup().await;
    }

    tracing::info!("Workflow run {} finished: {:?}", run.id, run.status);
    Ok(())
}

/// Build the maximum iteration safeguard for a given step count.
/// Formula: total_steps * 10 + 50.
pub(crate) fn max_iterations_for(total_steps: usize) -> usize {
    total_steps * 10 + 50
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── inject_trigger_context ──────────────────────────────────────────

    #[test]
    fn inject_trigger_context_issue_fields() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_title": "Fix the bug",
            "issue_body": "It crashes on startup",
            "issue_number": "42",
            "issue_url": "https://github.com/owner/repo/issues/42",
            "issue_labels": ["bug", "priority-high"],
        });
        inject_trigger_context(&mut ctx, &json);

        assert_eq!(ctx.render("{{issue.title}}").unwrap(), "Fix the bug");
        assert_eq!(ctx.render("{{issue.body}}").unwrap(), "It crashes on startup");
        assert_eq!(ctx.render("{{issue.number}}").unwrap(), "42");
        assert_eq!(ctx.render("{{issue.url}}").unwrap(), "https://github.com/owner/repo/issues/42");
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "bug, priority-high");
    }

    #[test]
    fn inject_trigger_context_issue_number_as_u64() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_number": 99,
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{issue.number}}").unwrap(), "99");
    }

    #[test]
    fn inject_trigger_context_generic_string_fields() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "type": "tracker",
            "custom_field": "hello",
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{type}}").unwrap(), "tracker");
        assert_eq!(ctx.render("{{custom_field}}").unwrap(), "hello");
    }

    #[test]
    fn inject_trigger_context_non_string_values_ignored() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "count": 42,
            "nested": {"key": "val"},
        });
        inject_trigger_context(&mut ctx, &json);
        // Non-string generic fields are NOT injected
        assert_eq!(ctx.render("{{count}}").unwrap(), "{{count}}");
        assert_eq!(ctx.render("{{nested}}").unwrap(), "{{nested}}");
    }

    #[test]
    fn inject_trigger_context_empty_object() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({});
        inject_trigger_context(&mut ctx, &json);
        // No variables set — template placeholders remain
        assert_eq!(ctx.render("{{issue.title}}").unwrap(), "{{issue.title}}");
    }

    #[test]
    fn inject_trigger_context_not_an_object() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!("just a string");
        inject_trigger_context(&mut ctx, &json);
        // Should silently do nothing
        assert_eq!(ctx.render("{{anything}}").unwrap(), "{{anything}}");
    }

    #[test]
    fn inject_trigger_context_null_value() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::Value::Null;
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{anything}}").unwrap(), "{{anything}}");
    }

    #[test]
    fn inject_trigger_context_empty_labels() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_labels": [],
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "");
    }

    #[test]
    fn inject_trigger_context_labels_with_non_string_items() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_labels": ["bug", 42, "feature"],
        });
        inject_trigger_context(&mut ctx, &json);
        // Non-string items in labels array are filtered out
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "bug, feature");
    }

    // ─── max_iterations_for ──────────────────────────────────────────────

    #[test]
    fn max_iterations_zero_steps() {
        assert_eq!(max_iterations_for(0), 50);
    }

    #[test]
    fn max_iterations_typical_workflow() {
        assert_eq!(max_iterations_for(3), 80);  // 3*10 + 50
        assert_eq!(max_iterations_for(5), 100); // 5*10 + 50
    }

    #[test]
    fn max_iterations_single_step() {
        assert_eq!(max_iterations_for(1), 60);  // 1*10 + 50
    }

    // ─── RunEvent serialization ──────────────────────────────────────────

    #[test]
    fn run_event_step_start_serializes() {
        let evt = RunEvent::StepStart {
            step_name: "analyze".into(),
            step_index: 0,
            total_steps: 3,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "StepStart");
        assert_eq!(json["data"]["step_name"], "analyze");
        assert_eq!(json["data"]["step_index"], 0);
        assert_eq!(json["data"]["total_steps"], 3);
    }

    #[test]
    fn run_event_run_done_serializes() {
        let evt = RunEvent::RunDone { status: RunStatus::Success };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "RunDone");
    }

    #[test]
    fn run_event_run_error_serializes() {
        let evt = RunEvent::RunError { error: "timeout".into() };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "RunError");
        assert_eq!(json["data"]["error"], "timeout");
    }
}
