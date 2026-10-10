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
    run_project: Option<&str>,
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

    let literal = step
        .page_publish
        .as_ref()
        .is_some_and(|config| super::run_scope::is_literal(&config.page_id));
    let run_project = run_project.map(str::to_owned);
    let db = state.db.clone();
    let page_for_db = page_id.clone();
    let result = match db
        .with_conn(move |conn| {
            let projects = super::run_scope::page_projects(conn, &page_for_db)?;
            if projects.iter().any(|project| {
                !super::run_scope::target_allowed(
                    project.as_deref(),
                    run_project.as_deref(),
                    literal,
                )
            }) {
                anyhow::bail!("Page '{page_for_db}' is outside this run's project");
            }
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
        let value = if from_exec(key, &value, context) {
            exec_value(write.dataset.trim(), key, value, context)?
        } else {
            value
        };
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

/// Whether a value is Exec output, from what the run actually recorded: the
/// producing step's type, or, when it is unknown, the Exec envelope shape.
fn from_exec(key: &str, value: &serde_json::Value, context: &TemplateContext) -> bool {
    let path = key.split_once("??").map_or(key, |(path, _)| path).trim();
    if let Some(kind) = context.producer_kind(path) {
        return kind == "Exec";
    }
    match value {
        serde_json::Value::Object(_) => is_exec_data(value),
        _ => path
            .rsplit_once('.')
            .and_then(|(parent, _)| context.resolve_value(parent))
            .is_some_and(|parent| is_exec_data(&parent)),
    }
}

/// The `data` of an Exec envelope: its four keys and the optional flags.
fn is_exec_data(value: &serde_json::Value) -> bool {
    const KEYS: [&str; 4] = ["exit_code", "stdout", "stderr", "duration_ms"];
    const FLAGS: [&str; 2] = ["stdout_truncated", "stderr_truncated"];
    value.as_object().is_some_and(|map| {
        KEYS.iter().all(|key| map.contains_key(*key))
            && map
                .keys()
                .all(|key| KEYS.contains(&key.as_str()) || FLAGS.contains(&key.as_str()))
    })
}

/// Refuses a value an Exec step truncated, and parses an Exec stdout that
/// holds JSON so the page gets the document itself, not a string of it.
fn exec_value(
    dataset: &str,
    key: &str,
    value: serde_json::Value,
    context: &TemplateContext,
) -> Result<serde_json::Value> {
    let path = key.split_once("??").map_or(key, |(path, _)| path).trim();
    let flagged = |flag: &str| {
        context
            .resolve_value(flag)
            .is_some_and(|value| value == serde_json::Value::Bool(true) || value == "true")
    };
    let truncated = match &value {
        serde_json::Value::String(text) => {
            text.contains(super::exec_step::TRUNCATION_MARKER_PREFIX)
                || flagged(&format!("{path}_truncated"))
        }
        serde_json::Value::Object(map) => ["stdout_truncated", "stderr_truncated"]
            .iter()
            .any(|flag| map.get(*flag) == Some(&serde_json::Value::Bool(true))),
        _ => false,
    };
    if truncated {
        bail!(
            "Dataset '{dataset}': `{path}` was truncated by its Exec step (limit {} MiB when a \
             PublishPageData step reads it); not published, the page keeps its previous data. \
             Publish from the script (POST /api/pages/{{id}}/publish) or reduce the output",
            super::exec_step::MAX_PUBLISH_OUTPUT_BYTES / (1024 * 1024)
        );
    }
    let serde_json::Value::String(text) = &value else {
        return Ok(value);
    };
    let trimmed = text.trim();
    if !path.ends_with(".stdout") || !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return Ok(value);
    }
    serde_json::from_str(trimmed).map_err(|error| {
        anyhow!(
            "Dataset '{dataset}': `{path}` looks like JSON but does not parse ({error}); \
             not published, the page keeps its previous data"
        )
    })
}

pub(crate) fn typed_source_key(source: &str) -> Result<&str> {
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
            tokens_used: Some(0),
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
            agent_provenance: None,
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

fn fail(step: &WorkflowStep, started: Instant, error: impl std::fmt::Display) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: error.to_string(),
            tokens_used: Some(0),
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
            agent_provenance: None,
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
    fn provenance_can_be_published_as_a_typed_value() {
        use crate::models::{
            AgentType, ModelTier, WorkflowAgentAttempt, WorkflowAgentAttemptRole,
            WorkflowAgentProvenance,
        };
        let mut context = TemplateContext::new();
        context.set_step_provenance(
            "advise",
            Some(&WorkflowAgentProvenance {
                attempts: vec![WorkflowAgentAttempt {
                    id: 1,
                    role: WorkflowAgentAttemptRole::Initial,
                    retry: 1,
                    agent: AgentType::LiteLlm,
                    tier: ModelTier::Default,
                    connection_id: None,
                    requested_model: None,
                    resolved_model: Some("claude-sonnet-4-6".into()),
                    preflight_warning: None,
                    model_applied: None,
                    observed_models: vec![],
                    format_fallback: false,
                    npx_fallback_command: None,
                    npx_fallback_version: None,
                    started_at: chrono::Utc::now(),
                    duration_ms: 1,
                    succeeded: true,
                    cached_prompt_tokens: None,
                    cache_write_prompt_tokens: None,
                    session_id: None,
                    cost_usd: None,
                    cost_unknown_reason: None,
                }],
                selected_attempt: Some(1),
            }),
        );
        let request = build_request(
            &step("steps.advise.provenance"),
            "workflow-1",
            "run-7",
            &context,
        )
        .expect("typed provenance");
        assert_eq!(request.writes[0].value["agent"], "LiteLlm");
        assert_eq!(request.writes[0].value["model"], "claude-sonnet-4-6");
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

    /// B4-01 — a rendered page id or slug outside the run's project is
    /// refused before anything is written.
    #[tokio::test]
    async fn a_rendered_page_outside_the_run_s_project_is_refused() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let now = "2026-01-01T00:00:00Z";
            for (id, project) in [("p", "p"), ("q", "q")] {
                conn.execute(
                    "INSERT INTO projects(id, name, path, created_at, updated_at) \
                     VALUES (?1, ?1, ?1, ?2, ?2)",
                    rusqlite::params![id, now],
                )?;
                conn.execute(
                    "INSERT INTO live_pages(id, project_id, title, slug, created_at, updated_at) \
                     VALUES (?1, ?2, ?1, ?3, ?4, ?4)",
                    rusqlite::params![format!("page-{id}"), project, format!("slug-{id}"), now],
                )?;
            }
            conn.execute(
                "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at) \
                 VALUES ('wf', 'wf', '{}', '[]', ?1, ?1)",
                [now],
            )?;
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, status, started_at) \
                 VALUES ('run', 'wf', 'Running', ?1)",
                [now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let state = crate::AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            std::sync::Arc::new(db),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let mut publish = step("steps.shape.data");
        publish.page_publish.as_mut().unwrap().page_id = "{{page}}".into();
        let mut context = TemplateContext::new();
        context.set_step_output(
            "shape",
            &super::super::step_output_format::format_step_output_simple(
                serde_json::json!({"value": 1}),
                "OK",
                "shaped",
            ),
        );
        for page in ["page-q", "slug-q"] {
            context.set("page", page);
            let outcome =
                execute_publish_page_data_step(&publish, "wf", "run", Some("p"), &state, &context)
                    .await;
            assert_eq!(outcome.result.status, RunStatus::Failed, "{page}");
            assert!(
                outcome.result.output.contains("outside this run's project"),
                "{page}: {}",
                outcome.result.output
            );
        }
        let revision: i64 = state
            .db
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT data_revision FROM live_pages WHERE id = 'page-q'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(revision, 0, "q's page is untouched");
        // A literal page id is the workflow author's choice, whatever its
        // project: it still publishes.
        state
            .db
            .with_conn(|conn| {
                crate::db::live_pages::add_live_page_dataset(
                    conn,
                    "page-q",
                    &crate::models::CreateLivePageDataset {
                        name: "latency".into(),
                        kind: crate::models::LivePageDatasetKind::TimeSeries,
                        initial: None,
                        schema: None,
                        max_points: None,
                        max_age_days: None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let mut literal = publish.clone();
        literal.page_publish.as_mut().unwrap().page_id = "page-q".into();
        let published =
            execute_publish_page_data_step(&literal, "wf", "run", Some("p"), &state, &context)
                .await;
        assert_eq!(
            published.result.status,
            RunStatus::Success,
            "{}",
            published.result.output
        );
        context.set("page", "page-p");
        let own =
            execute_publish_page_data_step(&publish, "wf", "run", Some("p"), &state, &context)
                .await;
        assert!(
            !own.result.output.contains("outside this run's project"),
            "{}",
            own.result.output
        );
    }

    /// A page with one snapshot dataset `errors` holding `{"previous": true}`.
    async fn page_with_snapshot() -> crate::AppState {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let now = "2026-01-01T00:00:00Z";
            conn.execute(
                "INSERT INTO live_pages(id, title, slug, created_at, updated_at) \
                 VALUES ('page-e', 'Errors', 'errors', ?1, ?1)",
                [now],
            )?;
            conn.execute(
                "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at) \
                 VALUES ('wf', 'wf', '{}', '[]', ?1, ?1)",
                [now],
            )?;
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, status, started_at) \
                 VALUES ('run', 'wf', 'Running', ?1)",
                [now],
            )?;
            crate::db::live_pages::add_live_page_dataset(
                conn,
                "page-e",
                &crate::models::CreateLivePageDataset {
                    name: "errors".into(),
                    kind: crate::models::LivePageDatasetKind::Snapshot,
                    initial: Some(serde_json::json!({"previous": true})),
                    schema: None,
                    max_points: None,
                    max_age_days: None,
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
        crate::AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            std::sync::Arc::new(db),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        )
    }

    async fn current_errors(state: &crate::AppState) -> serde_json::Value {
        state
            .db
            .with_read_conn(|conn| {
                let raw: String = conn.query_row(
                    "SELECT current_json FROM live_page_datasets WHERE name = 'errors'",
                    [],
                    |row| row.get(0),
                )?;
                Ok(serde_json::from_str(&raw)?)
            })
            .await
            .unwrap()
    }

    fn agrege_exec(stdout: &str) -> WorkflowStep {
        WorkflowStep {
            name: "agrege".into(),
            step_type: StepType::Exec,
            exec_command: Some("cat".into()),
            exec_stdin: Some(stdout.into()),
            exec_timeout_secs: Some(30),
            ..WorkflowStep::default()
        }
    }

    fn publish_stdout() -> WorkflowStep {
        publish_from("steps.agrege.data.stdout")
    }

    fn publish_from(value_from: &str) -> WorkflowStep {
        let mut publish = step(value_from);
        let config = publish.page_publish.as_mut().unwrap();
        config.page_id = "page-e".into();
        config.writes[0].dataset = "errors".into();
        config.writes[0].operation = LivePageWriteOperation::Replace;
        publish
    }

    /// Runs the Exec step with the limit the runner would give it, then the
    /// publish step reading its stdout.
    async fn exec_then_publish(
        state: &crate::AppState,
        stdout: &str,
        limit: Option<usize>,
    ) -> StepOutcome {
        exec_then_publish_from(state, stdout, limit, "steps.agrege.data.stdout").await
    }

    async fn exec_then_publish_from(
        state: &crate::AppState,
        stdout: &str,
        limit: Option<usize>,
        value_from: &str,
    ) -> StepOutcome {
        let exec = agrege_exec(stdout);
        let publish = publish_from(value_from);
        let limit = limit.unwrap_or_else(|| {
            super::super::exec_step::output_limit_for(&exec, &[exec.clone(), publish.clone()], &[])
        });
        let mut context = TemplateContext::new();
        run_recorded(&exec, limit, &mut context).await;
        execute_publish_page_data_step(&publish, "wf", "run", None, state, &context).await
    }

    /// Runs an Exec step and records it the way the runner does.
    async fn run_recorded(
        exec: &WorkflowStep,
        limit: usize,
        context: &mut TemplateContext,
    ) -> StepResult {
        let dir = tempfile::tempdir().unwrap();
        let mut ran = super::super::exec_step::execute_exec_step_with_output_limit(
            exec,
            &["cat".into()],
            dir.path().to_str().unwrap(),
            &TemplateContext::new(),
            limit,
            None,
        )
        .await
        .result;
        assert_eq!(ran.status, RunStatus::Success, "{}", ran.output);
        super::super::runner::record_step_completion(exec, &mut ran, context, None);
        ran
    }

    /// A JsonData result recorded the way the runner does.
    fn record_json(
        name: &str,
        data: serde_json::Value,
        context: &mut TemplateContext,
    ) -> StepResult {
        let output =
            super::super::step_output_format::format_step_output_simple(data, "OK", "data");
        record_output(name, StepType::JsonData, output, context)
    }

    /// A step result with this output, recorded the way the runner does.
    fn record_output(
        name: &str,
        step_type: StepType,
        output: String,
        context: &mut TemplateContext,
    ) -> StepResult {
        let step = WorkflowStep {
            name: name.into(),
            step_type,
            ..WorkflowStep::default()
        };
        let mut result = StepResult {
            step_name: name.into(),
            status: RunStatus::Success,
            output,
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
            agent_provenance: None,
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
        };
        super::super::runner::record_step_completion(&step, &mut result, context, None);
        result
    }

    fn errors_document(min_bytes: usize) -> serde_json::Value {
        let mut items = Vec::new();
        let mut size = 0;
        while size < min_bytes {
            let item = serde_json::json!({"url": format!("/article/{}", items.len()), "status": 503, "count": 7});
            size += item.to_string().len() + 1;
            items.push(item);
        }
        serde_json::json!({"generated_at": "2026-10-07T13:00:00Z", "items": items})
    }

    /// KT-1105 — the 2026-10-07 case: a 191 KB JSON stdout cut at 100 KB used
    /// to land on the page as a string; it is now refused and the page kept.
    #[tokio::test]
    async fn a_truncated_exec_stdout_is_refused_and_the_page_keeps_its_data() {
        let state = page_with_snapshot().await;
        let document = errors_document(191 * 1024).to_string();
        let outcome = exec_then_publish(&state, &document, Some(100 * 1024)).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("was truncated")
                && outcome.result.output.contains("keeps its previous data"),
            "{}",
            outcome.result.output
        );
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    /// The same 191 KB stdout, with the limit the runner gives a step a
    /// publish reads, lands intact and as a JSON object.
    #[tokio::test]
    async fn a_large_exec_stdout_read_by_a_publish_lands_intact_as_json() {
        let state = page_with_snapshot().await;
        let document = errors_document(191 * 1024);
        let outcome = exec_then_publish(&state, &document.to_string(), None).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "{}",
            outcome.result.output
        );
        assert_eq!(current_errors(&state).await, document);
    }

    #[tokio::test]
    async fn a_stdout_beyond_the_publish_limit_is_refused_not_cut() {
        let state = page_with_snapshot().await;
        let document = errors_document(super::super::exec_step::MAX_PUBLISH_OUTPUT_BYTES + 1024);
        let outcome = exec_then_publish(&state, &document.to_string(), None).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("limit 2 MiB"),
            "{}",
            outcome.result.output
        );
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    #[tokio::test]
    async fn an_invalid_json_stdout_is_refused_and_the_page_keeps_its_data() {
        let state = page_with_snapshot().await;
        let outcome = exec_then_publish(&state, "{\"items\": [1, 2", None).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("does not parse"),
            "{}",
            outcome.result.output
        );
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    /// A stored output from before the flag existed still carries the marker.
    #[test]
    fn a_marker_without_the_flag_is_still_refused() {
        let mut context = TemplateContext::new();
        context.set_step_output(
            "agrege",
            &super::super::step_output_format::format_step_output_simple(
                serde_json::json!({
                    "exit_code": 0,
                    "stdout": "{\"items\": [1\n\n[... output tronqué — limite 100 KB ...]",
                }),
                "OK",
                "exit 0",
            ),
        );
        context.set_step_kind("agrege", Some("Exec"));
        let error = build_request(&publish_stdout(), "wf", "run", &context)
            .expect_err("a truncated stdout is never published");
        assert!(error.to_string().contains("was truncated"), "{error}");
    }

    #[test]
    fn a_plain_text_stdout_is_still_published_as_text() {
        let mut context = TemplateContext::new();
        context.set_step_output(
            "agrege",
            &super::super::step_output_format::format_step_output_simple(
                serde_json::json!({"exit_code": 0, "stdout": "all green\n"}),
                "OK",
                "exit 0",
            ),
        );
        context.set_step_kind("agrege", Some("Exec"));
        let request = build_request(&publish_stdout(), "wf", "run", &context).unwrap();
        assert_eq!(request.writes[0].value, "all green\n");
    }

    /// The `previous_step` alias, bare or templated, gets the same 2 MiB limit
    /// and lands intact.
    #[tokio::test]
    async fn a_large_stdout_read_as_previous_step_lands_intact() {
        let document = errors_document(191 * 1024);
        for value_from in ["previous_step.data.stdout", "{{previous_step.data.stdout}}"] {
            let state = page_with_snapshot().await;
            let outcome =
                exec_then_publish_from(&state, &document.to_string(), None, value_from).await;
            assert_eq!(
                outcome.result.status,
                RunStatus::Success,
                "{value_from}: {}",
                outcome.result.output
            );
            assert_eq!(current_errors(&state).await, document, "{value_from}");
        }
    }

    /// The sink is general: a business object carrying fields named like the
    /// Exec flags publishes as is, whether its producer is recorded or not.
    #[test]
    fn exec_flags_on_a_non_exec_object_are_plain_data() {
        let business = serde_json::json!({
            "stdout": "{\"cut",
            "stdout_truncated": true,
            "stderr_truncated": true,
        });
        for recorded in [true, false] {
            let mut context = TemplateContext::new();
            if recorded {
                record_json("shape", business.clone(), &mut context);
            } else {
                context.set_step_output(
                    "shape",
                    &super::super::step_output_format::format_step_output_simple(
                        business.clone(),
                        "OK",
                        "shaped",
                    ),
                );
            }
            for value_from in [
                "steps.shape.data",
                "steps.shape.data.stdout",
                "previous_step.data",
            ] {
                let request = build_request(&publish_from(value_from), "wf", "run", &context)
                    .unwrap_or_else(|error| panic!("{value_from} ({recorded}): {error}"));
                let expected = if value_from.ends_with(".stdout") {
                    business["stdout"].clone()
                } else {
                    business.clone()
                };
                assert_eq!(
                    request.writes[0].value, expected,
                    "{value_from} ({recorded})"
                );
            }
        }
    }

    /// A Goto loop: list order says `previous_step` is the JsonData step, but
    /// the step that really ran last is a truncated Exec; the publish refuses.
    #[tokio::test]
    async fn previous_step_follows_the_run_not_the_list() {
        let state = page_with_snapshot().await;
        let exec = agrege_exec(&errors_document(191 * 1024).to_string());
        let publish = publish_from("previous_step.data.stdout");
        // Listed order: agrege, shape, publish (agrege loops back via Goto).
        let shape = WorkflowStep {
            name: "shape".into(),
            step_type: StepType::JsonData,
            ..WorkflowStep::default()
        };
        let listed = [exec.clone(), shape, publish.clone()];
        let limit = super::super::exec_step::output_limit_for(&exec, &listed, &[]);
        assert_eq!(limit, 100 * 1024, "list order does not predict the jump");
        let mut context = TemplateContext::new();
        record_json("shape", serde_json::json!({"stdout": "fine"}), &mut context);
        run_recorded(&exec, limit, &mut context).await;
        let outcome =
            execute_publish_page_data_step(&publish, "wf", "run", None, &state, &context).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("was truncated"),
            "{}",
            outcome.result.output
        );
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    /// A producer the context cannot name fails closed on the Exec shape.
    #[tokio::test]
    async fn an_unrecorded_exec_shape_is_still_checked() {
        let state = page_with_snapshot().await;
        let exec = agrege_exec(&errors_document(191 * 1024).to_string());
        let mut recorded = TemplateContext::new();
        let result = run_recorded(&exec, 100 * 1024, &mut recorded).await;
        let mut context = TemplateContext::new();
        context.set_step_output("agrege", &result.output);
        for value_from in ["steps.agrege.data.stdout", "previous_step.data.stdout"] {
            let outcome = execute_publish_page_data_step(
                &publish_from(value_from),
                "wf",
                "run",
                None,
                &state,
                &context,
            )
            .await;
            assert_eq!(outcome.result.status, RunStatus::Failed, "{value_from}");
        }
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    /// A resumed run replays its stored results: the provenance survives.
    #[tokio::test]
    async fn a_resumed_run_keeps_the_provenance() {
        let state = page_with_snapshot().await;
        let exec = agrege_exec(&errors_document(191 * 1024).to_string());
        let mut live = TemplateContext::new();
        let shape = record_json("shape", serde_json::json!({"stdout": "fine"}), &mut live);
        let ran = run_recorded(&exec, 100 * 1024, &mut live).await;
        let stored: Vec<StepResult> =
            serde_json::from_str(&serde_json::to_string(&vec![shape, ran]).unwrap()).unwrap();
        let mut resumed = TemplateContext::new();
        super::super::runner::replay_prior_results(&mut resumed, &stored);
        assert_eq!(
            resumed.producer_kind("previous_step.data.stdout"),
            Some("Exec")
        );
        assert_eq!(resumed.producer_kind("steps.shape.data"), Some("JsonData"));
        for value_from in ["previous_step.data.stdout", "steps.agrege.data.stdout"] {
            let outcome = execute_publish_page_data_step(
                &publish_from(value_from),
                "wf",
                "run",
                None,
                &state,
                &resumed,
            )
            .await;
            assert_eq!(outcome.result.status, RunStatus::Failed, "{value_from}");
        }
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    /// A truncated Exec, then an Agent answering in plain text: `previous_step`
    /// no longer holds the Exec's data, live or after a resume.
    #[tokio::test]
    async fn a_plain_text_step_after_a_truncated_exec_publishes_nothing() {
        let state = page_with_snapshot().await;
        let exec = agrege_exec(&errors_document(191 * 1024).to_string());
        let mut live = TemplateContext::new();
        let ran = run_recorded(&exec, 100 * 1024, &mut live).await;
        let said = record_output(
            "advise",
            StepType::Agent,
            "Looks fine to me.".into(),
            &mut live,
        );
        let stored: Vec<StepResult> =
            serde_json::from_str(&serde_json::to_string(&vec![ran, said]).unwrap()).unwrap();
        let mut resumed = TemplateContext::new();
        super::super::runner::replay_prior_results(&mut resumed, &stored);
        for (label, context) in [("live", &live), ("resumed", &resumed)] {
            let outcome = execute_publish_page_data_step(
                &publish_from("previous_step.data.stdout"),
                "wf",
                "run",
                None,
                &state,
                context,
            )
            .await;
            assert_eq!(outcome.result.status, RunStatus::Failed, "{label}");
            assert!(
                outcome
                    .result
                    .output
                    .contains("Unknown typed Page data source"),
                "{label}: {}",
                outcome.result.output
            );
        }
        assert_eq!(
            current_errors(&state).await,
            serde_json::json!({"previous": true})
        );
    }

    /// A step run again without an envelope drops its earlier structured data.
    #[test]
    fn a_rerun_without_an_envelope_drops_the_step_s_old_data() {
        let mut context = TemplateContext::new();
        record_json("shape", serde_json::json!({"rows": [1]}), &mut context);
        record_output("shape", StepType::Agent, "plain".into(), &mut context);
        assert!(context.resolve_value("steps.shape.data").is_none());
        assert!(context.resolve_value("previous_step.data").is_none());
        let error = build_request(&publish_from("steps.shape.data"), "wf", "run", &context)
            .expect_err("no stale data is published");
        assert!(
            error.to_string().contains("Unknown typed Page data source"),
            "{error}"
        );
    }
}
