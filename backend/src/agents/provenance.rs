//! Payload-free model observations owned by one agent launch. A caller keeps
//! the capture even when launch or output collection returns an error.

use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct AgentRuntimeProvenance {
    pub resolved_model: Option<String>,
    pub model_applied: Option<bool>,
    pub observed_models: Vec<String>,
    pub format_fallback: bool,
}

pub type AgentProvenanceCapture = Arc<Mutex<AgentRuntimeProvenance>>;

pub fn observe_model(capture: Option<&AgentProvenanceCapture>, model: &str) {
    // This is a protocol identifier, not arbitrary provider output. Bound
    // cardinality and length independently of the number of streamed chunks.
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return;
    }
    if let Some(capture) = capture {
        if let Ok(mut state) = capture.lock() {
            if state.observed_models.len() < 16 && !state.observed_models.iter().any(|m| m == model)
            {
                state.observed_models.push(model.to_owned());
            }
        }
    }
}

pub fn resolve_model(
    capture: Option<&AgentProvenanceCapture>,
    model: Option<&str>,
    applied: Option<bool>,
) {
    if let Some(capture) = capture {
        if let Ok(mut state) = capture.lock() {
            state.resolved_model = model.map(str::to_owned);
            state.model_applied = applied;
        }
    }
}

/// Only structured assistant/message-start model metadata counts as an
/// observation. Init configuration, synthetic errors and generated text do not.
pub fn claude_observed_model(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let message = match value["type"].as_str()? {
        "assistant" => &value["message"],
        "stream_event" if value["event"]["type"] == "message_start" => &value["event"]["message"],
        _ => return None,
    };
    message["model"]
        .as_str()
        .filter(|model| *model != "<synthetic>")
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_do_not_conflate_resolution_with_provider_reports() {
        let capture = AgentProvenanceCapture::default();
        resolve_model(Some(&capture), Some("proxy-alias"), Some(true));
        observe_model(Some(&capture), "provider-model");
        observe_model(Some(&capture), "provider-model");
        observe_model(Some(&capture), "other-model");
        observe_model(Some(&capture), "invalid\nmodel");
        observe_model(Some(&capture), &"x".repeat(257));
        let state = capture.lock().unwrap();
        assert_eq!(state.resolved_model.as_deref(), Some("proxy-alias"));
        assert_eq!(state.observed_models, ["provider-model", "other-model"]);
    }

    #[test]
    fn claude_observation_accepts_protocol_metadata_only() {
        assert_eq!(
            claude_observed_model(r#"{"type":"assistant","message":{"model":"actual-model"}}"#)
                .as_deref(),
            Some("actual-model")
        );
        assert_eq!(claude_observed_model(r#"{"type":"stream_event","event":{"type":"message_start","message":{"model":"actual-model"}}}"#).as_deref(), Some("actual-model"));
        for line in [
            r#"{"type":"system","subtype":"init","model":"configured"}"#,
            r#"{"type":"assistant","message":{"model":"<synthetic>"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"text":"model: invented"}}}"#,
        ] {
            assert!(claude_observed_model(line).is_none());
        }
    }
}
