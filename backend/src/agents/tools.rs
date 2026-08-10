//! Tool calling for the HTTP chat path.
//!
//! CLI agents reach Kronn's primitives through the `kronn-internal` stdio
//! bridge. Agents that run over HTTP (Ollama, LiteLLM) have no such process,
//! so the orchestrator has to own the loop: declare the tools, read back the
//! model's calls, execute them, feed the results in, repeat.
//!
//! Encoding is deliberately **native** (`tools` / `tool_calls`) rather than a
//! textual convention. Describing tools in prose was tried and taught the
//! model to hallucinate calls it could not make; a declared tool is validated
//! by the API and reported through `finish_reason`, so the model is
//! constrained rather than asked politely.
//!
//! `ToolExecutor` is a trait because `runner.rs` must not depend on
//! `AppState`: the API layer supplies the implementation, tests supply a fake.

use serde_json::{json, Value};

/// One call the model asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// Correlates the result back to the request. Ollama omits it, so the
    /// loop synthesises one; OpenAI requires it on the reply message.
    pub id: String,
    pub name: String,
    /// Parsed arguments. Providers disagree on whether this is a JSON object
    /// or a JSON-encoded string, so decoding normalises to an object.
    pub arguments: Value,
}

/// Result of executing one call, as fed back to the model.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub call: ToolCall,
    /// JSON payload the model sees. Errors are returned here rather than
    /// aborting the run: a model that gets "this failed and why" can recover
    /// or explain, whereas a killed turn just loses the conversation.
    pub content: Value,
    pub ok: bool,
}

/// Executes Kronn primitives on behalf of an HTTP agent.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Schemas advertised to the model, in OpenAI `tools` shape (Ollama
    /// accepts the same shape on `/api/chat`).
    fn catalogue(&self) -> Vec<Value>;

    /// Run one call. Implementations must not panic: a tool failure is
    /// reported through `ToolOutcome`, never by unwinding into the stream.
    async fn execute(&self, call: &ToolCall) -> ToolOutcome;
}

/// Hard ceiling on tool round-trips per run.
///
/// A model that keeps calling tools without converging would otherwise bill
/// tokens forever (and, on a local model, pin the GPU). Eight is generous for
/// "list, then call, then maybe retry once" while still terminating.
pub const MAX_TOOL_ITERATIONS: usize = 8;

/// A piece of a tool call as it appears on the wire.
///
/// Ollama emits a whole call in one go; the OpenAI streaming format may split
/// one call across frames (name first, then argument text in slices). Both
/// decode to fragments so the transport can merge them the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallFragment {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    /// Raw argument text. Kept unparsed because a slice of a JSON object is
    /// not valid JSON on its own — parsing happens once, after merging.
    pub arguments_delta: String,
}

/// Decode a provider's `tool_calls` array into fragments.
pub(crate) fn parse_tool_calls(raw: &Value) -> Vec<ToolCallFragment> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let f = &item["function"];
            ToolCallFragment {
                index: item["index"].as_u64().map(|n| n as usize).unwrap_or(i),
                id: item["id"].as_str().map(str::to_string),
                name: f["name"].as_str().map(str::to_string),
                arguments_delta: match &f["arguments"] {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    // Ollama sends a real object; re-encode so merging is
                    // uniform and the final parse sees valid JSON.
                    other => other.to_string(),
                },
            }
        })
        .collect()
}

/// Merges fragments across frames into finished calls.
#[derive(Debug, Default)]
pub(crate) struct ToolCallAccumulator {
    /// Keyed by wire index so out-of-order frames still land correctly.
    parts: std::collections::BTreeMap<usize, (Option<String>, Option<String>, String)>,
}

impl ToolCallAccumulator {
    pub fn push(&mut self, fragments: Vec<ToolCallFragment>) {
        for f in fragments {
            let slot = self.parts.entry(f.index).or_default();
            if f.id.is_some() {
                slot.0 = f.id;
            }
            if f.name.is_some() {
                slot.1 = f.name;
            }
            slot.2.push_str(&f.arguments_delta);
        }
    }

    /// Finished calls. Fragments that never carried a name are dropped —
    /// there is nothing executable in them.
    pub fn finish(self) -> Vec<ToolCall> {
        self.parts
            .into_iter()
            .filter_map(|(index, (id, name, args))| {
                let name = name?;
                let trimmed = args.trim();
                // Unparseable or absent arguments degrade to an empty object
                // rather than dropping the call: the tool can then answer with
                // a validation error instead of leaving the model waiting.
                let arguments = if trimmed.is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
                };
                Some(ToolCall {
                    id: id.unwrap_or_else(|| format!("call_{index}")),
                    name,
                    arguments,
                })
            })
            .collect()
    }
}

/// Render the assistant turn that requested the calls, so the next request
/// carries the history the provider expects.
///
/// `string_arguments` is not cosmetic: OpenAI requires `arguments` to be a
/// JSON-encoded **string**, while Ollama requires a real **object** and
/// rejects the string form outright ("Value looks like object, but can't find
/// closing '}' symbol", HTTP 400 — observed against qwen3:4b). Sending the
/// wrong one breaks the loop on the second turn, after the tool has already
/// run.
pub(crate) fn assistant_tool_call_message(calls: &[ToolCall], string_arguments: bool) -> Value {
    json!({
        "role": "assistant",
        "content": "",
        "tool_calls": calls.iter().map(|c| {
            let arguments = if string_arguments {
                Value::String(c.arguments.to_string())
            } else {
                c.arguments.clone()
            };
            json!({
                "id": c.id,
                "type": "function",
                "function": { "name": c.name, "arguments": arguments },
            })
        }).collect::<Vec<_>>(),
    })
}

/// Render one tool result. `tool_call_id` is what OpenAI correlates on;
/// `name` is what Ollama reads, so both are always present.
pub(crate) fn tool_result_message(outcome: &ToolOutcome) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": outcome.call.id,
        "name": outcome.call.name,
        "content": outcome.content.to_string(),
    })
}

/// Human-readable trace persisted alongside the reply, in the shape the UI
/// already parses for CLI agents (`frontend/src/lib/kronnToolParser.ts`), so
/// HTTP-agent calls render like every other agent's.
pub(crate) fn trace_line(outcome: &ToolOutcome) -> String {
    let args = if outcome
        .call
        .arguments
        .as_object()
        .is_some_and(|o| o.is_empty())
    {
        String::new()
    } else {
        outcome.call.arguments.to_string()
    };
    let status = if outcome.ok { "ok" } else { "error" };
    format!(
        "[kronn-internal: {}({}) → {}]",
        outcome.call.name, args, status
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(frames: &[Value]) -> Vec<ToolCall> {
        let mut acc = ToolCallAccumulator::default();
        for f in frames {
            acc.push(parse_tool_calls(f));
        }
        acc.finish()
    }

    #[test]
    fn decodes_a_whole_ollama_call_from_one_frame() {
        // Ollama sends `arguments` as a real object, in a single message.
        let calls = collect(&[json!([{ "function": { "name": "mcp_list", "arguments": {} } }])]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "mcp_list");
        assert_eq!(calls[0].arguments, json!({}));
        assert_eq!(calls[0].id, "call_0", "missing ids are synthesised");
    }

    #[test]
    fn decodes_a_whole_litellm_call_from_one_frame() {
        // Recorded from the live proxy: id, name and arguments all together.
        let calls = collect(&[json!([{
            "id": "call_mw5jzdnr", "type": "function", "index": 0,
            "function": { "arguments": "{}", "name": "mcp_list" },
        }])]);
        assert_eq!(calls[0].id, "call_mw5jzdnr");
        assert_eq!(calls[0].name, "mcp_list");
    }

    #[test]
    fn merges_openai_style_fragments_across_frames() {
        // A true OpenAI upstream splits one call: name first, then argument
        // text in slices. Parsing any slice alone would fail.
        let calls = collect(&[
            json!([{ "index": 0, "id": "call_a", "function": { "name": "api_call", "arguments": "" } }]),
            json!([{ "index": 0, "function": { "arguments": "{\"endpoint_" } }]),
            json!([{ "index": 0, "function": { "arguments": "path\":\"/v1/sites\"}" } }]),
        ]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].arguments["endpoint_path"], "/v1/sites");
    }

    #[test]
    fn keeps_parallel_calls_separate_by_index() {
        let calls = collect(&[json!([
            { "index": 0, "function": { "name": "qa_list", "arguments": "{}" } },
            { "index": 1, "function": { "name": "mcp_list", "arguments": "{}" } },
        ])]);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "qa_list");
        assert_eq!(calls[1].name, "mcp_list");
    }

    #[test]
    fn malformed_arguments_degrade_to_empty_rather_than_dropping_the_call() {
        // Losing the call entirely would leave the model waiting forever; an
        // empty object lets the tool reply with a validation error instead.
        let calls =
            collect(&[json!([{ "function": { "name": "qa_run", "arguments": "{not json" } }])]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, json!({}));
    }

    #[test]
    fn fragments_that_never_name_a_tool_are_dropped() {
        let calls = collect(&[json!([
            { "index": 0, "function": { "arguments": "{}" } },
            { "index": 1, "function": { "name": "ok", "arguments": "{}" } },
        ])]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "ok");
    }

    #[test]
    fn non_array_input_yields_nothing() {
        assert!(parse_tool_calls(&Value::Null).is_empty());
        assert!(parse_tool_calls(&json!({"function": {"name": "x"}})).is_empty());
        assert!(ToolCallAccumulator::default().finish().is_empty());
    }

    #[test]
    fn round_trip_messages_carry_ids_both_providers_need() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "mcp_list".into(),
            arguments: json!({}),
        };
        let asst = assistant_tool_call_message(std::slice::from_ref(&call), true);
        assert_eq!(asst["tool_calls"][0]["id"], "call_1");
        // OpenAI requires arguments as a string, even when empty.
        assert!(asst["tool_calls"][0]["function"]["arguments"].is_string());
        // Ollama requires the object form and 400s on the string one.
        let ollama = assistant_tool_call_message(std::slice::from_ref(&call), false);
        assert!(
            ollama["tool_calls"][0]["function"]["arguments"].is_object(),
            "Ollama rejects string arguments — see the doc comment"
        );

        let outcome = ToolOutcome {
            call,
            content: json!({"servers": []}),
            ok: true,
        };
        let msg = tool_result_message(&outcome);
        assert_eq!(msg["role"], "tool");
        assert_eq!(msg["tool_call_id"], "call_1");
        assert_eq!(msg["name"], "mcp_list", "Ollama correlates on name");
    }

    #[test]
    fn trace_matches_the_format_the_ui_already_parses() {
        let outcome = ToolOutcome {
            call: ToolCall {
                id: "c".into(),
                name: "qa_list".into(),
                arguments: json!({}),
            },
            content: json!([]),
            ok: true,
        };
        assert_eq!(trace_line(&outcome), "[kronn-internal: qa_list() → ok]");

        let failed = ToolOutcome {
            call: ToolCall {
                id: "c".into(),
                name: "api_call".into(),
                arguments: json!({"endpoint_path": "/x"}),
            },
            content: json!({"error": "boom"}),
            ok: false,
        };
        assert!(trace_line(&failed).ends_with("→ error]"));
        assert!(trace_line(&failed).contains("/x"));
    }
}
