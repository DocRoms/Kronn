//! Step execution: runs a single workflow step via the agent runner.
//!
//! Handles: prompt rendering, per-step MCPs, stall detection, retry,
//! and on_result condition evaluation.

use std::time::Instant;
use anyhow::{Context, Result};
use tokio::time::{timeout, Duration};

use crate::agents::runner::{self, OutputMode, StreamJsonEvent};
use crate::models::*;

use super::template::TemplateContext;

/// Result of executing a single step.
pub struct StepOutcome {
    pub result: StepResult,
    /// What the on_result conditions decided (None = continue normally)
    pub condition_action: Option<ConditionAction>,
}

/// Output from a single agent run, including token usage.
struct AgentOutput {
    text: String,
    tokens_used: u64,
}

/// Execute a single workflow step.
///
/// - `project_path`: original project path for MCP context resolution
/// - `work_dir`: agent's working directory (may be a worktree)
pub async fn execute_step(
    step: &WorkflowStep,
    project_path: &str,
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

    // Auto-inject structured output format instructions when output_format = Structured
    if step.output_format == crate::models::StepOutputFormat::Structured {
        prompt.push_str(crate::workflows::template::STRUCTURED_OUTPUT_INSTRUCTIONS);
    }

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

        match run_agent_with_timeout(step, project_path, work_dir, &prompt, tokens_config, full_access).await {
            Ok(agent_output) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                let mut final_output = agent_output.text.clone();
                let mut total_tokens = agent_output.tokens_used;

                // For Structured steps: verify envelope exists, try repair if missing
                if step.output_format == crate::models::StepOutputFormat::Structured
                    && crate::workflows::template::extract_step_envelope(&final_output).is_none()
                {
                        tracing::info!("Step '{}': structured output missing envelope, attempting repair", step.name);
                        let truncated = if final_output.len() > 2000 { &final_output[..2000] } else { &final_output };
                        let repair_prompt = crate::workflows::template::REPAIR_PROMPT_TEMPLATE
                            .replace("{PREVIOUS_OUTPUT}", truncated);
                    if let Ok(repair_output) = run_agent_with_timeout(step, project_path, work_dir, &repair_prompt, tokens_config, full_access).await {
                        total_tokens += repair_output.tokens_used;
                        if crate::workflows::template::extract_step_envelope(&repair_output.text).is_some() {
                            final_output = repair_output.text;
                            tracing::info!("Step '{}': repair succeeded", step.name);
                        } else {
                            tracing::warn!("Step '{}': repair failed, using raw output", step.name);
                        }
                    }
                }

                // Evaluate on_result conditions (check signals + structured status)
                let mut condition_action = evaluate_conditions(&step.on_result, &final_output);

                // For Structured: also check status field for NO_RESULTS
                if condition_action.is_none() && step.output_format == crate::models::StepOutputFormat::Structured {
                    if let Some(env) = crate::workflows::template::extract_step_envelope(&final_output) {
                        if env.status == "NO_RESULTS" && step.on_result.iter().any(|r| r.contains == "NO_RESULTS") {
                            condition_action = Some(ConditionAction::Stop);
                        }
                    }
                }

                let condition_result = condition_action.as_ref().map(|a| match a {
                    ConditionAction::Stop => "Stop".to_string(),
                    ConditionAction::Skip => "Skip".to_string(),
                    ConditionAction::Goto { step_name } => format!("Goto:{}", step_name),
                });

                return StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::Success,
                        output: final_output,
                        tokens_used: total_tokens,
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
/// Returns the agent output text and token usage.
///
/// - `project_path`: original project path for MCP context resolution
/// - `work_dir`: agent's working directory (may be a worktree)
async fn run_agent_with_timeout(
    step: &WorkflowStep,
    project_path: &str,
    work_dir: &str,
    prompt: &str,
    tokens_config: &TokensConfig,
    full_access: bool,
) -> Result<AgentOutput> {
    let stall_timeout = step.stall_timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(600)); // 10 min default

    let mut agent_process = runner::start_agent_with_config(runner::AgentStartConfig {
        agent_type: &step.agent,
        project_path,
        work_dir: Some(work_dir),
        prompt,
        tokens: tokens_config,
        full_access,
        skill_ids: &step.skill_ids,
        directive_ids: &step.directive_ids,
        profile_ids: &step.profile_ids,
        mcp_context_override: None,
        tier: step.agent_settings.as_ref()
            .and_then(|s| s.tier)
            .unwrap_or_default(),
        model_tiers: None,
    }).await.map_err(|e| anyhow::anyhow!(e))?;

    let mut output = String::new();
    let is_stream_json = agent_process.output_mode == OutputMode::StreamJson;
    let mut stream_json_tokens: u64 = 0;

    loop {
        match timeout(stall_timeout, agent_process.next_line()).await {
            Ok(Some(line)) => {
                if is_stream_json {
                    match runner::parse_claude_stream_line(&line) {
                        StreamJsonEvent::Text(text) => {
                            output.push_str(&text);
                        }
                        StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                            stream_json_tokens = input_tokens + output_tokens;
                        }
                        StreamJsonEvent::ToolStart(_) | StreamJsonEvent::ToolInputDelta(_) | StreamJsonEvent::ToolEnd | StreamJsonEvent::Skip => {}
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

    // Extract token usage — same logic as discussions:
    // 1. Claude Code: tokens from stream-json events (input + output)
    // 2. Codex/Kiro/etc: tokens parsed from stderr/stdout after execution
    let stderr_lines = agent_process.captured_stderr();
    let tokens_used = if stream_json_tokens > 0 {
        stream_json_tokens
    } else {
        let (cleaned, count) = runner::parse_token_usage(&step.agent, &output, &stderr_lines);
        if count > 0 {
            output = cleaned;
        }
        count
    };

    tracing::info!("Step '{}' finished — {} tokens used", step.name, tokens_used);

    Ok(AgentOutput {
        text: output,
        tokens_used,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConditionAction, StepConditionRule};

    fn rule(contains: &str, action: ConditionAction) -> StepConditionRule {
        StepConditionRule { contains: contains.to_string(), action }
    }

    #[test]
    fn test_contains_stop() {
        let rules = vec![rule("STOP_SIGNAL", ConditionAction::Stop)];
        let output = "Some output\n[SIGNAL: STOP_SIGNAL]";
        let action = evaluate_conditions(&rules, output);
        assert!(matches!(action, Some(ConditionAction::Stop)));
    }

    #[test]
    fn test_contains_skip() {
        let rules = vec![rule("SKIP_SIGNAL", ConditionAction::Skip)];
        let output = "Some output\n[SIGNAL: SKIP_SIGNAL]";
        let action = evaluate_conditions(&rules, output);
        assert!(matches!(action, Some(ConditionAction::Skip)));
    }

    #[test]
    fn test_contains_goto() {
        let rules = vec![rule("GO_NEXT", ConditionAction::Goto { step_name: "step_b".to_string() })];
        let output = "Some output\n[SIGNAL: GO_NEXT]";
        let action = evaluate_conditions(&rules, output);
        assert!(matches!(action, Some(ConditionAction::Goto { step_name }) if step_name == "step_b"));
    }

    #[test]
    fn test_no_match_returns_none() {
        let rules = vec![rule("STOP_SIGNAL", ConditionAction::Stop)];
        let output = "No matching signal here.\n[SIGNAL: CONTINUE]";
        let action = evaluate_conditions(&rules, output);
        assert!(action.is_none());
    }

    #[test]
    fn test_empty_rules_returns_none() {
        let rules: Vec<StepConditionRule> = vec![];
        let output = "Any output with [SIGNAL: STOP_SIGNAL]";
        let action = evaluate_conditions(&rules, output);
        assert!(action.is_none());
    }

    #[test]
    fn test_empty_contains_skipped() {
        // A rule with an empty `contains` field must never match anything
        let rules = vec![rule("", ConditionAction::Stop)];
        let output = "Some output\n[SIGNAL: ]";
        let action = evaluate_conditions(&rules, output);
        assert!(action.is_none());
    }

    #[test]
    fn test_signal_only_in_tail_matches() {
        // Signal is in the last 5 lines — should match
        let rules = vec![rule("STOP_SIGNAL", ConditionAction::Stop)];
        let output = "line1\nline2\nline3\nline4\nline5\n[SIGNAL: STOP_SIGNAL]";
        let action = evaluate_conditions(&rules, output);
        assert!(matches!(action, Some(ConditionAction::Stop)));
    }

    #[test]
    fn test_signal_deep_in_output_ignored() {
        // Signal is far from the end (beyond last 5 lines) — should NOT match
        let rules = vec![rule("STOP_SIGNAL", ConditionAction::Stop)];
        let output = "[SIGNAL: STOP_SIGNAL]\nline2\nline3\nline4\nline5\nline6\nline7";
        let action = evaluate_conditions(&rules, output);
        assert!(action.is_none());
    }
}
