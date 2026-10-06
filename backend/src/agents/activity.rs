//! The latest tool calls of a running agent, published for readers other than
//! the client streaming its output.
//!
//! A call leaves the run as a fixed category only (`ActivityCategory`), mapped
//! here from the agents' built-in tool names and ACP kinds. Its name,
//! arguments, targets and titles are never carried: any string an agent
//! controls can hold a secret.

use std::collections::VecDeque;

use serde_json::Value;

use crate::models::{ActivityCategory, AgentActivity, AuditActivityEntry, AuditRecentActivity};

/// Holds the latest activity of one launch; `None` until a tool call starts.
pub type AgentActivitySink = tokio::sync::watch::Sender<Option<AgentActivity>>;

/// Entries kept by [`RecentActivity`].
pub const RECENT_MAX_ENTRIES: usize = 15;

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

/// One step in a tool call's life: its start, or a later update of it.
/// Updates carrying the same id refine one call; they never announce another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivityUpdate {
    id: Option<String>,
    category: Option<ActivityCategory>,
}

impl ToolActivityUpdate {
    /// A call of the tool named `raw_name` started.
    pub fn named(id: Option<String>, raw_name: &str) -> Self {
        Self {
            id,
            category: Some(category_of(raw_name)),
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
            .map(|id| id.chars().take(128).collect::<String>());
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
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentActivity {
    entries: VecDeque<(Option<String>, AuditActivityEntry)>,
}

impl RecentActivity {
    /// Apply one update; `true` when it announced a call not seen before.
    pub fn apply(&mut self, update: &ToolActivityUpdate) -> bool {
        if let Some(id) = &update.id {
            if let Some((_, entry)) = self
                .entries
                .iter_mut()
                .find(|(known, _)| known.as_deref() == Some(id.as_str()))
            {
                if let Some(category) = update.category {
                    entry.category = category;
                }
                return false;
            }
        } else if update.category.is_none() {
            return false;
        }
        if self.entries.len() == RECENT_MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back((
            update.id.clone(),
            AuditActivityEntry {
                category: update.category.unwrap_or_default(),
                at: chrono::Utc::now(),
            },
        ));
        true
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
