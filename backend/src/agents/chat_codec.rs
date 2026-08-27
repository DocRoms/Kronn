//! Wire formats for the HTTP chat path.
//!
//! `start_ollama_http` owns the parts that are the same everywhere — client
//! timeouts, streaming, cancellation, the lifeline child, token accounting.
//! What actually differs between an Ollama daemon and an OpenAI-compatible
//! proxy is only the endpoint, the request body and how one streamed line
//! decodes. That seam lives here so a second backend does not mean a second
//! copy of the transport.
//!
//! Both formats are line-oriented, so the transport's split-on-newline loop
//! serves both: Ollama sends one JSON object per line, OpenAI sends SSE
//! `data:` frames terminated by `data: [DONE]`.

use serde_json::Value;

/// One decoded stream event, independent of wire format.
///
/// Token counts arrive with the terminal chunk on Ollama but in a separate
/// usage frame on OpenAI, so they are reported whenever seen and the caller
/// accumulates them until the stream ends.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ChatChunk {
    pub delta: Option<String>,
    /// Why the provider stopped, when it says so. Surfaced because an empty reply
    /// is otherwise undiagnosable: `length` means the model spent its output budget
    /// (a reasoning model can burn it all thinking), `stop` means it chose to end.
    /// Guessing between those in a user-facing message is what this replaces.
    pub finish_reason: Option<String>,
    pub done: bool,
    pub error: Option<String>,
    pub prompt_tokens: u64,
    pub eval_tokens: u64,
    /// Tool calls the model wants executed before it can answer. Ollama puts
    /// them on the terminal chunk; OpenAI streams them as indexed fragments
    /// and signals completion with `finish_reason: "tool_calls"`.
    pub tool_calls: Vec<crate::agents::tools::ToolCallFragment>,
}

impl ChatChunk {
    fn done() -> Self {
        Self {
            done: true,
            ..Default::default()
        }
    }
}

pub(crate) trait ChatCodec: Send + Sync {
    fn endpoint(&self, base: &str) -> String;
    /// `None` when the line carries no event (blank, SSE comment, unparseable
    /// fragment) — the transport skips it without treating it as an error.
    fn parse_line(&self, line: &str) -> Option<ChatChunk>;
}

/// Ollama's native `/api/chat`: newline-delimited JSON, counts on the
/// terminal `done` object.
pub(crate) struct OllamaCodec;

impl ChatCodec for OllamaCodec {
    fn endpoint(&self, base: &str) -> String {
        format!("{}/api/chat", base)
    }

    fn parse_line(&self, line: &str) -> Option<ChatChunk> {
        if line.trim().is_empty() {
            return None;
        }
        let json: Value = serde_json::from_str(line).ok()?;
        let mut chunk = ChatChunk::default();
        // In-band error on a 200 stream (model crashed mid-generation).
        if let Some(err) = json["error"].as_str() {
            chunk.error = Some(err.to_string());
        }
        if let Some(text) = json["message"]["content"].as_str() {
            if !text.is_empty() {
                chunk.delta = Some(text.to_string());
            }
        }
        // Ollama emits the whole call set at once, on the message itself.
        chunk.tool_calls = crate::agents::tools::parse_tool_calls(&json["message"]["tool_calls"]);
        if json["done"].as_bool() == Some(true) {
            chunk.done = true;
            chunk.prompt_tokens = json["prompt_eval_count"].as_u64().unwrap_or(0);
            chunk.eval_tokens = json["eval_count"].as_u64().unwrap_or(0);
        }
        Some(chunk)
    }
}

/// OpenAI-compatible `/v1/chat/completions` (LiteLLM, vLLM, …): SSE frames,
/// `[DONE]` sentinel, usage in its own frame when `stream_options
/// .include_usage` is set.
///
/// Selected by `AgentType::LiteLlm` on the shared HTTP path.
pub(crate) struct OpenAiCodec;

impl ChatCodec for OpenAiCodec {
    fn endpoint(&self, base: &str) -> String {
        format!("{}/v1/chat/completions", base)
    }

    fn parse_line(&self, line: &str) -> Option<ChatChunk> {
        let line = line.trim();
        // SSE comments/heartbeats and blank separators carry no event.
        if line.is_empty() || line.starts_with(':') {
            return None;
        }
        // Non-streaming replies arrive as a bare JSON body with no `data:`.
        let payload = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if payload == "[DONE]" {
            return Some(ChatChunk::done());
        }
        let json: Value = serde_json::from_str(payload).ok()?;
        let mut chunk = ChatChunk::default();
        if let Some(err) = json["error"]["message"].as_str() {
            chunk.error = Some(err.to_string());
        }
        let choice = &json["choices"][0];
        // Streaming puts text under `delta`, non-streaming under `message`.
        for key in ["delta", "message"] {
            if let Some(text) = choice[key]["content"].as_str() {
                if !text.is_empty() {
                    chunk.delta = Some(text.to_string());
                }
                break;
            }
        }
        chunk.tool_calls = crate::agents::tools::parse_tool_calls(&choice["delta"]["tool_calls"]);
        if chunk.tool_calls.is_empty() {
            // Non-streaming replies carry them on `message` instead.
            chunk.tool_calls =
                crate::agents::tools::parse_tool_calls(&choice["message"]["tool_calls"]);
        }
        if let Some(usage) = json.get("usage").filter(|u| !u.is_null()) {
            chunk.prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
            chunk.eval_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            chunk.finish_reason = Some(reason.to_string());
        }
        // A non-streaming body has no `[DONE]`; its finish_reason ends it.
        if !choice["finish_reason"].is_null() && choice["delta"].is_null() {
            chunk.done = true;
        }
        Some(chunk)
    }
}

/// Request body for an OpenAI-compatible endpoint. Mirrors
/// `build_ollama_chat_body`'s determinism (temperature 0, fixed seed) and
/// asks for usage so token accounting survives streaming.
pub(crate) fn build_openai_chat_body(
    model: &str,
    system_context: &str,
    user_prompt: &str,
    format: Option<&Value>,
    stream: bool,
) -> Value {
    let mut messages = Vec::new();
    if !system_context.is_empty() {
        messages.push(serde_json::json!({ "role": "system", "content": system_context }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": stream,
        "temperature": 0,
        "seed": 42,
    });
    if stream {
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    // Same envelope-wrapped schema Ollama gets, in OpenAI's spelling.
    if let Some(schema) = format {
        body["response_format"] = serde_json::json!({
            "type": "json_schema",
            "json_schema": { "name": "kronn_envelope", "strict": true, "schema": schema },
        });
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_differ_per_wire_format() {
        assert_eq!(
            OllamaCodec.endpoint("http://h:11434"),
            "http://h:11434/api/chat"
        );
        assert_eq!(
            OpenAiCodec.endpoint("http://h:4000"),
            "http://h:4000/v1/chat/completions"
        );
    }

    #[test]
    fn ollama_decodes_delta_then_terminal_counts() {
        let d = OllamaCodec
            .parse_line(r#"{"message":{"content":"hi"},"done":false}"#)
            .unwrap();
        assert_eq!(d.delta.as_deref(), Some("hi"));
        assert!(!d.done);

        let end = OllamaCodec
            .parse_line(r#"{"done":true,"prompt_eval_count":12,"eval_count":34}"#)
            .unwrap();
        assert!(end.done);
        assert_eq!((end.prompt_tokens, end.eval_tokens), (12, 34));
    }

    #[test]
    fn ollama_surfaces_in_band_error_and_skips_noise() {
        let e = OllamaCodec
            .parse_line(r#"{"error":"model runner stopped"}"#)
            .unwrap();
        assert_eq!(e.error.as_deref(), Some("model runner stopped"));
        assert!(OllamaCodec.parse_line("   ").is_none());
        assert!(OllamaCodec.parse_line("{not json").is_none());
    }

    #[test]
    fn openai_decodes_sse_delta_and_done_sentinel() {
        let d = OpenAiCodec
            .parse_line(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#)
            .unwrap();
        assert_eq!(d.delta.as_deref(), Some("hi"));
        assert!(!d.done);

        assert!(OpenAiCodec.parse_line("data: [DONE]").unwrap().done);
    }

    #[test]
    fn openai_reads_usage_from_its_own_frame() {
        // With include_usage the counts arrive after the last delta, in a
        // frame whose `choices` array is empty.
        let u = OpenAiCodec
            .parse_line(r#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":9}}"#)
            .unwrap();
        assert_eq!((u.prompt_tokens, u.eval_tokens), (7, 9));
        assert_eq!(u.delta, None);
    }

    #[test]
    fn openai_handles_non_streaming_body() {
        let r = OpenAiCodec
            .parse_line(
                r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4}}"#,
            )
            .unwrap();
        assert_eq!(r.delta.as_deref(), Some("done"));
        assert!(
            r.done,
            "finish_reason on a non-delta choice ends the stream"
        );
        assert_eq!((r.prompt_tokens, r.eval_tokens), (3, 4));
    }

    #[test]
    fn openai_surfaces_error_and_skips_sse_noise() {
        let e = OpenAiCodec
            .parse_line(r#"data: {"error":{"message":"model not found"}}"#)
            .unwrap();
        assert_eq!(e.error.as_deref(), Some("model not found"));
        assert!(OpenAiCodec.parse_line(": heartbeat").is_none());
        assert!(OpenAiCodec.parse_line("").is_none());
    }

    // ─── Frames recorded from a live LiteLLM 1.95.0 proxy fronting Ollama ───
    //
    // Invented fixtures agree with whatever the parser already does. These are
    // verbatim, so they catch the shapes a real proxy emits and a hand-written
    // example would miss — notably `reasoning_content` and the empty-`delta`
    // finish frame.

    #[test]
    fn live_reasoning_deltas_are_not_output() {
        // Thinking models stream their scratchpad on a separate key. Emitting
        // it would dump the chain of thought into the user's reply.
        let c = OpenAiCodec
            .parse_line(
                r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":"Hmm","role":"assistant"}}]}"#,
            )
            .unwrap();
        assert_eq!(c.delta, None);
        assert!(!c.done);
    }

    #[test]
    fn live_finish_frame_does_not_end_a_stream() {
        // The finish frame carries an EMPTY delta object, not a missing one —
        // so the non-streaming heuristic must not fire here. Only `[DONE]`
        // ends a stream, otherwise the trailing usage frame is never read.
        let c = OpenAiCodec
            .parse_line(r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert!(!c.done, "finish_reason with a present delta is mid-stream");

        let usage = OpenAiCodec
            .parse_line(
                r#"data: {"choices":[{"index":0,"delta":{}}],"usage":{"completion_tokens":161,"prompt_tokens":15,"total_tokens":176}}"#,
            )
            .unwrap();
        assert_eq!((usage.prompt_tokens, usage.eval_tokens), (15, 161));
        assert!(!usage.done);

        assert!(OpenAiCodec.parse_line("data: [DONE]").unwrap().done);
    }

    #[test]
    fn live_non_streaming_body_ends_on_its_own() {
        // No `delta` key at all — this is the whole reply, so it must complete
        // without a sentinel (the TypedSchema path runs with stream:false).
        let c = OpenAiCodec
            .parse_line(
                r#"{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"OK","role":"assistant"}}],"usage":{"completion_tokens":161,"prompt_tokens":15}}"#,
            )
            .unwrap();
        assert_eq!(c.delta.as_deref(), Some("OK"));
        assert!(c.done);
        assert_eq!((c.prompt_tokens, c.eval_tokens), (15, 161));
    }

    #[test]
    fn openai_body_is_deterministic_and_asks_for_usage() {
        let b = build_openai_chat_body("m", "sys", "hi", None, true);
        assert_eq!(b["temperature"], 0);
        assert_eq!(b["seed"], 42);
        assert_eq!(b["stream_options"]["include_usage"], true);
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["role"], "user");
    }

    #[test]
    fn openai_body_omits_usage_options_when_not_streaming() {
        let b = build_openai_chat_body("m", "", "hi", None, false);
        assert!(b["stream_options"].is_null());
        assert_eq!(b["messages"][0]["role"], "user", "empty system is dropped");
    }

    #[test]
    fn openai_body_maps_schema_to_response_format() {
        let schema = serde_json::json!({"type":"object"});
        let b = build_openai_chat_body("m", "", "hi", Some(&schema), false);
        assert_eq!(b["response_format"]["type"], "json_schema");
        assert_eq!(b["response_format"]["json_schema"]["schema"], schema);
    }
}
