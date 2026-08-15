//! Deterministic JSON transformation for `StepType::TransformData` (0.10.0).
//!
//! The recipe is intentionally smaller than a general-purpose programming
//! language: JSONPath selection, safe aggregations and explicit scalar casts.
//! This keeps workflow data shaping portable and removes the need for `Exec`
//! snippets without introducing arbitrary code execution.

use std::collections::HashSet;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Number, Value};

use crate::models::{
    RunStatus, StepResult, TransformDataField, TransformDataOperation, TransformDataValueType,
    WorkflowStep,
};

use super::api_call_step::apply_extract;
use super::steps::StepOutcome;
use super::template::TemplateContext;

const MAX_FIELDS: usize = 200;

pub async fn execute_transform_data_step(
    step: &WorkflowStep,
    context: &TemplateContext,
) -> StepOutcome {
    let started = Instant::now();
    match transform(step, context) {
        Ok(payload) => succeed(step, started, payload),
        Err(error) => fail(step, started, error),
    }
}

fn transform(step: &WorkflowStep, context: &TemplateContext) -> Result<Value> {
    let config = step
        .transform_data
        .as_ref()
        .ok_or_else(|| anyhow!("TransformData step missing `transform_data`"))?;
    let input_key = typed_source_key(&config.input_from)?;
    let input = context
        .resolve_value(input_key)
        .ok_or_else(|| anyhow!("Unknown typed transform source `{input_key}`"))?;

    transform_value(&config.fields, &input)
}

pub(crate) fn transform_value(fields: &[TransformDataField], input: &Value) -> Result<Value> {
    if fields.is_empty() {
        bail!("TransformData requires at least one output field");
    }
    if fields.len() > MAX_FIELDS {
        bail!("TransformData accepts at most {MAX_FIELDS} output fields");
    }
    let mut seen = HashSet::new();
    let mut output = Map::new();
    for field in fields {
        let target = field.target.trim();
        validate_target(target)?;
        if !seen.insert(target.to_string()) {
            bail!("Duplicate transform target `{target}`");
        }
        let value =
            evaluate_field(field, input).with_context(|| format!("Transform target `{target}`"))?;
        insert_target(&mut output, target, value)?;
    }
    Ok(Value::Object(output))
}

fn evaluate_field(field: &TransformDataField, input: &Value) -> Result<Value> {
    let source = field.source.trim();
    if source.is_empty() {
        bail!("source JSONPath cannot be empty");
    }
    let extracted = apply_extract(
        &crate::models::ExtractSpec {
            path: source.to_string(),
            fallback: field.fallback.clone(),
            fail_on_empty: false,
        },
        input,
    )
    .map_err(|error| anyhow!(error.to_string()))?;
    if extracted.is_empty && field.fallback.is_none() {
        bail!("JSONPath `{source}` matched no value and has no fallback");
    }

    let value = match field.operation {
        TransformDataOperation::Copy => extracted.value,
        TransformDataOperation::Count => count_value(&extracted.value),
        TransformDataOperation::Sum => aggregate_numbers(&extracted.value, NumericOp::Sum)?,
        TransformDataOperation::Average => aggregate_numbers(&extracted.value, NumericOp::Average)?,
        TransformDataOperation::Min => aggregate_numbers(&extracted.value, NumericOp::Min)?,
        TransformDataOperation::Max => aggregate_numbers(&extracted.value, NumericOp::Max)?,
        TransformDataOperation::First => edge_value(&extracted.value, true)?,
        TransformDataOperation::Last => edge_value(&extracted.value, false)?,
    };
    match field.value_type {
        Some(kind) => coerce(value, kind),
        None => Ok(value),
    }
}

fn count_value(value: &Value) -> Value {
    let count = match value {
        Value::Array(values) => values.len(),
        Value::Object(values) => values.len(),
        Value::String(value) => value.chars().count(),
        Value::Null => 0,
        _ => 1,
    };
    Value::Number(Number::from(count as u64))
}

#[derive(Clone, Copy)]
enum NumericOp {
    Sum,
    Average,
    Min,
    Max,
}

fn aggregate_numbers(value: &Value, operation: NumericOp) -> Result<Value> {
    let values: Vec<&Value> = match value {
        Value::Array(values) => values.iter().collect(),
        other => vec![other],
    };
    if values.is_empty() {
        bail!("numeric aggregation received an empty array");
    }
    let numbers = values
        .into_iter()
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| anyhow!("numeric aggregation received {value}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let result = match operation {
        NumericOp::Sum => numbers.iter().sum(),
        NumericOp::Average => numbers.iter().sum::<f64>() / numbers.len() as f64,
        NumericOp::Min => numbers.into_iter().fold(f64::INFINITY, f64::min),
        NumericOp::Max => numbers.into_iter().fold(f64::NEG_INFINITY, f64::max),
    };
    Number::from_f64(result)
        .map(Value::Number)
        .ok_or_else(|| anyhow!("numeric aggregation produced a non-finite value"))
}

fn edge_value(value: &Value, first: bool) -> Result<Value> {
    let Value::Array(values) = value else {
        bail!("first/last operation requires an array");
    };
    let selected = if first { values.first() } else { values.last() };
    selected
        .cloned()
        .ok_or_else(|| anyhow!("first/last operation received an empty array"))
}

fn coerce(value: Value, kind: TransformDataValueType) -> Result<Value> {
    match kind {
        TransformDataValueType::String => Ok(Value::String(match value {
            Value::String(value) => value,
            other => serde_json::to_string(&other)?,
        })),
        TransformDataValueType::Number => match value {
            Value::Number(_) => Ok(value),
            Value::String(value) => {
                let parsed: f64 = value
                    .parse()
                    .with_context(|| format!("cannot convert `{value}` to number"))?;
                Number::from_f64(parsed)
                    .map(Value::Number)
                    .ok_or_else(|| anyhow!("number is not finite"))
            }
            other => bail!("cannot convert {other} to number"),
        },
        TransformDataValueType::Boolean => match value {
            Value::Bool(_) => Ok(value),
            Value::String(value) if value.eq_ignore_ascii_case("true") => Ok(Value::Bool(true)),
            Value::String(value) if value.eq_ignore_ascii_case("false") => Ok(Value::Bool(false)),
            Value::Number(value) => Ok(Value::Bool(value.as_f64().unwrap_or(0.0) != 0.0)),
            other => bail!("cannot convert {other} to boolean"),
        },
    }
}

fn typed_source_key(source: &str) -> Result<&str> {
    let source = source.trim();
    if let Some(inner) = source
        .strip_prefix("{{")
        .and_then(|value| value.strip_suffix("}}"))
    {
        let key = inner.trim();
        if key.is_empty() || key.contains("{{") || key.contains("}}") {
            bail!("Invalid typed transform source `{source}`");
        }
        return Ok(key);
    }
    if source.is_empty() || source.contains("{{") || source.contains("}}") {
        bail!("Transform `input_from` must be one typed context path");
    }
    Ok(source)
}

fn validate_target(target: &str) -> Result<()> {
    if target.is_empty() {
        bail!("target cannot be empty");
    }
    if target.split('.').any(|segment| {
        segment.is_empty()
            || !segment.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
    }) {
        bail!("invalid target `{target}` (use dotted object keys)");
    }
    Ok(())
}

fn insert_target(root: &mut Map<String, Value>, target: &str, value: Value) -> Result<()> {
    let segments: Vec<&str> = target.split('.').collect();
    insert_segments(root, &segments, value, target)
}

fn insert_segments(
    current: &mut Map<String, Value>,
    segments: &[&str],
    value: Value,
    full_target: &str,
) -> Result<()> {
    if segments.len() == 1 {
        if current.contains_key(segments[0]) {
            bail!("transform target collision at `{full_target}`");
        }
        current.insert(segments[0].to_string(), value);
        return Ok(());
    }
    let entry = current
        .entry(segments[0].to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let object = entry
        .as_object_mut()
        .ok_or_else(|| anyhow!("transform target collision at `{full_target}`"))?;
    insert_segments(object, &segments[1..], value, full_target)
}

fn succeed(step: &WorkflowStep, started: Instant, payload: Value) -> StepOutcome {
    let fields = payload.as_object().map(Map::len).unwrap_or_default();
    build_outcome(
        step,
        started,
        RunStatus::Success,
        payload,
        "OK",
        format!("Transformed data ({fields} top-level field(s))"),
    )
}

fn fail(step: &WorkflowStep, started: Instant, error: impl std::fmt::Display) -> StepOutcome {
    build_outcome(
        step,
        started,
        RunStatus::Failed,
        serde_json::json!({ "error": format!("{error:#}") }),
        "ERROR",
        "Data transformation failed".to_string(),
    )
}

fn build_outcome(
    step: &WorkflowStep,
    started: Instant,
    run_status: RunStatus,
    payload: Value,
    status: &str,
    summary: String,
) -> StepOutcome {
    let output = super::step_output_format::format_step_output_simple(payload, status, &summary);
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|action| match action {
        crate::models::ConditionAction::Stop => "Stop".to_string(),
        crate::models::ConditionAction::Skip => "Skip".to_string(),
        crate::models::ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: run_status,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        StepType, TransformDataConfig, TransformDataField, TransformDataOperation,
    };

    fn field(target: &str, source: &str, operation: TransformDataOperation) -> TransformDataField {
        TransformDataField {
            target: target.into(),
            source: source.into(),
            operation,
            fallback: None,
            value_type: None,
        }
    }

    fn step(fields: Vec<TransformDataField>) -> WorkflowStep {
        WorkflowStep {
            name: "shape".into(),
            step_type: StepType::TransformData,
            transform_data: Some(TransformDataConfig {
                input_from: "steps.collect.data".into(),
                fields,
            }),
            ..WorkflowStep::default()
        }
    }

    fn context() -> TemplateContext {
        let mut context = TemplateContext::new();
        let output = super::super::step_output_format::format_step_output_simple(
            serde_json::json!({
                "sources": {
                    "usage": { "total": "42", "daily": [10, 12, 20] },
                    "errors": { "items": [{"count": 2}, {"count": 3}] }
                }
            }),
            "OK",
            "collected",
        );
        context.set_step_output("collect", &output);
        context
    }

    #[tokio::test]
    async fn builds_nested_output_and_aggregates_jsonpath_matches() {
        let mut total = field(
            "summary.requests",
            "$.sources.usage.total",
            TransformDataOperation::Copy,
        );
        total.value_type = Some(TransformDataValueType::Number);
        let step = step(vec![
            total,
            field(
                "summary.errors",
                "$.sources.errors.items[*].count",
                TransformDataOperation::Sum,
            ),
            field(
                "daily",
                "$.sources.usage.daily",
                TransformDataOperation::Copy,
            ),
        ]);
        let outcome = execute_transform_data_step(&step, &context()).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope =
            super::super::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["data"]["summary"]["requests"], 42.0);
        assert_eq!(envelope["data"]["summary"]["errors"], 5.0);
        assert_eq!(envelope["data"]["daily"], serde_json::json!([10, 12, 20]));
    }

    #[tokio::test]
    async fn missing_path_uses_typed_fallback_or_fails_clearly() {
        let mut with_fallback = field(
            "summary.optional",
            "$.sources.missing.value",
            TransformDataOperation::Copy,
        );
        with_fallback.fallback = Some(serde_json::json!(0));
        let outcome = execute_transform_data_step(&step(vec![with_fallback]), &context()).await;
        assert_eq!(outcome.result.status, RunStatus::Success);

        let outcome = execute_transform_data_step(
            &step(vec![field(
                "summary.required",
                "$.sources.missing.value",
                TransformDataOperation::Copy,
            )]),
            &context(),
        )
        .await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("matched no value"));
    }

    #[tokio::test]
    async fn target_collision_is_rejected() {
        let outcome = execute_transform_data_step(
            &step(vec![
                field("summary", "$.sources.usage", TransformDataOperation::Copy),
                field(
                    "summary.total",
                    "$.sources.usage.total",
                    TransformDataOperation::Copy,
                ),
            ]),
            &context(),
        )
        .await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("collision"));
    }

    #[tokio::test]
    async fn transformed_output_remains_typed_for_page_publication() {
        let step = step(vec![field(
            "summary.requests",
            "$.sources.usage.total",
            TransformDataOperation::Copy,
        )]);
        let outcome = execute_transform_data_step(&step, &context()).await;
        let mut downstream = TemplateContext::new();
        downstream.set_step_output("shape", &outcome.result.output);
        assert_eq!(
            downstream.resolve_value("steps.shape.data.summary"),
            Some(serde_json::json!({ "requests": "42" }))
        );
    }
}
