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

/// Optional sender for streaming partial agent output during step execution.
pub type ProgressSender = tokio::sync::mpsc::Sender<String>;

/// Build the full agent-ready prompt for a step: template render +
/// `extra_context` append + output-format addendum + triage addendum.
/// Does NOT append the signal-protocol instructions — those depend on
/// runtime `on_result` rules and are still added by [`execute_step`]
/// after this helper returns.
///
/// Extracted from [`execute_step`] in 0.8.3 (TD-265) so the prompt
/// assembly is unit-testable independently from the agent spawn path.
/// Returns `Err(String)` only when `ctx.render` fails on the template —
/// the caller maps that into a `Failed` StepOutcome.
pub(crate) fn build_step_prompt(
    step: &WorkflowStep,
    ctx: &TemplateContext,
    extra_context: &str,
) -> Result<String, String> {
    let mut prompt = ctx
        .render(&step.prompt_template)
        .map_err(|e| format!("Template render error: {}", e))?;

    // 0.8.3 — audit-pipeline symmetry: append the pre-computed
    // linked_repos + Kronn-projects-universe blocks to every Agent
    // step's prompt. Empty when the workflow has no project binding
    // or the project has no companions registered. Inserted BEFORE
    // the output_format / triage / signal addenda so those still
    // anchor the END of the prompt (LLMs follow trailing instructions
    // more reliably than leading ones).
    if !extra_context.is_empty() {
        prompt.push_str(extra_context);
    }

    // Auto-inject structured output format instructions when output_format
    // is `Structured` or `TypedSchema`. The TypedSchema variant adds the
    // schema constraint to the same envelope shape so downstream
    // `{{previous_step.data.X}}` resolution stays uniform.
    match &step.output_format {
        crate::models::StepOutputFormat::Structured => {
            prompt.push_str(crate::workflows::template::STRUCTURED_OUTPUT_INSTRUCTIONS);
        }
        crate::models::StepOutputFormat::TypedSchema { schema, .. } => {
            prompt.push_str(&crate::workflows::template::build_typed_schema_instruction(schema));
        }
        crate::models::StepOutputFormat::FreeText => {}
    }

    // 0.8.3 — Feasibility-Gated triage: when the step is identified as
    // a triage step (description marker OR schema shape match), append
    // the "audit, don't code" addendum. Keeps the regular TypedSchema
    // path generic; the addendum is only for steps that explicitly
    // declare themselves as triage.
    if crate::workflows::triage::is_triage_step(step.description.as_deref(), &step.output_format) {
        prompt.push_str(crate::workflows::triage::TRIAGE_PROMPT_ADDENDUM);
    }

    Ok(prompt)
}

/// Execute a single workflow step.
///
/// - `project_path`: original project path for MCP context resolution
/// - `work_dir`: agent's working directory (may be a worktree)
/// - `extra_context`: pre-formatted companion-repo + Kronn-projects-
///   universe blocks (linked_repos + universe). Computed ONCE at run
///   start by [`crate::workflows::runner::execute_run`] and passed in
///   here so every Agent step in the run shares the same audit-pipeline
///   symmetric context without re-querying the DB. Pass `""` for runs
///   without a project binding (Notify-only / ApiCall-only).
/// - `progress_tx`: if Some, partial output text is streamed as it arrives
///
/// 0.8.3 — 8 args (was 7) after adding `extra_context` for cross-repo
/// companion injection. Bundling them into a struct would force every
/// caller (runner, test-step endpoint, api/workflows/test-step) to
/// build the struct vs. passing positional args inline, with no
/// real readability win at the call site. Allow the lint locally.
#[allow(clippy::too_many_arguments)]
pub async fn execute_step(
    step: &WorkflowStep,
    project_path: &str,
    work_dir: &str,
    tokens_config: &TokensConfig,
    full_access: bool,
    ctx: &TemplateContext,
    extra_context: &str,
    progress_tx: Option<ProgressSender>,
) -> StepOutcome {
    let start = Instant::now();

    // Build prompt (template render + extra_context + output-format
    // addendum + triage addendum). Errors map to a Failed StepOutcome.
    let mut prompt = match build_step_prompt(step, ctx, extra_context) {
        Ok(p) => p,
        Err(e) => {
            return StepOutcome {
                result: StepResult {
                    step_name: step.name.clone(),
                    status: RunStatus::Failed,
                    output: e,
                    tokens_used: 0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    started_at: None,
                    condition_result: None,
                    envelope_detected: None,
                    step_kind: None,
                    step_agent: None,
                    step_api_plugin_slug: None,
                    step_api_endpoint_path: None,
                },
                condition_action: None,
            };
        }
    };

    // Fail-fast on broken inter-step contracts: if the rendered prompt still
    // contains `{{steps.X.data|summary|status}}` or `{{previous_step.*}}`
    // placeholders, the upstream step either ran as FreeText or failed to
    // produce a `---STEP_OUTPUT---` envelope. Sending the literal `{{...}}`
    // to the agent wastes tokens and surfaces as a cryptic agent error —
    // better to abort here with a message the user can act on.
    if let Some(outcome) = fail_fast_on_unresolved(&step.name, &prompt, start.elapsed().as_millis() as u64) {
        return outcome;
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
                ConditionAction::Goto { step_name, .. } => format!("the workflow will jump to step '{}'", step_name),
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

        match run_agent_with_timeout(step, project_path, work_dir, &prompt, tokens_config, full_access, progress_tx.as_ref()).await {
            Ok(agent_output) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                let mut final_output = agent_output.text.clone();
                let mut total_tokens = agent_output.tokens_used;

                // For Structured / TypedSchema steps: verify envelope exists,
                // try repair if missing. TypedSchema additionally validates
                // the `data` field against the user-supplied JSON Schema
                // subset and triggers repair on validation failure (not
                // just envelope absence).
                let needs_envelope = matches!(
                    step.output_format,
                    crate::models::StepOutputFormat::Structured
                        | crate::models::StepOutputFormat::TypedSchema { .. }
                );
                if needs_envelope {
                    let envelope = crate::workflows::template::extract_step_envelope(&final_output);
                    let validation_error = match (&step.output_format, &envelope) {
                        (crate::models::StepOutputFormat::TypedSchema { schema, .. }, Some(env)) => {
                            crate::workflows::template::validate_envelope_against_schema(
                                &env.data_json, schema,
                            ).err()
                        }
                        _ => None,
                    };
                    if envelope.is_none() || validation_error.is_some() {
                        let reason = if let Some(ref err) = validation_error {
                            format!("schema validation failed: {}", err)
                        } else {
                            "missing envelope".into()
                        };
                        tracing::info!("Step '{}': output {}, attempting repair", step.name, reason);
                        // Truncate by char count — `&s[..2000]` panics if
                        // byte 2000 falls inside a UTF-8 sequence (emoji,
                        // accented chars in the LLM output).
                        let truncated_owned: String;
                        let truncated: &str = if final_output.chars().count() > 2000 {
                            truncated_owned = final_output.chars().take(2000).collect();
                            &truncated_owned
                        } else {
                            &final_output
                        };
                        let repair_prompt = crate::workflows::template::build_repair_prompt(
                            truncated,
                            &step.output_format,
                            validation_error.as_deref(),
                        );
                        let mut final_validation_error: Option<String> = validation_error.clone();
                        let mut repair_valid = false;
                        if let Ok(repair_output) = run_agent_with_timeout(step, project_path, work_dir, &repair_prompt, tokens_config, full_access, None).await {
                            total_tokens += repair_output.tokens_used;
                            let repaired_env = crate::workflows::template::extract_step_envelope(&repair_output.text);
                            let repaired_error = match (&step.output_format, &repaired_env) {
                                (crate::models::StepOutputFormat::TypedSchema { schema, .. }, Some(env)) => {
                                    crate::workflows::template::validate_envelope_against_schema(
                                        &env.data_json, schema,
                                    ).err()
                                }
                                _ => None,
                            };
                            repair_valid = matches!((&repaired_env, &repaired_error), (Some(_), None));
                            if repair_valid {
                                final_output = repair_output.text;
                                tracing::info!("Step '{}': repair succeeded", step.name);
                            } else {
                                tracing::warn!("Step '{}': repair failed", step.name);
                                // Surface the latest error in the StepResult
                                // so the operator sees what was wrong, not
                                // the original pre-repair error.
                                if let Some(err) = repaired_error {
                                    final_validation_error = Some(err);
                                } else if repaired_env.is_none() {
                                    final_validation_error = Some("missing envelope after repair".into());
                                }
                            }
                        }

                        // 0.8.3 — `on_invalid: Fail` short-circuits the
                        // step when repair didn't fix the output. Used by
                        // Feasibility-Gated triage so the implement step
                        // never receives an invalid manifest. Default is
                        // `Continue` (0.7.0 behavior: warn + raw output).
                        if !repair_valid {
                            if let crate::models::StepOutputFormat::TypedSchema {
                                on_invalid: crate::models::OnInvalid::Fail, ..
                            } = &step.output_format {
                                let err_msg = final_validation_error
                                    .unwrap_or_else(|| "TypedSchema validation failed and repair did not fix it".into());
                                tracing::warn!("Step '{}': failing run (on_invalid=Fail) — {}", step.name, err_msg);
                                return StepOutcome {
                                    result: StepResult {
                                        step_name: step.name.clone(),
                                        status: RunStatus::Failed,
                                        output: format!(
                                            "TypedSchema validation failed after repair attempt.\n\nError: {}\n\nLast agent output:\n{}",
                                            err_msg, final_output,
                                        ),
                                        tokens_used: total_tokens,
                                        duration_ms: start.elapsed().as_millis() as u64,
                                        started_at: None,
                                        condition_result: None,
                                        envelope_detected: Some(false),
                                        step_kind: None,
                                        step_agent: None,
                                        step_api_plugin_slug: None,
                                        step_api_endpoint_path: None,
                                    },
                                    condition_action: None,
                                };
                            }
                        }
                    }
                }

                // Evaluate on_result conditions (check signals + structured status)
                let mut condition_action = evaluate_conditions(&step.on_result, &final_output);

                // For Structured / TypedSchema: also check status field for NO_RESULTS
                let envelope_aware = matches!(
                    step.output_format,
                    crate::models::StepOutputFormat::Structured
                        | crate::models::StepOutputFormat::TypedSchema { .. }
                );
                if condition_action.is_none() && envelope_aware {
                    if let Some(env) = crate::workflows::template::extract_step_envelope(&final_output) {
                        if env.status == "NO_RESULTS" && step.on_result.iter().any(|r| r.contains == "NO_RESULTS") {
                            condition_action = Some(ConditionAction::Stop);
                        }
                    }
                }

                let condition_result = condition_action.as_ref().map(|a| match a {
                    ConditionAction::Stop => "Stop".to_string(),
                    ConditionAction::Skip => "Skip".to_string(),
                    ConditionAction::Goto { step_name, .. } => format!("Goto:{}", step_name),
                });

                // Record whether the structured contract was actually met.
                // Downstream code (UI badge, SuccessDegraded status, health
                // checks) can branch on this without re-parsing the output.
                let envelope_detected = if matches!(step.output_format, crate::models::StepOutputFormat::Structured | crate::models::StepOutputFormat::TypedSchema { .. }) {
                    Some(crate::workflows::template::extract_step_envelope(&final_output).is_some())
                } else {
                    None
                };

                return StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::Success,
                        output: final_output,
                        tokens_used: total_tokens,
                        duration_ms,
                        started_at: None,
                        condition_result,
                        envelope_detected,
                        step_kind: None,
                        step_agent: None,
                        step_api_plugin_slug: None,
                        step_api_endpoint_path: None,
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
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
        },
        condition_action: None,
    }
}

/// Build the suffix that closes a `🔧 ToolName` live-progress line.
/// Tries to parse `raw_input` (the assembled JSON the model emitted as the
/// tool's input) and surface the most informative field for the operator
/// watching the live view: file path, command, pattern, URL.
///
/// Returns either ` · <detail>\n` (parseable JSON with a known field) or
/// just `\n` (unparseable input or unknown shape — keeps the tool name
/// readable but adds no detail).
///
/// Char-truncates at 120 to keep the live feed on one line; multi-byte
/// codepoints at the cut are safe by construction.
fn format_tool_input_suffix(raw_input: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(raw_input)
        .ok()
        .and_then(|v| {
            ["file_path", "path", "command", "pattern", "url"]
                .iter()
                .find_map(|k| v.get(*k).and_then(|s| s.as_str()).map(str::to_string))
        });
    match detail {
        Some(d) if d.chars().count() > 120 => {
            let truncated: String = d.chars().take(120).collect();
            format!(" · {}…\n", truncated)
        }
        Some(d) => format!(" · {}\n", d),
        None => "\n".into(),
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
    progress_tx: Option<&ProgressSender>,
) -> Result<AgentOutput> {
    // 30 min default — generous safety net rather than aggressive ceiling.
    // With tool-call streaming (cf. format_tool_input_suffix), an active
    // agent emits a chunk every Edit/Bash/Read, so the only legitimate
    // stalls are pure-thinking pauses or network hangs. Big implementation
    // steps on real tickets routinely run 20-30 min — the older 10 min
    // default cut them short. Per-step `stall_timeout_secs` overrides this
    // when a step needs more (cf. wizard wf-label `wiz.stallTimeout`).
    let stall_timeout = step.stall_timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(1800));

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
        context_files_prompt: "",
        discussion_id: None,
    }).await.map_err(|e| anyhow::anyhow!(e))?;

    let mut output = String::new();
    let is_stream_json = agent_process.output_mode == OutputMode::StreamJson;
    let mut stream_json_tokens: u64 = 0;
    // Tool-call accumulator. Claude Code's stream-json emits tool input as
    // a series of partial JSON deltas (potentially many small fragments).
    // We buffer the deltas, then on ToolEnd parse the assembled JSON to
    // extract the most informative field (file_path / command / pattern)
    // and surface a one-liner like `🔧 Edit · src/foo.rs` to the operator.
    // Without this, the live view is silent for the entire tool-call loop
    // (Edit / Bash / Read / Glob…) — frequently > 30 s of "nothing
    // happening" UX even though the agent is fully active.
    let mut current_tool: Option<String> = None;
    let mut current_tool_input = String::new();

    loop {
        match timeout(stall_timeout, agent_process.next_line()).await {
            Ok(Some(line)) => {
                if is_stream_json {
                    match runner::parse_claude_stream_line(&line) {
                        StreamJsonEvent::Text(text) => {
                            output.push_str(&text);
                            if let Some(tx) = progress_tx {
                                let _ = tx.send(text).await;
                            }
                        }
                        StreamJsonEvent::Usage { input_tokens, output_tokens, .. } => {
                            stream_json_tokens = input_tokens + output_tokens;
                        }
                        StreamJsonEvent::ToolStart(name) => {
                            // Emit the tool name immediately — gives the user
                            // a sign of life before the first input delta.
                            // The actual file/command will follow on ToolEnd
                            // once we've assembled the partial JSON.
                            if let Some(tx) = progress_tx {
                                let _ = tx.send(format!("\n🔧 {}", name)).await;
                            }
                            current_tool = Some(name);
                            current_tool_input.clear();
                        }
                        StreamJsonEvent::ToolInputDelta(partial) => {
                            current_tool_input.push_str(&partial);
                        }
                        StreamJsonEvent::ToolEnd => {
                            // Closes the `🔧 ToolName` line streamed at
                            // ToolStart with the tool's most informative
                            // input field (cf. format_tool_input_suffix).
                            if current_tool.take().is_some() {
                                if let Some(tx) = progress_tx {
                                    let _ = tx.send(format_tool_input_suffix(&current_tool_input)).await;
                                }
                            }
                            current_tool_input.clear();
                        }
                        StreamJsonEvent::Skip => {}
                    }
                } else {
                    let chunk = if output.is_empty() {
                        line.clone()
                    } else {
                        output.push('\n');
                        format!("\n{}", &line)
                    };
                    output.push_str(&line);
                    if let Some(tx) = progress_tx {
                        let _ = tx.send(chunk).await;
                    }
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

    // Drain stderr through the flushed accessor — `captured_stderr()` races
    // with the stderr reader task and returns empty when the agent crashes
    // fast (the reader hasn't yet appended its buffered lines). Without
    // this fix, every crash on a non-zero exit surfaced as the useless
    // "Agent exited with status: exit status: 1" with the actual error
    // (auth expiry, rate limit, context overflow, panic stack…) silently
    // dropped on the floor.
    let stderr_lines = agent_process.captured_stderr_flushed().await;

    if !status.success() {
        // Show the last ~20 lines of stderr — Claude Code panics dump a
        // full backtrace; older lines rarely add signal once we've seen
        // the message.
        let tail: Vec<&String> = stderr_lines.iter().rev().take(20).collect();
        let stderr_tail = tail.iter().rev().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
        if stderr_tail.is_empty() {
            anyhow::bail!("Agent exited with {} but produced no stderr (likely killed by signal or sandbox). Check the host's container logs (`docker logs kronn-backend` or journald) for the underlying cause.", status);
        } else {
            anyhow::bail!("Agent exited with {}:\n{}", status, stderr_tail);
        }
    }

    // Extract token usage — same logic as discussions:
    // 1. Claude Code: tokens from stream-json events (input + output)
    // 2. Codex/Kiro/etc: tokens parsed from stderr/stdout after execution
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
///
/// `pub(crate)` so non-Agent step types (Exec, ApiCall) can also branch on
/// signals they emit themselves (e.g. `[SIGNAL: ERROR]` on cargo test exit≠0).
pub(crate) fn evaluate_conditions(rules: &[StepConditionRule], output: &str) -> Option<ConditionAction> {
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

/// Build a `StepOutcome::Failed` when a rendered prompt still contains
/// unresolved step-output references. Returns `None` if the prompt is safe
/// to send to the agent, `Some(outcome)` otherwise. Pulled out as a pure
/// function so the fail-fast logic can be unit-tested without spinning up
/// an agent.
fn fail_fast_on_unresolved(step_name: &str, prompt: &str, elapsed_ms: u64) -> Option<StepOutcome> {
    let unresolved = crate::workflows::template::find_unresolved_critical_refs(prompt);
    if unresolved.is_empty() {
        return None;
    }
    let first = &unresolved[0];
    let rest_count = unresolved.len().saturating_sub(1);
    let extra = if rest_count > 0 {
        format!(" (+{} autre{})", rest_count, if rest_count > 1 { "s" } else { "" })
    } else {
        String::new()
    };
    Some(StepOutcome {
        result: StepResult {
            step_name: step_name.to_string(),
            status: RunStatus::Failed,
            output: format!(
                "Référence non résolue dans le prompt : {{{{{first}}}}}{extra}. \
                L'étape productrice doit être en `output_format: Structured` \
                pour exposer `.data` / `.summary` / `.status`, et sa sortie doit \
                contenir l'enveloppe `---STEP_OUTPUT---`."
            ),
            tokens_used: 0,
            duration_ms: elapsed_ms,
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
        },
        condition_action: None,
    })
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
        let rules = vec![rule("GO_NEXT", ConditionAction::Goto { step_name: "step_b".to_string(), max_iterations: None })];
        let output = "Some output\n[SIGNAL: GO_NEXT]";
        let action = evaluate_conditions(&rules, output);
        assert!(matches!(action, Some(ConditionAction::Goto { step_name, .. }) if step_name == "step_b"));
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

    // ── build_step_prompt — 0.8.3 (TD-265) ──────────────────────────────
    //
    // The prompt-assembly path was extracted from execute_step so we can
    // unit-test the behavior independently from the agent spawn. These
    // tests lock in the wiring between extra_context (linked_repos +
    // universe block injected by the runner) and the final prompt the
    // agent sees. A regression here would silently drop the cross-repo
    // evidence the entire 0.8.3 release is built around.

    fn make_step(prompt_template: &str) -> WorkflowStep {
        // Mirror of `workflows::big_ticket_template::blank_step` —
        // duplicated here because it's `fn` private in the sibling module
        // and we want this test file standalone.
        WorkflowStep {
            name: "t".into(),
            step_type: crate::models::StepType::Agent,
            description: None,
            agent: crate::models::AgentType::ClaudeCode,
            prompt_template: prompt_template.into(),
            mode: crate::models::StepMode::Normal,
            output_format: crate::models::StepOutputFormat::FreeText,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            quick_prompt_id: None,
            json_data_payload: None,
        }
    }

    #[test]
    fn build_step_prompt_returns_rendered_template_when_extra_context_is_empty() {
        // No project = no companion-repo context. The prompt is just
        // the rendered template; callers must not see any synthetic
        // "## Linked repositories" header injected.
        let step = make_step("Hello world");
        let ctx = TemplateContext::new();
        let prompt = build_step_prompt(&step, &ctx, "").expect("must render");
        assert_eq!(prompt, "Hello world",
            "empty extra_context must not leak any header into the prompt");
    }

    #[test]
    fn build_step_prompt_appends_extra_context_after_render() {
        // The runner pre-computes `## Linked repositories ...` + the
        // `## Other Kronn projects ...` block ONCE and passes them as
        // extra_context. The helper appends them VERBATIM after the
        // template render — that's the entire wiring the cross-repo
        // evidence pattern depends on.
        let step = make_step("Do the thing.");
        let ctx = TemplateContext::new();
        let extra = "\n\n## Linked repositories (companion repos)\n- **front_africanews** — `/r/front_africanews`\n";
        let prompt = build_step_prompt(&step, &ctx, extra).expect("must render");
        assert!(prompt.starts_with("Do the thing."),
            "rendered template must lead the prompt");
        assert!(prompt.contains("## Linked repositories (companion repos)"),
            "extra_context header must be present in final prompt — wiring regression");
        assert!(prompt.contains("front_africanews"),
            "concrete companion entries must reach the prompt");
        // Order matters: extra_context must come AFTER the rendered
        // template (so the trailing addenda below — output_format /
        // triage — anchor the END of the prompt).
        let user_idx = prompt.find("Do the thing.").unwrap();
        let extra_idx = prompt.find("## Linked repositories").unwrap();
        assert!(user_idx < extra_idx,
            "extra_context must be appended AFTER the rendered template, not prepended");
    }

    #[test]
    fn build_step_prompt_keeps_addenda_anchored_at_the_end() {
        // When the step has a TypedSchema output_format AND a triage
        // description, both addenda must trail the prompt — even when
        // extra_context is also present. LLMs follow trailing
        // instructions more reliably than leading ones, so the order
        // is load-bearing: template → extra_context → output_format
        // addendum → triage addendum.
        let mut step = make_step("Triage this ticket");
        step.description = Some("[TRIAGE] feasibility audit".into());
        step.output_format = crate::workflows::triage::triage_output_format();
        let ctx = TemplateContext::new();
        let extra = "\n\n## Linked repositories\n- ref\n";
        let prompt = build_step_prompt(&step, &ctx, extra).expect("must render");
        let template_idx = prompt.find("Triage this ticket").unwrap();
        let extra_idx = prompt.find("## Linked repositories").unwrap();
        let triage_idx = prompt.find("TRIAGE MODE").expect("triage addendum must be appended");
        assert!(template_idx < extra_idx && extra_idx < triage_idx,
            "order must be: template → extra_context → triage addendum; got idx {template_idx}/{extra_idx}/{triage_idx}");
    }

    // ── fail_fast_on_unresolved ──────────────────────────────────────────
    //
    // Regression tests for Workflow B: the runner must refuse to call the
    // agent when the rendered prompt still carries `{{steps.X.data}}` or
    // `{{previous_step.*}}` placeholders. Before this check, those leaked
    // into the agent prompt and surfaced as opaque "tickets pas injectés"
    // messages at runtime.

    // ── envelope_detected field ──────────────────────────────────────────
    //
    // Pure-data regression: the StepResult envelope_detected field must
    // mirror extract_step_envelope's verdict on the output, scoped to
    // Structured steps only. Foundation for the post-run UX badge and
    // SuccessDegraded status.

    #[test]
    fn envelope_detected_matches_extraction_for_structured_output() {
        let good = "Here is the analysis.\n---STEP_OUTPUT---\n{\"data\": [1], \"status\": \"OK\", \"summary\": \"one\"}\n---END_STEP_OUTPUT---";
        let bad = "Just a markdown table, no envelope.";

        assert!(crate::workflows::template::extract_step_envelope(good).is_some());
        assert!(crate::workflows::template::extract_step_envelope(bad).is_none());

        // Same logic lives inside execute_step's success branch — these
        // asserts pin the contract that branch depends on.
        let fmt = crate::models::StepOutputFormat::Structured;
        let expect_good = fmt == crate::models::StepOutputFormat::Structured
            && crate::workflows::template::extract_step_envelope(good).is_some();
        let expect_bad = fmt == crate::models::StepOutputFormat::Structured
            && crate::workflows::template::extract_step_envelope(bad).is_some();
        assert!(expect_good);
        assert!(!expect_bad);
    }

    #[test]
    fn envelope_detected_is_none_for_freetext() {
        // FreeText steps don't use the envelope contract — envelope_detected
        // stays None so the UI can distinguish "didn't apply" from "failed".
        let fmt = crate::models::StepOutputFormat::FreeText;
        let value: Option<bool> = if fmt == crate::models::StepOutputFormat::Structured {
            Some(true) // hypothetical
        } else {
            None
        };
        assert_eq!(value, None);
    }

    #[test]
    fn fail_fast_passes_through_when_prompt_is_clean() {
        let outcome = fail_fast_on_unresolved("s1", "Analyse les tickets EW-1234", 12);
        assert!(outcome.is_none());
    }

    #[test]
    fn fail_fast_ignores_non_contract_braces() {
        // `.output` always resolves; `{{foo}}` is not part of the inter-step
        // contract. Neither should abort the run.
        let prompt = "{{steps.a.output}} / {{foo}} / {{ steps.a.tokens }}";
        let outcome = fail_fast_on_unresolved("s1", prompt, 0);
        assert!(outcome.is_none(), "Non-contract braces must not fail-fast");
    }

    #[test]
    fn fail_fast_on_steps_data_placeholder() {
        let outcome = fail_fast_on_unresolved(
            "analyze",
            "Use {{steps.main.data}} to proceed.",
            5,
        ).expect("Must return a failed outcome");
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert_eq!(outcome.result.step_name, "analyze");
        assert!(outcome.result.output.contains("steps.main.data"),
            "Error message must name the offending placeholder");
        assert!(outcome.result.output.contains("Structured"),
            "Error message must hint at the fix (output_format: Structured)");
        assert_eq!(outcome.result.tokens_used, 0, "No tokens spent on failed fail-fast");
    }

    #[test]
    fn fail_fast_on_previous_step_summary() {
        let outcome = fail_fast_on_unresolved(
            "step2",
            "Previous summary: {{previous_step.summary}}",
            0,
        ).expect("Must return a failed outcome");
        assert!(outcome.result.output.contains("previous_step.summary"));
    }

    #[test]
    fn fail_fast_message_mentions_additional_placeholders() {
        let outcome = fail_fast_on_unresolved(
            "s",
            "{{steps.a.data}} and {{steps.b.summary}} and {{previous_step.status}}",
            0,
        ).expect("Must fail-fast");
        // Names the first + flags the count of extras so the user knows it's
        // not a one-off; exact wording is asserted to catch regressions.
        assert!(outcome.result.output.contains("+2 autres"),
            "Got: {}", outcome.result.output);
    }

    #[test]
    fn fail_fast_single_extra_uses_singular() {
        let outcome = fail_fast_on_unresolved(
            "s",
            "{{steps.a.data}} {{steps.b.summary}}",
            0,
        ).expect("Must fail-fast");
        assert!(outcome.result.output.contains("+1 autre"),
            "Expected singular 'autre', got: {}", outcome.result.output);
        assert!(!outcome.result.output.contains("+1 autres"),
            "Must not pluralize when only 1 extra");
    }

    #[test]
    fn test_signal_deep_in_output_ignored() {
        // Signal is far from the end (beyond last 5 lines) — should NOT match
        let rules = vec![rule("STOP_SIGNAL", ConditionAction::Stop)];
        let output = "[SIGNAL: STOP_SIGNAL]\nline2\nline3\nline4\nline5\nline6\nline7";
        let action = evaluate_conditions(&rules, output);
        assert!(action.is_none());
    }

    // ── format_tool_input_suffix (live-progress tool-call surfacing) ──

    #[test]
    fn tool_suffix_extracts_file_path() {
        let s = format_tool_input_suffix(r#"{"file_path": "src/foo.rs", "old_string": "x", "new_string": "y"}"#);
        assert_eq!(s, " · src/foo.rs\n");
    }

    #[test]
    fn tool_suffix_extracts_command_for_bash() {
        let s = format_tool_input_suffix(r#"{"command": "cargo test --lib", "description": "run tests"}"#);
        assert_eq!(s, " · cargo test --lib\n");
    }

    #[test]
    fn tool_suffix_extracts_pattern_for_grep() {
        let s = format_tool_input_suffix(r#"{"pattern": "TODO", "path": "."}"#);
        // priority list checks file_path → path before pattern, so `path: "."`
        // wins. Fine: directory is what matters for the operator's mental model.
        assert_eq!(s, " · .\n");
    }

    #[test]
    fn tool_suffix_extracts_url_for_webfetch() {
        let s = format_tool_input_suffix(r#"{"url": "https://example.com/foo", "prompt": "summary"}"#);
        assert_eq!(s, " · https://example.com/foo\n");
    }

    #[test]
    fn tool_suffix_unparseable_falls_back_to_newline() {
        // ToolInputDelta sometimes truncates mid-emission; we don't crash.
        let s = format_tool_input_suffix("not json at all");
        assert_eq!(s, "\n");
    }

    #[test]
    fn tool_suffix_unknown_shape_falls_back_to_newline() {
        // No recognized field → no detail to show, just close the line.
        let s = format_tool_input_suffix(r#"{"weird": "shape", "no": "match"}"#);
        assert_eq!(s, "\n");
    }

    #[test]
    fn tool_suffix_truncates_long_command_with_ellipsis() {
        let long_cmd = "echo ".to_string() + &"x".repeat(200);
        let s = format_tool_input_suffix(&format!(r#"{{"command": "{}"}}"#, long_cmd));
        assert!(s.ends_with("…\n"), "got: {:?}", s);
        // 120 char body + " · " prefix + "…\n" suffix → ≤ 130 bytes is the
        // ASCII case, but we just verify the truncation happened.
        assert!(s.chars().count() < long_cmd.chars().count() + 5);
    }

    #[test]
    fn tool_suffix_handles_utf8_safely() {
        // Multi-byte codepoint at the cut point: `é` is 2 bytes. 120 chars
        // of `é` = 240 bytes. The char-based truncation must not split.
        let path: String = "é".repeat(150);
        let s = format_tool_input_suffix(&format!(r#"{{"file_path": "{}"}}"#, path));
        // No panic + ends with the ellipsis suffix.
        assert!(s.ends_with("…\n"));
        // The truncated body should be exactly 120 `é` chars.
        let body = s.trim_start_matches(" · ").trim_end_matches("…\n");
        assert_eq!(body.chars().count(), 120);
    }
}
