//! The latest tool call of a running agent, published for readers other than
//! the client streaming its output.

use crate::models::AgentActivity;

/// Holds the latest activity of one launch; `None` until a tool call starts.
pub type AgentActivitySink = tokio::sync::watch::Sender<Option<AgentActivity>>;

/// Longest target kept, in characters.
const TARGET_MAX_CHARS: usize = 120;

pub fn tool_started(sink: Option<&AgentActivitySink>, tool: &str) {
    if let Some(sink) = sink {
        sink.send_replace(Some(AgentActivity {
            tool: tool.to_owned(),
            target: None,
            at: chrono::Utc::now(),
        }));
    }
}

/// Attach the completed input's target to the call started last.
pub fn tool_target(sink: Option<&AgentActivitySink>, target: String) {
    if let Some(sink) = sink {
        sink.send_if_modified(|current| match current {
            Some(activity) if activity.target.as_deref() != Some(target.as_str()) => {
                activity.target = Some(target);
                true
            }
            _ => false,
        });
    }
}

/// The most informative field of a tool's JSON input — file path, command,
/// pattern or URL — on one line and truncated.
pub fn tool_input_target(raw_input: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(raw_input).ok()?;
    let detail = ["file_path", "path", "command", "pattern", "url"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|field| field.as_str()))?
        .replace('\n', " ");
    if detail.chars().count() > TARGET_MAX_CHARS {
        let mut truncated: String = detail.chars().take(TARGET_MAX_CHARS).collect();
        truncated.push('…');
        Some(truncated)
    } else {
        Some(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_completes_the_call_started_last_and_never_invents_one() {
        let (sink, rx) = tokio::sync::watch::channel(None);
        tool_target(Some(&sink), "orphan".into());
        assert_eq!(*rx.borrow(), None, "no call started, nothing to complete");

        tool_started(Some(&sink), "Read");
        tool_target(Some(&sink), "src/lib.rs".into());
        let first = rx.borrow().clone().unwrap();
        assert_eq!(
            (first.tool.as_str(), first.target.as_deref()),
            ("Read", Some("src/lib.rs"))
        );

        tool_started(Some(&sink), "Bash");
        let second = rx.borrow().clone().unwrap();
        assert_eq!((second.tool.as_str(), second.target), ("Bash", None));
        assert!(second.at >= first.at);
    }

    #[test]
    fn the_target_is_the_informative_field_on_one_line_and_bounded() {
        assert_eq!(
            tool_input_target(r#"{"file_path":"docs/é.md","content":"x"}"#).as_deref(),
            Some("docs/é.md")
        );
        assert_eq!(
            tool_input_target(r#"{"command":"cargo test\n--lib"}"#).as_deref(),
            Some("cargo test --lib")
        );
        let long = format!(r#"{{"pattern":"{}"}}"#, "é".repeat(200));
        let target = tool_input_target(&long).unwrap();
        assert_eq!(target.chars().count(), TARGET_MAX_CHARS + 1);
        assert!(target.ends_with('…'));
        assert_eq!(tool_input_target(r#"{"todos":[]}"#), None);
        assert_eq!(tool_input_target("{not json"), None);
    }
}
