//! Step execution: runs a single workflow step via the agent runner.
//!
//! Handles: prompt rendering, per-step MCPs, stall detection, retry,
//! and on_result condition evaluation.

use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{timeout, Duration};

use crate::agents::activity::AgentActivitySink;
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
#[derive(Debug)]
struct AgentOutput {
    attempt_id: u32,
    text: String,
    /// `None`: the runtime reported no usage for this run.
    tokens_used: Option<u64>,
    prompt_cache: runner::PromptCacheUsage,
    /// The usage `tokens_used` came from, parts kept apart: an attempt's cost is
    /// computed from these, never from the total (KT-894). `None` when the
    /// runtime reported only a total, or nothing.
    reported_usage: Option<runner::ReportedUsage>,
    /// The cost the agent itself reported, when it gives one.
    reported_cost_usd: Option<f64>,
    native_tool_calls: Vec<NativeToolCallLog>,
    runtime_notices: Vec<String>,
}

/// A total that includes an unmeasured run is itself unknown: summing only
/// the known parts would present a lower bound as the step's cost.
fn add_tokens(total: Option<u64>, more: Option<u64>) -> Option<u64> {
    Some(total?.saturating_add(more?))
}

// Keep transport notices when repair/escalation replaces the model's answer.
// They precede the final envelope and must not participate in schema/signals.
fn with_runtime_notices(output: String, notices: &[String]) -> String {
    let mut missing = Vec::new();
    for notice in notices {
        if !output.lines().any(|line| line == notice) && !missing.contains(notice) {
            missing.push(notice.clone());
        }
    }
    if missing.is_empty() {
        output
    } else {
        format!("{}\n\n{output}", missing.join("\n"))
    }
}

/// Optional sender for streaming partial agent output during step execution.
pub type ProgressSender = tokio::sync::mpsc::Sender<String>;

/// The refusal of a native ACP step agent whose full-access setting is off,
/// read as the runner reads it; `None` when the agent may start.
pub(crate) fn native_full_access_refusal_for(agent: &AgentType) -> Option<String> {
    (runner::requires_explicit_full_access(agent) && !crate::core::config::saved_full_access(agent))
        .then(|| runner::native_full_access_refusal(agent))
}

/// The step fails once on that refusal (`native_full_access_required: …`):
/// the same on every attempt, so never retried and never a quota wait; an
/// ordinary failure otherwise, so `on_failure` still runs.
fn native_full_access_outcome(step: &WorkflowStep, refusal: String) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: refusal,
            tokens_used: Some(0),
            duration_ms: 0,
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: Some(Box::default()),
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
        },
        condition_action: None,
    }
}

pub(crate) fn step_model_override(
    step: &WorkflowStep,
    connection: Option<&ExternalApiConnection>,
) -> Option<String> {
    step.agent_settings
        .as_ref()
        .and_then(|settings| settings.model.clone())
        .or_else(|| {
            let tier = step
                .agent_settings
                .as_ref()
                .and_then(|settings| settings.tier)
                .unwrap_or_default();
            connection.and_then(|connection| {
                crate::http_transport::connection_tier_model(connection, tier)
            })
        })
}

/// KT-910 — the run's artifacts directory, granted read-only to a Claude Code
/// or Codex step whose prompt names it. Other agents get no grant: their
/// launch has no read-only policy to carry it.
pub(crate) fn artifacts_read_only_dirs(
    step: &WorkflowStep,
    ctx: &TemplateContext,
    prompt: &str,
) -> Vec<String> {
    match ctx.get("run.artifacts_dir") {
        Some(dir)
            if !dir.is_empty()
                && prompt.contains(dir)
                && matches!(
                    step.agent,
                    crate::models::AgentType::ClaudeCode | crate::models::AgentType::Codex
                ) =>
        {
            vec![dir.to_string()]
        }
        _ => Vec::new(),
    }
}

/// Build the full agent-ready prompt for a step: template render +
/// output-format addendum + triage addendum.
/// Does NOT append the signal-protocol instructions — those depend on
/// runtime `on_result` rules and are still added by [`execute_step`]
/// after this helper returns.
///
/// Extracted from [`execute_step`] in 0.8.3 (TD-265) so the prompt
/// assembly is unit-testable independently from the agent spawn path.
/// Returns `Err(String)` when strict rendering rejects an unknown variable,
/// incomplete placeholder or unsupported filter —
/// the caller maps that into a `Failed` StepOutcome.
pub(crate) fn build_step_prompt(
    step: &WorkflowStep,
    ctx: &TemplateContext,
) -> Result<String, String> {
    let mut prompt = ctx
        .render_strict(&step.prompt_template)
        .map_err(|e| format!("Template render error: {}", e))?;

    // KT-926 — nothing about the user's other Kronn projects is appended here:
    // the prompt goes to the model provider. The output_format / triage /
    // signal addenda below anchor the END of the prompt (LLMs follow trailing
    // instructions more reliably than leading ones).

    // Only a step that declares read-only repositories says so: an undeclared
    // step's prompt stays exactly its rendered template (KT-926).
    if !step.read_only_repos.is_empty() {
        prompt.push_str("\n\nWORKFLOW REPOSITORY ACCESS\nA repository path in a ticket, project list or companion document is a location, not a filesystem permission grant. Do not assume that a main checkout outside the working directory is readable. If a read is denied, report that limitation instead of claiming to have inspected the repository.\n");
        prompt.push_str("This step declares the following local repositories for read-only access (including Git history). Never edit them; the launch must fail if the read-only policy cannot be configured:\n");
        for path in &step.read_only_repos {
            prompt.push_str(&format!(
                "- {}\n",
                serde_json::to_string(path).unwrap_or_default()
            ));
        }
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
            prompt.push_str(&crate::workflows::template::build_typed_schema_instruction(
                schema,
            ));
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

/// One retry step of an agent step's backoff (15 s, 30 s, 45 s… capped at 90 s).
#[cfg(not(test))]
const RETRY_BACKOFF_UNIT: Duration = Duration::from_secs(15);
/// Unit tests check the retry order, not the wait: same shape, in milliseconds.
#[cfg(test)]
const RETRY_BACKOFF_UNIT: Duration = Duration::from_millis(15);

/// Execute a single workflow step.
///
/// - `project_path`: original project path for MCP context resolution
/// - `work_dir`: agent's working directory (may be a worktree)
/// - `progress_tx`: if Some, partial output text is streamed as it arrives
///
/// Bundling the arguments into a struct would force every caller (runner,
/// test-step endpoint, api/workflows/test-step) to build the struct vs.
/// passing positional args inline, with no real readability win at the call
/// site. Allow the lint locally.
#[allow(clippy::too_many_arguments)]
pub async fn execute_step(
    step: &WorkflowStep,
    project_path: &str,
    // Decides whether the step's agents get a GitHub token (D2).
    project_id: Option<&str>,
    work_dir: &str,
    tokens_config: &TokensConfig,
    full_access: bool,
    ctx: &TemplateContext,
    progress_tx: Option<ProgressSender>,
    activity: Option<&AgentActivitySink>,
    // 2026-06-12 (run-9 finding) — the user's [agents.model_tiers] overrides
    // never reached workflow agent spawns: a step pinned to Reasoning ran on
    // the BUILT-IN fallback (opus) instead of the configured model (fable).
    // Discussions already plumb this (streaming.rs:484); workflows now do too.
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
    http_endpoints: Option<&crate::models::setup::HttpEndpoints>,
    ollama_context_overrides: Option<&std::collections::HashMap<String, u64>>,
    native_tools: Option<Arc<dyn crate::agents::tools::ToolExecutor>>,
    catalog_db: Option<&crate::db::Database>,
    // KT-793 — the step's room capability; only its own agent attempts carry it.
    room: Option<&runner::WorkflowStepBridgeContext>,
    // ADR-005 slice 1 (KT-847) — `Some(WorkflowRun.id)` pins the
    // skills/directives/profiles every agent spawn in this call resolves,
    // so a later step of the SAME run isn't affected by an edit or
    // deletion made mid-run (see `RunSnapshotCache`). `None` for ad-hoc,
    // non-persisted invocations (e.g. the workflow "Test step" preview).
    run_id: Option<&str>,
) -> StepOutcome {
    let start = Instant::now();

    if let Some(refusal) = native_full_access_refusal_for(&step.agent) {
        return native_full_access_outcome(step, refusal);
    }

    // Build prompt (template render + output-format addendum + triage
    // addendum). Errors map to a Failed StepOutcome.
    let mut prompt = match build_step_prompt(step, ctx) {
        Ok(p) => p,
        Err(e) => {
            return StepOutcome {
                result: StepResult {
                    step_name: step.name.clone(),
                    status: RunStatus::Failed,
                    output: e,
                    tokens_used: Some(0),
                    duration_ms: start.elapsed().as_millis() as u64,
                    started_at: None,
                    condition_result: None,
                    envelope_detected: None,
                    step_kind: None,
                    step_agent: None,
                    step_model: None,
                    step_api_plugin_slug: None,
                    step_api_endpoint_path: None,
                    is_rollback: false,
                    child_run_id: None,
                    agent_provenance: Some(Box::default()),
                    native_tool_calls: Box::default(),
                    cached_prompt_tokens: None,
                    cache_write_prompt_tokens: None,
                    last_activity: None,
                    quota_wait: None,
                    terminal_stop: None,
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
    if let Some(outcome) =
        fail_fast_on_unresolved(&step.name, &prompt, start.elapsed().as_millis() as u64)
    {
        return outcome;
    }
    let artifact_dirs = artifacts_read_only_dirs(step, ctx, &prompt);

    // Auto-inject on_result signal instructions into the prompt
    let valid_rules: Vec<_> = step
        .on_result
        .iter()
        .filter(|r| !r.contains.is_empty())
        .collect();
    if !valid_rules.is_empty() {
        // Reconcile with the Structured/TypedSchema envelope instruction, which
        // says the `---STEP_OUTPUT---` block "must be the LAST thing". A step
        // that is BOTH structured AND branches on a signal would otherwise get
        // two contradictory "must be last" rules. Spell out the exact order:
        // envelope first, then the signal as the only trailing line.
        let structured = !matches!(
            step.output_format,
            crate::models::StepOutputFormat::FreeText
        );
        if structured {
            prompt.push_str("\n\n---\nIMPORTANT — output order: FIRST emit the ---STEP_OUTPUT--- envelope exactly as described above, THEN a single signal line as the VERY LAST line (the signal line is the only thing allowed after the envelope).\n");
        } else {
            prompt.push_str(
                "\n\n---\nIMPORTANT — After your response, you MUST end with a signal line.\n",
            );
        }
        prompt.push_str(
            "The signal MUST be the very last line of your response, in this exact format:\n\n",
        );
        for rule in &valid_rules {
            let action_label = match &rule.action {
                ConditionAction::Stop => {
                    "the workflow will stop (no further steps needed)".to_string()
                }
                ConditionAction::Skip => "the next step will be skipped".to_string(),
                ConditionAction::Goto { step_name, .. } => {
                    format!("the workflow will jump to step '{}'", step_name)
                }
            };
            prompt.push_str(&format!(
                "  [SIGNAL: {}]  — use this if {} — {}\n",
                rule.contains,
                condition_description(&rule.contains),
                action_label
            ));
        }
        prompt
            .push_str("  [SIGNAL: CONTINUE]  — use this if none of the above apply (default)\n\n");
        prompt.push_str("You MUST include exactly one [SIGNAL: ...] as the very last line. Do NOT mention or repeat signal names anywhere else in your response.\n");
    }

    // Execute with retry logic
    let max_attempts = step.retry.as_ref().map(|r| r.max_retries + 1).unwrap_or(1);
    let mut last_error = String::new();
    let mut provenance = WorkflowAgentProvenance::default();

    let resolved_connection = match resolve_step_connection(step, catalog_db).await {
        Ok(connection) => connection,
        Err(error) => {
            return StepOutcome {
                result: StepResult {
                    step_name: step.name.clone(),
                    status: RunStatus::Failed,
                    output: format!("preflight_failed:{error}"),
                    tokens_used: Some(0),
                    duration_ms: start.elapsed().as_millis() as u64,
                    started_at: None,
                    condition_result: None,
                    envelope_detected: None,
                    step_kind: Some("preflight_failed".into()),
                    step_agent: Some(step.agent.clone()),
                    step_model: None,
                    step_api_plugin_slug: None,
                    step_api_endpoint_path: None,
                    is_rollback: false,
                    child_run_id: None,
                    agent_provenance: Some(Box::default()),
                    native_tool_calls: Box::default(),
                    cached_prompt_tokens: None,
                    cache_write_prompt_tokens: None,
                    last_activity: None,
                    quota_wait: None,
                    terminal_stop: None,
                },
                condition_action: None,
            };
        }
    };
    let model_override = step_model_override(step, resolved_connection.as_ref());

    // Authoritative check immediately before the first possible provider
    // dispatch. Initial workflow-wide validation remains in the runner, but
    // availability can change while earlier steps execute. The returned
    // effective model is the only value allowed to reach the transport.
    let preflight = match preflight_workflow_launch(
        catalog_db,
        work_dir,
        step,
        resolved_connection.as_ref(),
        model_override.as_deref(),
        model_tiers,
    )
    .await
    {
        Ok(resolution) => resolution,
        Err(error) => {
            return StepOutcome {
                result: StepResult {
                    step_name: step.name.clone(),
                    status: RunStatus::Failed,
                    output: error.to_string(),
                    tokens_used: Some(0),
                    duration_ms: start.elapsed().as_millis() as u64,
                    started_at: None,
                    condition_result: None,
                    envelope_detected: None,
                    step_kind: Some("preflight_failed".into()),
                    step_agent: Some(step.agent.clone()),
                    step_model: model_override,
                    step_api_plugin_slug: None,
                    step_api_endpoint_path: None,
                    is_rollback: false,
                    child_run_id: None,
                    agent_provenance: Some(Box::default()),
                    native_tool_calls: Box::default(),
                    cached_prompt_tokens: None,
                    cache_write_prompt_tokens: None,
                    last_activity: None,
                    quota_wait: None,
                    terminal_stop: None,
                },
                condition_action: None,
            };
        }
    };
    let requested_model = preflight.requested_model;
    let effective_model = preflight.effective_model;
    let preflight_warning = preflight.warning;

    // Resolve the step's named external connection once, outside the retry
    // loop. Without this a step pointed at a `Custom` connection failed every
    // attempt with "the selected external API connection is unavailable" —
    // accurate but unactionable, since nothing carried WHICH connection the
    // step meant. Discussions and Quick Prompts already resolve this identity;
    // steps now do too (KT-545).
    let external_http = resolved_connection.as_ref().and_then(|connection| {
        crate::http_transport::external_http_runtime(connection, tokens_config)
    });

    for attempt in 0..max_attempts {
        if attempt > 0 {
            // Rate-limit-aware backoff. The empirical failure mode (run-10,
            // 2026-06-13) is the provider rate-limit / transient auth blip —
            // an agent exits 1 with no output. The old 2^attempt (2s/4s/8s)
            // rarely outlived a 429 window; 15s·attempt (15s/30s/45s, capped
            // 90s) gives a per-minute limit time to clear without stalling a
            // legitimate transient retry for minutes.
            let delay = (RETRY_BACKOFF_UNIT * attempt).min(RETRY_BACKOFF_UNIT * 6);
            tracing::info!(
                "Step '{}' retry {}/{} after {:?}",
                step.name,
                attempt,
                max_attempts - 1,
                delay
            );
            tokio::time::sleep(delay).await;
        }

        match run_agent_with_timeout(
            step,
            project_path,
            project_id,
            work_dir,
            &prompt,
            &artifact_dirs,
            tokens_config,
            full_access,
            model_tiers,
            http_endpoints,
            ollama_context_overrides,
            native_tools.clone(),
            progress_tx.as_ref(),
            activity,
            external_http.as_ref(),
            requested_model.as_deref(),
            effective_model.as_deref(),
            preflight_warning.as_ref(),
            &mut provenance,
            WorkflowAgentAttemptRole::Initial,
            attempt + 1,
            room,
            run_id,
        )
        .await
        {
            Ok(agent_output) => {
                provenance.selected_attempt = Some(agent_output.attempt_id);
                let duration_ms = start.elapsed().as_millis() as u64;
                let mut final_output = agent_output.text.clone();
                let mut total_tokens = agent_output.tokens_used;
                let mut native_tool_calls = agent_output.native_tool_calls;
                let mut runtime_notices = agent_output.runtime_notices;

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
                        (
                            crate::models::StepOutputFormat::TypedSchema { schema, .. },
                            Some(env),
                        ) => crate::workflows::template::validate_envelope_against_schema(
                            &env.data_json,
                            schema,
                        )
                        .err(),
                        _ => None,
                    };
                    if envelope.is_none() || validation_error.is_some() {
                        let reason = if let Some(ref err) = validation_error {
                            format!("schema validation failed: {}", err)
                        } else {
                            "missing envelope".into()
                        };
                        tracing::warn!(
                            target: "kronn::invariant",
                            step = %step.name, reason = %reason,
                            "step output broke the envelope contract — repair attempt"
                        );
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
                        let repair_preflight = match preflight_workflow_launch(
                            catalog_db,
                            work_dir,
                            step,
                            resolved_connection.as_ref(),
                            model_override.as_deref(),
                            model_tiers,
                        )
                        .await
                        {
                            Ok(resolution) => resolution,
                            Err(error) => {
                                tracing::warn!(
                                    "Step '{}': repair preflight failed: {}",
                                    step.name,
                                    error
                                );
                                last_error = error.to_string();
                                continue;
                            }
                        };
                        let repair_res = run_agent_with_timeout(
                            step,
                            project_path,
                            project_id,
                            work_dir,
                            &repair_prompt,
                            &artifact_dirs,
                            tokens_config,
                            full_access,
                            model_tiers,
                            http_endpoints,
                            ollama_context_overrides,
                            native_tools.clone(),
                            None,
                            activity,
                            external_http.as_ref(),
                            repair_preflight.requested_model.as_deref(),
                            repair_preflight.effective_model.as_deref(),
                            repair_preflight.warning.as_ref(),
                            &mut provenance,
                            WorkflowAgentAttemptRole::Repair,
                            attempt + 1,
                            room,
                            run_id,
                        )
                        .await;
                        if let Err(ref e) = repair_res {
                            // The repair RUN itself failed (spawn/timeout) —
                            // distinct from "repair produced invalid output".
                            tracing::warn!("Step '{}': repair run failed: {}", step.name, e);
                        }
                        if let Ok(repair_output) = repair_res {
                            runtime_notices.extend(repair_output.runtime_notices.clone());
                            total_tokens = add_tokens(total_tokens, repair_output.tokens_used);
                            native_tool_calls.extend(repair_output.native_tool_calls.clone());
                            let repaired_env = crate::workflows::template::extract_step_envelope(
                                &repair_output.text,
                            );
                            let repaired_error = match (&step.output_format, &repaired_env) {
                                (
                                    crate::models::StepOutputFormat::TypedSchema { schema, .. },
                                    Some(env),
                                ) => crate::workflows::template::validate_envelope_against_schema(
                                    &env.data_json,
                                    schema,
                                )
                                .err(),
                                _ => None,
                            };
                            repair_valid =
                                matches!((&repaired_env, &repaired_error), (Some(_), None));
                            if repair_valid {
                                provenance.selected_attempt = Some(repair_output.attempt_id);
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
                                    final_validation_error =
                                        Some("missing envelope after repair".into());
                                }
                            }
                        }

                        // 0.8.10 — quality escalation local→Claude. If a LOCAL
                        // (Ollama) step still fails validation after repair,
                        // retry ONCE on the paid reasoning tier before giving
                        // up. The escalation RATE (logged on target
                        // kronn::ollama::escalation) is the health metric that
                        // reveals which steps are too hard for the chosen local
                        // model. Fires only for TypedSchema (needs a schema to
                        // validate against) that ran on Ollama.
                        if !repair_valid
                            && step.agent == crate::models::AgentType::Ollama
                            && matches!(
                                step.output_format,
                                crate::models::StepOutputFormat::TypedSchema { .. }
                            )
                        {
                            tracing::warn!(
                                target: "kronn::ollama::escalation",
                                step = %step.name,
                                "local schema validation failed after repair — escalating to Claude"
                            );
                            let escalated = escalation_step(step);
                            let escalated_model = step_model_override(&escalated, None);
                            let escalation_preflight = match preflight_workflow_launch(
                                catalog_db,
                                work_dir,
                                &escalated,
                                None,
                                escalated_model.as_deref(),
                                model_tiers,
                            )
                            .await
                            {
                                Ok(resolution) => resolution,
                                Err(error) => {
                                    tracing::warn!(
                                        target: "kronn::ollama::escalation",
                                        step = %step.name,
                                        error = %error,
                                        "escalation catalog preflight refused dispatch"
                                    );
                                    continue;
                                }
                            };
                            let esc_res = run_agent_with_timeout(
                                &escalated,
                                project_path,
                                project_id,
                                work_dir,
                                &prompt,
                                &artifact_dirs,
                                tokens_config,
                                full_access,
                                model_tiers,
                                http_endpoints,
                                ollama_context_overrides,
                                native_tools.clone(),
                                None,
                                activity,
                                None,
                                escalation_preflight.requested_model.as_deref(),
                                escalation_preflight.effective_model.as_deref(),
                                escalation_preflight.warning.as_ref(),
                                &mut provenance,
                                WorkflowAgentAttemptRole::Escalation,
                                attempt + 1,
                                room,
                                run_id,
                            )
                            .await;
                            if let Err(ref e) = esc_res {
                                tracing::warn!(
                                    target: "kronn::ollama::escalation",
                                    step = %step.name,
                                    error = %e,
                                    "escalation run itself failed (spawn/timeout) — falling through to on_invalid handling"
                                );
                            }
                            if let Ok(esc) = esc_res {
                                runtime_notices.extend(esc.runtime_notices.clone());
                                total_tokens = add_tokens(total_tokens, esc.tokens_used);
                                native_tool_calls.extend(esc.native_tool_calls.clone());
                                let esc_env =
                                    crate::workflows::template::extract_step_envelope(&esc.text);
                                let esc_error = match (&step.output_format, &esc_env) {
                                    (crate::models::StepOutputFormat::TypedSchema { schema, .. }, Some(env)) => {
                                        crate::workflows::template::validate_envelope_against_schema(&env.data_json, schema).err()
                                    }
                                    _ => None,
                                };
                                if matches!((&esc_env, &esc_error), (Some(_), None)) {
                                    provenance.selected_attempt = Some(esc.attempt_id);
                                    final_output = esc.text;
                                    repair_valid = true; // valid now → don't fail below
                                    tracing::info!(target: "kronn::ollama::escalation", step = %step.name, "escalation to Claude succeeded");
                                } else {
                                    tracing::warn!(target: "kronn::ollama::escalation", step = %step.name, "escalation to Claude also failed");
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
                                on_invalid: crate::models::OnInvalid::Fail,
                                ..
                            } = &step.output_format
                            {
                                let err_msg = final_validation_error.unwrap_or_else(|| {
                                    "TypedSchema validation failed and repair did not fix it".into()
                                });
                                tracing::warn!(
                                    "Step '{}': failing run (on_invalid=Fail) — {}",
                                    step.name,
                                    err_msg
                                );
                                let (cached_prompt_tokens, cache_write_prompt_tokens) =
                                    provenance.prompt_cache_totals();
                                return StepOutcome {
                                    result: StepResult {
                                        step_name: step.name.clone(),
                                        status: RunStatus::Failed,
                                        output: format!(
                                            "TypedSchema validation failed after repair attempt.\n\nError: {}\n\nLast agent output:\n{}",
                                            err_msg, with_runtime_notices(final_output, &runtime_notices),
                                        ),
                                        tokens_used: total_tokens,
                                        duration_ms: start.elapsed().as_millis() as u64,
                                        started_at: None,
                                        condition_result: None,
                                        envelope_detected: Some(false),
                                        step_kind: None,
                                        step_agent: None,
                                        step_model: None,
                                        step_api_plugin_slug: None,
                                        step_api_endpoint_path: None,
                                        is_rollback: false,
                                        child_run_id: None,
                                        agent_provenance: Some(Box::new(provenance)),
                                        native_tool_calls: native_tool_calls.into_boxed_slice(),
                                        cached_prompt_tokens,
                                        cache_write_prompt_tokens,
                                        last_activity: None,
                                        quota_wait: None,
                                        terminal_stop: None,
                                    },
                                    condition_action: None,
                                };
                            }
                        }
                    }
                }

                // 2026-06-13 — Multi-agent review: instead of a successive
                // `Goto` re-run loop (each round re-reads everything from
                // scratch), debate the output with a SECOND agent (different
                // model family) in a shared transcript until they agree. The
                // reviewer reads the artifact once, then only the conversation
                // delta — cheaper AND a real back-and-forth. The converged
                // output replaces this step's result before signal evaluation.
                if let Some(mar) = step.multi_agent_review.clone() {
                    let pre_debate = final_output.clone();
                    match run_multi_agent_debate(
                        step, &mar, &final_output, project_path, project_id, work_dir,
                        tokens_config, full_access, model_tiers, http_endpoints,
                        ollama_context_overrides,
                        native_tools.clone(),
                        progress_tx.as_ref(),
                        activity,
                        external_http.as_ref(),
                        model_override.as_deref(),
                        resolved_connection.as_ref(),
                        catalog_db,
                        &mut provenance,
                        attempt + 1,
                        run_id,
                    ).await {
                        Ok((converged, debate_tokens, debate_tool_calls, selected_attempt)) => {
                            total_tokens = add_tokens(total_tokens, debate_tokens);
                            native_tool_calls.extend(debate_tool_calls);
                            // Envelope safety: on a Structured/TypedSchema step
                            // the converged output MUST still carry a valid
                            // envelope (the debate author re-emits it). If the
                            // author dropped/broke it, keep the pre-debate
                            // (already-validated) output so ingestion never
                            // sees a corrupted manifest.
                            let ok = if needs_envelope {
                                match crate::workflows::template::extract_step_envelope(&converged) {
                                    None => false,
                                    Some(env) => match &step.output_format {
                                        crate::models::StepOutputFormat::TypedSchema { schema, .. } =>
                                            crate::workflows::template::validate_envelope_against_schema(&env.data_json, schema).is_ok(),
                                        _ => true,
                                    },
                                }
                            } else { true };
                            if ok {
                                if let Some(selected) = selected_attempt {
                                    provenance.selected_attempt = Some(selected);
                                }
                                final_output = converged;
                            } else {
                                tracing::warn!(
                                    "Step '{}': debate output lacks a valid envelope — keeping the pre-debate manifest",
                                    step.name
                                );
                                final_output = pre_debate;
                            }
                        }
                        Err(e) => tracing::warn!(
                            "Step '{}' multi_agent_review debate errored ({}) — keeping the pre-debate output",
                            step.name, e
                        ),
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
                    if let Some(env) =
                        crate::workflows::template::extract_step_envelope(&final_output)
                    {
                        if env.status == "NO_RESULTS" {
                            // Honor the action the author DECLARED on the
                            // matching rule (a NO_RESULTS → Goto("handle_empty")
                            // recovery step must run) — the old hardcoded Stop
                            // hijacked the branch whenever the agent emitted the
                            // envelope but forgot the [SIGNAL: …] line.
                            condition_action = step
                                .on_result
                                .iter()
                                .find(|r| r.contains == "NO_RESULTS")
                                .map(|r| r.action.clone());
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
                let envelope_detected = if matches!(
                    step.output_format,
                    crate::models::StepOutputFormat::Structured
                        | crate::models::StepOutputFormat::TypedSchema { .. }
                ) {
                    Some(crate::workflows::template::extract_step_envelope(&final_output).is_some())
                } else {
                    None
                };

                let (cached_prompt_tokens, cache_write_prompt_tokens) =
                    provenance.prompt_cache_totals();
                return StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::Success,
                        output: with_runtime_notices(final_output, &runtime_notices),
                        tokens_used: total_tokens,
                        duration_ms,
                        started_at: None,
                        condition_result,
                        envelope_detected,
                        step_kind: None,
                        step_agent: None,
                        step_model: None,
                        step_api_plugin_slug: None,
                        step_api_endpoint_path: None,
                        is_rollback: false,
                        child_run_id: None,
                        agent_provenance: Some(Box::new(provenance)),
                        native_tool_calls: native_tool_calls.into_boxed_slice(),
                        cached_prompt_tokens,
                        cache_write_prompt_tokens,
                        last_activity: None,
                        quota_wait: None,
                        terminal_stop: None,
                    },
                    condition_action,
                };
            }
            Err(e) => {
                last_error = format!("{}", e);
                tracing::warn!(
                    "Step '{}' attempt {} failed: {}",
                    step.name,
                    attempt + 1,
                    last_error
                );
                // KT-811 — a quota refusal does not clear within the retry
                // backoff; retrying only spends the run's attempts.
                if let Some(quota) = super::quota_wait::classify(&last_error, chrono::Utc::now()) {
                    provenance.selected_attempt = None;
                    return StepOutcome {
                        result: StepResult {
                            step_name: step.name.clone(),
                            status: RunStatus::Failed,
                            output: format!("Provider quota or session limit: {last_error}"),
                            tokens_used: None,
                            duration_ms: start.elapsed().as_millis() as u64,
                            started_at: None,
                            condition_result: None,
                            envelope_detected: None,
                            step_kind: None,
                            step_agent: None,
                            step_model: None,
                            step_api_plugin_slug: None,
                            step_api_endpoint_path: None,
                            is_rollback: false,
                            child_run_id: None,
                            agent_provenance: Some(Box::new(provenance)),
                            native_tool_calls: Box::default(),
                            cached_prompt_tokens: None,
                            cache_write_prompt_tokens: None,
                            last_activity: None,
                            quota_wait: Some(quota),
                            terminal_stop: None,
                        },
                        condition_action: None,
                    };
                }
            }
        }
    }

    // All retries exhausted. #8 — if the failure was a STALL and the step
    // declares `on_timeout`, route via that action (Goto a recovery/notify
    // step, or Stop) instead of failing the run: the runner honours a Goto
    // on a Failed step (see runner.rs). The Failed result is kept for the
    // audit trail; the run continues gracefully.
    let stalled = is_stall_error(&last_error);
    let (output, condition_result, condition_action) =
        timeout_routing(stalled, &step.on_timeout, max_attempts, &last_error);
    provenance.selected_attempt = None;
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output,
            // Failed attempts may have spent tokens that were never collected.
            tokens_used: None,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: Some(Box::new(provenance)),
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
        },
        condition_action,
    }
}

/// True when a step's last error came from the stall watchdog (no agent output
/// for `stall_timeout_secs`), as opposed to a crash/non-zero exit. Kept in sync
/// with the `bail!` in `drive_agent_to_output`; pure + unit-tested.
pub(crate) fn is_stall_error(err_msg: &str) -> bool {
    err_msg.contains("Agent stalled")
}

/// #8 — decide how an EXHAUSTED step resolves. A stall (no output for the
/// stall timeout) with a declared `on_timeout` routes via that action
/// (Goto a recovery/notify step, or Stop) so the run continues gracefully;
/// anything else (crash, or a stall without `on_timeout`) fails normally and
/// tips into the rollback chain. Returns `(output, condition_result,
/// condition_action)`. Pure so the routing is unit-tested without an agent.
pub(crate) fn timeout_routing(
    stalled: bool,
    on_timeout: &Option<ConditionAction>,
    max_attempts: u32,
    last_error: &str,
) -> (String, Option<String>, Option<ConditionAction>) {
    if stalled {
        if let Some(action) = on_timeout {
            return (
                format!("Step stalled after {max_attempts} attempt(s) ({last_error}). Routed via on_timeout."),
                Some("on_timeout".to_string()),
                Some(action.clone()),
            );
        }
    }
    (
        format!("Failed after {max_attempts} attempts. Last error: {last_error}"),
        None,
        None,
    )
}

/// Wrap a `TypedSchema` step's author schema in the canonical envelope shape
/// ({data, status, summary}) so Ollama's grammar-constrained `format` emits a
/// bare envelope object that `extract_step_envelope` (strategy-2) recovers —
/// the post-extract schema validation on `data` then runs unchanged. Returns
/// `None` for non-TypedSchema steps (free text / vanilla Structured), which
/// keep the streaming, prompt-injection path. Ollama-only: consumed solely by
/// `AgentStartConfig.ollama_format`; other agents get their schema via the
/// prompt (see the output_format addendum near the top of this module).
fn ollama_envelope_format(
    output_format: &crate::models::StepOutputFormat,
) -> Option<serde_json::Value> {
    match output_format {
        crate::models::StepOutputFormat::TypedSchema { schema, .. } => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "data": schema,
                "status": { "type": "string" },
                "summary": { "type": "string" }
            },
            "required": ["data", "status"]
        })),
        _ => None,
    }
}

/// Build the escalation variant of a step: the same task, but forced onto the
/// paid reasoning tier (Claude) instead of the local model. Used as a quality
/// safety-net when a LOCAL (Ollama) `TypedSchema` step still fails schema
/// validation after one repair attempt — rare, but a garbage manifest would
/// poison downstream steps. Clears any pinned local model so tier resolution
/// applies. Pure so the transform is unit-tested.
fn escalation_step(step: &WorkflowStep) -> WorkflowStep {
    let mut escalated = step.clone();
    escalated.agent = crate::models::AgentType::ClaudeCode;
    escalated.agent_settings = Some(crate::models::AgentSettings {
        tools: step.agent_settings.as_ref().and_then(|s| s.tools.clone()),
        model: None,
        tier: Some(crate::models::ModelTier::Reasoning),
        reasoning_effort: None,
        max_tokens: None,
        connection_id: None,
    });
    escalated
}

/// Resolve the named external HTTP connection a step points at.
///
/// Returns `Ok(None)` only when the step names no connection. A requested id
/// that cannot be read, no longer exists, or belongs to another agent is an
/// error so dispatch cannot fall through to a provider default.
/// Whether the step's named connection is the reviewer's endpoint too.
///
/// A connection belongs to an agent, not to a role. When the reviewer runs on
/// the SAME agent as the author, they share it — dropping it sent the reviewer
/// to the provider default while the author spoke to the configured endpoint,
/// on the same step, with nothing on screen to say so. A reviewer on a
/// DIFFERENT agent has no connection of its own to name yet, and inheriting
/// this one would point it at an endpoint that does not serve it.
fn reviewer_shares_step_connection(step: &WorkflowStep, reviewer_agent: &AgentType) -> bool {
    *reviewer_agent == step.agent
}

pub(crate) async fn resolve_step_connection(
    step: &WorkflowStep,
    catalog_db: Option<&crate::db::Database>,
) -> Result<Option<ExternalApiConnection>> {
    let connection_id = step
        .agent_settings
        .as_ref()
        .and_then(|settings| settings.connection_id.clone());
    let Some(connection_id) = connection_id else {
        return Ok(None);
    };
    let db = catalog_db.ok_or_else(|| {
        anyhow::anyhow!(
            "named connection {connection_id} cannot be resolved without the workflow database"
        )
    })?;
    let lookup_id = connection_id.clone();
    let connection = db
        .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &lookup_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("External API connection {connection_id} was not found"))?;

    if crate::db::external_api_connections::target_for_connection(&connection).agent_type
        != step.agent
    {
        anyhow::bail!(
            "External API connection {connection_id} no longer matches agent {:?}",
            step.agent
        );
    }

    if connection
        .endpoint
        .as_deref()
        .is_none_or(|endpoint| endpoint.trim().is_empty())
    {
        anyhow::bail!("External API connection {connection_id} has no HTTP endpoint");
    }

    Ok(Some(connection))
}

async fn preflight_workflow_launch(
    catalog_db: Option<&crate::db::Database>,
    #[cfg_attr(not(test), allow(unused_variables))] work_dir: &str,
    step: &WorkflowStep,
    connection: Option<&ExternalApiConnection>,
    effective_model: Option<&str>,
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
) -> Result<CatalogPreflightResolution> {
    let Some(database) = catalog_db else {
        let model = runner::effective_model_flag(
            effective_model,
            &step.agent,
            step.agent_settings
                .as_ref()
                .and_then(|settings| settings.tier)
                .unwrap_or_default(),
            model_tiers,
        );
        return Ok(CatalogPreflightResolution {
            requested_model: model.clone(),
            effective_model: model,
            warning: None,
            notice: None,
        });
    };
    let runtime_target_id = connection
        .map(|connection| crate::db::model_catalog::http_runtime_target_id(&connection.id));
    let tier = step
        .agent_settings
        .as_ref()
        .and_then(|settings| settings.tier)
        .unwrap_or_default();
    let resolution = crate::core::model_catalog::preflight_resolve(
        database,
        runtime_target_id.as_deref(),
        step.agent.clone(),
        tier,
        effective_model,
        model_tiers,
    );
    // A routed work dir stands in for the CLI, as in the runner's install check.
    #[cfg(test)]
    let resolution = crate::core::model_catalog::test_routed_discovery(
        runner::resolve_agent_work_dir(Some(work_dir), work_dir)
            .is_ok_and(|dir| runner::test_acp_routes::is_routed(&dir)),
        resolution,
    );
    resolution.await.map_err(|failure| {
        anyhow::anyhow!(
            "model_catalog_preflight_failed:{}",
            serde_json::to_string(&failure).unwrap_or_default()
        )
    })
}

/// A step's catalog skill ids, and the repository skills it names taken from
/// its run's pin: read from the default branch when the run started, so the
/// checkout's branch or the run's worktree never changes them. One the pin
/// lacks stops the step by name instead of running without it.
pub(super) fn step_skills(
    skill_ids: &[String],
    project_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<(Vec<String>, Vec<crate::models::Skill>)> {
    use crate::api::projects::used_skills::parse_repository_skill_id;
    let mut catalog = Vec::new();
    let mut repository = Vec::new();
    for id in skill_ids {
        let Some((owner, slug)) = parse_repository_skill_id(id) else {
            catalog.push(id.clone());
            continue;
        };
        let pinned = run_id.and_then(|run| {
            crate::core::skills::get_skills_snapshot(run, std::slice::from_ref(id)).pop()
        });
        match pinned {
            Some(skill) if Some(owner) == project_id => repository.push(skill),
            _ => anyhow::bail!(
                "Repository skill `{slug}` cannot be loaded: {}",
                if Some(owner) != project_id {
                    "it belongs to another project"
                } else if run_id.is_none() {
                    "it is read from the default branch when a workflow run starts, and this launch is not a run"
                } else {
                    "it was not readable on the default branch when the run started (not committed there, or no longer used by the project)"
                }
            ),
        }
    }
    Ok((catalog, repository))
}

/// Run an agent with optional stall timeout.
/// Returns the agent output text and token usage.
///
/// - `project_path`: original project path for MCP context resolution
/// - `work_dir`: agent's working directory (may be a worktree)
#[allow(clippy::too_many_arguments)]
async fn run_agent_with_timeout(
    step: &WorkflowStep,
    project_path: &str,
    project_id: Option<&str>,
    work_dir: &str,
    prompt: &str,
    read_only_dirs: &[String],
    tokens_config: &TokensConfig,
    full_access: bool,
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
    http_endpoints: Option<&crate::models::setup::HttpEndpoints>,
    ollama_context_overrides: Option<&std::collections::HashMap<String, u64>>,
    native_tools: Option<Arc<dyn crate::agents::tools::ToolExecutor>>,
    progress_tx: Option<&ProgressSender>,
    activity: Option<&AgentActivitySink>,
    external_http: Option<&runner::ExternalHttpRuntime>,
    requested_model: Option<&str>,
    effective_model: Option<&str>,
    preflight_warning: Option<&CatalogPreflightWarning>,
    provenance: &mut WorkflowAgentProvenance,
    role: WorkflowAgentAttemptRole,
    retry: u32,
    room: Option<&runner::WorkflowStepBridgeContext>,
    // See `execute_step`'s `run_id` doc — forwarded unchanged.
    run_id: Option<&str>,
) -> Result<AgentOutput> {
    let started_at = chrono::Utc::now();
    let started = Instant::now();
    let capture = crate::agents::provenance::AgentProvenanceCapture::default();
    // 30 min default — generous safety net rather than aggressive ceiling.
    // With tool-call streaming (cf. format_tool_input_suffix), an active
    // agent emits a chunk every Edit/Bash/Read, so the only legitimate
    // stalls are pure-thinking pauses or network hangs. Big implementation
    // steps on real tickets routinely run 20-30 min — the older 10 min
    // default cut them short. Per-step `stall_timeout_secs` overrides this
    // when a step needs more (cf. wizard wf-label `wiz.stallTimeout`).
    let stall_timeout = step
        .stall_timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(1800));

    // TypedSchema → constrain Ollama decoding to the envelope-wrapped schema
    // (owned here so it outlives the borrow in AgentStartConfig below).
    let ollama_format = ollama_envelope_format(&step.output_format);

    let result = async {
        let (catalog_skill_ids, repository_skills) =
            step_skills(&step.skill_ids, project_id, run_id)?;
        let agent_process = runner::start_agent_with_config(runner::AgentStartConfig {
            provenance: Some(capture.clone()),
            activity: activity.cloned(),
            work_dir: Some(work_dir),
            read_only_repos: &step.read_only_repos,
            read_only_dirs,
            step_tools: step.agent_settings.as_ref().and_then(|s| s.tools.as_ref()),
            full_access,
            skill_ids: &catalog_skill_ids,
            repository_skills: &repository_skills,
            directive_ids: &step.directive_ids,
            profile_ids: &step.profile_ids,
            tier: step
                .agent_settings
                .as_ref()
                .and_then(|s| s.tier)
                .unwrap_or_default(),
            model_tiers,
            http_endpoints,
            ollama_context_overrides,
            // Workflow steps already expose their own timeout; use that exact
            // value for the initial HTTP request too instead of the discussion
            // Settings budget or a transport-only constant.
            http_request_timeout: Some(stall_timeout),
            // KT-932 — the same delay, but measured against the model's real
            // progress (bytes, ACP frames) and ending with the generation
            // cancelled on the model server, not just the step abandoned.
            idle_timeout: Some(stall_timeout),
            ollama_format: ollama_format.as_ref(),
            // Explicit per-step model (from the wizard's model picker) — now
            // actually consumed at run time, not just stamped for display.
            model_override: effective_model,
            // KT-646 — explicit per-step reasoning effort, from the wizard's
            // effort picker. Wins over the tier's configured preset; `None`
            // falls back to that preset, then to the CLI default (see
            // `runner::effective_reasoning_effort`).
            reasoning_effort_override: step
                .agent_settings
                .as_ref()
                .and_then(|s| s.reasoning_effort.as_deref()),
            max_tokens_override: step.agent_settings.as_ref().and_then(|s| s.max_tokens),
            // The named connection this step points at. `AgentType::Custom` is
            // shared by every OpenAI-compatible connection, so without this the
            // runner refuses the spawn outright.
            external_http,
            run_snapshot_id: run_id,
            tools: native_tools,
            // The bridge's discussion is the room the capability names.
            discussion_id: room.map(|room| room.discussion_id.as_str()),
            workflow_step_context: room,
            project_id,
            ..runner::AgentStartConfig::new(&step.agent, project_path, prompt, tokens_config)
        })
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

        // 0.8.8 — the post-spawn consumption loop + finalize is extracted into
        // `drive_agent_to_output` (generic over `runner::AgentIo`) so it's
        // unit-testable with a `ScriptedProcess` — no real CLI, no tokens.
        drive_agent_to_output(
            agent_process,
            progress_tx,
            activity,
            stall_timeout,
            &step.agent,
            &step.name,
        )
        .await
    }
    .await;
    let runtime = capture
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let id = provenance.attempts.len() as u32 + 1;
    let (cost_usd, cost_unknown_reason) = attempt_cost(
        &step.agent,
        &runtime.observed_models,
        runtime.resolved_model.as_deref().or(requested_model),
        &result,
    );
    provenance.attempts.push(WorkflowAgentAttempt {
        id,
        role,
        retry,
        agent: step.agent.clone(),
        tier: step
            .agent_settings
            .as_ref()
            .and_then(|settings| settings.tier)
            .unwrap_or_default(),
        connection_id: step
            .agent_settings
            .as_ref()
            .and_then(|settings| settings.connection_id.clone()),
        requested_model: requested_model.map(str::to_string),
        resolved_model: runtime.resolved_model,
        preflight_warning: preflight_warning.cloned(),
        model_applied: runtime.model_applied,
        observed_models: runtime.observed_models,
        format_fallback: runtime.format_fallback,
        npx_fallback_command: runtime.npx_fallback_command,
        npx_fallback_version: runtime.npx_fallback_version,
        started_at,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        succeeded: result.is_ok(),
        cached_prompt_tokens: result
            .as_ref()
            .ok()
            .and_then(|output| output.prompt_cache.cached_prompt_tokens),
        cache_write_prompt_tokens: result
            .as_ref()
            .ok()
            .and_then(|output| output.prompt_cache.cache_write_prompt_tokens),
        session_id: runtime.session_id,
        cost_usd,
        cost_unknown_reason,
    });
    result.map(|mut output: AgentOutput| {
        output.attempt_id = id;
        output
    })
}

/// What one finished attempt cost, or why that is unknown — priced exactly like
/// a discussion reply (`core::pricing::price_reply`, KT-894): the agent's own
/// figure when it reports one, else the detailed counters at the rates of the
/// model that served it. Unknown is `None` plus a reason, never `0.0`.
///
/// The serving model is the one the provider reported when it reported exactly
/// one; with none reported the resolved (else requested) model stands in, as it
/// does for a reply. Several reported models leave one aggregate usage that no
/// single rate prices.
fn attempt_cost(
    agent: &AgentType,
    observed_models: &[String],
    fallback_model: Option<&str>,
    result: &Result<AgentOutput>,
) -> (Option<f64>, Option<String>) {
    let Ok(output) = result else {
        return (
            None,
            Some("the attempt failed before the runtime reported its usage".into()),
        );
    };
    let tokens_used = output.tokens_used.unwrap_or(0);
    if tokens_used == 0 && output.reported_cost_usd.is_none() {
        return (
            None,
            Some("the runtime reported no usage for this attempt".into()),
        );
    }
    let model =
        match observed_models {
            [] => fallback_model,
            [only] => Some(only.as_str()),
            _ => return (
                None,
                Some(
                    "several models served the attempt, so its aggregate usage cannot be priced"
                        .into(),
                ),
            ),
        };
    let agent_label = serde_json::to_string(agent)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string();
    let counters = output.reported_usage.and_then(|usage| {
        crate::core::pricing::TokenCounters::from_agent_report(
            &agent_label,
            usage.input_tokens,
            usage.output_tokens,
            usage.prompt_cache.cached_prompt_tokens,
            usage.prompt_cache.cache_write_prompt_tokens,
        )
    });
    let priced = crate::core::pricing::price_reply(
        &agent_label,
        model,
        tokens_used,
        output.reported_cost_usd,
        counters,
    );
    let reason = match priced.cost_usd {
        Some(_) => None,
        None => priced
            .cost_unknown
            .map(|reason| reason.reason().to_string()),
    };
    (priced.cost_usd, reason)
}

/// 2026-06-10 — format a useful error when an agent process fails with an
/// EMPTY stderr (the worst diagnostic case: a 23-min `implement` on a huge
/// ticket exited 1 silently, and all we used to say was "killed by signal
/// or sandbox"). Two cheap signals salvage it: whether the process was
/// killed by a SIGNAL (no exit code ⇒ OOM/timeout), and the tail of the
/// agent's STDOUT (the streamed reply — usually non-empty even when stderr
/// is: a partial answer, a rate-limit line, a "context too long" message
/// right before death). Pure + tested; the caller just `bail!`s the string.
fn format_silent_exit_error(exit_desc: &str, killed_by_signal: bool, stdout: &str) -> String {
    let stdout_tail = {
        let lines: Vec<&str> = stdout.lines().collect();
        let start = lines.len().saturating_sub(15);
        lines[start..].join("\n")
    };
    let hint = if killed_by_signal {
        "killed by a SIGNAL (no exit code) — most often the OOM killer (the agent + backend share the container memory limit) or a host-level timeout. Lower the step's blast radius (split a big `implement` into atomic sub-tasks) or raise the container memory."
    } else {
        "exited non-zero with no stderr — usually a rate-limit, an auth expiry, or a context-length overflow surfaced on stdout below, not stderr."
    };
    if stdout_tail.trim().is_empty() {
        format!("Agent exited with {exit_desc} and produced NO output at all — {hint}")
    } else {
        format!("Agent exited with {exit_desc} and no stderr — {hint}\n\n── last stdout (agent stream) ──\n{stdout_tail}")
    }
}

/// Consume an agent process to completion : stream output (with live
/// `progress_tx` updates + tool-call breadcrumbs), enforce the stall
/// timeout, then collect text + tokens. Generic over [`runner::AgentIo`]
/// (0.8.8 test-seam) so the loop is exercised by a `ScriptedProcess` in
/// tests without spawning a CLI.
async fn drive_agent_to_output(
    mut process: impl runner::AgentIo,
    progress_tx: Option<&ProgressSender>,
    activity: Option<&AgentActivitySink>,
    stall_timeout: Duration,
    agent: &AgentType,
    step_name: &str,
) -> Result<AgentOutput> {
    let mut output = String::new();
    let mut text_blocks = runner::TextBlockJoiner::default();
    let is_stream_json = process.output_mode() == OutputMode::StreamJson;
    let mut stream_json_tokens: u64 = 0;
    let mut stream_json_cache = runner::PromptCacheUsage::default();
    let mut stream_json_usage: Option<runner::ReportedUsage> = None;
    let mut stream_json_cost: Option<f64> = None;
    let mut stream_json_failure: Option<runner::StreamJsonFailure> = None;
    // Tool-call accumulator (see run_agent_with_timeout's doc): Claude Code's
    // stream-json emits tool input as partial JSON deltas; we buffer them and
    // surface a `🔧 Edit · src/foo.rs` one-liner on ToolEnd.
    let mut current_tool: Option<()> = None;
    // Ollama streams raw token fragments (no '\n' re-join); CLI text agents
    // stream lines.
    let raw_stream = process.raw_token_stream();
    // An ACP run's own watchdog sees tool calls, thoughts and keepalives and
    // already ends a silent run with this delay; timing text alone would kill
    // a run busy with a tool.
    let text_stall = (!process.activity_watched()).then_some(stall_timeout);

    loop {
        let next = match text_stall {
            Some(limit) => timeout(limit, process.next_line()).await,
            None => Ok(process.next_line().await),
        };
        match next {
            Ok(Some(line)) => {
                if is_stream_json {
                    match runner::parse_claude_stream_line(&line) {
                        StreamJsonEvent::Text(text) => {
                            let text = text_blocks.join(text);
                            output.push_str(&text);
                            if let Some(tx) = progress_tx {
                                let _ = tx.send(text).await;
                            }
                        }
                        StreamJsonEvent::Usage {
                            input_tokens,
                            output_tokens,
                            cost_usd,
                            prompt_cache,
                        } => {
                            stream_json_tokens = input_tokens + output_tokens;
                            stream_json_cache = prompt_cache;
                            stream_json_usage = Some(runner::ReportedUsage {
                                input_tokens,
                                output_tokens,
                                prompt_cache,
                            });
                            if let Some(cost) = cost_usd {
                                stream_json_cost = Some(cost);
                            }
                        }
                        StreamJsonEvent::TerminalError(failure) => {
                            stream_json_tokens = stream_json_tokens
                                .max(failure.input_tokens + failure.output_tokens);
                            stream_json_failure = Some(failure);
                        }
                        StreamJsonEvent::ToolStart(name) => {
                            // A sign of life as soon as a tool starts: its
                            // category only, never its name or its input.
                            let category = crate::agents::activity::category_of(&name);
                            if let Some(tx) = progress_tx {
                                let _ = tx.send(format!("\n🔧 {}", category.as_str())).await;
                            }
                            crate::agents::activity::tool_started(activity, category);
                            current_tool = Some(());
                        }
                        StreamJsonEvent::ToolInputDelta(_) => {}
                        StreamJsonEvent::ToolEnd => {
                            text_blocks.block_ended();
                            // Closes the `🔧 Category` line streamed at ToolStart.
                            if current_tool.take().is_some() {
                                if let Some(tx) = progress_tx {
                                    let _ = tx.send("\n".to_owned()).await;
                                }
                            }
                        }
                        // A workflow step shares no thread with the next one.
                        StreamJsonEvent::SessionId(_) | StreamJsonEvent::Skip => {}
                    }
                } else {
                    let chunk = if raw_stream || output.is_empty() {
                        line.clone()
                    } else {
                        output.push('\n');
                        format!("\n{}", line)
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
                tracing::warn!(
                    "Step '{}' stalled (no output for {:?}), killing agent",
                    step_name,
                    stall_timeout
                );
                process.kill().await;
                anyhow::bail!("Agent stalled (no output for {}s)", stall_timeout.as_secs());
            }
        }
    }

    // Wait for process to finish.
    let status = process.wait().await;
    process.fix_ownership();

    // Drain stderr through the flushed accessor — `captured_stderr()` races
    // with the stderr reader task and returns empty when the agent crashes
    // fast (the reader hasn't yet appended its buffered lines). Without
    // this fix, every crash on a non-zero exit surfaced as the useless
    // "Agent exited with status: exit status: 1" with the actual error
    // (auth expiry, rate limit, context overflow, panic stack…) silently
    // dropped on the floor.
    let stderr_lines = process.captured_stderr_flushed().await;

    if let Some(failure) = stream_json_failure {
        anyhow::bail!(failure.user_message());
    }

    let success = status.map(|s| s.success).unwrap_or(false);
    if !success {
        let exit_desc = match status {
            Some(s) => format!("exit code {:?}", s.code),
            None => "unknown exit status".to_string(),
        };
        // Show the last ~20 lines of stderr — Claude Code panics dump a
        // full backtrace; older lines rarely add signal once we've seen
        // the message.
        let tail: Vec<&String> = stderr_lines.iter().rev().take(20).collect();
        let stderr_tail = tail
            .iter()
            .rev()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !stderr_tail.is_empty() {
            anyhow::bail!("Agent exited with {}:\n{}", exit_desc, stderr_tail);
        }
        // 2026-06-10 — empty stderr is the WORST diagnostic: a 23-min
        // `implement` on a huge ticket exited 1 silently and all we said
        // was "killed by signal or sandbox". Two cheap signals salvage it:
        //  1. `code: None` ⇒ the process was killed by a SIGNAL (SIGKILL on
        //     OOM, SIGTERM on a host timeout) — say so explicitly.
        //  2. The agent's STDOUT (the streamed response, accumulated in
        //     `output`) is usually NON-empty even when stderr is: a partial
        //     reply, a rate-limit line, a "context too long" message right
        //     before it died. Surface its tail.
        let killed_by_signal = matches!(status, Some(ref s) if s.code.is_none());
        anyhow::bail!(
            "{}",
            format_silent_exit_error(&exit_desc, killed_by_signal, &output)
        );
    }

    // Extract token usage — same precedence as discussions:
    // 1. Claude Code CLI: tokens from stream-json events (input + output)
    // 2. Structured transports (ACP): usage events reported by the runtime
    // 3. Codex/Kiro/etc: tokens parsed from stderr/stdout after execution
    // A model run is never free, so a zero from every source means unknown.
    let prompt_cache = if stream_json_tokens > 0 {
        stream_json_cache
    } else {
        process.reported_prompt_cache()
    };
    let reported_usage = if stream_json_tokens > 0 {
        stream_json_usage
    } else {
        process.reported_usage_counters()
    };
    let tokens_used = if stream_json_tokens > 0 {
        Some(stream_json_tokens)
    } else if let Some(reported) = process.reported_token_usage() {
        Some(reported)
    } else {
        let (cleaned, count) = runner::parse_token_usage(agent, &output, &stderr_lines);
        // Adopt the cleaned output unconditionally: some agents (GeminiCli)
        // return count=0 but still strip real noise (MCP handshake markers,
        // "[MCP error]…" lines) — gating on count>0 silently re-injected that
        // noise into the recorded step output and every {{steps.X.output}}.
        output = cleaned;
        (count > 0).then_some(count)
    };

    match tokens_used {
        Some(tokens) => tracing::info!("Step '{}' finished — {} tokens used", step_name, tokens),
        None => tracing::info!("Step '{}' finished — token usage not reported", step_name),
    }

    let native_tool_calls = native_tool_calls_from_stderr(&stderr_lines);

    Ok(AgentOutput {
        attempt_id: 0, // Assigned by the launch recorder, outside the IO loop.
        text: output,
        tokens_used,
        prompt_cache,
        reported_usage,
        reported_cost_usd: stream_json_cost,
        native_tool_calls,
        runtime_notices: stderr_lines
            .into_iter()
            .filter(|line| line.starts_with("[structured-output fallback:"))
            .collect(),
    })
}

/// Extract the safe, durable part of an HTTP agent's tool trace. The runner's
/// stderr breadcrumb contains arguments for live diagnostics; workflow rows
/// intentionally retain only the public tool name and success bit.
fn native_tool_calls_from_stderr(lines: &[String]) -> Vec<NativeToolCallLog> {
    lines
        .iter()
        .filter_map(|line| {
            let rest = line.strip_prefix("[kronn-internal: ")?;
            let name_end = rest.find('(')?;
            let name = &rest[..name_end];
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return None;
            }
            let ok = if rest.ends_with("→ ok]") {
                true
            } else if rest.ends_with("→ error]") {
                false
            } else {
                return None;
            };
            Some(NativeToolCallLog {
                name: name.to_string(),
                ok,
            })
        })
        .collect()
}

/// 2026-06-13 — "Multi-agent review" debate (see `WorkflowStep.multi_agent_review`).
/// The step's own agent has just produced `planner_output`; now a SECOND agent
/// (the reviewer, ideally a different model family) challenges it in a shared
/// transcript. Rounds alternate reviewer → author until the reviewer emits
/// `[CONSENSUS: APPROVED]` (or the author does), or `max_rounds` is hit. Returns
/// the converged output (the author's final turn — or the original if the very
/// first reviewer already approved) + the debate's token cost.
///
/// Why a transcript loop and not the `Goto`-to-review loop it replaces: each
/// turn reads only the DELTA (the growing transcript), not the whole codebase
/// from scratch every round — the dominant cost of the old loop. Reuses
/// `run_agent_with_timeout`, so tiers / MCP / worktree all behave identically.
#[allow(clippy::too_many_arguments)]
async fn run_multi_agent_debate(
    step: &WorkflowStep,
    cfg: &crate::models::MultiAgentReviewConfig,
    planner_output: &str,
    project_path: &str,
    project_id: Option<&str>,
    work_dir: &str,
    tokens_config: &TokensConfig,
    full_access: bool,
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
    http_endpoints: Option<&crate::models::setup::HttpEndpoints>,
    ollama_context_overrides: Option<&std::collections::HashMap<String, u64>>,
    native_tools: Option<Arc<dyn crate::agents::tools::ToolExecutor>>,
    progress_tx: Option<&ProgressSender>,
    activity: Option<&AgentActivitySink>,
    // The step's own connection. It applies to the AUTHOR, which runs on the
    // step's agent — never to the reviewer, which runs on its own.
    external_http: Option<&runner::ExternalHttpRuntime>,
    author_model: Option<&str>,
    author_connection: Option<&ExternalApiConnection>,
    catalog_db: Option<&crate::db::Database>,
    provenance: &mut WorkflowAgentProvenance,
    retry: u32,
    // See `execute_step`'s `run_id` doc — forwarded unchanged.
    run_id: Option<&str>,
) -> Result<(String, Option<u64>, Vec<NativeToolCallLog>, Option<u32>)> {
    let max_rounds = cfg.max_rounds.unwrap_or(3).clamp(1, 5);
    let approved = |t: &str| {
        t.lines()
            .rev()
            .take(4)
            .any(|l| l.contains("[CONSENSUS: APPROVED]"))
    };

    let mut transcript = format!(
        "=== PLAN / OUTPUT (authored by {:?}) ===\n{}",
        step.agent, planner_output
    );
    let mut converged = planner_output.to_string();
    let mut tokens = Some(0u64);
    let mut native_tool_calls = Vec::new();
    let mut selected_attempt = None;

    // A reviewer turn: a synthetic step running the reviewer agent (different
    // family + its own tier), FreeText, no nested debate / on_result.
    let reviewer_shares_the_step_connection =
        reviewer_shares_step_connection(step, &cfg.reviewer_agent);
    let reviewer_step = {
        let mut s = step.clone();
        s.agent = cfg.reviewer_agent.clone();
        s.multi_agent_review = None;
        s.on_result = vec![];
        s.output_format = crate::models::StepOutputFormat::FreeText;
        s.agent_settings = Some(AgentSettings {
            tools: None,
            model: None,
            tier: cfg.reviewer_tier,
            reasoning_effort: None,
            max_tokens: None,
            connection_id: reviewer_shares_the_step_connection
                .then(|| {
                    step.agent_settings
                        .as_ref()
                        .and_then(|settings| settings.connection_id.clone())
                })
                .flatten(),
        });
        s
    };
    // An author turn: the original step's agent (keep its tier) AND its
    // output_format — so on a TypedSchema/Structured step the author re-emits
    // the FULL envelope (the converged manifest must stay valid for the
    // downstream ingestion). No nested debate / on_result.
    let author_step = {
        let mut s = step.clone();
        s.multi_agent_review = None;
        s.on_result = vec![];
        s
    };
    let reviewer_model = reviewer_shares_the_step_connection
        .then(|| step_model_override(&reviewer_step, author_connection))
        .flatten();
    // The reviewer runs with its own agent's access, never its author's.
    let reviewer_full_access = if cfg.reviewer_agent == step.agent {
        full_access
    } else {
        crate::core::config::saved_full_access(&cfg.reviewer_agent)
    };

    for round in 0..max_rounds {
        // ---- reviewer challenges ----
        let rprompt = format!(
            "{debate}\n\n=== DEBATE TRANSCRIPT ===\n{transcript}\n\n\
             You are the REVIEWER (round {n}/{max}). Read the relevant project files, then challenge the plan/output above on relevance, completeness, correctness and scope. Be concrete and actionable — do NOT rewrite it yourself. If, and ONLY if, you genuinely judge it ready, end your reply with a line containing exactly [CONSENSUS: APPROVED].",
            debate = cfg.debate_prompt, transcript = transcript, n = round + 1, max = max_rounds
        );
        let reviewer_connection = reviewer_shares_the_step_connection
            .then_some(author_connection)
            .flatten();
        let reviewer_preflight = preflight_workflow_launch(
            catalog_db,
            work_dir,
            &reviewer_step,
            reviewer_connection,
            reviewer_model.as_deref(),
            model_tiers,
        )
        .await?;
        let rev = run_agent_with_timeout(
            &reviewer_step,
            project_path,
            project_id,
            work_dir,
            &rprompt,
            &[],
            tokens_config,
            reviewer_full_access,
            model_tiers,
            http_endpoints,
            ollama_context_overrides,
            native_tools.clone(),
            progress_tx,
            activity,
            // Only when it is the same agent: a reviewer on a DIFFERENT agent
            // has no connection of its own to name yet, and handing it this
            // one would point it at an endpoint that does not serve it.
            reviewer_shares_the_step_connection
                .then_some(external_http)
                .flatten(),
            reviewer_preflight.requested_model.as_deref(),
            reviewer_preflight.effective_model.as_deref(),
            reviewer_preflight.warning.as_ref(),
            provenance,
            WorkflowAgentAttemptRole::Review,
            retry,
            None,
            run_id,
        )
        .await?;
        tokens = add_tokens(tokens, rev.tokens_used);
        native_tool_calls.extend(rev.native_tool_calls);
        transcript.push_str(&format!(
            "\n\n=== REVIEWER ({:?}, round {}) ===\n{}",
            cfg.reviewer_agent,
            round + 1,
            rev.text
        ));
        if approved(&rev.text) {
            tracing::info!(
                "multi_agent_review: reviewer approved at round {}",
                round + 1
            );
            break;
        }

        // ---- author addresses the critique + re-emits the full output ----
        // When the step is Structured/TypedSchema, append the SAME envelope
        // instruction `build_step_prompt` would (we bypass it here by passing
        // a raw prompt), so the author re-emits a valid envelope — otherwise
        // the envelope-safety guard would discard the refinement.
        let envelope_addendum = match &step.output_format {
            crate::models::StepOutputFormat::Structured => {
                crate::workflows::template::STRUCTURED_OUTPUT_INSTRUCTIONS.to_string()
            }
            crate::models::StepOutputFormat::TypedSchema { schema, .. } => {
                crate::workflows::template::build_typed_schema_instruction(schema)
            }
            crate::models::StepOutputFormat::FreeText => String::new(),
        };
        let aprompt = format!(
            "=== DEBATE TRANSCRIPT ===\n{transcript}\n\n\
             You are the PLAN AUTHOR ({author:?}). Address the reviewer's critique above: revise your plan/output accordingly. Re-emit your COMPLETE updated output in the SAME format you used originally. If you have addressed everything and now agree the result is ready, additionally end with a line containing exactly [CONSENSUS: APPROVED].{addendum}",
            transcript = transcript, author = step.agent, addendum = envelope_addendum
        );
        let author_preflight = preflight_workflow_launch(
            catalog_db,
            work_dir,
            &author_step,
            author_connection,
            author_model,
            model_tiers,
        )
        .await?;
        let auth = run_agent_with_timeout(
            &author_step,
            project_path,
            project_id,
            work_dir,
            &aprompt,
            &[],
            tokens_config,
            full_access,
            model_tiers,
            http_endpoints,
            ollama_context_overrides,
            native_tools.clone(),
            progress_tx,
            activity,
            external_http,
            author_preflight.requested_model.as_deref(),
            author_preflight.effective_model.as_deref(),
            author_preflight.warning.as_ref(),
            provenance,
            WorkflowAgentAttemptRole::Author,
            retry,
            None,
            run_id,
        )
        .await?;
        tokens = add_tokens(tokens, auth.tokens_used);
        native_tool_calls.extend(auth.native_tool_calls);
        transcript.push_str(&format!(
            "\n\n=== AUTHOR ({:?}, round {}) ===\n{}",
            step.agent,
            round + 1,
            auth.text
        ));
        converged = auth.text.clone();
        selected_attempt = Some(auth.attempt_id);
        if approved(&auth.text) {
            tracing::info!(
                "multi_agent_review: author+reviewer converged at round {}",
                round + 1
            );
            break;
        }
    }
    Ok((converged, tokens, native_tool_calls, selected_attempt))
}

/// Evaluate on_result conditions against the step output.
/// Only checks the last 5 lines for `[SIGNAL: keyword]` to avoid false positives
/// from the agent quoting instruction text in its response.
///
/// `pub(crate)` so non-Agent step types (Exec, ApiCall) can also branch on
/// signals they emit themselves (e.g. `[SIGNAL: ERROR]` on cargo test exit≠0).
///
/// 2026-06-10 audit P2 — a signal must be at the **end of a line**
/// (`line.trim().ends_with("[SIGNAL: X]")`), not anywhere as a substring.
/// `format_step_output` emits each signal alone on its line, and presets
/// instruct agents to end their response with the `[SIGNAL: …]` line — both
/// satisfy `ends_with`, including a content-then-signal line ("Done.
/// [SIGNAL: OK]"). But an API body excerpt or an instruction recap that
/// *quotes* `[SIGNAL: ERROR]` in the MIDDLE of a sentence in the last 5
/// lines no longer triggers a false Stop/Goto. (`contains` did; strict
/// equality would have regressed content-then-signal lines.)
pub(crate) fn evaluate_conditions(
    rules: &[StepConditionRule],
    output: &str,
) -> Option<ConditionAction> {
    // Look at the last 5 lines for a signal
    let tail: Vec<&str> = output.lines().rev().take(5).collect();
    for rule in rules {
        // Skip empty conditions — they would match everything
        if rule.contains.is_empty() {
            continue;
        }
        let signal = format!("[SIGNAL: {}]", rule.contains);
        if tail.iter().any(|line| line.trim().ends_with(&signal)) {
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
        format!(
            " (+{} autre{})",
            rest_count,
            if rest_count > 1 { "s" } else { "" }
        )
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
            tokens_used: Some(0),
            duration_ms: elapsed_ms,
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: Some(Box::default()),
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
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

    #[test]
    fn a_step_loads_its_repository_skills_from_the_run_pin_and_names_a_missing_one() {
        let run = "kt1128-step-skills-run";
        let pinned = crate::models::Skill {
            id: "repository:p1:block-migration".into(),
            name: "Block migration".into(),
            description: String::new(),
            icon: "📂".into(),
            category: crate::models::SkillCategory::Domain,
            content: "Committed on main.".into(),
            is_builtin: false,
            token_estimate: 4,
            license: None,
            allowed_tools: None,
            auto_triggers: None,
            external: false,
            source_url: None,
            arguments: vec![],
            argument_hint: None,
            variables: vec![],
            project_id: None,
        };
        crate::core::skills::pin_skill_snapshot(run, &pinned.id, pinned.clone());
        let ids = |list: &[&str]| list.iter().map(|id| id.to_string()).collect::<Vec<_>>();

        let (catalog, repository) = step_skills(
            &ids(&["rust", "repository:p1:block-migration"]),
            Some("p1"),
            Some(run),
        )
        .unwrap();
        assert_eq!(catalog, ids(&["rust"]));
        assert_eq!(repository.len(), 1);
        assert_eq!(repository[0].content, "Committed on main.");

        for (skill_ids, project, run_id, reason) in [
            (
                ids(&["repository:p2:block-migration"]),
                Some("p1"),
                Some(run),
                "another project",
            ),
            (
                ids(&["repository:p1:other"]),
                Some("p1"),
                Some(run),
                "not readable on the default branch",
            ),
            (
                ids(&["repository:p1:block-migration"]),
                Some("p1"),
                None,
                "this launch is not a run",
            ),
        ] {
            let error = step_skills(&skill_ids, project, run_id)
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{error}");
            assert!(error.starts_with("Repository skill `"), "{error}");
        }
        crate::core::skills::release_skills_snapshot(run);
    }

    fn rule(contains: &str, action: ConditionAction) -> StepConditionRule {
        StepConditionRule {
            contains: contains.to_string(),
            action,
        }
    }

    // ─── #8 — stall detection + on_timeout routing ────────────────────────
    #[test]
    fn is_stall_error_matches_the_watchdog_bail() {
        // Must stay in sync with drive_agent_to_output's bail! message.
        assert!(is_stall_error("Agent stalled (no output for 300s)"));
        assert!(!is_stall_error("Agent exited with exit code 1: boom"));
        assert!(!is_stall_error("schema validation failed: missing field"));
    }

    #[test]
    fn timeout_routing_routes_a_stall_when_on_timeout_is_set() {
        let goto = Some(ConditionAction::Goto {
            step_name: "note_failed".into(),
            max_iterations: None,
        });
        let (output, cond, action) =
            timeout_routing(true, &goto, 1, "Agent stalled (no output for 900s)");
        assert!(
            matches!(action, Some(ConditionAction::Goto { ref step_name, .. }) if step_name == "note_failed")
        );
        assert_eq!(cond.as_deref(), Some("on_timeout"));
        assert!(output.contains("Routed via on_timeout"), "output: {output}");
    }

    #[test]
    fn timeout_routing_supports_stop_action() {
        let (_, cond, action) = timeout_routing(
            true,
            &Some(ConditionAction::Stop),
            2,
            "Agent stalled (no output for 60s)",
        );
        assert!(matches!(action, Some(ConditionAction::Stop)));
        assert_eq!(cond.as_deref(), Some("on_timeout"));
    }

    #[test]
    fn timeout_routing_fails_normally_without_on_timeout() {
        // A stall but no on_timeout declared → plain failure (rollback path).
        let (output, cond, action) =
            timeout_routing(true, &None, 3, "Agent stalled (no output for 300s)");
        assert!(action.is_none());
        assert!(cond.is_none());
        assert!(
            output.contains("Failed after 3 attempts"),
            "output: {output}"
        );
    }

    #[test]
    fn timeout_routing_ignores_on_timeout_for_non_stall_failures() {
        // A crash (not a stall) must NOT consume on_timeout — it fails normally
        // so the real error surfaces / rollback fires.
        let goto = Some(ConditionAction::Goto {
            step_name: "x".into(),
            max_iterations: None,
        });
        let (output, cond, action) =
            timeout_routing(false, &goto, 1, "Agent exited with exit code 1");
        assert!(action.is_none());
        assert!(cond.is_none());
        assert!(output.contains("Failed after"), "output: {output}");
    }

    #[test]
    fn ollama_envelope_format_wraps_typed_schema_data() {
        use crate::models::{OnInvalid, StepOutputFormat};
        let data_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "score": { "type": "integer" },
                "note": { "type": "string" }
            },
            "required": ["score"]
        });
        let of = StepOutputFormat::TypedSchema {
            schema: data_schema.clone(),
            on_invalid: OnInvalid::Continue,
        };
        let wrapped = ollama_envelope_format(&of).expect("TypedSchema → envelope schema");
        assert_eq!(
            wrapped,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "data": data_schema,
                    "status": { "type": "string" },
                    "summary": { "type": "string" }
                },
                "required": ["data", "status"]
            })
        );

        let openai =
            crate::agents::chat_codec::build_openai_chat_body("m", "", "hi", Some(&wrapped), false);
        assert_eq!(openai["response_format"]["json_schema"]["strict"], false);
        assert_eq!(openai["response_format"]["json_schema"]["schema"], wrapped);

        let ollama = runner::build_ollama_chat_body("m", "", "hi", Some(&wrapped), 8192, None);
        assert_eq!(ollama["format"], wrapped);
        assert_eq!(ollama["stream"], false);
        assert!(ollama.get("response_format").is_none());
    }

    #[test]
    fn ollama_envelope_format_none_for_freetext_and_structured() {
        use crate::models::StepOutputFormat;
        assert!(ollama_envelope_format(&StepOutputFormat::FreeText).is_none());
        assert!(ollama_envelope_format(&StepOutputFormat::Structured).is_none());
    }

    #[test]
    fn escalation_step_forces_claude_reasoning_and_clears_local_model() {
        let mut local = make_step("summarize {{x}}");
        local.agent = crate::models::AgentType::Ollama;
        local.agent_settings = Some(crate::models::AgentSettings {
            tools: None,
            model: Some("qwen3:8b".into()),
            tier: Some(crate::models::ModelTier::Default),
            reasoning_effort: None,
            max_tokens: None,
            connection_id: None,
        });
        let esc = escalation_step(&local);
        assert_eq!(
            esc.agent,
            crate::models::AgentType::ClaudeCode,
            "escalate to Claude"
        );
        let s = esc.agent_settings.expect("settings present");
        assert_eq!(
            s.model, None,
            "pinned local model cleared → tier resolution"
        );
        assert_eq!(s.tier, Some(crate::models::ModelTier::Reasoning));
        assert_eq!(esc.prompt_template, "summarize {{x}}", "task preserved");
    }

    // ─── Catalogue near-miss refusal reaches the workflow step launch path ────

    #[tokio::test]
    #[serial_test::serial(model_catalog_reasoning_modes)]
    async fn workflow_step_reasoning_effort_override_names_the_catalogue_spelling_on_launch() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let target = crate::db::model_catalog::agent_runtime_target_id(&AgentType::ClaudeCode);
        db.with_conn({
            let target = target.clone();
            move |conn| {
                crate::db::model_catalog::create_manual(
                    conn,
                    &crate::models::UpsertManualModelRequest {
                        runtime_target_id: target,
                        agent_type: AgentType::ClaudeCode,
                        model_id: "manual-model".into(),
                        display_name: "Manual model".into(),
                        capabilities: vec!["chat".into()],
                        reasoning_modes: vec!["high".into()],
                        default_reasoning_mode: Some("high".into()),
                        tier_assignment: None,
                        cost_hint: None,
                        privacy_note: None,
                    },
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();
        crate::core::model_catalog::refresh_runtime_cache(&db)
            .await
            .unwrap();

        let step = WorkflowStep {
            agent: AgentType::ClaudeCode,
            agent_settings: Some(AgentSettings {
                model: Some("manual-model".into()),
                tier: Some(ModelTier::Default),
                reasoning_effort: Some("High".into()),
                max_tokens: None,
                connection_id: None,
                tools: None,
            }),
            ..WorkflowStep::default()
        };
        let mut provenance = WorkflowAgentProvenance::default();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy().to_string();
        let tokens = TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let error = run_agent_with_timeout(
            &step,
            &project,
            None,
            &project,
            "does it matter",
            &[],
            &tokens,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("manual-model"),
            None,
            &mut provenance,
            WorkflowAgentAttemptRole::Initial,
            0,
            None,
            None,
        )
        .await
        .expect_err("a case-mismatched override must still be refused, never silently applied");
        assert!(
            error
                .to_string()
                .contains("the catalogue lists 'high', not 'High'"),
            "the refusal must name the actual mismatch, not the generic catalogue-absence reason: {error}"
        );
    }

    #[test]
    fn workflow_preflight_uses_the_selected_connections_model() {
        fn connection(id: &str, model: &str) -> ExternalApiConnection {
            let now = chrono::Utc::now();
            ExternalApiConnection {
                id: id.into(),
                display_name: id.into(),
                mention_alias: id.into(),
                endpoint: Some(format!("http://{id}.test")),
                credential_slug: format!("credential-{id}"),
                origin_preset: ExternalApiConnectionPreset::Other,
                economy_model: None,
                default_model: Some(model.into()),
                reasoning_model: None,
                created_at: now,
                updated_at: now,
                image_model: None,
                video_model: None,
                media_endpoint: None,
            }
        }

        let mut step = make_step("anything");
        step.agent = AgentType::Custom;
        step.agent_settings = Some(AgentSettings {
            tools: None,
            model: None,
            tier: Some(ModelTier::Default),
            connection_id: Some("connection-b".into()),
            reasoning_effort: None,
            max_tokens: None,
        });
        let connection_a = connection("connection-a", "model-a");
        let connection_b = connection("connection-b", "model-b");

        assert_eq!(
            step_model_override(&step, Some(&connection_a)).as_deref(),
            Some("model-a")
        );
        assert_eq!(
            step_model_override(&step, Some(&connection_b)).as_deref(),
            Some("model-b")
        );
        assert_eq!(
            crate::db::model_catalog::http_runtime_target_id(&connection_b.id),
            "http:connection-b",
            "the selected same-agent connection keeps its own catalog identity"
        );
    }

    fn saved_connection(id: &str, model: &str) -> ExternalApiConnection {
        let now = chrono::Utc::now();
        ExternalApiConnection {
            id: id.into(),
            display_name: id.into(),
            mention_alias: id.into(),
            endpoint: Some(format!("http://{id}.test")),
            credential_slug: format!("credential-{id}"),
            origin_preset: ExternalApiConnectionPreset::Other,
            economy_model: None,
            default_model: Some(model.into()),
            reasoning_model: None,
            created_at: now,
            updated_at: now,
            image_model: None,
            video_model: None,
            media_endpoint: None,
        }
    }

    #[tokio::test]
    async fn named_step_connection_resolves_the_exact_saved_target() {
        let db = crate::db::Database::open_in_memory().unwrap();
        for connection in [
            saved_connection("connection-a", "model-a"),
            saved_connection("connection-b", "model-b"),
        ] {
            db.with_conn(move |conn| {
                crate::db::external_api_connections::insert(conn, &connection)
            })
            .await
            .unwrap();
        }
        let mut step = make_step("anything");
        step.agent = AgentType::Custom;
        step.agent_settings = Some(AgentSettings {
            tools: None,
            model: None,
            tier: Some(ModelTier::Default),
            connection_id: Some("connection-b".into()),
            reasoning_effort: None,
            max_tokens: None,
        });

        let resolved = resolve_step_connection(&step, Some(&db))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.id, "connection-b");
        assert_eq!(
            step_model_override(&step, Some(&resolved)).as_deref(),
            Some("model-b")
        );
    }

    #[tokio::test]
    async fn deleted_named_step_connection_is_an_error_not_default_fallback() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let mut step = make_step("anything");
        step.agent = AgentType::Custom;
        step.agent_settings = Some(AgentSettings {
            tools: None,
            model: None,
            tier: Some(ModelTier::Default),
            connection_id: Some("deleted-connection".into()),
            reasoning_effort: None,
            max_tokens: None,
        });

        let error = resolve_step_connection(&step, Some(&db)).await.unwrap_err();
        assert!(error.to_string().contains("deleted-connection"), "{error}");
    }

    #[test]
    fn explicit_workflow_model_remains_the_dispatch_and_preflight_model() {
        let mut step = make_step("anything");
        step.agent_settings = Some(AgentSettings {
            tools: None,
            model: Some("expert-model".into()),
            tier: Some(ModelTier::Default),
            connection_id: Some("connection-b".into()),
            reasoning_effort: None,
            max_tokens: None,
        });
        assert_eq!(
            step_model_override(&step, None).as_deref(),
            Some("expert-model")
        );
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
        let rules = vec![rule(
            "GO_NEXT",
            ConditionAction::Goto {
                step_name: "step_b".to_string(),
                max_iterations: None,
            },
        )];
        let output = "Some output\n[SIGNAL: GO_NEXT]";
        let action = evaluate_conditions(&rules, output);
        assert!(
            matches!(action, Some(ConditionAction::Goto { step_name, .. }) if step_name == "step_b")
        );
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
    // unit-test the behavior independently from the agent spawn. KT-926: the
    // step prompt is the rendered template plus the addenda below and nothing
    // about the user's other Kronn projects.

    #[test]
    fn a_reviewer_on_the_same_agent_speaks_to_the_same_endpoint() {
        // The author reached the step's named connection while the reviewer
        // silently fell back to the provider default — same step, two
        // endpoints, nothing on screen to say so.
        let step = make_step("anything");
        assert_eq!(step.agent, crate::models::AgentType::ClaudeCode);
        assert!(reviewer_shares_step_connection(
            &step,
            &crate::models::AgentType::ClaudeCode
        ));
    }

    #[test]
    fn a_reviewer_on_another_agent_never_inherits_the_connection() {
        // That endpoint does not serve it; a reviewer pointed at a connection
        // of its own is a separate, still-missing piece of configuration.
        let step = make_step("anything");
        assert!(!reviewer_shares_step_connection(
            &step,
            &crate::models::AgentType::Codex
        ));
    }

    fn make_step(prompt_template: &str) -> WorkflowStep {
        // Mirror of `workflows::big_ticket_template::blank_step` —
        // duplicated here because it's `fn` private in the sibling module
        // and we want this test file standalone.
        WorkflowStep {
            id: None,
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
            on_timeout: None,
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
            api_response: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            quick_prompt_variables: std::collections::HashMap::new(),
            json_data_payload: None,
            collect_api_data: None,
            transform_data: None,
            page_publish: None,
            task_board: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
            room_id: None,
            read_only_repos: vec![],
            delegate_subtasks: None,
            exec_script_files: vec![],
            exec_unmodelled_args_approved: None,
            exec_agent_written: None,
            exec_agent_lines: vec![],
            sub_workflow_variables: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn build_step_prompt_returns_the_rendered_template_and_nothing_else() {
        // A free-text step is just its rendered template: no synthetic
        // "## Linked repositories" / "## Other Kronn projects" header.
        let step = make_step("Hello world");
        let ctx = TemplateContext::new();
        let prompt = build_step_prompt(&step, &ctx).expect("must render");
        assert_eq!(
            prompt, "Hello world",
            "the prompt must not carry anything the template did not say"
        );
    }

    #[test]
    fn build_step_prompt_rejects_a_typo_before_an_agent_can_start() {
        let step = make_step("Review {{issue.titel}}");
        let mut ctx = TemplateContext::new();
        ctx.set_issue("Correct title", "Body", "7", "https://example/7", &[]);

        let error = build_step_prompt(&step, &ctx)
            .expect_err("strict runtime rendering must stop before agent startup");

        assert!(error.contains("issue.titel"));
        assert!(error.contains("Unknown workflow template variable"));
    }

    #[test]
    fn build_step_prompt_states_declared_read_only_access() {
        let mut step = make_step("Review the ticket");
        step.read_only_repos = vec!["/repos/API équipe".into()];
        let prompt = build_step_prompt(&step, &TemplateContext::new()).unwrap();
        assert!(prompt.starts_with("Review the ticket"));
        assert!(prompt.contains("\"/repos/API équipe\""));
        assert!(prompt.contains("Never edit them"));
        assert!(prompt.contains("not a filesystem permission grant"));
    }

    #[test]
    fn build_step_prompt_keeps_addenda_anchored_at_the_end() {
        // When the step has a TypedSchema output_format AND a triage
        // description, both addenda must trail the prompt. LLMs follow
        // trailing instructions more reliably than leading ones, so the order
        // is load-bearing: template → output_format addendum → triage
        // addendum.
        let mut step = make_step("Triage this ticket");
        step.description = Some("[TRIAGE] feasibility audit".into());
        step.output_format = crate::workflows::triage::triage_output_format();
        let ctx = TemplateContext::new();
        let prompt = build_step_prompt(&step, &ctx).expect("must render");
        let template_idx = prompt.find("Triage this ticket").unwrap();
        let triage_idx = prompt
            .find("TRIAGE MODE")
            .expect("triage addendum must be appended");
        assert_eq!(template_idx, 0, "the rendered template leads the prompt");
        assert!(
            template_idx < triage_idx,
            "order must be: template → triage addendum; got idx {template_idx}/{triage_idx}"
        );
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
        let outcome = fail_fast_on_unresolved("analyze", "Use {{steps.main.data}} to proceed.", 5)
            .expect("Must return a failed outcome");
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert_eq!(outcome.result.step_name, "analyze");
        assert!(
            outcome.result.output.contains("steps.main.data"),
            "Error message must name the offending placeholder"
        );
        assert!(
            outcome.result.output.contains("Structured"),
            "Error message must hint at the fix (output_format: Structured)"
        );
        assert_eq!(
            outcome.result.tokens_used,
            Some(0),
            "No tokens spent on failed fail-fast"
        );
    }

    #[test]
    fn fail_fast_on_previous_step_summary() {
        let outcome =
            fail_fast_on_unresolved("step2", "Previous summary: {{previous_step.summary}}", 0)
                .expect("Must return a failed outcome");
        assert!(outcome.result.output.contains("previous_step.summary"));
    }

    #[test]
    fn fail_fast_message_mentions_additional_placeholders() {
        let outcome = fail_fast_on_unresolved(
            "s",
            "{{steps.a.data}} and {{steps.b.summary}} and {{previous_step.status}}",
            0,
        )
        .expect("Must fail-fast");
        // Names the first + flags the count of extras so the user knows it's
        // not a one-off; exact wording is asserted to catch regressions.
        assert!(
            outcome.result.output.contains("+2 autres"),
            "Got: {}",
            outcome.result.output
        );
    }

    #[test]
    fn fail_fast_single_extra_uses_singular() {
        let outcome = fail_fast_on_unresolved("s", "{{steps.a.data}} {{steps.b.summary}}", 0)
            .expect("Must fail-fast");
        assert!(
            outcome.result.output.contains("+1 autre"),
            "Expected singular 'autre', got: {}",
            outcome.result.output
        );
        assert!(
            !outcome.result.output.contains("+1 autres"),
            "Must not pluralize when only 1 extra"
        );
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

#[cfg(test)]
mod drive_agent_to_output_tests {
    //! Unit tests for the workflow Agent-step consumption loop, driven by a
    //! scripted `AgentIo` (0.8.8 test-seam). Pins text/token accumulation,
    //! tool-call progress breadcrumbs, raw-vs-stream-json handling, and the
    //! non-zero-exit error path — without spawning a CLI or burning tokens.
    use super::format_silent_exit_error;
    use super::{drive_agent_to_output, native_tool_calls_from_stderr};

    /// An ACP run's own watchdog owns inactivity: a step waits through a long
    /// text silence (tool work) instead of killing it, and a plain CLI is
    /// still killed on the same silence.
    #[tokio::test(start_paused = true)]
    async fn a_step_leaves_inactivity_to_an_acp_run_s_own_watchdog() {
        use crate::agents::runner::ScriptedProcess;
        let quiet = std::time::Duration::from_secs(5);
        let stall = std::time::Duration::from_secs(1);
        let watched = ScriptedProcess::raw(["built"])
            .with_first_line_after(quiet)
            .with_activity_watched();
        let output =
            drive_agent_to_output(watched, None, None, stall, &AgentType::OpenCode, "build")
                .await
                .expect("not killed on text silence");
        assert_eq!(output.text, "built");

        let plain = ScriptedProcess::raw(["built"]).with_first_line_after(quiet);
        let error = drive_agent_to_output(plain, None, None, stall, &AgentType::Codex, "build")
            .await
            .expect_err("a text-only run is still timed by its text");
        assert!(error
            .to_string()
            .contains("Agent stalled (no output for 1s)"));
    }
    use crate::agents::runner::ScriptedProcess;
    use crate::models::AgentType;
    use std::time::Duration;

    // ── format_silent_exit_error (2026-06-10, the silent `implement` crash) ──

    #[test]
    fn silent_exit_signal_kill_names_oom_and_atomicity() {
        let msg = format_silent_exit_error("exit code None", true, "");
        assert!(msg.contains("SIGNAL"), "signal kill must be named: {msg}");
        assert!(msg.contains("OOM"), "OOM hint expected: {msg}");
        assert!(
            msg.contains("atomic"),
            "atomicity remediation expected: {msg}"
        );
    }

    #[test]
    fn silent_exit_surfaces_stdout_tail_when_stderr_empty() {
        let stdout = (1..=30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let msg = format_silent_exit_error("exit code Some(1)", false, &stdout);
        // Tail (last 15 lines) is surfaced; an early line is dropped.
        assert!(
            msg.contains("line 30"),
            "must surface the stdout tail: {msg}"
        );
        assert!(
            msg.contains("last stdout"),
            "must label the stdout block: {msg}"
        );
        assert!(
            !msg.contains("line 1\n"),
            "old lines beyond the 15-tail are trimmed: {msg}"
        );
    }

    #[test]
    fn silent_exit_no_output_at_all_is_explicit() {
        let msg = format_silent_exit_error("exit code Some(1)", false, "   \n  ");
        assert!(
            msg.contains("NO output at all"),
            "blank stdout must be called out: {msg}"
        );
    }

    fn text_delta(s: &str) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":{}}}}}}}"#,
            serde_json::to_string(s).unwrap()
        )
    }
    fn usage(input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"type":"message_delta","usage":{{"input_tokens":{},"output_tokens":{}}}}}}}"#,
            input, output
        )
    }
    fn tool_start(name: &str) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_start","content_block":{{"type":"tool_use","name":"{}"}}}}}}"#,
            name
        )
    }

    const LONG: Duration = Duration::from_secs(3600);

    #[tokio::test]
    async fn structured_output_notice_survives_collection_and_replaced_answer() {
        let notice = "[structured-output fallback: Ollama rejected constrained JSON]";
        let process = ScriptedProcess::raw(["initial invalid answer"]).with_stderr([notice]);
        let collected =
            drive_agent_to_output(process, None, None, LONG, &AgentType::Ollama, "advise")
                .await
                .expect("successful collection");
        assert_eq!(collected.runtime_notices, [notice]);
        let repaired = r#"{"data":{"ok":true},"status":"OK"}"#;
        let recorded = super::with_runtime_notices(repaired.into(), &collected.runtime_notices);
        assert!(recorded.starts_with(notice));
        assert!(crate::workflows::template::extract_step_envelope(&recorded).is_some());
        assert_eq!(
            super::with_runtime_notices(recorded.clone(), &collected.runtime_notices),
            recorded,
            "the initial streamed notice must not be duplicated"
        );
        let duplicate = vec![notice.to_string(), notice.to_string()];
        assert_eq!(
            super::with_runtime_notices(repaired.into(), &duplicate),
            recorded
        );
    }

    #[test]
    fn native_tool_history_keeps_name_and_status_but_drops_arguments() {
        let lines = vec![
            r#"[kronn-internal: api_call({"token":"must-not-persist","path":"/usage"}) → ok]"#
                .to_string(),
            "[kronn-internal: task_get({\"task_id\":\"KT-189\"}) → error]".to_string(),
            "LiteLLM request completed".to_string(),
        ];
        let calls = native_tool_calls_from_stderr(&lines);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "api_call");
        assert!(calls[0].ok);
        assert_eq!(calls[1].name, "task_get");
        assert!(!calls[1].ok);
        let durable = serde_json::to_string(&calls).expect("serialize safe log");
        assert!(!durable.contains("must-not-persist"));
        assert!(!durable.contains("KT-189"));
    }

    #[tokio::test]
    async fn stream_json_collects_text_and_tokens() {
        let proc = ScriptedProcess::stream_json([
            text_delta("Hello "),
            usage(100, 50),
            text_delta("world"),
        ]);
        let out = drive_agent_to_output(proc, None, None, LONG, &AgentType::ClaudeCode, "step1")
            .await
            .expect("clean exit");
        assert_eq!(out.text, "Hello world");
        assert_eq!(
            out.tokens_used,
            Some(150),
            "tokens come from the stream-json Usage event"
        );
    }

    /// KT-795 — the direct `--print` route: the cache counts of the final
    /// `result` and the latest tool call reach the step, `tokens_used` unchanged.
    #[tokio::test]
    async fn stream_json_reports_cache_usage_and_the_latest_tool_call() {
        let proc = ScriptedProcess::stream_json([
            tool_start("Grep"),
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"pattern\":\"StepProgress\",\"path\":\"src\"}"}}}"#.to_string(),
            r#"{"type":"stream_event","event":{"type":"content_block_stop"}}"#.to_string(),
            text_delta("done"),
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":48,"cache_creation_input_tokens":80271,"cache_read_input_tokens":1554330,"output_tokens":21545}}"#.to_string(),
        ]);
        let (activity, activity_rx) = tokio::sync::watch::channel(None);
        let out = drive_agent_to_output(
            proc,
            None,
            Some(&activity),
            LONG,
            &AgentType::ClaudeCode,
            "orchestrateur",
        )
        .await
        .expect("clean exit");
        assert_eq!(out.tokens_used, Some(48 + 21_545));
        assert_eq!(
            out.prompt_cache,
            crate::agents::runner::PromptCacheUsage {
                cached_prompt_tokens: Some(1_554_330),
                cache_write_prompt_tokens: Some(80_271),
            }
        );
        let latest = activity_rx.borrow().clone().expect("tool call recorded");
        assert_eq!(latest.category, crate::models::ActivityCategory::Search);
    }

    #[tokio::test]
    async fn an_acp_process_hands_its_reported_cache_usage_to_the_step() {
        let cache = crate::agents::runner::PromptCacheUsage {
            cached_prompt_tokens: Some(7),
            cache_write_prompt_tokens: None,
        };
        let proc = ScriptedProcess::raw(["answer"])
            .with_reported_usage(12)
            .with_reported_prompt_cache(cache);
        let out = drive_agent_to_output(proc, None, None, LONG, &AgentType::ClaudeCode, "acp")
            .await
            .expect("clean exit");
        assert_eq!((out.tokens_used, out.prompt_cache), (Some(12), cache));
    }

    #[tokio::test]
    async fn raw_mode_joins_lines() {
        let proc = ScriptedProcess::raw(["first", "second"]);
        let out = drive_agent_to_output(proc, None, None, LONG, &AgentType::Vibe, "step-raw")
            .await
            .expect("clean exit");
        assert!(out.text.contains("first"));
        assert!(out.text.contains("second"));
    }

    #[tokio::test]
    async fn progress_tx_receives_text_and_tool_breadcrumbs() {
        // A ToolStart must surface a `🔧 <name>` breadcrumb on progress_tx so
        // the workflow run view shows a sign of life during tool loops.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);
        let proc = ScriptedProcess::stream_json([text_delta("thinking"), tool_start("Edit")]);
        let out = drive_agent_to_output(proc, Some(&tx), None, LONG, &AgentType::ClaudeCode, "s")
            .await
            .expect("clean exit");
        drop(tx);
        assert_eq!(out.text, "thinking");
        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        assert!(
            msgs.iter().any(|m| m == "thinking"),
            "text streamed to progress_tx"
        );
        assert!(
            msgs.iter().any(|m| m.contains("🔧") && m.contains("Edit")),
            "tool-start breadcrumb streamed: {msgs:?}"
        );
    }

    #[tokio::test]
    async fn failed_exit_bails_with_stderr_tail() {
        let proc = ScriptedProcess::stream_json(Vec::<String>::new())
            .with_exit(false, Some(1))
            .with_stderr(["Error: rate limit reached", "retry later"]);
        let err = drive_agent_to_output(proc, None, None, LONG, &AgentType::ClaudeCode, "boom")
            .await
            .expect_err("non-zero exit must bail");
        let msg = err.to_string();
        assert!(msg.contains("Agent exited"), "got: {msg}");
        assert!(
            msg.contains("rate limit reached"),
            "stderr tail should surface: {msg}"
        );
    }

    #[tokio::test]
    async fn failed_fable_result_bails_with_structured_stdout_cause() {
        let fable_429 = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"You've hit your org's monthly spend limit · run /usage-credits to manage your plan.","api_error_status":429,"terminal_reason":"api_error","cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0}}"#;
        let proc = ScriptedProcess::stream_json([fable_429]).with_exit(false, Some(1));
        let err = drive_agent_to_output(
            proc,
            None,
            None,
            LONG,
            &AgentType::ClaudeCode,
            "fable-quota",
        )
        .await
        .expect_err("structured provider error must fail the step");
        let msg = err.to_string();
        assert!(msg.contains("monthly spend limit"), "got: {msg}");
        assert!(msg.contains("HTTP 429"), "got: {msg}");
        assert!(!msg.contains("NO output at all"), "got: {msg}");
    }

    #[tokio::test]
    async fn failed_exit_no_stderr_gives_actionable_message() {
        let proc = ScriptedProcess::stream_json(Vec::<String>::new()).with_exit(false, Some(137)); // SIGKILL-ish, no stderr
        let err = drive_agent_to_output(proc, None, None, LONG, &AgentType::ClaudeCode, "killed")
            .await
            .expect_err("non-zero exit must bail");
        let msg = err.to_string();
        assert!(
            msg.contains("no stderr"),
            "should hint at signal/sandbox: {msg}"
        );
    }

    #[tokio::test]
    async fn clean_exit_empty_output_is_ok() {
        let proc = ScriptedProcess::stream_json(Vec::<String>::new());
        let out = drive_agent_to_output(proc, None, None, LONG, &AgentType::ClaudeCode, "empty")
            .await
            .expect("clean empty exit is ok");
        assert_eq!(out.text, "");
        assert_eq!(
            out.tokens_used, None,
            "no reported usage is unknown, not zero"
        );
    }

    #[tokio::test]
    async fn acp_reported_usage_reaches_the_step_output() {
        let proc = ScriptedProcess::raw(["orchestrated"]).with_reported_usage(48_213);
        let out = drive_agent_to_output(
            proc,
            None,
            None,
            LONG,
            &AgentType::ClaudeCode,
            "orchestrate",
        )
        .await
        .expect("clean exit");
        assert_eq!(out.text, "orchestrated");
        assert_eq!(out.tokens_used, Some(48_213));
    }

    #[tokio::test]
    async fn acp_run_without_reported_usage_is_unknown_not_zero() {
        let proc = ScriptedProcess::raw(["orchestrated"]);
        let out = drive_agent_to_output(
            proc,
            None,
            None,
            LONG,
            &AgentType::ClaudeCode,
            "orchestrate",
        )
        .await
        .expect("clean exit");
        assert_eq!(out.tokens_used, None);
    }

    #[test]
    fn a_step_total_with_an_unmeasured_run_is_unknown() {
        assert_eq!(super::add_tokens(Some(10), Some(5)), Some(15));
        assert_eq!(super::add_tokens(Some(10), None), None);
        assert_eq!(super::add_tokens(None, Some(5)), None);
    }
}

#[cfg(test)]
#[path = "steps_provenance_test.rs"]
mod provenance_tests;

#[cfg(test)]
#[path = "steps_attempt_session_test.rs"]
mod attempt_session_tests;

#[cfg(test)]
mod http_native_tool_step_tests {
    use super::*;
    use crate::agents::tools::{ToolCall, ToolExecutor, ToolOutcome};
    use serde_json::json;
    use serial_test::serial;
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct ReadOnlyTools {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for ReadOnlyTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            vec![json!({
                "type": "function",
                "function": {
                    "name": "task_list",
                    "description": "List workflow project tasks.",
                    "parameters": { "type": "object", "properties": {}, "required": [] }
                }
            })]
        }

        async fn execute(&self, call: &ToolCall) -> ToolOutcome {
            self.calls.lock().unwrap().push(call.name.clone());
            ToolOutcome {
                call: call.clone(),
                content: json!({ "items": [{ "reference": "KT-189", "title": "Native tools" }] }),
                ok: true,
            }
        }
    }

    fn sse(frames: &[&str]) -> String {
        frames
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n"
    }

    fn named_connection(id: &str, endpoint: String, model: &str) -> ExternalApiConnection {
        let now = chrono::Utc::now();
        ExternalApiConnection {
            id: id.into(),
            display_name: id.into(),
            mention_alias: id.into(),
            endpoint: Some(endpoint),
            credential_slug: format!("credential-{id}"),
            origin_preset: ExternalApiConnectionPreset::Other,
            economy_model: None,
            default_model: Some(model.into()),
            reasoning_model: None,
            created_at: now,
            updated_at: now,
            image_model: None,
            video_model: None,
            media_endpoint: None,
        }
    }

    fn named_custom_step(connection_id: &str, model: Option<&str>) -> WorkflowStep {
        WorkflowStep {
            name: "named-http-step".into(),
            step_type: StepType::Agent,
            agent: AgentType::Custom,
            prompt_template: "Answer from the selected connection".into(),
            agent_settings: Some(AgentSettings {
                tools: None,
                model: model.map(str::to_string),
                tier: Some(ModelTier::Default),
                reasoning_effort: None,
                max_tokens: None,
                connection_id: Some(connection_id.into()),
            }),
            ..WorkflowStep::default()
        }
    }

    async fn insert_connection_and_catalog(
        db: &crate::db::Database,
        connection: ExternalApiConnection,
        capabilities: &[&str],
    ) {
        let runtime_target = crate::db::model_catalog::http_runtime_target_id(&connection.id);
        let model = connection.default_model.clone().unwrap();
        let capabilities = capabilities
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        db.with_conn(move |conn| {
            crate::db::external_api_connections::insert(conn, &connection)?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &runtime_target,
                &AgentType::Custom,
                &[crate::db::model_catalog::DiscoveredModel {
                    model_id: model.clone(),
                    display_name: model,
                    resolved_model: None,
                    description: None,
                    capabilities,
                    reasoning_modes: Vec::new(),
                    default_reasoning_mode: None,
                }],
            )
        })
        .await
        .unwrap();
    }

    fn empty_tokens() -> TokensConfig {
        TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        }
    }

    #[tokio::test]
    async fn incompatible_named_workflow_model_refuses_before_any_provider_request() {
        let provider_a = MockServer::start().await;
        let provider_b = MockServer::start().await;
        let db = crate::db::Database::open_in_memory().unwrap();
        insert_connection_and_catalog(
            &db,
            named_connection("connection-a", provider_a.uri(), "model-a"),
            &["chat"],
        )
        .await;
        insert_connection_and_catalog(
            &db,
            named_connection("connection-b", provider_b.uri(), "model-b"),
            &["video"],
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy();

        let outcome = execute_step(
            &named_custom_step("connection-b", None),
            &project,
            None,
            &project,
            &empty_tokens(),
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&db),
            None,
            Some("test-run"),
        )
        .await;

        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert_eq!(
            outcome.result.step_kind.as_deref(),
            Some("preflight_failed")
        );
        assert!(outcome.result.output.contains("unsupported"));
        assert!(provider_a.received_requests().await.unwrap().is_empty());
        assert!(provider_b.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn named_workflow_connection_b_dispatches_its_endpoint_and_model_never_a() {
        let provider_a = MockServer::start().await;
        let provider_b = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("\"model\":\"model-b\""))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"selected B"}}]}"#,
            ])))
            .mount(&provider_b)
            .await;
        let db = crate::db::Database::open_in_memory().unwrap();
        insert_connection_and_catalog(
            &db,
            named_connection("connection-a", provider_a.uri(), "model-a"),
            &["chat"],
        )
        .await;
        insert_connection_and_catalog(
            &db,
            named_connection("connection-b", provider_b.uri(), "model-b"),
            &["chat"],
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy();

        let outcome = execute_step(
            &named_custom_step("connection-b", None),
            &project,
            None,
            &project,
            &empty_tokens(),
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&db),
            None,
            Some("test-run"),
        )
        .await;

        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("selected B"));
        let provenance = outcome.result.agent_provenance.as_ref().unwrap();
        assert_eq!(provenance.selected_attempt, Some(1));
        assert_eq!(provenance.attempts.len(), 1);
        assert_eq!(
            provenance.attempts[0].connection_id.as_deref(),
            Some("connection-b")
        );
        assert_eq!(
            provenance.attempts[0].resolved_model.as_deref(),
            Some("model-b")
        );
        assert_eq!(
            provenance.attempts[0].requested_model.as_deref(),
            Some("model-b")
        );
        assert!(
            provenance.attempts[0].observed_models.is_empty(),
            "provider did not report a model"
        );
        assert!(provider_a.received_requests().await.unwrap().is_empty());
        let requests = provider_b.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(String::from_utf8_lossy(&requests[0].body).contains("\"model\":\"model-b\""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn disappeared_model_dispatches_same_target_replacement_and_records_both_models() {
        let dir = tempfile::tempdir().unwrap();
        let argv = dir.path().join("argv.txt");
        let fixture_body = [
            format!(
                "cat >/dev/null\nprintf '%s\\n' \"$*\" > '{}'\n",
                argv.display()
            ),
            r#"
printf '%s\n' '{"type":"assistant","message":{"model":"provider/canonical","content":[]}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"replacement used"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":2}}'
"#
            .to_owned(),
        ]
        .concat();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), &fixture_body);
        let project = dir.path().to_string_lossy().into_owned();
        let work_dir = runner::resolve_agent_work_dir(Some(&project), &project).unwrap();
        let _route = runner::test_acp_routes::route(
            &work_dir,
            Arc::new(crate::acp::ClaudeAcpAdapter::new_with_program(
                fixture.to_string_lossy(),
                Some("current-alias".into()),
                false,
            )),
        );

        let db = crate::db::Database::open_in_memory().unwrap();
        let target = crate::db::model_catalog::agent_runtime_target_id(&AgentType::ClaudeCode);
        db.with_conn(move |conn| {
            let retired = crate::db::model_catalog::DiscoveredModel {
                model_id: "retired-alias".into(),
                display_name: "Retired alias".into(),
                resolved_model: Some("provider/canonical".into()),
                description: None,
                capabilities: vec!["chat".into()],
                reasoning_modes: Vec::new(),
                default_reasoning_mode: None,
            };
            let current = crate::db::model_catalog::DiscoveredModel {
                model_id: "current-alias".into(),
                display_name: "Current alias".into(),
                resolved_model: Some("provider/canonical".into()),
                description: None,
                capabilities: vec!["chat".into()],
                reasoning_modes: Vec::new(),
                default_reasoning_mode: None,
            };
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::ClaudeCode,
                &[retired, current.clone()],
            )?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::ClaudeCode,
                &[current],
            )
        })
        .await
        .unwrap();
        let step = WorkflowStep {
            name: "fallback".into(),
            step_type: StepType::Agent,
            agent: AgentType::ClaudeCode,
            prompt_template: "Use the selected model".into(),
            agent_settings: Some(AgentSettings {
                tools: None,
                model: Some("retired-alias".into()),
                tier: Some(ModelTier::Reasoning),
                reasoning_effort: None,
                max_tokens: None,
                connection_id: None,
            }),
            ..WorkflowStep::default()
        };
        let outcome = execute_step(
            &step,
            &project,
            None,
            &project,
            &empty_tokens(),
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&db),
            None,
            Some("test-run"),
        )
        .await;

        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("replacement used"));
        let attempt = &outcome.result.agent_provenance.as_ref().unwrap().attempts[0];
        assert_eq!(attempt.requested_model.as_deref(), Some("retired-alias"));
        assert_eq!(attempt.resolved_model.as_deref(), Some("current-alias"));
        let warning = attempt
            .preflight_warning
            .as_ref()
            .expect("the automatic replacement must be visible in provenance");
        assert_eq!(warning.requested_model, "retired-alias");
        assert_eq!(warning.effective_model, "current-alias");
        assert_eq!(warning.reason, ModelUnavailableReason::Disappeared);
        let args = std::fs::read_to_string(argv).unwrap();
        assert!(args.contains("--model current-alias"), "argv: {args}");
        assert!(!args.contains("retired-alias"), "argv: {args}");
    }

    #[tokio::test]
    async fn deleted_named_workflow_connection_surfaces_refusal_without_fallback() {
        let fallback = MockServer::start().await;
        let db = crate::db::Database::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy();
        let endpoints = crate::models::setup::HttpEndpoints {
            lite_llm: Some(fallback.uri()),
            nvidia: None,
        };

        let outcome = execute_step(
            &named_custom_step("deleted-connection", None),
            &project,
            None,
            &project,
            &empty_tokens(),
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            Some(&endpoints),
            None,
            None,
            Some(&db),
            None,
            Some("test-run"),
        )
        .await;

        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert_eq!(
            outcome.result.step_kind.as_deref(),
            Some("preflight_failed")
        );
        assert!(outcome.result.output.contains("deleted-connection"));
        assert!(fallback.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn named_litellm_without_endpoint_refuses_without_legacy_fallback() {
        let fallback = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"wrong fallback"}}]}"#,
            ])))
            .mount(&fallback)
            .await;
        let db = crate::db::Database::open_in_memory().unwrap();
        let mut selected = named_connection("connection-b", fallback.uri(), "model-b");
        selected.origin_preset = ExternalApiConnectionPreset::LiteLlm;
        selected.endpoint = Some("   ".into());
        insert_connection_and_catalog(&db, selected, &["chat"]).await;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy();
        let endpoints = crate::models::setup::HttpEndpoints {
            lite_llm: Some(fallback.uri()),
            nvidia: None,
        };

        let mut step = named_custom_step("connection-b", None);
        step.agent = AgentType::LiteLlm;
        let outcome = execute_step(
            &step,
            &project,
            None,
            &project,
            &empty_tokens(),
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            Some(&endpoints),
            None,
            None,
            Some(&db),
            None,
            Some("test-run"),
        )
        .await;

        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert_eq!(
            outcome.result.step_kind.as_deref(),
            Some("preflight_failed")
        );
        assert!(outcome.result.output.contains("connection-b"));
        assert!(outcome.result.output.contains("endpoint"));
        assert!(fallback.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn litellm_workflow_step_executes_a_native_read_and_consumes_its_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("\"name\":\"task_list\""))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"tool-1","function":{"name":"task_list","arguments":"{}"}}]}}]}"#,
            ])))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("KT-189"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"KT-189 is Native tools"}}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":5}}"#,
            ])))
            .mount(&server)
            .await;

        let calls = Arc::new(Mutex::new(Vec::new()));
        let tools: Arc<dyn ToolExecutor> = Arc::new(ReadOnlyTools {
            calls: calls.clone(),
        });
        let step = WorkflowStep {
            name: "inspect-planning".into(),
            step_type: StepType::Agent,
            agent: AgentType::LiteLlm,
            prompt_template: "Which task is active?".into(),
            agent_settings: Some(AgentSettings {
                tools: None,
                model: Some("test-model".into()),
                tier: None,
                reasoning_effort: None,
                max_tokens: None,
                connection_id: None,
            }),
            ..WorkflowStep::default()
        };
        let tokens = TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let dir = tempfile::tempdir().expect("temporary project");
        let project = dir.path().to_string_lossy();
        let outcome = execute_step(
            &step,
            &project,
            None,
            &project,
            &tokens,
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            Some(&crate::models::setup::HttpEndpoints {
                lite_llm: Some(server.uri()),
                nvidia: None,
            }),
            None,
            Some(tools),
            None,
            None,
            Some("test-run"),
        )
        .await;

        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("KT-189 is Native tools"));
        assert_eq!(calls.lock().unwrap().as_slice(), &["task_list"]);
        assert_eq!(outcome.result.native_tool_calls.len(), 1);
        assert_eq!(outcome.result.native_tool_calls[0].name, "task_list");
        assert!(outcome.result.native_tool_calls[0].ok);
    }

    #[tokio::test]
    #[serial]
    async fn ollama_workflow_step_executes_a_native_read_and_consumes_its_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_string_contains("\"name\":\"task_list\""))
            .and(body_string_contains("\"num_ctx\":12345"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "{\"message\":{\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"task_list\",\"arguments\":{}}}]},\"done\":false}\n\
                 {\"done\":true,\"prompt_eval_count\":6,\"eval_count\":2}\n",
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_string_contains("KT-189"))
            .and(body_string_contains(r#""role":"tool""#))
            .and(body_string_contains("\"num_ctx\":12345"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "{\"message\":{\"content\":\"KT-189 is Native tools\"},\"done\":false}\n\
                 {\"done\":true,\"prompt_eval_count\":10,\"eval_count\":4}\n",
            ))
            .mount(&server)
            .await;

        let previous_host = crate::core::child_env::var("OLLAMA_HOST").ok();
        let previous_ctx_cap = crate::core::child_env::var("KRONN_OLLAMA_NUM_CTX_CAP").ok();
        crate::core::child_env::set_var("OLLAMA_HOST", server.uri());
        crate::core::child_env::remove_var("KRONN_OLLAMA_NUM_CTX_CAP");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tools: Arc<dyn ToolExecutor> = Arc::new(ReadOnlyTools {
            calls: calls.clone(),
        });
        let step = WorkflowStep {
            name: "inspect-local-planning".into(),
            step_type: StepType::Agent,
            agent: AgentType::Ollama,
            prompt_template: "Which task is active?".into(),
            agent_settings: Some(AgentSettings {
                tools: None,
                model: Some("test-model".into()),
                tier: None,
                reasoning_effort: None,
                max_tokens: None,
                connection_id: None,
            }),
            ..WorkflowStep::default()
        };
        let tokens = TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let dir = tempfile::tempdir().expect("temporary project");
        let project = dir.path().to_string_lossy();
        let overrides = std::collections::HashMap::from([("test-model".to_string(), 12_345)]);
        let outcome = execute_step(
            &step,
            &project,
            None,
            &project,
            &tokens,
            false,
            &TemplateContext::new(),
            None,
            None,
            None,
            None,
            Some(&overrides),
            Some(tools),
            None,
            None,
            Some("test-run"),
        )
        .await;
        match previous_host {
            Some(value) => crate::core::child_env::set_var("OLLAMA_HOST", value),
            None => crate::core::child_env::remove_var("OLLAMA_HOST"),
        }
        match previous_ctx_cap {
            Some(value) => crate::core::child_env::set_var("KRONN_OLLAMA_NUM_CTX_CAP", value),
            None => crate::core::child_env::remove_var("KRONN_OLLAMA_NUM_CTX_CAP"),
        }

        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("KT-189 is Native tools"));
        assert_eq!(calls.lock().unwrap().as_slice(), &["task_list"]);
        assert_eq!(outcome.result.native_tool_calls.len(), 1);
        assert_eq!(outcome.result.native_tool_calls[0].name, "task_list");
        assert!(outcome.result.native_tool_calls[0].ok);
    }
}
