//! Explicit per-run generation controls. Never silently discard a saved limit.
use crate::models::AgentType;
use serde_json::{json, Value};

pub(crate) fn validate(
    agent: &AgentType,
    effort: Option<&str>,
    max_tokens: Option<u64>,
) -> Result<(), String> {
    if let Some(limit) = max_tokens {
        if limit == 0 || limit > i64::MAX as u64 {
            return Err("max_tokens must be a positive signed 64-bit integer".into());
        }
        if !super::runner::is_http_chat_agent(agent) {
            return Err("This CLI transport cannot apply max_tokens. Clear the saved limit or choose an HTTP model provider.".into());
        }
    }
    if let Some(effort) = effort.map(str::trim).filter(|s| !s.is_empty()) {
        if *agent == AgentType::Ollama {
            ollama_think(effort)?;
        } else if !super::runner::is_http_chat_agent(agent)
            && !super::runner::agent_supports_reasoning_effort(agent)
        {
            return Err("This agent transport cannot apply the saved reasoning effort. Clear it or choose a supported agent.".into());
        }
    }
    Ok(())
}

fn ollama_think(effort: &str) -> Result<Value, String> {
    match effort {
        "none" | "false" => Ok(json!(false)),
        "true" => Ok(json!(true)),
        "low" | "medium" | "high" | "max" => Ok(json!(effort)),
        _ => Err(format!("Unsupported Ollama reasoning effort '{effort}'; use none, low, medium, high or max, subject to the selected model's support.")),
    }
}

/// Apply after context fitting, which otherwise supplies its default num_predict.
/// The shared tool loop keeps this body, so the same controls survive each turn.
pub(crate) fn apply_http(
    body: &mut Value,
    agent: &AgentType,
    effort: Option<&str>,
    max_tokens: Option<u64>,
) -> Result<(), String> {
    validate(agent, effort, max_tokens)?;
    if let Some(limit) = max_tokens {
        if *agent == AgentType::Ollama {
            body["options"]["num_predict"] = json!(limit);
        } else {
            body["max_tokens"] = json!(limit);
        }
    }
    if let Some(effort) = effort.map(str::trim).filter(|s| !s.is_empty()) {
        if *agent == AgentType::Ollama {
            let think = ollama_think(effort)?;
            if think != json!(false) {
                // Older Qwen runtimes get this generated control message by
                // default. It must not contradict an explicit thinking request.
                if let Some(messages) = body["messages"].as_array_mut() {
                    messages.retain(|m| !(m["role"] == "system" && m["content"] == "/no_think"));
                }
            }
            body["think"] = think;
        } else {
            body["reasoning_effort"] = json!(effort);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_ollama_controls_replace_defaults_without_a_conflicting_no_think_message() {
        let mut body = json!({"options":{"num_ctx":8192,"num_predict":2048},"think":false,
            "messages":[{"role":"system","content":"/no_think"},{"role":"user","content":"hello"}]});
        apply_http(&mut body, &AgentType::Ollama, Some(" high "), Some(3210)).unwrap();
        assert_eq!(body["options"], json!({"num_ctx":8192,"num_predict":3210}));
        assert_eq!(body["think"], "high");
        assert_eq!(body["messages"], json!([{"role":"user","content":"hello"}]));
        assert!(body.get("reasoning_effort").is_none());
        apply_http(&mut body, &AgentType::Ollama, Some("none"), None).unwrap();
        assert_eq!(body["think"], false);
    }

    #[test]
    fn absent_controls_preserve_provider_defaults_and_unsupported_controls_refuse() {
        let original = json!({"options":{"num_predict":2048},"think":false});
        let mut body = original.clone();
        apply_http(&mut body, &AgentType::Ollama, Some(" "), None).unwrap();
        assert_eq!(body, original);
        for agent in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::GeminiCli,
        ] {
            assert!(validate(&agent, None, Some(100))
                .unwrap_err()
                .contains("cannot apply max_tokens"));
        }
        assert!(validate(&AgentType::Ollama, Some("xhigh"), None).is_err());
        assert!(validate(&AgentType::Custom, None, Some(0)).is_err());
        assert!(validate(&AgentType::Custom, None, Some(u64::MAX)).is_err());
        assert!(validate(&AgentType::GeminiCli, Some("high"), None).is_err());
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::agents::runner::{start_agent_with_config, AgentStartConfig, ExternalHttpRuntime};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn ollama_generation_controls_reach_the_actual_http_request_after_context_fitting() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/version"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"version":"0.18.0"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"model_info":{"qwen3.context_length":32768}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    "{\"message\":{\"content\":\"limit applied\"},\"done\":true}\n",
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        let runtime = ExternalHttpRuntime {
            display_name: "fixture".into(),
            mention_alias: "fixture".into(),
            endpoint: server.uri(),
            api_key: None,
        };
        let config = crate::core::config::default_config();
        let directory = tempfile::tempdir().unwrap();
        let mut process = start_agent_with_config(AgentStartConfig {
            model_override: Some("qwen3-fixture"),
            external_http: Some(&runtime),
            reasoning_effort_override: Some("high"),
            max_tokens_override: Some(3210),
            http_request_timeout: Some(std::time::Duration::from_secs(10)),
            ..AgentStartConfig::new(
                &AgentType::Ollama,
                directory.path().to_str().unwrap(),
                "hello",
                &config.tokens,
            )
        })
        .await
        .unwrap();
        let mut output = String::new();
        while let Some(line) = process.next_line().await {
            output.push_str(&line);
        }
        assert!(process.child.wait().await.unwrap().success());
        assert!(output.contains("limit applied"));
        let requests = server.received_requests().await.unwrap();
        let request = requests
            .iter()
            .find(|r| r.url.path() == "/api/chat")
            .unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["options"]["num_predict"], 3210);
        assert_eq!(body["think"], "high");
        assert!(!body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "/no_think"));
    }
}
