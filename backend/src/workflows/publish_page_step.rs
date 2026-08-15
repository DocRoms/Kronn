//! Executor for `StepType::PublishPageData` (0.10.0).
//!
//! This is deliberately a typed sink: `value_from` resolves directly to a
//! `serde_json::Value`. It never stringifies an object/array through template
//! interpolation, which keeps chart data lossless and deterministic.

use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};

use crate::models::{
    LivePageWrite, LivePageWriteOperation, PublishLivePageRequest, RunStatus, StepResult,
    WorkflowStep,
};
use crate::AppState;

use super::steps::StepOutcome;
use super::template::TemplateContext;

pub async fn execute_publish_page_data_step(
    step: &WorkflowStep,
    workflow_id: &str,
    run_id: &str,
    state: &AppState,
    context: &TemplateContext,
) -> StepOutcome {
    let started = Instant::now();
    let request = match build_request(step, workflow_id, run_id, context) {
        Ok(request) => request,
        Err(error) => return fail(step, started, error),
    };
    let page_id = match step.page_publish.as_ref() {
        Some(config) => match context.render_strict(&config.page_id) {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => return fail(step, started, "Page id/slug cannot be empty"),
            Err(error) => return fail(step, started, error),
        },
        None => return fail(step, started, "PublishPageData step missing `page_publish`"),
    };

    let db = state.db.clone();
    let page_for_db = page_id.clone();
    let result = match db
        .with_conn(move |conn| {
            crate::db::live_pages::publish_live_page(conn, &page_for_db, &request)
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return fail(step, started, format!("Page publication failed: {error}")),
    };

    let summary = format!(
        "Page '{}' published at data revision {} ({} changed, {} unchanged, {} point(s) added)",
        page_id,
        result.data_revision,
        result.changed_datasets.len(),
        result.unchanged_datasets.len(),
        result.points_added
    );
    let payload = match serde_json::to_value(&result) {
        Ok(value) => value,
        Err(error) => return fail(step, started, error),
    };
    succeed(step, started, payload, summary)
}

fn build_request(
    step: &WorkflowStep,
    workflow_id: &str,
    run_id: &str,
    context: &TemplateContext,
) -> Result<PublishLivePageRequest> {
    let config = step
        .page_publish
        .as_ref()
        .ok_or_else(|| anyhow!("PublishPageData step missing `page_publish`"))?;
    if config.writes.is_empty() {
        bail!("PublishPageData requires at least one dataset write");
    }

    let mut writes = Vec::with_capacity(config.writes.len());
    for (index, write) in config.writes.iter().enumerate() {
        if write.dataset.trim().is_empty() {
            bail!("Page dataset name cannot be empty");
        }
        let key = typed_source_key(&write.value_from)?;
        let value = context
            .resolve_value(key)
            .ok_or_else(|| anyhow!("Unknown typed Page data source '{key}'"))?;
        let observed_at = write
            .observed_at
            .as_deref()
            .map(|template| {
                let rendered = context.render_strict(template)?;
                DateTime::parse_from_rfc3339(&rendered)
                    .map(|value| value.with_timezone(&Utc))
                    .with_context(|| format!("Invalid Page observed_at '{rendered}'"))
            })
            .transpose()?;
        let dedupe_key = match write.dedupe_key.as_deref() {
            Some(template) => Some(context.render_strict(template)?),
            None if write.operation == LivePageWriteOperation::Append => {
                Some(format!("{run_id}:{index}"))
            }
            None => None,
        };
        writes.push(LivePageWrite {
            dataset: write.dataset.trim().to_string(),
            operation: write.operation,
            value,
            observed_at,
            dedupe_key,
            key_field: write.key_field.clone(),
        });
    }

    Ok(PublishLivePageRequest {
        workflow_id: Some(workflow_id.to_string()),
        workflow_run_id: Some(run_id.to_string()),
        writes,
    })
}

fn typed_source_key(source: &str) -> Result<&str> {
    let source = source.trim();
    if let Some(inner) = source
        .strip_prefix("{{")
        .and_then(|value| value.strip_suffix("}}"))
    {
        let key = inner.trim();
        if key.is_empty() || key.contains("{{") || key.contains("}}") {
            bail!("Invalid typed Page data source '{source}'");
        }
        return Ok(key);
    }
    if source.is_empty() || source.contains("{{") || source.contains("}}") {
        bail!("Page `value_from` must be one typed context path, not mixed text");
    }
    Ok(source)
}

fn succeed(
    step: &WorkflowStep,
    started: Instant,
    payload: serde_json::Value,
    summary: String,
) -> StepOutcome {
    let output = super::step_output_format::format_step_output_simple(payload, "OK", &summary);
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|action| match action {
        crate::models::ConditionAction::Stop => "Stop".to_string(),
        crate::models::ConditionAction::Skip => "Skip".to_string(),
        crate::models::ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Success,
            output,
            tokens_used: 0,
            duration_ms: started.elapsed().as_millis() as u64,
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
            native_tool_calls: Box::default(),
        },
        condition_action,
    }
}

fn fail(step: &WorkflowStep, started: Instant, error: impl std::fmt::Display) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: error.to_string(),
            tokens_used: 0,
            duration_ms: started.elapsed().as_millis() as u64,
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
            native_tool_calls: Box::default(),
        },
        condition_action: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        PublishPageDataConfig, PublishPageDataWrite, StepType, TransformDataConfig,
        TransformDataField, TransformDataOperation, WorkflowStep,
    };

    fn step(value_from: &str) -> WorkflowStep {
        WorkflowStep {
            name: "publish_metrics".into(),
            step_type: StepType::PublishPageData,
            page_publish: Some(PublishPageDataConfig {
                page_id: "adobe-health".into(),
                writes: vec![PublishPageDataWrite {
                    dataset: "latency".into(),
                    operation: LivePageWriteOperation::Append,
                    value_from: value_from.into(),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            }),
            ..WorkflowStep::default()
        }
    }

    #[test]
    fn request_preserves_typed_json_and_defaults_idempotency_key() {
        let mut context = TemplateContext::new();
        let envelope = super::super::step_output_format::format_step_output_simple(
            serde_json::json!({"series": [{"value": 12}, {"value": 19}]}),
            "OK",
            "fixture",
        );
        context.set_step_output("fetch", &envelope);

        let request = build_request(
            &step("{{steps.fetch.data.series}}"),
            "workflow-1",
            "run-42",
            &context,
        )
        .expect("typed request");

        assert!(request.writes[0].value.is_array());
        assert_eq!(request.writes[0].value[1]["value"], 19);
        assert_eq!(request.writes[0].dedupe_key.as_deref(), Some("run-42:0"));
    }

    #[test]
    fn mixed_text_source_is_rejected_instead_of_stringifying_json() {
        let context = TemplateContext::new();
        let error = build_request(
            &step("prefix {{steps.fetch.data}}"),
            "workflow-1",
            "run-42",
            &context,
        )
        .expect_err("mixed interpolation must fail");
        assert!(error.to_string().contains("one typed context path"));
    }

    #[tokio::test]
    async fn collect_transform_publish_contract_remains_typed() {
        let mut context = TemplateContext::new();
        let collected = super::super::step_output_format::format_step_output_simple(
            serde_json::json!({
                "sources": {
                    "adobe": { "requests": 1240 },
                    "errors": { "items": [{"count": 2}, {"count": 3}] }
                },
                "meta": { "succeeded": 2, "failed": 0 }
            }),
            "OK",
            "Collected 2/2 Quick API source(s)",
        );
        context.set_step_output("collect", &collected);

        let transform = WorkflowStep {
            name: "shape".into(),
            step_type: StepType::TransformData,
            transform_data: Some(TransformDataConfig {
                input_from: "steps.collect.data".into(),
                fields: vec![
                    TransformDataField {
                        target: "summary.requests".into(),
                        source: "$.sources.adobe.requests".into(),
                        operation: TransformDataOperation::Copy,
                        fallback: None,
                        value_type: None,
                    },
                    TransformDataField {
                        target: "summary.errors".into(),
                        source: "$.sources.errors.items[*].count".into(),
                        operation: TransformDataOperation::Sum,
                        fallback: None,
                        value_type: None,
                    },
                ],
            }),
            ..WorkflowStep::default()
        };
        let transformed =
            super::super::transform_data_step::execute_transform_data_step(&transform, &context)
                .await;
        assert_eq!(transformed.result.status, RunStatus::Success);
        context.set_step_output("shape", &transformed.result.output);

        let request = build_request(
            &step("steps.shape.data.summary"),
            "workflow-1",
            "run-42",
            &context,
        )
        .expect("typed Page request");
        assert_eq!(
            request.writes[0].value,
            serde_json::json!({"requests": 1240, "errors": 5.0})
        );
    }
}
