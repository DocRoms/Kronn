//! Bounded, redacted metadata for native tool calls. Tool output is never copied.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const MARKER: &str = "[acp-tool-trace] ";
const MAX_INPUT_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolTraceUpdate {
    pub id: String,
    pub name: Option<String>,
    pub input: Option<String>,
    pub status: Option<String>,
}

pub fn safe_excerpt(raw: &str, max_chars: usize) -> String {
    // Redact before truncating: otherwise a cut can hide the suffix that
    // identifies a credential, leaving its beginning in the transcript.
    let safe = crate::core::redact::redact_for_audit_artifact(raw).0;
    let mut text: String = safe.chars().take(max_chars).collect();
    if safe.chars().nth(max_chars).is_some() {
        text.push('…');
    }
    text.replace(['\n', '\r'], " ")
}

fn input_excerpt(value: &Value) -> String {
    fn mask(value: &Value, depth: usize) -> Value {
        if depth > 32 {
            return Value::String("[nested input omitted]".into());
        }
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| {
                        let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
                        let sensitive = [
                            "password",
                            "secret",
                            "token",
                            "apikey",
                            "authorization",
                            "cookie",
                            "privatekey",
                            "credential",
                        ]
                        .iter()
                        .any(|part| normalized.contains(part));
                        (
                            key.clone(),
                            if sensitive {
                                Value::String("***REDACTED***".into())
                            } else {
                                mask(value, depth + 1)
                            },
                        )
                    })
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| mask(item, depth + 1)).collect())
            }
            // Redact strings separately too: shell assignments and nested JSON
            // must be checked before JSON escapes obscure their delimiters.
            Value::String(text) => {
                Value::String(crate::core::redact::redact_for_audit_artifact(text).0)
            }
            other => other.clone(),
        }
    }
    safe_excerpt(&mask(value, 0).to_string(), MAX_INPUT_CHARS)
}

pub fn safe_input(raw: &str) -> String {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => input_excerpt(&value),
        Err(_) => safe_excerpt(raw, MAX_INPUT_CHARS),
    }
}

fn status(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        match value {
            "completed" | "succeeded" | "success" => "completed",
            "failed" | "error" => "failed",
            "cancelled" | "canceled" => "cancelled",
            "pending" | "in_progress" | "running" => "in_progress",
            _ => "unknown",
        }
        .to_string()
    })
}

impl ToolTraceUpdate {
    fn new(id: &str, name: Option<&str>, input: Option<&Value>, state: Option<&str>) -> Self {
        Self {
            id: safe_excerpt(id, 200),
            name: name
                .filter(|name| !name.is_empty())
                .map(|name| safe_excerpt(name, 200)),
            input: input.map(input_excerpt),
            status: status(state),
        }
    }

    pub fn marker(&self) -> String {
        format!(
            "{MARKER}{}",
            serde_json::to_string(self).expect("trace fields serialize")
        )
    }
}

/// ACP updates are partial: an ending frame often carries only id and status.
pub fn from_acp(update: &Value) -> Option<ToolTraceUpdate> {
    let call = update.get("toolCall").unwrap_or(update);
    let id = call.get("toolCallId")?.as_str()?;
    Some(ToolTraceUpdate::new(
        id,
        call.get("title").and_then(Value::as_str),
        call.get("rawInput"),
        call.get("status").and_then(Value::as_str),
    ))
}

/// Codex exec item metadata; omit command output, MCP results and patch bodies.
pub fn from_codex(item: &Value) -> Option<ToolTraceUpdate> {
    let kind = item.get("type")?.as_str()?;
    let id = item.get("id")?.as_str()?;
    let (name, input) = match kind {
        "mcp_tool_call" => {
            let server = item.get("server")?.as_str()?;
            let tool = item.get("tool")?.as_str()?;
            (
                format!("mcp__{server}__{tool}"),
                item.get("arguments").cloned(),
            )
        }
        "command_execution" => (
            kind.to_owned(),
            item.get("command")
                .map(|command| json!({"command": command})),
        ),
        "file_change" => {
            let changes = item.get("changes").and_then(Value::as_array).map(|changes| {
                Value::Array(changes.iter().map(|change| json!({"path": change.get("path"), "kind": change.get("kind")})).collect())
            });
            (kind.to_owned(), changes)
        }
        "web_search" => (
            kind.to_owned(),
            item.get("query").map(|query| json!({"query": query})),
        ),
        "collab_tool_call" => (
            item.get("tool")
                .and_then(Value::as_str)
                .unwrap_or(kind)
                .to_owned(),
            None,
        ),
        _ => return None,
    };
    let state = if item
        .get("exit_code")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0)
        || item.get("error").is_some_and(|error| !error.is_null())
    {
        Some("failed")
    } else {
        item.get("status").and_then(Value::as_str)
    };
    Some(ToolTraceUpdate::new(id, Some(&name), input.as_ref(), state))
}

/// Claude snapshots provide the complete input, and user tool_result blocks
/// correlate completion/error by tool_use_id. A snapshot alone proves no result.
pub fn from_claude_line(line: &str) -> Vec<ToolTraceUpdate> {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    if !matches!(
        event.get("type").and_then(Value::as_str),
        Some("assistant" | "user")
    ) {
        return Vec::new();
    }
    event
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => Some(ToolTraceUpdate::new(
                block.get("id")?.as_str()?,
                block.get("name").and_then(Value::as_str),
                block.get("input"),
                Some("in_progress"),
            )),
            Some("tool_result") => Some(ToolTraceUpdate::new(
                block.get("tool_use_id")?.as_str()?,
                None,
                None,
                Some(
                    if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                        "failed"
                    } else {
                        "completed"
                    },
                ),
            )),
            _ => None,
        })
        .collect()
}

/// Merge partial updates by call id, retaining first-observed order and input.
/// A name shared by two calls never collapses their identities.
pub fn collect(lines: &[String]) -> Vec<ToolTraceUpdate> {
    let mut records = Vec::<ToolTraceUpdate>::new();
    let mut ids = BTreeMap::new();
    for mut update in lines
        .iter()
        .filter_map(|line| line.strip_prefix(MARKER))
        .filter_map(|json| serde_json::from_str::<ToolTraceUpdate>(json).ok())
    {
        // Capture is an internal wire, but redact again at the persistence edge.
        update.id = safe_excerpt(&update.id, 200);
        update.name = update.name.map(|name| safe_excerpt(&name, 200));
        update.input = update.input.map(|input| safe_input(&input));
        update.status = status(update.status.as_deref());
        if let Some(&index) = ids.get(&update.id) {
            let record: &mut ToolTraceUpdate = &mut records[index];
            if update.name.is_some() {
                record.name = update.name;
            }
            if update.input.is_some() {
                record.input = update.input;
            }
            // A cumulative snapshot cannot reopen a terminal call.
            if !matches!(
                record.status.as_deref(),
                Some("completed" | "failed" | "cancelled")
            ) && update.status.is_some()
            {
                record.status = update.status;
            }
        } else {
            ids.insert(update.id.clone(), records.len());
            records.push(update);
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_keeps_the_mcp_name_inputs_and_failure_without_results() {
        let trace = from_codex(&json!({
            "id":"call-1", "type":"mcp_tool_call", "server":"kronn-internal", "tool":"disc_append",
            "arguments":{"content":"hello", "nested":{"apiKey":"fixture-private"}, "headers":{"Authorization":"Bearer fixture-private"}},
            "status":"failed", "error":{"message":"fixture-private"}, "result":{"secret":"fixture-private"},
        })).unwrap();
        assert_eq!(
            trace.name.as_deref(),
            Some("mcp__kronn-internal__disc_append")
        );
        assert_eq!(trace.status.as_deref(), Some("failed"));
        assert!(trace.input.as_deref().unwrap().contains("hello"));
        assert!(!trace.marker().contains("fixture-private"));
        let command = from_codex(&json!({"id":"shell", "type":"command_execution", "command":"APP_SECRET=fixture-command echo ok", "status":"completed", "exit_code":1, "aggregated_output":"fixture-output"})).unwrap();
        assert_eq!(command.status.as_deref(), Some("failed"));
        assert!(!command.marker().contains("fixture-command"));
        assert!(!command.marker().contains("fixture-output"));
    }

    #[test]
    fn acp_partial_updates_preserve_input_and_do_not_merge_same_named_calls() {
        let start = from_acp(&json!({"toolCallId":"one","title":"Read","rawInput":{"path":"a.rs"},"status":"pending"})).unwrap();
        let end = from_acp(&json!({"toolCallId":"one","status":"completed"})).unwrap();
        let second = from_acp(&json!({"toolCallId":"two","title":"Read","rawInput":{"path":"b.rs"},"status":"failed"})).unwrap();
        let records = collect(&[
            start.marker(),
            end.marker(),
            start.marker(),
            second.marker(),
        ]);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name.as_deref(), Some("Read"));
        assert_eq!(records[0].status.as_deref(), Some("completed"));
        assert!(records[0].input.as_deref().unwrap().contains("a.rs"));
        assert!(records[1].input.as_deref().unwrap().contains("b.rs"));
        assert_eq!(records[1].status.as_deref(), Some("failed"));
    }

    #[test]
    fn claude_correlates_results_without_copying_them_or_inventing_success() {
        let calls = from_claude_line(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"one","name":"Bash","input":{"command":"echo héllo","password":"fixture-password"}}]}}).to_string());
        assert_eq!(calls[0].status.as_deref(), Some("in_progress"));
        assert!(!calls[0].marker().contains("fixture-password"));
        let results = from_claude_line(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"one","is_error":true,"content":"fixture-private-result"}]}}).to_string());
        let combined = collect(&[calls[0].marker(), results[0].marker()]);
        assert_eq!(combined[0].name.as_deref(), Some("Bash"));
        assert_eq!(combined[0].status.as_deref(), Some("failed"));
        assert!(!combined[0].marker().contains("fixture-private-result"));
    }

    #[test]
    fn inputs_are_bounded_after_redaction_and_missing_metadata_stays_unknown() {
        let trace =
            from_acp(&json!({"toolCallId":"é","rawInput":{"content":"é".repeat(8000)}})).unwrap();
        assert!(trace.input.unwrap().chars().count() <= MAX_INPUT_CHARS + 1);
        assert_eq!(trace.name, None);
        assert_eq!(trace.status, None);
        assert!(from_codex(&json!({"type":"agent_message","text":"not a tool"})).is_none());
        assert!(from_claude_line("noise").is_empty());
        assert!(collect(&[format!("{MARKER}invalid")]).is_empty());
    }
}
