//! Executor for `StepType::JsonData` (0.7+).
//!
//! Source de données déterministe : émet le payload littéral stocké dans
//! `step.json_data_payload` sous forme d'envelope Structured. Zéro token,
//! zéro réseau.
//!
//! # Cas d'usage
//!
//! 1. **Workflow batch sans API** — 10 hosts hardcodés alimentent un
//!    `BatchQuickPrompt` ou un `BatchApiCall`. Évite de monter une fake
//!    API juste pour tenir la liste.
//! 2. **Fixture de dev** — on construit le pipeline sur du JsonData puis
//!    on remplace par un `ApiCall` quand la vraie source est prête.
//! 3. **Test deterministe** — un workflow d'audit qu'on rejoue sur des
//!    fixtures réplique exactement le même comportement run-après-run.
//!
//! # Output
//!
//! Toujours `Structured`, peu importe `step.output_format` (ce dernier
//! est ignoré). L'envelope a la même shape que pour les autres step
//! types pour que `{{steps.<name>.data}}` et `{{steps.<name>.summary}}`
//! marchent uniformément :
//!
//! ```json
//! {
//!   "data": <payload>,
//!   "status": "OK",
//!   "summary": "JSON data (N items)"
//! }
//! ```
//!
//! Le compteur d'items du summary suit la convention :
//! - array → length de l'array
//! - object → "1 object"
//! - scalaire → "1 value"
//!
//! # Pas de templating
//!
//! Le payload est retourné tel quel, sans substitution `{{var}}`. Si tu
//! veux du templating, utilise `Notify` (sink), `Agent` (LLM), ou un
//! `ApiCall` qui fabrique le JSON dynamiquement. Garder JsonData pur
//! évite l'ambiguïté "est-ce que le template a tourné ou pas ?" — la
//! source-of-truth = la valeur stockée.

use std::time::Instant;

use crate::models::{RunStatus, StepResult, WorkflowStep};

use super::steps::StepOutcome;

pub async fn execute_json_data_step(step: &WorkflowStep) -> StepOutcome {
    let start = Instant::now();

    let payload = match step.json_data_payload.as_ref() {
        Some(p) => p.clone(),
        None => {
            return fail(
                step,
                start,
                "JsonData step missing `json_data_payload`. Set the JSON value to emit.",
            );
        }
    };

    let summary = build_summary(&payload);

    // 0.8.5 — canonical envelope via shared formatter. Pre-fix this
    // step emitted bare compact JSON, which `extract_step_envelope`
    // absorbed via the strategy-2 fallback. The unified shape (with
    // `---STEP_OUTPUT---` markers + `[SIGNAL: OK]`) means consumers
    // and the run-log viewer see the same structure regardless of
    // which step type produced the data. Cf.
    // [[project_step_output_homogenisation_0_9_0]].
    let output = super::step_output_format::format_step_output_simple(
        payload,
        "OK",
        &summary,
    );

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Success,
            output,
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

/// Compteur d'items lisible. Un downstream batch fonctionne sur arrays,
/// donc le cas array est privilégié dans la formulation. Les autres cas
/// sont supportés mais le summary les flagge clairement pour que le
/// debugging au pied du workflow soit immédiat.
fn build_summary(payload: &serde_json::Value) -> String {
    match payload {
        serde_json::Value::Array(arr) => format!("JSON data ({} item(s))", arr.len()),
        serde_json::Value::Object(obj) => format!("JSON data (1 object, {} field(s))", obj.len()),
        serde_json::Value::Null => String::from("JSON data (null)"),
        _ => String::from("JSON data (1 value)"),
    }
}

fn fail(step: &WorkflowStep, start: Instant, msg: impl Into<String>) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: msg.into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentType, StepMode, StepOutputFormat, StepType};

    fn blank_step(payload: Option<serde_json::Value>) -> WorkflowStep {
        WorkflowStep {
            name: "json_step".to_string(),
            step_type: StepType::JsonData,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText, // forcé Structured côté run
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
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            quick_prompt_id: None,
            json_data_payload: payload,
        }
    }

    #[tokio::test]
    async fn missing_payload_fails_clearly() {
        let step = blank_step(None);
        let outcome = execute_json_data_step(&step).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("json_data_payload"),
            "error mentions field: {}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn array_payload_emits_item_count_summary() {
        let payload = serde_json::json!([
            { "host": "fr.example.com" },
            { "host": "de.example.com" },
            { "host": "en.example.com" },
        ]);
        let step = blank_step(Some(payload.clone()));
        let outcome = execute_json_data_step(&step).await;
        assert_eq!(outcome.result.status, RunStatus::Success);

        let envelope =
            crate::workflows::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["status"], "OK");
        assert_eq!(envelope["data"], payload);
        assert!(
            envelope["summary"].as_str().unwrap().contains("3"),
            "summary mentions count: {}",
            envelope["summary"]
        );
    }

    #[tokio::test]
    async fn object_payload_emits_field_count_summary() {
        let payload = serde_json::json!({
            "host": "fr.example.com",
            "limit": 5,
            "tags": ["a", "b"],
        });
        let step = blank_step(Some(payload.clone()));
        let outcome = execute_json_data_step(&step).await;
        assert_eq!(outcome.result.status, RunStatus::Success);

        let envelope =
            crate::workflows::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["data"], payload);
        let summary = envelope["summary"].as_str().unwrap();
        assert!(summary.contains("3"), "summary mentions field count");
        assert!(summary.contains("object"), "summary tags as object");
    }

    #[tokio::test]
    async fn scalar_payload_works() {
        // Cas marginal mais légitime : un single value pour chainer un
        // BatchQuickPrompt avec un seul item, ou pour passer une constante
        // à un step d'aval.
        let payload = serde_json::json!("just-a-string");
        let step = blank_step(Some(payload.clone()));
        let outcome = execute_json_data_step(&step).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope =
            crate::workflows::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["data"], payload);
    }

    #[tokio::test]
    async fn payload_returned_verbatim_no_templating() {
        // Garantit qu'on ne fait PAS de substitution {{var}} sur le payload.
        // Si un user veut du templating, qu'il utilise un Agent ou un
        // ApiCall — le contrat de JsonData est "valeur littérale".
        let payload = serde_json::json!({
            "raw_template": "{{not_substituted}}",
            "literal": "stays as-is",
        });
        let step = blank_step(Some(payload.clone()));
        let outcome = execute_json_data_step(&step).await;
        let envelope =
            crate::workflows::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(
            envelope["data"]["raw_template"], "{{not_substituted}}",
            "templates are NOT rendered in JsonData payloads"
        );
    }
}
