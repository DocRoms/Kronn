//! Executor for `StepType::BatchApiCall` (0.6.0).
//!
//! Mechanical fan-out of an API call over a list of items. Same intent as
//! `BatchQuickPrompt` (one parent step, N children), but the children are
//! HTTP requests instead of LLM runs — **zero tokens consumed**.
//!
//! Typical use case (Feature Planner):
//!   1. Agent step plans 30 sub-tasks → outputs a JSON array of objects.
//!   2. BatchApiCall step fans out POST /issue × 30, each item filling
//!      the body template with its fields. Parallel up to
//!      `batch_concurrent_limit` (default 5).
//!   3. Agent step reads the aggregated outcome and posts blocking links.
//!
//! Per-item templating: each rendered API call has access to
//! `{{batch.item.<field>}}` for every key of its JSON object. Strings and
//! numbers are rendered as-is; nested objects/arrays are JSON-stringified.
//!
//! Output envelope:
//! ```json
//! {
//!   "data": {
//!     "items": [
//!       { "input": {...}, "status": "OK", "response": <extract>, "http_status": 201 },
//!       { "input": {...}, "status": "ERROR", "error": "HTTP 4xx ...", "http_status": 401 }
//!     ],
//!     "total": 30, "succeeded": 28, "failed": 2
//!   },
//!   "status": "OK" | "PARTIAL" | "ERROR",
//!   "summary": "BatchApiCall: 28/30 succeeded"
//! }
//! ```
//!
//! Trailing `[SIGNAL: ...]` lines so workflows can branch:
//! - `[SIGNAL: OK]` if all items succeeded
//! - `[SIGNAL: PARTIAL]` if some failed
//! - `[SIGNAL: ERROR]` if all failed
//!
//! Idempotency is NOT enforced at this level — the upstream planner step
//! is responsible for filtering `items_from` to only include items that
//! actually need creating (list-then-skip-existing). This keeps BatchApiCall
//! a pure mechanical fan-out and keeps idempotency expressible per use case.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde_json::{Map, Value};
use tokio::sync::Semaphore;

use crate::models::*;

use super::steps::StepOutcome;
use super::template::TemplateContext;
use super::api_call_executor::{
    execute_api_call_step_with_db_as, ApiCallLogContext, SecurityPolicy,
};

/// Default concurrent fan-out cap. HTTP can scale higher than agents (no
/// LLM, just network), but providers rate-limit — Jira/GitHub typically
/// safe up to 10-20 in parallel, beyond that 429s land. 5 is conservative,
/// users can override per step.
const DEFAULT_CONCURRENT_LIMIT: u32 = 5;

/// Hard cap on concurrent fan-out. Even if the user sets a higher value,
/// we don't go above this — protects upstream APIs and our own
/// connection pool from accidental DDoS.
const MAX_CONCURRENT_LIMIT: u32 = 20;

/// Default safety cap on items per batch (matches BatchQuickPrompt).
const DEFAULT_MAX_ITEMS: u32 = 50;

pub async fn execute_batch_apicall_step(
    step: &WorkflowStep,
    project_id: Option<&str>,
    state: &crate::AppState,
    ctx: &TemplateContext,
    log_ctx: ApiCallLogContext,
) -> StepOutcome {
    let start = Instant::now();

    // ── Validate base config ────────────────────────────────────────────
    let items_from = match step.batch_items_from.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return fail(step, start, "BatchApiCall step missing `batch_items_from`."),
    };

    // ── Optional QuickApi reference ─────────────────────────────────────
    // Délégué à `quick_api_hydrate::hydrate_step_from_quick_api` pour
    // partager la même règle de per-field override entre BatchApiCall et
    // ApiCall single (cf. quick_api_hydrate.rs).
    let mut step = step.clone();
    if let Err(e) = crate::workflows::quick_api_hydrate::hydrate_step_from_quick_api(
        &mut step,
        &state.db,
    )
    .await
    {
        return fail(&step, start, e);
    }

    if step.api_plugin_slug.is_none() || step.api_config_id.is_none() {
        return fail(&step, start, "BatchApiCall step missing `api_plugin_slug` / `api_config_id`.");
    }
    if step.api_endpoint_path.is_none() {
        return fail(&step, start, "BatchApiCall step missing `api_endpoint_path`.");
    }

    let concurrent_limit = step
        .batch_concurrent_limit
        .unwrap_or(DEFAULT_CONCURRENT_LIMIT)
        .clamp(1, MAX_CONCURRENT_LIMIT);
    let max_items = step.batch_max_items.unwrap_or(DEFAULT_MAX_ITEMS);

    // ── Render items_from + parse JSON array of objects ────────────────
    let rendered = match ctx.render(items_from) {
        Ok(s) => s,
        Err(e) => return fail(&step, start, format!("Template render error on items_from: {e}")),
    };
    let items = match parse_items_as_objects(&rendered) {
        Ok(items) => items,
        Err(e) => return fail(&step, start, format!("Could not parse items_from: {e}")),
    };
    if items.is_empty() {
        return fail(&step, start, "BatchApiCall: items_from resolved to an empty list.");
    }
    if items.len() > max_items as usize {
        return fail(&step, start, format!(
            "BatchApiCall: {} items exceeds max {} (raise `batch_max_items` to allow more).",
            items.len(), max_items
        ));
    }

    tracing::info!(
        target: "kronn::batch_apicall",
        step = %step.name,
        item_count = items.len(),
        concurrent_limit,
        "fan-out start"
    );

    // ── Fan out: parallel HTTP, capped by Semaphore ────────────────────
    let semaphore = Arc::new(Semaphore::new(concurrent_limit as usize));
    let mut handles = Vec::with_capacity(items.len());

    for (idx, item) in items.into_iter().enumerate() {
        let sem = semaphore.clone();
        // Clone the ctx + step + state for each child task — each child
        // sets its own `batch.item.*` keys without polluting the parent.
        let mut child_ctx = ctx.clone();
        inject_item_vars(&mut child_ctx, &item, idx);
        let child_step = step.clone();
        let project_id = project_id.map(String::from);
        let state_clone = state.clone();

        let log_ctx_clone = log_ctx.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            // 0.8.6 (#59) — one api_call_logs row per batch item.
            // The fan-out happens at the executor level, so each spawned
            // task records independently.
            let outcome = execute_api_call_step_with_db_as(
                &child_step,
                project_id.as_deref(),
                &state_clone,
                &child_ctx,
                SecurityPolicy::production(),
                log_ctx_clone,
            ).await;
            (idx, item, outcome)
        }));
    }

    // ── Collect outcomes (preserves item order) ────────────────────────
    let mut item_results: Vec<(usize, Value, ItemOutcome)> = Vec::with_capacity(handles.len());
    for h in handles {
        match h.await {
            Ok((idx, item, outcome)) => {
                let io = ItemOutcome::from_step(&outcome);
                item_results.push((idx, item, io));
            }
            Err(join_err) => {
                tracing::warn!(target: "kronn::batch_apicall", "child task panicked: {join_err}");
                // Synthesize a failure entry for the panicked task. We don't
                // know which item it was (item moved into the spawn) — emit a
                // sentinel so the aggregate still reports the count correctly.
                item_results.push((
                    usize::MAX,
                    Value::Null,
                    ItemOutcome::Failed { error: format!("internal panic: {join_err}"), http_status: None },
                ));
            }
        }
    }
    // Sort by original index so the aggregated `items` array preserves
    // input order — essential for the downstream `set_links` step that
    // needs to correlate "item #3 became key EW-7303".
    item_results.sort_by_key(|(idx, _, _)| *idx);

    // ── Aggregate ──────────────────────────────────────────────────────
    let total = item_results.len();
    let succeeded = item_results.iter().filter(|(_, _, io)| matches!(io, ItemOutcome::Ok { .. })).count();
    let failed = total - succeeded;

    let items_json: Vec<Value> = item_results.iter().map(|(_, input, io)| {
        let mut m = Map::new();
        m.insert("input".into(), input.clone());
        match io {
            ItemOutcome::Ok { response, http_status } => {
                m.insert("status".into(), Value::String("OK".into()));
                m.insert("response".into(), response.clone());
                if let Some(s) = http_status { m.insert("http_status".into(), Value::Number((*s).into())); }
            }
            ItemOutcome::Failed { error, http_status } => {
                m.insert("status".into(), Value::String("ERROR".into()));
                m.insert("error".into(), Value::String(error.clone()));
                if let Some(s) = http_status { m.insert("http_status".into(), Value::Number((*s).into())); }
            }
        }
        Value::Object(m)
    }).collect();

    let aggregate_status = if failed == 0 { "OK" }
        else if succeeded == 0 { "ERROR" }
        else { "PARTIAL" };
    let summary = format!("BatchApiCall: {succeeded}/{total} succeeded ({failed} failed)");
    // 0.8.5 — canonical envelope via shared formatter (markers + signal).
    // Signal name matches `aggregate_status` so `on_result.contains`
    // rules can branch via "OK" / "PARTIAL" / "ERROR" without parsing
    // the JSON. Cf. [[project_step_output_homogenisation_0_9_0]].
    let signal = match aggregate_status {
        "OK" => "OK",
        "PARTIAL" => "PARTIAL",
        _ => "ERROR",
    };
    let output = super::step_output_format::format_step_output(
        serde_json::json!({
            "items": items_json,
            "total": total,
            "succeeded": succeeded,
            "failed": failed,
        }),
        aggregate_status,
        &summary,
        None,
        &[signal],
    );

    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|a| match a {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{}", step_name),
    });

    let run_status = if failed == 0 {
        RunStatus::Success
    } else {
        RunStatus::Failed
    };

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: run_status,
            output,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: Some(true),
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
        },
        condition_action,
    }
}

/// Per-child outcome flattened to what the aggregate envelope needs.
enum ItemOutcome {
    Ok { response: Value, http_status: Option<u64> },
    Failed { error: String, http_status: Option<u64> },
}

impl ItemOutcome {
    fn from_step(outcome: &StepOutcome) -> Self {
        let raw = &outcome.result.output;
        if outcome.result.status == RunStatus::Success {
            // 0.8.5 broke this path silently: children emit the canonical
            // envelope (`summary\n---STEP_OUTPUT---\n{json}\n---END…---\n[SIGNAL:…]`),
            // but this parser still assumed the pre-0.8.5 `{json}\n[SIGNAL:`
            // shape — `from_str` failed on the human prefix and every
            // `items[].response` aggregated as Null (caught by the 2026-06-10
            // executor audit). Parse via the shared envelope extractor, with
            // the legacy split kept as a fallback for old persisted outputs.
            let response = super::template::extract_step_envelope(raw)
                .and_then(|e| serde_json::from_str::<Value>(&e.data_json).ok())
                .or_else(|| {
                    let json_part = raw.split("\n[SIGNAL:").next().unwrap_or(raw);
                    serde_json::from_str::<Value>(json_part)
                        .ok()
                        .and_then(|v| v.get("data").cloned())
                })
                .unwrap_or(Value::Null);
            ItemOutcome::Ok { response, http_status: None }
        } else {
            // Failure path: output starts with "HTTP <status> on <method> <url> — <body>".
            // Try to extract the numeric status for richer downstream branching.
            let http_status = raw.strip_prefix("HTTP ")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|s| s.parse::<u64>().ok());
            ItemOutcome::Failed { error: raw.clone(), http_status }
        }
    }
}

/// Parse the rendered `items_from` expression into a list of JSON values.
///
/// Accepts:
/// 1. JSON array — preferred. Each element can be a primitive or an object.
/// 2. JSON envelope `{ data: [...], status, summary }` — unwraps `data`.
/// 3. JSON object with a single array field — unwraps it.
///
/// Plain-text fallback (newline-separated) is intentionally NOT supported
/// for BatchApiCall: we want structured items so per-field templating
/// works (`{{batch.item.title}}` etc). A user with text wraps in [..] first.
fn parse_items_as_objects(rendered: &str) -> Result<Vec<Value>> {
    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Value = serde_json::from_str(trimmed)
        .map_err(|e| anyhow::anyhow!("expected a JSON array (or envelope): {e}"))?;

    match &parsed {
        Value::Array(arr) => Ok(arr.clone()),
        Value::Object(obj) => {
            // Envelope shape: `{ data: [...], status, summary }`
            if let Some(Value::Array(arr)) = obj.get("data") {
                return Ok(arr.clone());
            }
            // Single-array-field shape: `{ tickets: [...] }`
            for v in obj.values() {
                if let Value::Array(arr) = v {
                    return Ok(arr.clone());
                }
            }
            Err(anyhow::anyhow!("JSON object did not contain an array field; pass an array or `{{ data: [...] }}` envelope"))
        }
        _ => Err(anyhow::anyhow!("expected a JSON array, got {}", value_kind(&parsed))),
    }
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Inject `batch.item.<field>` and `batch.index` template variables for the
/// current child task. Strings/numbers render as-is; nested objects/arrays
/// are JSON-stringified so they can still be referenced from the body
/// template if needed.
///
/// **Object keys are also exposed as bare top-level variables** so that QA
/// templates using `{{host}}` (the user's QA-defined variable name) resolve
/// directly. This is what makes the QA-batch path work: the
/// `/api/quick-apis/:id/batch` handler normalizes strings into objects
/// keyed by the QA's first variable name, and the executor here exposes
/// those keys both as `{{batch.item.X}}` AND `{{X}}` for substitution.
/// The dual exposure is non-breaking for workflows that already use the
/// `batch.item.*` namespace explicitly.
fn inject_item_vars(ctx: &mut TemplateContext, item: &Value, idx: usize) {
    ctx.set("batch.index", idx.to_string());
    match item {
        Value::Object(obj) => {
            for (k, v) in obj {
                let val = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => String::new(),
                    _ => serde_json::to_string(v).unwrap_or_default(),
                };
                // `batch.item.<key>` — explicit/namespaced (workflow steps
                // that pass items_from = a previous step's data array).
                ctx.set(format!("batch.item.{}", k), val.clone());
                // `<key>` — bare top-level (QA-batch path: the QA's
                // `{{host}}` template needs `host` resolved directly).
                ctx.set(k.clone(), val);
            }
            // `batch.item` itself = the JSON-stringified object, useful if
            // the user wants to dump the whole item into a body.
            ctx.set("batch.item", serde_json::to_string(item).unwrap_or_default());
        }
        Value::String(s) => {
            // String item: expose as `batch.item` directly. Users with a
            // simple `["a","b","c"]` items_from get `{{batch.item}}` = "a".
            ctx.set("batch.item", s.clone());
        }
        _ => {
            ctx.set("batch.item", serde_json::to_string(item).unwrap_or_default());
        }
    }
}

fn fail(step: &WorkflowStep, start: Instant, msg: impl Into<String>) -> StepOutcome {
    let msg = msg.into();
    tracing::warn!(
        target: "kronn::batch_apicall",
        step = %step.name,
        "batch API call step failed: {msg}"
    );
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: msg,
            tokens_used: 0,
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
        },
        condition_action: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_items_array_of_objects() {
        let raw = r#"[{"id":1,"title":"a"},{"id":2,"title":"b"}]"#;
        let items = parse_items_as_objects(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], Value::Number(1.into()));
    }

    #[test]
    fn parse_items_envelope_unwraps_data() {
        // Upstream Structured step renders to `{{steps.X}}` = full envelope;
        // we should follow the data field.
        let raw = r#"{"data":[{"id":1}],"status":"OK","summary":"1 item"}"#;
        let items = parse_items_as_objects(raw).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn parse_items_object_with_inner_array_field() {
        // `{ tickets: [...] }` shape.
        let raw = r#"{"tickets":[{"id":1},{"id":2}]}"#;
        let items = parse_items_as_objects(raw).unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn parse_items_empty_string_yields_empty_vec() {
        // Empty render should yield empty list (executor will fail at the
        // empty-list check, not on a parse error — keeps the error message
        // user-actionable).
        let items = parse_items_as_objects("").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn parse_items_rejects_non_array_root() {
        // A plain string is not a valid items_from for BatchApiCall.
        let result = parse_items_as_objects(r#""just a string""#);
        assert!(result.is_err());
    }

    #[test]
    fn inject_item_vars_object_fields_become_dotted_keys() {
        let mut ctx = TemplateContext::new();
        let item = serde_json::json!({ "title": "Foo", "count": 42, "type": "auto_ai" });
        inject_item_vars(&mut ctx, &item, 7);
        assert_eq!(ctx.render("{{batch.item.title}}").unwrap(), "Foo");
        assert_eq!(ctx.render("{{batch.item.count}}").unwrap(), "42");
        assert_eq!(ctx.render("{{batch.item.type}}").unwrap(), "auto_ai");
        assert_eq!(ctx.render("{{batch.index}}").unwrap(), "7");
    }

    #[test]
    fn inject_item_vars_object_fields_also_become_bare_top_level_vars() {
        // Regression guard for the QA-batch path: the QA template uses
        // `{{host}}` (the QA-declared variable name), and the QA-batch
        // handler normalizes string items to `{<var_name>: <value>}`.
        // The executor must expose those keys as bare top-level vars so
        // `{{host}}` resolves at render time.
        let mut ctx = TemplateContext::new();
        let item = serde_json::json!({ "host": "fr.euronews.com" });
        inject_item_vars(&mut ctx, &item, 0);
        assert_eq!(ctx.render("{{host}}").unwrap(), "fr.euronews.com");
        // The `batch.item.host` namespaced form still works (back-compat).
        assert_eq!(ctx.render("{{batch.item.host}}").unwrap(), "fr.euronews.com");
    }

    #[test]
    fn inject_item_vars_nested_objects_jsonify() {
        // Nested objects/arrays must still be templatable — we stringify
        // them so the body author can paste them into a JSON body field.
        let mut ctx = TemplateContext::new();
        let item = serde_json::json!({ "labels": ["bug", "p1"] });
        inject_item_vars(&mut ctx, &item, 0);
        let rendered = ctx.render("{{batch.item.labels}}").unwrap();
        assert!(rendered.contains("\"bug\""));
        assert!(rendered.contains("\"p1\""));
    }

    #[test]
    fn inject_item_vars_string_item_exposes_as_batch_item() {
        // For a simple `["EW-1", "EW-2"]` input, `{{batch.item}}` = "EW-1".
        let mut ctx = TemplateContext::new();
        inject_item_vars(&mut ctx, &Value::String("EW-1".into()), 0);
        assert_eq!(ctx.render("{{batch.item}}").unwrap(), "EW-1");
    }

    #[test]
    fn value_kind_covers_all_serde_variants() {
        assert_eq!(value_kind(&Value::Null), "null");
        assert_eq!(value_kind(&Value::Bool(true)), "bool");
        assert_eq!(value_kind(&Value::Number(1.into())), "number");
        assert_eq!(value_kind(&Value::String("x".into())), "string");
        assert_eq!(value_kind(&Value::Array(vec![])), "array");
        assert_eq!(
            value_kind(&Value::Object(serde_json::Map::new())),
            "object"
        );
    }

    #[test]
    fn parse_items_rejects_number_root() {
        let err = parse_items_as_objects("42").unwrap_err().to_string();
        assert!(err.contains("expected a JSON array"), "got {err}");
        assert!(err.contains("number"), "kind should be reported: {err}");
    }

    #[test]
    fn parse_items_rejects_bool_root() {
        let err = parse_items_as_objects("true").unwrap_err().to_string();
        assert!(err.contains("expected a JSON array"));
        assert!(err.contains("bool"));
    }

    #[test]
    fn parse_items_rejects_null_root() {
        let err = parse_items_as_objects("null").unwrap_err().to_string();
        assert!(err.contains("null"));
    }

    #[test]
    fn parse_items_object_without_any_array_field_errors() {
        // Object with NO array-valued field → fall-through error.
        let raw = r#"{"x": 1, "y": "z"}"#;
        let err = parse_items_as_objects(raw).unwrap_err().to_string();
        assert!(err.contains("did not contain an array field"), "got {err}");
    }

    #[test]
    fn parse_items_invalid_json_reports_parser_error() {
        let err = parse_items_as_objects("{not json").unwrap_err().to_string();
        assert!(err.contains("expected a JSON array (or envelope)"), "got {err}");
    }

    #[test]
    fn parse_items_envelope_data_must_be_array_or_fallback() {
        // `data` is a string (not array) — falls through to "no array field" error.
        let raw = r#"{"data": "not-an-array"}"#;
        let result = parse_items_as_objects(raw);
        // Either it falls through to the no-array error, or it surfaces the bad shape.
        assert!(result.is_err());
    }

    #[test]
    fn parse_items_object_first_array_field_wins() {
        // When multiple array fields exist, the iteration picks one (HashMap order
        // is not stable but at least one IS picked — we just verify no error).
        let raw = r#"{"a":[1,2], "b":[3,4,5]}"#;
        let items = parse_items_as_objects(raw).unwrap();
        assert!(items.len() == 2 || items.len() == 3, "got {items:?}");
    }

    #[test]
    fn inject_item_vars_bool_and_null_field_values_render_safely() {
        let mut ctx = TemplateContext::new();
        let item = serde_json::json!({ "enabled": true, "deleted": null, "n": 5 });
        inject_item_vars(&mut ctx, &item, 0);
        assert_eq!(ctx.render("{{batch.item.enabled}}").unwrap(), "true");
        assert_eq!(ctx.render("{{batch.item.deleted}}").unwrap(), "");
        assert_eq!(ctx.render("{{batch.item.n}}").unwrap(), "5");
    }

    #[test]
    fn inject_item_vars_number_item_jsonifies() {
        // A bare number item must not panic — falls into the `_` arm
        // and gets JSON-stringified into `batch.item`.
        let mut ctx = TemplateContext::new();
        inject_item_vars(&mut ctx, &Value::Number(42.into()), 0);
        assert_eq!(ctx.render("{{batch.item}}").unwrap(), "42");
    }

    #[test]
    fn fail_helper_builds_failed_outcome_with_message() {
        // Smoke test the `fail` helper — it MUST mark the step Failed,
        // copy the step name, and surface our message verbatim.
        let step = WorkflowStep {
            name: "test-step".into(),
            step_type: crate::models::StepType::BatchApiCall,
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let outcome = fail(&step, start, "boom");
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert_eq!(outcome.result.step_name, "test-step");
        assert_eq!(outcome.result.output, "boom");
        assert_eq!(outcome.result.tokens_used, 0);
        assert!(outcome.condition_action.is_none());
    }

    /// Regression (2026-06-10 audit P0): since 0.8.5 the child ApiCall
    /// emits the canonical envelope — the old `{json}\n[SIGNAL:` split
    /// failed to parse it and EVERY successful item aggregated with
    /// `response: Null` (downstream correlation impossible). Pin that the
    /// canonical shape now yields the real `data` value, and that the
    /// legacy pre-0.8.5 shape still parses (fallback).
    #[test]
    fn item_outcome_parses_canonical_envelope_response() {
        use crate::models::StepResult;
        let mk = |output: &str| StepOutcome {
            result: StepResult {
                step_name: "child".into(),
                status: RunStatus::Success,
                output: output.into(),
                tokens_used: 0,
                duration_ms: 1,
                started_at: None,
                condition_result: None,
                envelope_detected: Some(true),
                step_kind: None,
                step_agent: None,
                step_model: None,
                step_api_plugin_slug: None,
                step_api_endpoint_path: None,
                is_rollback: false,
                child_run_id: None,
            },
            condition_action: None,
        };

        // Canonical 0.8.5+ envelope (human prefix + markers + signal).
        let canonical = "POST ok → object\n---STEP_OUTPUT---\n{\"data\":{\"number\":42},\"status\":\"OK\",\"summary\":\"created\"}\n---END_STEP_OUTPUT---\n[SIGNAL: OK]";
        match ItemOutcome::from_step(&mk(canonical)) {
            ItemOutcome::Ok { response, .. } => {
                assert_eq!(response["number"], 42, "canonical envelope must yield real data, got {response}");
            }
            ItemOutcome::Failed { error, .. } => panic!("expected Ok, got Failed: {error}"),
        }

        // Legacy pre-0.8.5 shape still supported via fallback.
        let legacy = "{\"data\":{\"number\":7},\"status\":\"OK\"}\n[SIGNAL: OK]";
        match ItemOutcome::from_step(&mk(legacy)) {
            ItemOutcome::Ok { response, .. } => assert_eq!(response["number"], 7),
            ItemOutcome::Failed { error, .. } => panic!("expected Ok, got Failed: {error}"),
        }
    }
}
