//! Executor for `StepType::Notify` (0.3.5).
//!
//! Direct HTTP webhook — zero agent tokens. Typical use cases:
//! - send a Slack/Teams/Discord notification when a workflow completes
//! - trigger a downstream pipeline via a custom webhook
//! - create a ticket in an external tracker from the aggregated output
//!
//! Design:
//! - URL + body are rendered via the template engine just like agent
//!   prompts (so `{{steps.audit.summary}}` etc. work).
//! - Supported methods: POST, PUT, GET. GET skips the body.
//! - 30 s timeout. Status 2xx = Success, else Failed with the response
//!   excerpt (first 512 bytes) as output so the operator can debug.
//! - Structured output: `{data: {http_status, response_excerpt}, status, summary}`.

use std::time::{Duration, Instant};

use crate::models::*;

use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Per-request timeout. Matches the default agent stall ceiling roughly —
/// notification endpoints should answer in seconds, not minutes.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// Size of the response snippet recorded in the step output (bytes).
const RESPONSE_EXCERPT_LIMIT: usize = 512;

pub async fn execute_notify_step(step: &WorkflowStep, ctx: &TemplateContext) -> StepOutcome {
    execute_notify_step_with_policy(step, ctx, true).await
}

/// `enforce_public_ip = false` is for tests hitting a local wiremock —
/// mirrors `SecurityPolicy::allow_loopback_for_tests` on the ApiCall side.
pub async fn execute_notify_step_with_policy(
    step: &WorkflowStep,
    ctx: &TemplateContext,
    enforce_public_ip: bool,
) -> StepOutcome {
    let start = Instant::now();

    // ── Validate + extract config ───────────────────────────────────────
    let config = match step.notify_config.as_ref() {
        Some(c) => c,
        None => return fail(step, start, "Notify step missing `notify_config`"),
    };
    if config.url.is_empty() {
        return fail(step, start, "Notify step: `url` is empty");
    }

    let method_upper = config.method.to_uppercase();
    let method = match method_upper.as_str() {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "GET" => reqwest::Method::GET,
        other => {
            return fail(
                step,
                start,
                format!(
                    "Notify step: unsupported method `{}` (allowed: POST, PUT, GET)",
                    other
                ),
            )
        }
    };

    // ── Render URL + body via the template engine ───────────────────────
    let url = match ctx.render(&config.url) {
        Ok(u) => u,
        Err(e) => return fail(step, start, format!("Template render error on url: {}", e)),
    };
    let body = match ctx.render(&config.body_template) {
        Ok(b) => b,
        Err(e) => return fail(step, start, format!("Template render error on body: {}", e)),
    };

    // ── SSRF guard (2026-06-10 audit P1) ────────────────────────────────
    // The URL is templated — a value coming from an upstream step's output
    // could point the webhook at localhost / private ranges (cloud metadata,
    // the Kronn backend itself, …). ApiCall has had this guard since 0.6;
    // Notify fired unchecked. Same public-IP assertion, same loud failure.
    let parsed_url = match reqwest::Url::parse(&url) {
        Ok(u) => u,
        Err(e) => {
            return fail(
                step,
                start,
                format!("Notify: invalid URL after templating: {e}"),
            )
        }
    };
    if enforce_public_ip {
        if let Err(e) = super::api_call_security::assert_public_ip(&parsed_url).await {
            return fail(step, start, format!("Security: {e}"));
        }
    }
    // Redacted form for EVERYTHING we persist (output, summary). Webhook
    // secrets often live in the PATH (hooks.slack.com/services/T…/B…/xxx),
    // so we keep scheme + host + first path segment only.
    let redacted_url = redact_notify_url(&parsed_url);

    // ── Build and fire the request ──────────────────────────────────────
    let client = match reqwest::Client::builder().timeout(NOTIFY_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return fail(step, start, format!("HTTP client build failed: {}", e)),
    };

    let mut req = client.request(method.clone(), &url);
    for (k, v) in &config.headers {
        req = req.header(k, v);
    }
    if method != reqwest::Method::GET && !body.is_empty() {
        req = req.body(body.clone());
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => return fail(step, start, format!("HTTP request failed: {}", e)),
    };

    let status = response.status();
    let excerpt = match response.bytes().await {
        Ok(bytes) => {
            let take = bytes.len().min(RESPONSE_EXCERPT_LIMIT);
            String::from_utf8_lossy(&bytes[..take]).to_string()
        }
        Err(_) => String::new(),
    };

    let http_status = status.as_u16();
    let is_success = status.is_success();

    // ── Build structured output so downstream steps can chain on data ───
    // 0.8.5 — canonical envelope via shared formatter. Always emits
    // `[SIGNAL: OK|ERROR]` so `on_result` rules can branch on the
    // delivery success without parsing the JSON. Cf.
    // [[project_step_output_homogenisation_0_9_0]].
    let status_str = if is_success { "OK" } else { "ERROR" };
    // Persist only the REDACTED url (audit P1: full webhook URLs — Slack
    // tokens live in the path — used to land verbatim in run outputs).
    let summary = format!("{} {} → {}", method.as_str(), redacted_url, http_status);
    let output = super::step_output_format::format_step_output(
        serde_json::json!({
            "http_status": http_status,
            "response_excerpt": excerpt,
            "url": redacted_url,
            "method": method.as_str(),
        }),
        status_str,
        &summary,
        None,
        &[status_str],
    );

    // 2026-06-10 audit P1 — the runner only honours `outcome.condition_action`;
    // returning None here meant `on_result` rules on Notify steps were
    // silently dead: a Slack 5xx with a declared `ERROR → Skip` recovery
    // still tipped the whole run into rollback. Evaluate like ApiCall does.
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(condition_label);
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: if is_success {
                RunStatus::Success
            } else {
                RunStatus::Failed
            },
            output,
            tokens_used: 0,
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
        },
        condition_action,
    }
}

/// Redact a webhook URL for persistence: scheme + host + FIRST path
/// segment only. Unlike API base URLs, webhook secrets commonly live in
/// the path itself (`hooks.slack.com/services/T…/B…/secret`) — query-only
/// redaction would still leak them into run outputs and logs.
fn redact_notify_url(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or("?");
    let first_seg = url
        .path_segments()
        .and_then(|mut s| s.next())
        .filter(|s| !s.is_empty());
    match first_seg {
        Some(seg) => format!("{}://{}/{}/…", url.scheme(), host, seg),
        None => format!("{}://{}/", url.scheme(), host),
    }
}

/// Human label for a fired condition — same convention as the ApiCall
/// executor (`Stop` / `Skip` / `Goto:<target>`).
fn condition_label(a: &crate::models::ConditionAction) -> String {
    use crate::models::ConditionAction;
    match a {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    }
}

fn fail(step: &WorkflowStep, start: Instant, msg: impl Into<String>) -> StepOutcome {
    let output: String = msg.into();
    // Same fix as the success path: config/transport failures must still
    // honour declared `on_result` recovery rules.
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(condition_label);
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output,
            tokens_used: 0,
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
        },
        condition_action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_step(config: NotifyConfig) -> WorkflowStep {
        WorkflowStep {
            name: "notify".into(),
            step_type: StepType::Notify,
            description: None,
            agent: AgentType::ClaudeCode, // ignored for Notify, default for serialization
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText,
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
            notify_config: Some(config),
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
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
        }
    }

    #[tokio::test]
    async fn notify_rejects_missing_config() {
        let mut step = make_step(NotifyConfig {
            url: "http://x".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: String::new(),
        });
        step.notify_config = None;
        let ctx = TemplateContext::new();
        let out = execute_notify_step_with_policy(&step, &ctx, false).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(out.result.output.contains("missing `notify_config`"));
    }

    #[tokio::test]
    async fn notify_rejects_empty_url() {
        let step = make_step(NotifyConfig {
            url: "".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: String::new(),
        });
        let ctx = TemplateContext::new();
        let out = execute_notify_step_with_policy(&step, &ctx, false).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(out.result.output.contains("`url` is empty"));
    }

    #[tokio::test]
    async fn notify_rejects_unsupported_method() {
        let step = make_step(NotifyConfig {
            url: "http://x".into(),
            method: "DELETE".into(),
            headers: HashMap::new(),
            body_template: String::new(),
        });
        let ctx = TemplateContext::new();
        let out = execute_notify_step_with_policy(&step, &ctx, false).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(out.result.output.contains("unsupported method"));
    }

    #[tokio::test]
    async fn notify_renders_templates_in_url_and_body() {
        // Spin up a tiny local echo server; capture the request body.
        // 0.8.6 (#58) — bind directly with tokio's TcpListener (no std
        // bind + drop + rebind race). Pre-fix this flaked under
        // `--test-threads=8` because another test could grab the port
        // between the std listener drop and the tokio re-bind.
        let received_body = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let received_path = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let body_clone = received_body.clone();
        let path_clone = received_path.clone();

        let app = axum::Router::new().route(
            "/hook/{stage}",
            axum::routing::post(
                move |axum::extract::Path(stage): axum::extract::Path<String>, body: String| {
                    let body_store = body_clone.clone();
                    let path_store = path_clone.clone();
                    async move {
                        *body_store.lock().await = body;
                        *path_store.lock().await = stage;
                        axum::Json(serde_json::json!({"received": true}))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Give the server a moment to start listening.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut ctx = TemplateContext::new();
        ctx.set_step_output("audit", "plan_ready");
        let step = make_step(NotifyConfig {
            url: format!("http://127.0.0.1:{}/hook/{{{{steps.audit.output}}}}", port),
            method: "POST".into(),
            headers: {
                let mut h = HashMap::new();
                h.insert("Content-Type".into(), "application/json".into());
                h
            },
            body_template: r#"{"stage":"{{steps.audit.output}}"}"#.into(),
        });

        let out = execute_notify_step_with_policy(&step, &ctx, false).await;
        assert_eq!(
            out.result.status,
            RunStatus::Success,
            "unexpected failure: {}",
            out.result.output
        );
        assert_eq!(
            out.result.tokens_used, 0,
            "Notify must never consume tokens"
        );

        // URL template expanded
        assert_eq!(received_path.lock().await.as_str(), "plan_ready");
        // Body template expanded
        assert!(received_body
            .lock()
            .await
            .contains(r#""stage":"plan_ready""#));

        // Structured output carries http_status + summary for downstream chaining
        let parsed =
            crate::workflows::step_output_format::parse_envelope_for_test(&out.result.output);
        assert_eq!(parsed["status"], "OK");
        assert_eq!(parsed["data"]["http_status"], 200);
        assert!(parsed["summary"].as_str().unwrap().contains("200"));
    }

    #[tokio::test]
    async fn notify_marks_non_2xx_as_failed_with_excerpt() {
        // 0.8.6 (#58) — same race fix as notify_renders_templates_in_url_and_body.
        let app = axum::Router::new().route(
            "/fail",
            axum::routing::post(|| async {
                (axum::http::StatusCode::BAD_REQUEST, "nope, bad payload")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let step = make_step(NotifyConfig {
            url: format!("http://127.0.0.1:{}/fail", port),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: "{}".into(),
        });
        let out = execute_notify_step_with_policy(&step, &TemplateContext::new(), false).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        let parsed =
            crate::workflows::step_output_format::parse_envelope_for_test(&out.result.output);
        assert_eq!(parsed["status"], "ERROR");
        assert_eq!(parsed["data"]["http_status"], 400);
        assert!(parsed["data"]["response_excerpt"]
            .as_str()
            .unwrap()
            .contains("nope, bad payload"));
    }

    /// 2026-06-10 audit P1 — webhook secrets often live in the URL PATH
    /// (Slack: /services/T…/B…/<token>). Persisted outputs carry only
    /// scheme + host + first segment.
    #[test]
    fn redact_notify_url_keeps_only_first_path_segment() {
        let url = reqwest::Url::parse(
            "https://hooks.slack.com/services/T0001/B0002/supersecrettoken?x=1",
        )
        .unwrap();
        let red = redact_notify_url(&url);
        assert_eq!(red, "https://hooks.slack.com/services/…");
        assert!(!red.contains("supersecrettoken"));
        assert!(!red.contains("B0002"));
        // Bare host stays readable.
        let bare = reqwest::Url::parse("https://example.com/").unwrap();
        assert_eq!(redact_notify_url(&bare), "https://example.com/");
    }

    /// 2026-06-10 audit P1 — SSRF guard: a templated URL resolving to
    /// localhost/private ranges must fail the step, like ApiCall does.
    #[tokio::test]
    async fn notify_refuses_private_target() {
        let step = make_step(NotifyConfig {
            url: "http://127.0.0.1:9/hook".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: "{}".into(),
        });
        let out = execute_notify_step(&step, &TemplateContext::new()).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(
            out.result.output.contains("Security"),
            "got: {}",
            out.result.output
        );
    }
}
