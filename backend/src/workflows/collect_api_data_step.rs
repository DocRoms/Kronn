//! Executor for `StepType::CollectApiData` (0.10.0).
//!
//! Runs independent saved Quick APIs concurrently and exposes their extracted
//! JSON values under stable aliases. It deliberately does not reshape data:
//! `TransformData` owns that concern.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;

use crate::models::{CollectQuickExecOutputFormat, RunStatus, StepResult, StepType, WorkflowStep};

use super::api_call_executor::{ApiCallLogContext, SecurityPolicy};
use super::steps::StepOutcome;
use super::template::{extract_step_envelope, TemplateContext};

const DEFAULT_CONCURRENT_LIMIT: u32 = 5;
const MAX_CONCURRENT_LIMIT: u32 = 20;
const MAX_SOURCES: usize = 50;

pub async fn execute_collect_api_data_step(
    step: &WorkflowStep,
    project_id: Option<&str>,
    state: &crate::AppState,
    context: &TemplateContext,
    log_context: ApiCallLogContext,
    workflow_allowlist: &[String],
    work_dir: &str,
) -> StepOutcome {
    let started = Instant::now();
    let Some(config) = step.collect_api_data.as_ref() else {
        return fail_plain(
            step,
            started,
            "CollectApiData step missing `collect_api_data`",
        );
    };
    if let Err(error) = validate_sources(config) {
        return fail_plain(step, started, error);
    }

    let limit = config
        .concurrent_limit
        .unwrap_or(DEFAULT_CONCURRENT_LIMIT)
        .clamp(1, MAX_CONCURRENT_LIMIT);
    let semaphore = Arc::new(Semaphore::new(limit as usize));
    let mut handles = Vec::with_capacity(config.sources.len());
    // Data collectors do not need a repository. Global workflows therefore
    // get a stable, existing cwd instead of inheriting the backend process cwd.
    let quick_exec_work_dir = if work_dir.trim().is_empty() {
        std::env::temp_dir().to_string_lossy().into_owned()
    } else {
        work_dir.to_string()
    };

    for (index, source) in config.sources.iter().cloned().enumerate() {
        let semaphore = semaphore.clone();
        let state = state.clone();
        let project_id = project_id.map(str::to_owned);
        let mut child_context = context.clone();
        let log_context = log_context.clone();
        let workflow_allowlist = workflow_allowlist.to_vec();
        let work_dir = quick_exec_work_dir.clone();
        handles.push(tokio::spawn(async move {
            let source_kind = if source.quick_exec.is_some() || !source.quick_exec_id.is_empty() {
                "quick_exec"
            } else {
                "quick_api"
            };
            let permit = semaphore.acquire_owned().await;
            if permit.is_err() {
                return SourceResult::failed(
                    index,
                    source.alias,
                    source.required,
                    "collector semaphore closed".to_string(),
                )
                .with_kind(source_kind);
            }
            for (name, template) in &source.variables {
                let rendered = match child_context.render_strict(template) {
                    Ok(value) => value,
                    Err(error) => {
                        return SourceResult::failed(
                            index,
                            source.alias,
                            source.required,
                            format!("variable `{name}`: {error}"),
                        )
                        .with_kind(source_kind)
                    }
                };
                child_context.set(name.clone(), rendered);
            }

            let saved_exec =
                if source.quick_exec.is_none() && !source.quick_exec_id.trim().is_empty() {
                    let quick_exec_id = source.quick_exec_id.clone();
                    match state
                        .db
                        .with_conn(move |conn| {
                            crate::db::quick_execs::get_quick_exec(conn, &quick_exec_id)
                        })
                        .await
                    {
                        Ok(Some(exec)) => Some(exec),
                        Ok(None) => {
                            return SourceResult::failed(
                                index,
                                source.alias,
                                source.required,
                                "Saved Quick Exec not found".to_string(),
                            )
                            .with_kind(source_kind)
                        }
                        Err(error) => {
                            return SourceResult::failed(
                                index,
                                source.alias,
                                source.required,
                                format!("Cannot load saved Quick Exec: {error}"),
                            )
                            .with_kind(source_kind)
                        }
                    }
                } else {
                    None
                };
            let inline_exec = source.quick_exec;
            let exec_config = inline_exec
                .map(|exec| {
                    (
                        exec.command,
                        exec.args,
                        exec.timeout_secs,
                        exec.output_format,
                    )
                })
                .or_else(|| {
                    saved_exec.map(|exec| {
                        (
                            exec.command,
                            exec.args,
                            Some(exec.timeout_secs),
                            exec.output_format,
                        )
                    })
                });

            let (outcome, output_format, quick_exec_identity) =
                if let Some((command, args, timeout_secs, output_format)) = exec_config {
                    let quick_exec_identity = Some((command.clone(), args.clone()));
                    let child_step = WorkflowStep {
                        name: source.alias.clone(),
                        step_type: StepType::Exec,
                        exec_command: Some(command),
                        exec_args: args,
                        exec_timeout_secs: timeout_secs,
                        ..WorkflowStep::default()
                    };
                    (
                        super::exec_step::execute_exec_step_with_output_limit(
                            &child_step,
                            &workflow_allowlist,
                            &work_dir,
                            &child_context,
                            super::exec_step::MAX_COLLECT_OUTPUT_BYTES,
                        )
                        .await,
                        Some(output_format),
                        quick_exec_identity,
                    )
                } else {
                    let child_step = WorkflowStep {
                        name: source.alias.clone(),
                        step_type: StepType::ApiCall,
                        quick_api_id: Some(source.quick_api_id),
                        ..WorkflowStep::default()
                    };
                    (
                        super::api_call_executor::execute_api_call_step_with_db_as(
                            &child_step,
                            project_id.as_deref(),
                            &state,
                            &child_context,
                            SecurityPolicy::production(),
                            log_context,
                        )
                        .await,
                        None,
                        None,
                    )
                };

            if outcome.result.status != RunStatus::Success {
                let error =
                    collector_failure_message(&outcome.result.output, quick_exec_identity.as_ref());
                return SourceResult::failed(index, source.alias, source.required, error)
                    .with_duration(outcome.result.duration_ms)
                    .with_kind(source_kind);
            }
            let Some(envelope) = extract_step_envelope(&outcome.result.output) else {
                return SourceResult::failed(
                    index,
                    source.alias,
                    source.required,
                    "Collector source returned no structured envelope".to_string(),
                )
                .with_kind(source_kind);
            };
            let envelope_value: Value = match serde_json::from_str(&envelope.data_json) {
                Ok(value) => value,
                Err(error) => {
                    return SourceResult::failed(
                        index,
                        source.alias,
                        source.required,
                        format!("Collector source returned invalid JSON: {error}"),
                    )
                    .with_kind(source_kind)
                }
            };
            let value = match output_format {
                Some(format) => match quick_exec_value(&envelope_value, format) {
                    Ok(value) => value,
                    Err(error) => {
                        return SourceResult::failed(index, source.alias, source.required, error)
                            .with_kind(source_kind)
                    }
                },
                None => envelope_value,
            };
            SourceResult {
                index,
                alias: source.alias,
                required: source.required,
                value: Some(value),
                status: envelope.status,
                summary: envelope.summary,
                error: None,
                duration_ms: outcome.result.duration_ms,
                source_kind,
            }
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(result) => results.push(result),
            Err(error) => results.push(SourceResult::failed(
                usize::MAX,
                "unknown".to_string(),
                true,
                format!("collector task failed: {error}"),
            )),
        }
    }
    results.sort_by_key(|result| result.index);

    finish(step, started, results)
}

fn validate_sources(config: &crate::models::CollectApiDataConfig) -> Result<(), String> {
    if config.sources.is_empty() {
        return Err("CollectApiData requires at least one Quick API source".to_string());
    }
    if config.sources.len() > MAX_SOURCES {
        return Err(format!(
            "CollectApiData accepts at most {MAX_SOURCES} sources"
        ));
    }
    let mut aliases = HashSet::new();
    for source in &config.sources {
        let alias = source.alias.trim();
        if alias.is_empty() || !valid_alias(alias) {
            return Err(format!(
                "Invalid collector alias `{}` (use letters, numbers, `_` or `-`)",
                source.alias
            ));
        }
        if !aliases.insert(alias.to_string()) {
            return Err(format!("Duplicate collector alias `{alias}`"));
        }
        let has_quick_api = !source.quick_api_id.trim().is_empty();
        let has_saved_quick_exec = !source.quick_exec_id.trim().is_empty();
        let has_inline_quick_exec = source.quick_exec.is_some();
        if usize::from(has_quick_api)
            + usize::from(has_saved_quick_exec)
            + usize::from(has_inline_quick_exec)
            != 1
        {
            return Err(format!(
                "Collector source `{alias}` must configure exactly one Quick API or Quick Exec"
            ));
        }
        if let Some(exec) = &source.quick_exec {
            if exec.command.trim().is_empty() {
                return Err(format!("Collector Quick Exec `{alias}` has no command"));
            }
            let command = exec.command.trim().to_ascii_lowercase();
            if crate::core::quick_exec::DENIED_BINARIES.contains(&command.as_str()) {
                return Err(format!(
                    "Collector Quick Exec `{alias}` cannot invoke a shell; select the CLI binary directly"
                ));
            }
            if exec.args.len() > 64 {
                return Err(format!(
                    "Collector Quick Exec `{alias}` accepts at most 64 arguments"
                ));
            }
            if matches!(exec.timeout_secs, Some(0 | 1801..)) {
                return Err(format!(
                    "Collector Quick Exec `{alias}` timeout must be between 1 and 1800 seconds"
                ));
            }
        }
    }
    if matches!(config.concurrent_limit, Some(0 | 21..)) {
        return Err("Collector concurrency must be between 1 and 20".to_string());
    }
    Ok(())
}

fn valid_alias(alias: &str) -> bool {
    alias
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

#[derive(Debug)]
struct SourceResult {
    index: usize,
    alias: String,
    required: bool,
    value: Option<Value>,
    status: String,
    summary: String,
    error: Option<String>,
    duration_ms: u64,
    source_kind: &'static str,
}

impl SourceResult {
    fn failed(index: usize, alias: String, required: bool, error: String) -> Self {
        Self {
            index,
            alias,
            required,
            value: None,
            status: "ERROR".to_string(),
            summary: "Data source failed".to_string(),
            error: Some(error),
            duration_ms: 0,
            source_kind: "unknown",
        }
    }

    fn with_kind(mut self, source_kind: &'static str) -> Self {
        self.source_kind = source_kind;
        self
    }

    fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}

fn collector_failure_message(
    output: &str,
    quick_exec_identity: Option<&(String, Vec<String>)>,
) -> String {
    let envelope = extract_step_envelope(output);
    let structured = envelope
        .as_ref()
        .and_then(|envelope| serde_json::from_str::<Value>(&envelope.data_json).ok());
    let stderr = structured
        .as_ref()
        .and_then(|value| value.get("stderr"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let stdout = structured
        .as_ref()
        .and_then(|value| value.get("stdout"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let detail = stderr.or(stdout).unwrap_or_else(|| output.trim());

    if quick_exec_identity.is_some_and(|(command, _)| command.eq_ignore_ascii_case("aws"))
        && (detail.contains("Token has expired and refresh failed")
            || detail.contains("SSO session associated with this profile has expired"))
    {
        let profile = quick_exec_identity.and_then(|(_, args)| {
            args.iter()
                .position(|argument| argument == "--profile")
                .and_then(|index| args.get(index + 1))
                .filter(|value| !value.contains("{{"))
        });
        return match profile {
            Some(profile) => format!(
                "AWS SSO session expired for profile `{profile}`. Run `aws sso login --profile {profile}` on the Kronn host, then retry. AWS CLI: {detail}"
            ),
            None => format!(
                "AWS SSO session expired. Run `aws sso login --profile <profile>` on the Kronn host, then retry. AWS CLI: {detail}"
            ),
        };
    }

    detail.to_string()
}

pub(crate) fn quick_exec_value(
    envelope_data: &Value,
    output_format: CollectQuickExecOutputFormat,
) -> Result<Value, String> {
    let stdout = envelope_data
        .get("stdout")
        .and_then(Value::as_str)
        .ok_or_else(|| "Quick Exec returned no stdout".to_string())?;
    match output_format {
        CollectQuickExecOutputFormat::Json => serde_json::from_str(stdout.trim()).map_err(|error| {
            format!(
                "Quick Exec stdout is not valid JSON: {error}. Configure the CLI for JSON output or select text/lines."
            )
        }),
        CollectQuickExecOutputFormat::Text => Ok(Value::String(stdout.trim_end().to_string())),
        CollectQuickExecOutputFormat::Lines => Ok(Value::Array(
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| Value::String(line.to_string()))
                .collect(),
        )),
        CollectQuickExecOutputFormat::Csv => parse_csv(stdout),
    }
}

fn parse_csv(stdout: &str) -> Result<Value, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(stdout.as_bytes());
    let headers = reader
        .headers()
        .map_err(|error| format!("Quick Exec stdout is not valid CSV: {error}"))?
        .clone();
    if headers.is_empty() {
        return Ok(Value::Array(vec![]));
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        let record =
            record.map_err(|error| format!("Quick Exec stdout is not valid CSV: {error}"))?;
        let mut row = Map::new();
        for (header, value) in headers.iter().zip(record.iter()) {
            row.insert(header.to_string(), Value::String(value.to_string()));
        }
        rows.push(Value::Object(row));
    }
    Ok(Value::Array(rows))
}

fn finish(step: &WorkflowStep, started: Instant, results: Vec<SourceResult>) -> StepOutcome {
    let mut sources = Map::new();
    let mut source_meta = Vec::with_capacity(results.len());
    let mut failed = 0usize;
    let mut required_failed = 0usize;

    let first_failure = results.iter().find_map(|result| {
        result.error.as_ref().map(|error| {
            let concise = error
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(error);
            let concise = concise.trim();
            let concise = if concise.chars().count() > 240 {
                format!("{}…", concise.chars().take(239).collect::<String>())
            } else {
                concise.to_string()
            };
            format!("{}: {concise}", result.alias)
        })
    });

    for result in results {
        if let Some(value) = result.value {
            sources.insert(result.alias.clone(), value);
        } else {
            failed += 1;
            if result.required {
                required_failed += 1;
            }
            sources.insert(result.alias.clone(), Value::Null);
        }
        source_meta.push(json!({
            "alias": result.alias,
            "kind": result.source_kind,
            "required": result.required,
            "status": result.status,
            "summary": result.summary,
            "error": result.error,
            "duration_ms": result.duration_ms,
        }));
    }
    let total = sources.len();
    let succeeded = total.saturating_sub(failed);
    // Optional sources permit a degraded PARTIAL result only when at least one
    // source produced data. A collector that produced nothing is not a
    // successful run: this prevents expired credentials from painting an
    // entirely empty dashboard green.
    let failed_step = required_failed > 0 || (total > 0 && succeeded == 0);
    let status = if failed_step {
        "ERROR"
    } else if failed > 0 {
        "PARTIAL"
    } else {
        "OK"
    };
    let mut summary = format!("Collected {succeeded}/{total} data source(s)");
    if let Some(error) = first_failure {
        summary.push_str(" — ");
        summary.push_str(&error);
    }
    let payload = json!({
        "sources": sources,
        "meta": {
            "collected_at": Utc::now().to_rfc3339(),
            "total": total,
            "succeeded": succeeded,
            "failed": failed,
            "required_failed": required_failed,
            "sources": source_meta,
        }
    });
    outcome(step, started, payload, status, summary, failed_step)
}

fn outcome(
    step: &WorkflowStep,
    started: Instant,
    payload: Value,
    status: &str,
    summary: String,
    failed: bool,
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
            status: if failed {
                RunStatus::Failed
            } else {
                RunStatus::Success
            },
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

fn fail_plain(step: &WorkflowStep, started: Instant, error: impl Into<String>) -> StepOutcome {
    outcome(
        step,
        started,
        json!({ "sources": {}, "meta": { "error": error.into() } }),
        "ERROR",
        "Data collection failed".to_string(),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CollectApiDataConfig, CollectApiDataSource, CollectQuickExecSource};
    use std::collections::HashMap;

    fn source(alias: &str, required: bool) -> CollectApiDataSource {
        CollectApiDataSource {
            alias: alias.to_string(),
            quick_api_id: format!("qa-{alias}"),
            quick_exec_id: String::new(),
            quick_exec: None,
            required,
            variables: HashMap::new(),
        }
    }

    fn exec_source(
        alias: &str,
        output_format: CollectQuickExecOutputFormat,
    ) -> CollectApiDataSource {
        CollectApiDataSource {
            alias: alias.to_string(),
            quick_api_id: String::new(),
            quick_exec_id: String::new(),
            quick_exec: Some(CollectQuickExecSource {
                command: "aws".to_string(),
                args: vec!["cloudwatch".to_string(), "list-metrics".to_string()],
                timeout_secs: Some(60),
                output_format,
            }),
            required: true,
            variables: HashMap::new(),
        }
    }

    #[test]
    fn validation_rejects_duplicate_and_unsafe_aliases() {
        let config = CollectApiDataConfig {
            sources: vec![source("adobe", true), source("adobe", false)],
            concurrent_limit: Some(5),
        };
        assert!(validate_sources(&config).unwrap_err().contains("Duplicate"));

        let config = CollectApiDataConfig {
            sources: vec![source("bad.alias", true)],
            concurrent_limit: Some(5),
        };
        assert!(validate_sources(&config).unwrap_err().contains("Invalid"));
    }

    #[test]
    fn validation_accepts_one_source_kind_and_rejects_ambiguous_sources() {
        let config = CollectApiDataConfig {
            sources: vec![exec_source(
                "cloudwatch",
                CollectQuickExecOutputFormat::Json,
            )],
            concurrent_limit: Some(2),
        };
        assert!(validate_sources(&config).is_ok());

        let mut both = exec_source("ambiguous", CollectQuickExecOutputFormat::Json);
        both.quick_api_id = "qa-also-set".to_string();
        let error = validate_sources(&CollectApiDataConfig {
            sources: vec![both],
            concurrent_limit: Some(2),
        })
        .expect_err("two source kinds must be rejected");
        assert!(error.contains("exactly one"), "got: {error}");

        let mut shell = exec_source("unsafe", CollectQuickExecOutputFormat::Json);
        shell.quick_exec.as_mut().unwrap().command = "bash".to_string();
        let error = validate_sources(&CollectApiDataConfig {
            sources: vec![shell],
            concurrent_limit: Some(1),
        })
        .expect_err("shell binaries must be rejected");
        assert!(error.contains("cannot invoke a shell"), "got: {error}");
    }

    #[test]
    fn quick_exec_stdout_supports_json_text_lines_and_csv() {
        let envelope = json!({ "stdout": "{\"total\":42}\n" });
        assert_eq!(
            quick_exec_value(&envelope, CollectQuickExecOutputFormat::Json).unwrap(),
            json!({ "total": 42 })
        );
        assert_eq!(
            quick_exec_value(
                &json!({ "stdout": "ready\n" }),
                CollectQuickExecOutputFormat::Text
            )
            .unwrap(),
            json!("ready")
        );
        assert_eq!(
            quick_exec_value(
                &json!({ "stdout": "a\n\nb\n" }),
                CollectQuickExecOutputFormat::Lines
            )
            .unwrap(),
            json!(["a", "b"])
        );
        assert_eq!(
            quick_exec_value(
                &json!({ "stdout": "name,total\nParis,12\n\"Lyon, FR\",8\n" }),
                CollectQuickExecOutputFormat::Csv
            )
            .unwrap(),
            json!([
                { "name": "Paris", "total": "12" },
                { "name": "Lyon, FR", "total": "8" }
            ])
        );
        assert!(quick_exec_value(
            &json!({ "stdout": "not-json" }),
            CollectQuickExecOutputFormat::Json
        )
        .unwrap_err()
        .contains("not valid JSON"));
    }

    #[test]
    fn optional_failure_yields_partial_but_successful_step() {
        let step = WorkflowStep {
            name: "collect".into(),
            step_type: StepType::CollectApiData,
            ..WorkflowStep::default()
        };
        let results = vec![
            SourceResult {
                index: 0,
                alias: "usage".into(),
                required: true,
                value: Some(json!({ "total": 42 })),
                status: "OK".into(),
                summary: "ok".into(),
                error: None,
                duration_ms: 12,
                source_kind: "quick_api",
            },
            SourceResult::failed(1, "billing".into(), false, "timeout".into()),
        ];
        let outcome = finish(&step, Instant::now(), results);
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope =
            super::super::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["status"], "PARTIAL");
        assert_eq!(envelope["data"]["sources"]["usage"]["total"], 42);
        assert!(envelope["data"]["sources"]["billing"].is_null());
        assert!(envelope["summary"]
            .as_str()
            .unwrap()
            .contains("billing: timeout"));
    }

    #[test]
    fn all_optional_failures_still_fail_an_empty_collection() {
        let step = WorkflowStep {
            name: "collect".into(),
            step_type: StepType::CollectApiData,
            ..WorkflowStep::default()
        };
        let outcome = finish(
            &step,
            Instant::now(),
            vec![SourceResult::failed(
                0,
                "cloudwatch".into(),
                false,
                "expired token".into(),
            )],
        );
        assert_eq!(outcome.result.status, RunStatus::Failed);
        let envelope =
            super::super::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["status"], "ERROR");
        assert_eq!(envelope["data"]["meta"]["required_failed"], 0);
        assert_eq!(envelope["data"]["meta"]["succeeded"], 0);
    }

    #[test]
    fn aws_sso_failure_prefers_stderr_and_gives_an_actionable_login() {
        let output = r#"exit 255
---STEP_OUTPUT---
{"data":{"exit_code":255,"stdout":"","stderr":"Token has expired and refresh failed"},"status":"ERROR","summary":"exit 255"}
---END_STEP_OUTPUT---"#;
        let identity = (
            "aws".to_string(),
            vec![
                "logs".into(),
                "start-query".into(),
                "--profile".into(),
                "front_prod".into(),
            ],
        );
        let message = collector_failure_message(output, Some(&identity));
        assert!(message.contains("AWS SSO session expired"));
        assert!(message.contains("aws sso login --profile front_prod"));
        assert!(message.contains("Token has expired and refresh failed"));
    }

    #[test]
    fn required_failure_yields_failed_step_with_debuggable_payload() {
        let step = WorkflowStep {
            name: "collect".into(),
            step_type: StepType::CollectApiData,
            ..WorkflowStep::default()
        };
        let outcome = finish(
            &step,
            Instant::now(),
            vec![SourceResult::failed(0, "usage".into(), true, "401".into())],
        );
        assert_eq!(outcome.result.status, RunStatus::Failed);
        let envelope =
            super::super::step_output_format::parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["status"], "ERROR");
        assert_eq!(envelope["data"]["meta"]["required_failed"], 1);
    }
}
