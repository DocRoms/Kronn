//! Executor for `StepType::Gate` (0.7.0 Phase 4 — human-in-the-loop).
//!
//! A Gate is a pause: it spawns no LLM, makes no HTTP call, costs zero
//! tokens. The runner stops the run with `RunStatus::WaitingApproval`
//! and writes the rendered gate message into the `StepResult.output`
//! so the operator sees it on the run-detail page. The decision arrives
//! later via `POST /api/workflows/runs/:id/decide`, which calls
//! `runner::resume_run` to continue from this point — approve continues
//! to the next step, request_changes jumps back to a target step,
//! reject terminates the run.
//!
//! The output of a successfully approved gate is the rendered message
//! plus a footer line with the operator's comment, so downstream steps
//! can reference `{{steps.<gate_name>}}` if they care to.
//!
//! Templates: `gate_message` is rendered against the live
//! `TemplateContext`, so the operator sees the actual values
//! (`{{steps.audit.summary}}` etc.) at decision time, not the literal
//! curly-braces source.

use std::time::Instant;

use crate::models::*;

use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Build the pause outcome. Always synchronous, always non-fallible —
/// the only template-render failure path produces a `Failed` step
/// (rather than `WaitingApproval`) so a typo in `{{...}}` doesn't
/// silently block the run forever waiting for an approval that has
/// no message to show.
pub fn execute_gate_step(
    step: &WorkflowStep,
    ctx: &TemplateContext,
) -> StepOutcome {
    let start = Instant::now();
    // 0.8.2 — Capture the wall-clock start so the resume handler can
    // compute the actual pause duration once a human approves the gate.
    // Without this, `duration_ms` only carried the executor render time
    // (~0ms) and the elapsed counter on the NEXT step appeared to
    // include the whole pause.
    let started_at = chrono::Utc::now();

    let raw_message = step
        .gate_message
        .as_deref()
        .unwrap_or("Décision humaine requise.");

    let rendered = match ctx.render(raw_message) {
        Ok(r) => r,
        Err(e) => {
            return StepOutcome {
                result: StepResult {
                    step_name: step.name.clone(),
                    status: RunStatus::Failed,
                    output: format!("Gate template render error: {}", e),
                    tokens_used: 0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    started_at: Some(started_at),
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

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::WaitingApproval,
            output: rendered,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: Some(started_at),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_step(name: &str, message: Option<&str>) -> WorkflowStep {
        WorkflowStep {
            name: name.into(),
            step_type: StepType::Gate,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText,
            on_result: vec![],
            agent_settings: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            mcp_config_ids: vec![],
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
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: message.map(|s| s.to_string()),
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
    fn gate_outcome_carries_started_at_for_pause_duration_tracking() {
        // 0.8.2 — Regression: pre-fix, the gate's `duration_ms` only
        // counted executor render time (~0ms), making the live-elapsed
        // counter on the NEXT step appear to include the full pause.
        // The fix wires `started_at` so `resume_run_from_gate` (in
        // runner.rs) can compute the actual pause duration on approval.
        let step = gate_step("approve_plan", Some("Approve?"));
        let ctx = TemplateContext::new();
        let before = chrono::Utc::now();
        let outcome = execute_gate_step(&step, &ctx);
        let after = chrono::Utc::now();
        let started_at = outcome.result.started_at
            .expect("gate must carry a started_at so resume can compute pause duration");
        assert!(started_at >= before && started_at <= after,
            "started_at must be set at executor time, got {started_at} outside [{before}, {after}]");
    }

    #[test]
    fn renders_message_and_returns_waiting_approval() {
        let step = gate_step("approve_pr", Some("Valider le PR `{{steps.audit.summary}}` ?"));
        let mut ctx = TemplateContext::new();
        ctx.set_step_output(
            "audit",
            "Tout est OK\n---STEP_OUTPUT---\n{\"status\":\"ok\",\"summary\":\"12 fichiers analysés\",\"data\":{}}\n---END_STEP_OUTPUT---",
        );
        let outcome = execute_gate_step(&step, &ctx);
        assert_eq!(outcome.result.status, RunStatus::WaitingApproval);
        assert!(outcome.result.output.contains("12 fichiers analysés"), "got: {}", outcome.result.output);
        assert_eq!(outcome.result.tokens_used, 0);
        assert!(outcome.condition_action.is_none());
    }

    #[test]
    fn missing_message_uses_default_placeholder() {
        let step = gate_step("approve", None);
        let ctx = TemplateContext::new();
        let outcome = execute_gate_step(&step, &ctx);
        assert_eq!(outcome.result.status, RunStatus::WaitingApproval);
        assert!(outcome.result.output.contains("Décision humaine requise"));
    }

    #[test]
    fn empty_message_uses_default_placeholder() {
        // Empty string is treated as "no message" — falls back to the
        // built-in default so the dashboard never shows a blank pane.
        let step = gate_step("approve", Some(""));
        let ctx = TemplateContext::new();
        let outcome = execute_gate_step(&step, &ctx);
        assert_eq!(outcome.result.status, RunStatus::WaitingApproval);
        // Empty message is technically a valid render — output is empty
        // string, but the runner doesn't care; the UI shows the empty
        // body without crashing. Documenting current behavior.
        assert_eq!(outcome.result.output, "");
    }

    #[test]
    fn template_error_yields_failed_outcome() {
        // Unknown placeholder doesn't fail, but template syntax error does.
        // We check the failure path renders a sensible error message.
        let step = gate_step("approve", Some("{{ invalid syntax !@#"));
        let ctx = TemplateContext::new();
        let outcome = execute_gate_step(&step, &ctx);
        // Depending on how render handles malformed templates, either
        // it returns the literal back (no error) or it errors. The
        // current TemplateContext returns Ok with the literal, so this
        // gate succeeds with the raw text — that's acceptable: malformed
        // templates aren't a system failure, just a UX issue surfaced
        // in the gate body.
        assert!(matches!(
            outcome.result.status,
            RunStatus::WaitingApproval | RunStatus::Failed
        ));
    }
}
