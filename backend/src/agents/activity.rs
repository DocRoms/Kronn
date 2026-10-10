//! The latest tool calls of a running agent, published for readers other than
//! the client streaming its output.
//!
//! A call leaves the run as a fixed category only (`ActivityCategory`), mapped
//! here from the agents' built-in tool names and ACP kinds. Its name,
//! arguments, targets and titles are never carried: any string an agent
//! controls can hold a secret.

use std::collections::{HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};

use serde_json::Value;

use crate::models::{ActivityCategory, AgentActivity, AuditActivityEntry, AuditRecentActivity};

/// Holds the latest activity of one launch; `None` until a tool call starts.
pub type AgentActivitySink = tokio::sync::watch::Sender<Option<AgentActivity>>;

/// Entries kept by [`RecentActivity`].
pub const RECENT_MAX_ENTRIES: usize = 15;
/// Call ids remembered to count each call once, well past the display buffer.
const SEEN_CALLS_MAX: usize = 4096;

/// One more tool call of `category`, counted where every call passes: a
/// reader polling the sink would miss the calls between two reads.
pub fn tool_started(sink: Option<&AgentActivitySink>, category: ActivityCategory) {
    if let Some(sink) = sink {
        sink.send_modify(|current| {
            let calls = current
                .as_ref()
                .map_or(0, |previous| previous.calls)
                .saturating_add(1);
            *current = Some(AgentActivity {
                category,
                at: chrono::Utc::now(),
                calls,
            });
        });
    }
}

/// The category of a tool, by its name as the agent's runtime reports it.
/// Unknown and custom names are `Other`; MCP tools are `Mcp`, Kronn's own
/// `Kronn`.
pub fn category_of(name: &str) -> ActivityCategory {
    use ActivityCategory::*;
    let name = name.trim();
    if let Some(rest) = name.strip_prefix("mcp__") {
        return if rest.starts_with("kronn") {
            Kronn
        } else {
            Mcp
        };
    }
    match name {
        // Claude Code, Codex, OpenCode, Vibe, Copilot, Gemini, Kronn's HTTP loop.
        "Read" | "NotebookRead" | "LS" | "read" | "list" | "read_file" | "list_dir"
        | "list_directory" | "read_many_files" | "view" | "view_image" | "list_files"
        | "git_status" | "git_diff" | "git_log" => Read,
        "Glob"
        | "Grep"
        | "ToolSearch"
        | "glob"
        | "grep"
        | "grep_files"
        | "search_file_content"
        | "search_text" => Search,
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "write" | "edit" | "multiedit"
        | "patch" | "apply_patch" | "write_file" | "search_replace" | "create" | "str_replace"
        | "str_replace_editor" | "insert" | "replace" | "edit_file" | "edit_lines"
        | "git_commit" => Edit,
        "Bash" | "BashOutput" | "KillShell" | "KillBash" | "bash" | "shell" | "local_shell"
        | "exec_command" | "write_stdin" | "run_shell_command" => Execute,
        "WebFetch" | "WebSearch" | "webfetch" | "websearch" | "web_search" | "web_fetch"
        | "google_web_search" | "api_call" | "api_endpoints" => Web,
        "TodoWrite" | "TodoRead" | "todowrite" | "todoread" | "todo" | "update_plan"
        | "ExitPlanMode" | "Task" | "Agent" | "task" | "Skill" | "SlashCommand"
        | "report_intent" | "save_memory" => Think,
        _ if KRONN_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) => Kronn,
        _ => Other,
    }
}

/// Kronn's own tools in the HTTP tool loop.
const KRONN_PREFIXES: &[&str] = &[
    "disc_",
    "task_",
    "plan_",
    "qa_",
    "qe_",
    "qp_",
    "agent_",
    "media_",
    "mcp_list",
    "tool_manual",
    "tools_load",
];

/// The category of a generic ACP tool kind.
pub fn category_of_acp_kind(kind: &str) -> ActivityCategory {
    use ActivityCategory::*;
    match kind {
        "read" => Read,
        "search" => Search,
        "edit" | "delete" | "move" => Edit,
        "execute" => Execute,
        "fetch" => Web,
        "think" | "switch_mode" => Think,
        _ => Other,
    }
}

/// Whether a runtime's line is the model reasoning: Claude's thinking blocks,
/// Codex's reasoning items, an HTTP model's reasoning field. Only the shape is
/// read, never the text.
pub fn is_reasoning_frame(line: &str) -> bool {
    let line = line.trim();
    // An OpenAI-wire stream frames each chunk as server-sent `data:`.
    let line = line.strip_prefix("data:").map_or(line, str::trim);
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let non_empty = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    // Ollama's `message.thinking`, an OpenAI-wire `reasoning_content`/`reasoning`.
    if non_empty("/message/thinking")
        || non_empty("/choices/0/delta/reasoning_content")
        || non_empty("/choices/0/delta/reasoning")
    {
        return true;
    }
    let event = value.get("event").unwrap_or(&value);
    let str_at = |root: &Value, pointer: &str| {
        root.pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    matches!(
        str_at(event, "/delta/type").as_deref(),
        Some("thinking_delta" | "signature_delta")
    ) || str_at(event, "/content_block/type").as_deref() == Some("thinking")
        || str_at(&value, "/item/type").as_deref() == Some("reasoning")
}

/// One step in a tool call's life: its start, or a later update of it.
/// Updates carrying the same id refine one call; they never announce another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivityUpdate {
    /// The call id's hash: bounded whatever the runtime sends, and two ids
    /// sharing a long prefix stay apart.
    id: Option<u64>,
    category: Option<ActivityCategory>,
}

fn call_identity(id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

impl ToolActivityUpdate {
    /// A call of the tool named `raw_name` started.
    pub fn named(id: Option<String>, raw_name: &str) -> Self {
        Self {
            id: id.as_deref().map(call_identity),
            category: Some(category_of(raw_name)),
        }
    }

    /// A call of a known category, as a runtime that reports kinds gives it.
    pub fn of_category(id: Option<&str>, category: ActivityCategory) -> Self {
        Self {
            id: id.map(call_identity),
            category: Some(category),
        }
    }

    pub fn category(&self) -> Option<ActivityCategory> {
        self.category
    }

    /// A generic ACP `tool_call` / `tool_call_update`: its id and kind. Its
    /// title, raw input and locations are never read.
    pub fn from_acp(update: &Value) -> Option<Self> {
        let call = update.get("toolCall").unwrap_or(update);
        let id = call
            .get("toolCallId")
            .or_else(|| update.get("toolCallId"))
            .and_then(Value::as_str)
            .map(call_identity);
        let category = call
            .get("kind")
            .and_then(Value::as_str)
            .map(category_of_acp_kind);
        (id.is_some() || category.is_some()).then_some(Self { id, category })
    }

    /// Whether it says more than the call's id.
    pub fn carries_detail(&self) -> bool {
        self.category.is_some()
    }
}

/// The latest tool calls of a running agent, shown to users. In memory only.
/// Which calls were already counted is tracked apart from the 15 shown, so a
/// call's later update never counts it again once its entry scrolled away.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentActivity {
    entries: VecDeque<(Option<u64>, AuditActivityEntry)>,
    seen: HashSet<u64>,
    seen_order: VecDeque<u64>,
}

impl RecentActivity {
    /// Apply one update; `true` when it announced a call not seen before. A
    /// call first seen without a kind counts, as `Other`.
    pub fn apply(&mut self, update: &ToolActivityUpdate) -> bool {
        match update.id {
            Some(id) if self.seen.contains(&id) => {
                if let (Some(category), Some((_, entry))) = (
                    update.category,
                    self.entries
                        .iter_mut()
                        .find(|(known, _)| *known == Some(id)),
                ) {
                    entry.category = category;
                }
                false
            }
            Some(id) => {
                if self.seen_order.len() == SEEN_CALLS_MAX {
                    if let Some(oldest) = self.seen_order.pop_front() {
                        self.seen.remove(&oldest);
                    }
                }
                self.seen.insert(id);
                self.seen_order.push_back(id);
                self.push(Some(id), update.category.unwrap_or_default());
                true
            }
            None => match update.category {
                Some(category) => {
                    self.push(None, category);
                    true
                }
                None => false,
            },
        }
    }

    fn push(&mut self, id: Option<u64>, category: ActivityCategory) {
        if self.entries.len() == RECENT_MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back((
            id,
            AuditActivityEntry {
                category,
                at: chrono::Utc::now(),
            },
        ));
    }

    /// Newest first.
    pub fn snapshot(&self) -> AuditRecentActivity {
        AuditRecentActivity {
            entries: self
                .entries
                .iter()
                .rev()
                .map(|(_, entry)| entry.clone())
                .collect(),
        }
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
pub(crate) mod tests;
