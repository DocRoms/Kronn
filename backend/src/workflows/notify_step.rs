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

pub async fn execute_notify_step(
    step: &WorkflowStep,
    ctx: &TemplateContext,
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
        other => return fail(step, start, format!(
            "Notify step: unsupported method `{}` (allowed: POST, PUT, GET)", other
        )),
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

    // ── Build and fire the request ──────────────────────────────────────
    let client = match reqwest::Client::builder()
        .timeout(NOTIFY_TIMEOUT)
        .build()
    {
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
    let output_json = serde_json::json!({
        "data": {
            "http_status": http_status,
            "response_excerpt": excerpt,
            "url": url,
            "method": method.as_str(),
        },
        "status": if is_success { "OK" } else { "ERROR" },
        "summary": format!(
            "{} {} → {}",
            method.as_str(),
            url,
            http_status
        ),
    });
    let output = serde_json::to_string(&output_json).unwrap_or_default();

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: if is_success { RunStatus::Success } else { RunStatus::Failed },
            output,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result: None,
            envelope_detected: None,
        },
        condition_action: None,
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
            condition_result: None,
            envelope_detected: None,
        },
        condition_action: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::TcpListener;

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
            notify_config: Some(config),
        }
    }

    #[tokio::test]
    async fn notify_rejects_missing_config() {
        let mut step = make_step(NotifyConfig {
            url: "http://x".into(), method: "POST".into(),
            headers: HashMap::new(), body_template: String::new(),
        });
        step.notify_config = None;
        let ctx = TemplateContext::new();
        let out = execute_notify_step(&step, &ctx).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(out.result.output.contains("missing `notify_config`"));
    }

    #[tokio::test]
    async fn notify_rejects_empty_url() {
        let step = make_step(NotifyConfig {
            url: "".into(), method: "POST".into(),
            headers: HashMap::new(), body_template: String::new(),
        });
        let ctx = TemplateContext::new();
        let out = execute_notify_step(&step, &ctx).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(out.result.output.contains("`url` is empty"));
    }

    #[tokio::test]
    async fn notify_rejects_unsupported_method() {
        let step = make_step(NotifyConfig {
            url: "http://x".into(), method: "DELETE".into(),
            headers: HashMap::new(), body_template: String::new(),
        });
        let ctx = TemplateContext::new();
        let out = execute_notify_step(&step, &ctx).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        assert!(out.result.output.contains("unsupported method"));
    }

    #[tokio::test]
    async fn notify_renders_templates_in_url_and_body() {
        // Spin up a tiny local echo server; capture the request body.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let received_body = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let received_path = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let body_clone = received_body.clone();
        let path_clone = received_path.clone();

        let app = axum::Router::new()
            .route("/hook/:stage", axum::routing::post(
                move |axum::extract::Path(stage): axum::extract::Path<String>, body: String| {
                    let body_store = body_clone.clone();
                    let path_store = path_clone.clone();
                    async move {
                        *body_store.lock().await = body;
                        *path_store.lock().await = stage;
                        axum::Json(serde_json::json!({"received": true}))
                    }
                }
            ));
        let addr = format!("127.0.0.1:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
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

        let out = execute_notify_step(&step, &ctx).await;
        assert_eq!(out.result.status, RunStatus::Success, "unexpected failure: {}", out.result.output);
        assert_eq!(out.result.tokens_used, 0, "Notify must never consume tokens");

        // URL template expanded
        assert_eq!(received_path.lock().await.as_str(), "plan_ready");
        // Body template expanded
        assert!(received_body.lock().await.contains(r#""stage":"plan_ready""#));

        // Structured output carries http_status + summary for downstream chaining
        let parsed: serde_json::Value = serde_json::from_str(&out.result.output).unwrap();
        assert_eq!(parsed["status"], "OK");
        assert_eq!(parsed["data"]["http_status"], 200);
        assert!(parsed["summary"].as_str().unwrap().contains("200"));
    }

    #[tokio::test]
    async fn notify_marks_non_2xx_as_failed_with_excerpt() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let app = axum::Router::new()
            .route("/fail", axum::routing::post(|| async {
                (axum::http::StatusCode::BAD_REQUEST, "nope, bad payload")
            }));
        let addr = format!("127.0.0.1:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
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
        let out = execute_notify_step(&step, &TemplateContext::new()).await;
        assert_eq!(out.result.status, RunStatus::Failed);
        let parsed: serde_json::Value = serde_json::from_str(&out.result.output).unwrap();
        assert_eq!(parsed["status"], "ERROR");
        assert_eq!(parsed["data"]["http_status"], 400);
        assert!(parsed["data"]["response_excerpt"].as_str().unwrap().contains("nope, bad payload"));
    }
}
