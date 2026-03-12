//! Step execution: runs a single workflow step via the agent runner.
//!
//! Handles: prompt rendering, per-step MCPs, stall detection, retry,
//! and on_result condition evaluation.

use std::time::Instant;
use anyhow::{Context, Result};
use tokio::time::{timeout, Duration};

use crate::models::*;

use super::template::TemplateContext;

/// Result of executing a single step.
pub struct StepOutcome {
    pub result: StepResult,
    /// What the on_result conditions decided (None = continue normally)
    pub condition_action: Option<ConditionAction>,
}

/// Execute a single workflow step.
pub async fn execute_step(
    step: &WorkflowStep,
    work_dir: &str,
    tokens_config: &TokensConfig,
    full_access: bool,
    ctx: &TemplateContext,
) -> StepOutcome {
    let start = Instant::now();

    // Render the prompt template
    let mut prompt = match ctx.render(&step.prompt_template) {
        Ok(p) => p,
        Err(e) => {
            return StepOutcome {
                result: StepResult {
                    step_name: step.name.clone(),
                    status: RunStatus::Failed,
                    output: format!("Template render error: {}", e),
                    tokens_used: 0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    condition_result: None,
                },
                condition_action: None,
            };
        }
    };

    // Auto-inject on_result signal instructions into the prompt
    let valid_rules: Vec<_> = step.on_result.iter().filter(|r| !r.contains.is_empty()).collect();
    if !valid_rules.is_empty() {
        prompt.push_str("\n\n---\nIMPORTANT — After your response, you MUST end with a signal line.\n");
        prompt.push_str("The signal MUST be the very last line of your response, in this exact format:\n\n");
        for rule in &valid_rules {
            let action_label = match &rule.action {
                ConditionAction::Stop => "the workflow will stop (no further steps needed)".to_string(),
                ConditionAction::Skip => "the next step will be skipped".to_string(),
                ConditionAction::Goto { step_name } => format!("the workflow will jump to step '{}'", step_name),
            };
            prompt.push_str(&format!(
                "  [SIGNAL: {}]  — use this if {} — {}\n",
                rule.contains, condition_description(&rule.contains), action_label
            ));
        }
        prompt.push_str("  [SIGNAL: CONTINUE]  — use this if none of the above apply (default)\n\n");
        prompt.push_str("You MUST include exactly one [SIGNAL: ...] as the very last line. Do NOT mention or repeat signal names anywhere else in your response.\n");
    }

    // Execute with retry logic
    let max_attempts = step.retry.as_ref().map(|r| r.max_retries + 1).unwrap_or(1);
    let mut last_error = String::new();

    for attempt in 0..max_attempts {
        if attempt > 0 {
            // Exponential backoff: 2^attempt seconds (2s, 4s, 8s...)
            let delay = Duration::from_secs(2u64.pow(attempt));
            tracing::info!("Step '{}' retry {}/{} after {:?}", step.name, attempt, max_attempts - 1, delay);
            tokio::time::sleep(delay).await;
        }

        match run_agent_with_timeout(step, work_dir, &prompt, tokens_config, full_access).await {
            Ok(output) => {
                let duration_ms = start.elapsed().as_millis() as u64;

                // Evaluate on_result conditions
                let condition_action = evaluate_conditions(&step.on_result, &output);

                let condition_result = condition_action.as_ref().map(|a| match a {
                    ConditionAction::Stop => "Stop".to_string(),
                    ConditionAction::Skip => "Skip".to_string(),
                    ConditionAction::Goto { step_name } => format!("Goto:{}", step_name),
                });

                return StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::Success,
                        output,
                        tokens_used: 0, // TODO: extract from agent output when available
                        duration_ms,
                        condition_result,
                    },
                    condition_action,
                };
            }
            Err(e) => {
                last_error = format!("{}", e);
                tracing::warn!("Step '{}' attempt {} failed: {}", step.name, attempt + 1, last_error);
            }
        }
    }

    // All retries exhausted
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: format!("Failed after {} attempts. Last error: {}", max_attempts, last_error),
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result: None,
        },
        condition_action: None,
    }
}

/// Run an agent with optional stall timeout.
async fn run_agent_with_timeout(
    step: &WorkflowStep,
    work_dir: &str,
    prompt: &str,
    tokens_config: &TokensConfig,
    full_access: bool,
) -> Result<String> {
    let stall_timeout = step.stall_timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(600)); // 10 min default

    let mut agent_process = crate::agents::runner::start_agent_with_skills(
        &step.agent,
        work_dir,
        prompt,
        tokens_config,
        full_access,
        &step.skill_ids,
    ).await.map_err(|e| anyhow::anyhow!(e))?;

    let mut output = String::new();
    let is_stream_json = agent_process.output_mode == crate::agents::runner::OutputMode::StreamJson;

    loop {
        match timeout(stall_timeout, agent_process.next_line()).await {
            Ok(Some(line)) => {
                if is_stream_json {
                    if let crate::agents::runner::StreamJsonEvent::Text(text) = crate::agents::runner::parse_claude_stream_line(&line) {
                        output.push_str(&text);
                    }
                } else {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&line);
                }
            }
            Ok(None) => {
                // Stream ended — agent finished
                break;
            }
            Err(_) => {
                // Stall timeout — kill the process
                tracing::warn!("Step '{}' stalled (no output for {:?}), killing agent",
                    step.name, stall_timeout);
                let _ = agent_process.child.kill().await;
                anyhow::bail!("Agent stalled (no output for {}s)", stall_timeout.as_secs());
            }
        }
    }

    // Wait for process to finish
    let status = agent_process.child.wait().await
        .context("Failed to wait for agent process")?;
    agent_process.fix_ownership();

    if !status.success() {
        let stderr = agent_process.captured_stderr().join("\n");
        if stderr.is_empty() {
            anyhow::bail!("Agent exited with status: {}", status);
        } else {
            anyhow::bail!("Agent failed: {}", stderr);
        }
    }

    Ok(output)
}

/// Evaluate on_result conditions against the step output.
/// Only checks the last 5 lines for `[SIGNAL: keyword]` to avoid false positives
/// from the agent quoting instruction text in its response.
fn evaluate_conditions(rules: &[StepConditionRule], output: &str) -> Option<ConditionAction> {
    // Look at the last 5 lines for a signal
    let tail: Vec<&str> = output.lines().rev().take(5).collect();
    for rule in rules {
        // Skip empty conditions — they would match everything
        if rule.contains.is_empty() {
            continue;
        }
        let signal = format!("[SIGNAL: {}]", rule.contains);
        if tail.iter().any(|line| line.contains(&signal)) {
            return Some(rule.action.clone());
        }
    }
    None
}

/// Generate a human-readable description of what a keyword means.
/// Used in the auto-injected prompt section.
fn condition_description(keyword: &str) -> &str {
    match keyword {
        "NO_RESULTS" => "there are no results to report or nothing was found",
        _ => "this condition is met",
    }
}
