//! Payload-free runtime observations owned by one agent launch: the models it
//! served, the CLI session it ran in and any npx fallback. A caller keeps the
//! capture even when launch or output collection returns an error.

use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct AgentRuntimeProvenance {
    pub resolved_model: Option<String>,
    pub model_applied: Option<bool>,
    pub observed_models: Vec<String>,
    pub format_fallback: bool,
    /// The CLI's own conversation id, as its `init` line reports it — the name
    /// of the transcript it writes (KT-911). `None` until observed, and for a
    /// runtime that has no CLI session (HTTP providers).
    pub session_id: Option<String>,
    pub npx_fallback_command: Option<Vec<String>>,
    pub npx_fallback_version: Option<String>,
}

pub type AgentProvenanceCapture = Arc<Mutex<AgentRuntimeProvenance>>;

pub fn record_npx_fallback(
    capture: Option<&AgentProvenanceCapture>,
    command: Vec<String>,
    version: Option<String>,
) {
    if let Some(capture) = capture {
        if let Ok(mut state) = capture.lock() {
            state.npx_fallback_command = Some(command);
            state.npx_fallback_version = version;
        }
    }
}

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

/// Record the CLI session the launch runs in. The first id wins: one launch is
/// one process, so a later line naming another session is not this one's.
pub fn observe_session(capture: Option<&AgentProvenanceCapture>, session_id: &str) {
    // A protocol identifier, bounded like a model id: it is persisted and shown.
    if session_id.is_empty()
        || session_id.len() > 256
        || session_id
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return;
    }
    if let Some(capture) = capture {
        if let Ok(mut state) = capture.lock() {
            if state.session_id.is_none() {
                state.session_id = Some(session_id.to_owned());
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

/// The session a Claude Code stream names on its `system`/`init` line. Only that
/// line counts: `result` repeats the id, but an interrupted turn never reaches it.
pub fn claude_session_id(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value["type"] != "system" || value["subtype"] != "init" {
        return None;
    }
    value["session_id"].as_str().map(str::to_owned)
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

    #[test]
    fn the_session_comes_from_the_init_line_and_the_first_one_wins() {
        assert_eq!(
            claude_session_id(r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#)
                .as_deref(),
            Some("abc-123")
        );
        for line in [
            r#"{"type":"result","subtype":"success","session_id":"abc-123"}"#,
            r#"{"type":"system","subtype":"hook_started","session_id":"abc-123"}"#,
            r#"{"type":"assistant","session_id":"abc-123","message":{}}"#,
            "not json",
        ] {
            assert!(claude_session_id(line).is_none(), "{line}");
        }

        let capture = AgentProvenanceCapture::default();
        observe_session(Some(&capture), "");
        observe_session(Some(&capture), "has space");
        observe_session(Some(&capture), "bad\nid");
        observe_session(Some(&capture), &"x".repeat(257));
        assert_eq!(capture.lock().unwrap().session_id, None);
        observe_session(Some(&capture), "first");
        observe_session(Some(&capture), "second");
        assert_eq!(capture.lock().unwrap().session_id.as_deref(), Some("first"));
    }
}
