//! Kronn primitives exposed to HTTP agents (Ollama, LiteLLM).
//!
//! CLI agents reach these through the `kronn-internal` stdio bridge. HTTP
//! agents have no bridge process, so the orchestrator executes on their
//! behalf — in-process, against the same handlers the bridge calls over HTTP.
//!
//! The catalogue is deliberately small. Every tool costs context on a local
//! model with a tight window, and a model given forty options picks worse
//! than one given four. It exposes the API/Quick-API primitives plus the
//! compact Planning contract that every discussion agent needs to keep durable
//! task context honest.

use crate::agents::tools::{ToolCall, ToolExecutor, ToolOutcome};
use crate::models::{
    AddPlanningBlockerRequest, AgentType, CreatePlanningTaskRequest, LinkPlanningDiscussionRequest,
    PlanningActor, PlanningActorKind, PlanningTaskListQuery, RemovePlanningBlockerRequest,
    UpdatePlanningDodItemRequest, UpdatePlanningTaskRequest,
};
use crate::AppState;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(test)]
#[path = "agent_quick_prompt_tests.rs"]
mod quick_prompt_tests;

#[cfg(test)]
#[path = "agent_quick_prompt_bench.rs"]
mod quick_prompt_bench;

#[cfg(test)]
#[path = "agent_signal_bench.rs"]
mod signal_bench;

#[path = "agent_workflow_tools.rs"]
mod workflow_tools;

pub struct KronnToolExecutor {
    state: AppState,
    /// Scopes `api_call` to the calling conversation when there is one, so the
    /// broker can resolve project-scoped credentials the same way it does for
    /// CLI agents.
    disc_id: Option<String>,
    /// Explicit workflow project scope. Discussions derive the project from
    /// `disc_id`; workflow runs have no discussion and must never fall back to
    /// an arbitrary plugin configuration from another project.
    project_id: Option<String>,
    /// Present for workflow steps. Used for durable Planning attribution and
    /// to expose only the workflow-safe subset of the native catalogue.
    workflow_run_id: Option<String>,
    /// Durable attribution written to Planning's event log. The model cannot
    /// override this value in tool arguments.
    actor_id: String,
    /// Typed native provider for a discussion agent. Kept separately from the
    /// display label so execution authorization never parses presentation text.
    actor_type: Option<AgentType>,
    /// The User message that caused this run, when known.
    source_message_id: Option<String>,
    /// Durable dispatch that owns this native run. Unlike the message id this
    /// survives transcript rewrites and anchors resume-chain accounting.
    source_dispatch_job_id: Option<String>,
    /// This run is a worker inside its own execution room (KT-398). It narrows
    /// the catalogue: a worker has one task, already briefed, and no business
    /// browsing or mutating the backlog.
    worker_room: bool,
    /// Principal-authored mechanical target for a tiny native HTTP worker.
    /// Kept separate from the prompt so the runner can freeze its catalogue.
    worker_scope: Option<crate::models::TaskWorkerScope>,
}

impl KronnToolExecutor {
    pub fn new(state: AppState, disc_id: Option<String>) -> Self {
        Self {
            state,
            disc_id,
            project_id: None,
            workflow_run_id: None,
            actor_id: "Kronn HTTP agent".into(),
            actor_type: None,
            source_message_id: None,
            source_dispatch_job_id: None,
            worker_room: false,
            worker_scope: None,
        }
    }

    /// Wrap into the `Arc` the runner takes, or `None` when tools are off.
    pub fn arc(
        state: AppState,
        disc_id: Option<String>,
        actor_type: AgentType,
        source_message_id: Option<String>,
        source_dispatch_job_id: Option<String>,
    ) -> std::sync::Arc<dyn ToolExecutor> {
        let actor_id = crate::api::disc_helpers::agent_display_name(&actor_type);
        std::sync::Arc::new(Self {
            state,
            disc_id,
            project_id: None,
            workflow_run_id: None,
            actor_id,
            actor_type: Some(actor_type),
            source_message_id,
            source_dispatch_job_id,
            worker_room: false,
            worker_scope: None,
        })
    }

    /// Same run, but inside a worker's execution room: the catalogue is cut
    /// down to what delivering that one task needs.
    #[allow(clippy::too_many_arguments)]
    pub fn arc_for_worker_room(
        state: AppState,
        disc_id: Option<String>,
        actor_type: AgentType,
        source_message_id: Option<String>,
        source_dispatch_job_id: Option<String>,
        worker_scope: Option<crate::models::TaskWorkerScope>,
    ) -> std::sync::Arc<dyn ToolExecutor> {
        let actor_id = crate::api::disc_helpers::agent_display_name(&actor_type);
        std::sync::Arc::new(Self {
            state,
            disc_id,
            project_id: None,
            workflow_run_id: None,
            actor_id,
            actor_type: Some(actor_type),
            source_message_id,
            source_dispatch_job_id,
            worker_room: true,
            worker_scope,
        })
    }

    /// Native tools for an HTTP Agent step. The workflow-safe catalogue has
    /// no discussion-bound plan mutations and carries explicit project/run
    /// attribution into API and Planning reads.
    pub fn workflow_arc(
        state: AppState,
        project_id: Option<String>,
        workflow_run_id: String,
        step_name: String,
    ) -> std::sync::Arc<dyn ToolExecutor> {
        std::sync::Arc::new(Self {
            state,
            disc_id: None,
            project_id,
            actor_id: format!("Workflow {workflow_run_id} · {step_name}"),
            actor_type: None,
            workflow_run_id: Some(workflow_run_id),
            source_message_id: None,
            source_dispatch_job_id: None,
            worker_room: false,
            worker_scope: None,
        })
    }
}

fn ok(call: &ToolCall, content: Value) -> ToolOutcome {
    ToolOutcome {
        call: call.clone(),
        content,
        ok: true,
    }
}

/// Failures come back as data, not as a killed turn: a model that reads
/// "unknown tool" or "missing field" can correct itself, whereas aborting the
/// run just loses the conversation.
fn fail(call: &ToolCall, message: impl Into<String>) -> ToolOutcome {
    ToolOutcome {
        call: call.clone(),
        content: json!({ "error": message.into() }),
        ok: false,
    }
}

/// The advertised tool schemas. A free function so it can be asserted on
/// without standing up an `AppState` — it depends on nothing else.
/// Optional tool families used when KRONN_TIERED_TOOLS=1; full declarations are
/// the default. Core tools stay directly available for discovery. Loaded families
/// remain available for the run. Quick Prompt batch launch and deletion are omitted
/// to prevent unbounded fan-out and removal of saved run history.
pub(crate) const TOOL_FAMILIES: &[(&str, &str, &[&str])] = &[
    (
        "media",
        "make an image or a video: `media_generate`, `media_job_status` (the connection comes from `agent_list`, in the core)",
        &["media_generate", "media_job_status"],
    ),
    (
        "delegation",
        "give a task to a worker agent and follow it: `task_exec_prepare`, `task_exec_launch`, `task_exec_status`, `task_exec_review`",
        &[
            // Keep agent_list in the core so worker and media-provider discovery needs no
            // preliminary family load.
            "task_exec_prepare",
            "task_exec_launch",
            "task_exec_status",
            "task_exec_deliver",
            "task_exec_review",
            "task_exec_cancel",
            "task_exec_reassign",
            "agent_job_start",
            "agent_schedule_wake",
            "agent_resume_status",
            "agent_resume_cancel",
        ],
    ),
    (
        "edit",
        "change files in the workspace and commit: `write_file`, `edit_file`, `edit_lines`, `insert_after_line`, `git_commit`",
        &[
            "write_file",
            "edit_file",
            "edit_lines",
            "insert_after_line",
            "git_commit",
        ],
    ),
    (
        "automations",
        "save and run APIs, commands and prompts; author disabled workflows: `qa_create_draft`, `qa_update`, `qe_create_draft`, `qe_update`, `qe_run`, `qe_list`, `qp_list`, `qp_run`, `qp_create_draft`, `qp_update`, `workflow_list`, `workflow_get`, `workflow_step_schema`, `workflow_create_draft`, `workflow_update`",
        &[
            "qa_create_draft",
            "qa_update",
            "qe_create_draft",
            "qe_update",
            "qe_run",
            "qe_list",
            "qp_list",
            "qp_run",
            "qp_create_draft",
            "qp_update",
            "workflow_list",
            "workflow_get",
            "workflow_step_schema",
            "workflow_create_draft",
            "workflow_update",
        ],
    ),
];

/// Use full declarations by default. Native-model measurements in docs/research/
/// showed lower success with tiering; keep it opt-in via KRONN_TIERED_TOOLS=1.
pub(crate) fn tiered_tools_enabled() -> bool {
    explicit_tiering(std::env::var("KRONN_TIERED_TOOLS").ok().as_deref())
}

fn explicit_tiering(raw: Option<&str>) -> bool {
    raw.map(str::trim) == Some("1")
}

/// The first sentence of a tool's description, for the index a loaded family
/// returns. A weak model that received only names went looking elsewhere.
fn first_sentence(description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.find(". ") {
        Some(end) => flat[..=end].trim().to_string(),
        None => flat.chars().take(160).collect(),
    }
}

fn family_of(name: &str) -> Option<&'static str> {
    TOOL_FAMILIES
        .iter()
        .find(|(_, _, tools)| tools.contains(&name))
        .map(|(family, _, _)| *family)
}

pub(crate) fn declarations_for_family(family: &str) -> Vec<Value> {
    let Some((_, _, names)) = TOOL_FAMILIES.iter().find(|(id, _, _)| *id == family) else {
        return Vec::new();
    };
    full_discussion_catalogue()
        .into_iter()
        .filter(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_some_and(|name| names.contains(&name))
        })
        .collect()
}

fn tools_load_declaration() -> Value {
    let index = TOOL_FAMILIES
        .iter()
        .map(|(family, what, _)| format!("`{family}` — {what}"))
        .collect::<Vec<_>>()
        .join(" | ");
    json!({
        "type": "function",
        "function": {
            "name": "tools_load",
            "description": format!(
                "Load one family of Kronn tools, then use them. Families: {index}. A tool named there EXISTS and works; it is simply not declared yet, to keep this conversation small. So a tool you cannot see is never unavailable: it is one `tools_load` away, and saying it is missing is always wrong. Call `tools_load` with its family FIRST, then call the tool it named. A request needing two families loads them ONE AT A TIME: load the first, finish that part, then load the second and finish the rest. Do not look for another route, and never answer that it cannot be done."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "family": {
                        "type": "string",
                        "enum": TOOL_FAMILIES.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
                    },
                },
                "required": ["family"],
            },
        },
    })
}

/// The catalogue as it stands before the tiers are applied.
pub(crate) fn full_discussion_catalogue() -> Vec<Value> {
    let mut catalogue = tool_catalogue();
    catalogue.extend(workspace_tool_catalogue());
    catalogue.extend(orchestration_tool_catalogue());
    catalogue.extend(agent_resume_tool_catalogue());
    catalogue
}

/// Split a catalogue into what a run starts with, plus the index.
pub(crate) fn tiered(catalogue: Vec<Value>) -> Vec<Value> {
    let mut core: Vec<Value> = catalogue
        .into_iter()
        .filter(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_none_or(|name| family_of(name).is_none())
        })
        .collect();
    core.push(tools_load_declaration());
    core
}

pub fn tool_catalogue() -> Vec<Value> {
    let mut catalogue = vec![
        json!({
            "type": "function",
            "function": {
                "name": "mcp_list",
                "description": "List the API plugins available in this Kronn instance \
                                (slug + what each is for). Call this first to find a slug, \
                                then api_endpoints(slug) to see its paths.",
                "parameters": { "type": "object", "properties": {}, "required": [] },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "api_endpoints",
                "description": "List the endpoint paths one plugin exposes. Use the slug from \
                                mcp_list. Returns the paths you can pass to api_call.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "api_plugin_slug": { "type": "string", "description": "slug from mcp_list" },
                    },
                    "required": ["api_plugin_slug"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qa_list",
                "description": "List saved Quick APIs (pre-configured calls). Prefer running \
                                one of these over hand-building an api_call when it matches.",
                "parameters": { "type": "object", "properties": {}, "required": [] },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qe_list",
                "description": "List saved Quick Execs — the commands this instance can run \
                                for you. Pass one's id to agent_job_start; there is no other \
                                way to learn it.",
                "parameters": { "type": "object", "properties": {}, "required": [] },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qa_run",
                "description": "Execute a saved Quick API by id and return its result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "quick_api_id": { "type": "string", "description": "id from qa_list" },
                        "variables": {
                            "type": "object",
                            "additionalProperties": { "type": "string" },
                            "description": "Values for the Quick API's variables, keyed by the \
                                            names qa_list reports in required_variables.",
                        },
                    },
                    "required": ["quick_api_id"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qe_run",
                "description": "Run a saved Quick Exec now and return its output. \
                                Shell-free: the saved argv runs directly, bounded to \
                                its project. For a long one, prefer agent_job_start, \
                                which survives this turn.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "quick_exec_id": { "type": "string", "description": "id from qe_list" },
                        "variables": {
                            "type": "object",
                            "additionalProperties": { "type": "string" },
                            "description": "Values for the names qe_list reports in required_variables.",
                        },
                    },
                    "required": ["quick_exec_id"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qa_create_draft",
                "description": "Save a new Quick API so this call can be replayed and \
                                audited instead of hand-built each time. Kronn injects \
                                the credentials; you never pass one. Shape and examples: \
                                tool_manual({tool: \"qa_create_draft\"}).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "api_plugin_slug": { "type": "string", "description": "slug from mcp_list" },
                        "api_config_id": { "type": "string", "description": "config id from mcp_list" },
                        "api_endpoint_path": { "type": "string", "description": "path from api_endpoints" },
                        "api_method": { "type": "string", "description": "Defaults to GET." },
                        "api_query": { "type": "object", "additionalProperties": { "type": "string" } },
                        "api_path_params": { "type": "object", "additionalProperties": { "type": "string" } },
                        "api_body": { "type": "object" },
                        "project_id": { "type": "string", "description": "Omit to make it general." },
                        "variables": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "Each: {name, label, placeholder, required}. `label` and `placeholder` are what a human sees in the launcher, and both are required.",
                        },
                    },
                    "required": ["name", "api_plugin_slug", "api_config_id", "api_endpoint_path"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qa_update",
                "description": "Change a saved Quick API. Send only the fields you are \
                                changing; Kronn keeps the rest of the stored definition \
                                as it is.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "quick_api_id": { "type": "string", "description": "id from qa_list" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "api_plugin_slug": { "type": "string" },
                        "api_config_id": { "type": "string" },
                        "api_endpoint_path": { "type": "string" },
                        "api_method": { "type": "string" },
                        "api_query": { "type": "object", "additionalProperties": { "type": "string" } },
                        "api_path_params": { "type": "object", "additionalProperties": { "type": "string" } },
                        "api_body": { "type": "object" },
                        "project_id": { "type": "string" },
                        "variables": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "Each: {name, label, placeholder, required}. `label` and `placeholder` are what a human sees in the launcher, and both are required.",
                        },
                    },
                    "required": ["quick_api_id"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qp_list",
                "description": "List saved Quick Prompts available in this discussion, with their ids and required variables.",
                "parameters": {"type": "object", "properties": {}, "required": []},
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qp_run",
                "description": "Launch one saved Quick Prompt in a new discussion, using its saved model, persona and rules. Tracking: tool_manual({tool: \"qp_run\"}).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "qp_id": {"type": "string", "description": "id from qp_list"},
                        "vars": {"type": "object", "additionalProperties": {"type": "string"}},
                        "title": {"type": "string"},
                    },
                    "required": ["qp_id"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qp_create_draft",
                "description": "Save a reusable prompt without running it. Shape and model defaults: tool_manual({tool: \"qp_create_draft\"}).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "description": {"type": "string"},
                        "icon": {"type": "string"},
                        "prompt_template": {"type": "string"},
                        "project_id": {"type": ["string", "null"], "description": "Defaults to this discussion's project. Explicit null creates a general prompt."},
                        "variables": {
                            "type": "array", "items": {"type": "object"},
                            "description": "Each: {name, label, placeholder, required}. Both label and placeholder are required.",
                        },
                    },
                    "required": ["name", "prompt_template"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qp_update",
                "description": "Change only the named fields of a saved Quick Prompt, keeping its model, persona and rules. Merge details: tool_manual({tool: \"qp_update\"}).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "qp_id": {"type": "string", "description": "id from qp_list"},
                        "name": {"type": "string"},
                        "description": {"type": "string"},
                        "icon": {"type": "string"},
                        "prompt_template": {"type": "string"},
                        "variables": {
                            "type": "array", "items": {"type": "object"},
                            "description": "Each: {name, label, placeholder, required}. Both label and placeholder are required.",
                        },
                    },
                    "required": ["qp_id"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qe_create_draft",
                "description": "Save a new Quick Exec: one command, its argv, its \
                                timeout. No shell — no pipes, no redirection, no \
                                globbing. Shape and the argv rules: \
                                tool_manual({tool: \"qe_create_draft\"}).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "command": { "type": "string", "description": "The binary alone, no arguments." },
                        "args": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "One element per argument. `{{var}}` is substituted.",
                        },
                        "timeout_secs": { "type": "integer" },
                        "output_format": {
                            "type": "string",
                            "enum": ["text", "json", "csv", "lines"],
                            "description": "How the output is parsed for a caller.",
                        },
                        "project_id": { "type": "string", "description": "Omit to make it general." },
                        "variables": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "Each: {name, label, placeholder, required}. `label` and `placeholder` are what a human sees in the launcher, and both are required.",
                        },
                    },
                    "required": ["name", "command"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "qe_update",
                "description": "Change a saved Quick Exec. Send only the fields you are \
                                changing; Kronn keeps the rest of the stored definition \
                                as it is. The argv rules are the same as qe_create_draft.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "quick_exec_id": { "type": "string", "description": "id from qe_list" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "command": { "type": "string" },
                        "args": { "type": "array", "items": { "type": "string" } },
                        "timeout_secs": { "type": "integer" },
                        "output_format": { "type": "string", "enum": ["text", "json", "csv", "lines"] },
                        "project_id": { "type": "string" },
                        "variables": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "Each: {name, label, placeholder, required}. `label` and `placeholder` are what a human sees in the launcher, and both are required.",
                        },
                    },
                    "required": ["quick_exec_id"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tool_manual",
                "description": "Read one tool's contract, or `signals` for proposal schemas and UI actions. Omit `tool` to list manuals.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "tool": { "type": "string", "description": "Tool name or signals; omit to list." },
                    },
                    "required": [],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "api_call",
                "description": "Call a Kronn-configured API. Credentials are injected by \
                                Kronn server-side and never exposed. Use mcp_list first to \
                                find the plugin slug and endpoint path.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "api_plugin_slug": { "type": "string", "description": "slug from mcp_list" },
                        "api_config_id": { "type": "string", "description": "optional — Kronn resolves it from the slug; only pass one to disambiguate a plugin wired several times" },
                        "endpoint_path": { "type": "string", "description": "path from api_endpoints, e.g. /v1/sites" },
                        "method": { "type": "string", "description": "GET (default), POST, …" },
                        "query": { "type": "object", "description": "query-string parameters" },
                    },
                    "required": ["api_plugin_slug", "endpoint_path"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "plan_get",
                "description": "Read this discussion's compact shared plan: primary objective, active/later tasks and progress. Call before creating or changing tracked work.",
                "parameters": { "type": "object", "properties": {}, "required": [] },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_list",
                "description": "Find compact Planning task summaries linked to this discussion. Use task_get only after choosing one result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "search": { "type": "string" },
                        "status": { "type": "string", "enum": ["idea", "todo", "in_progress", "blocked", "done", "archived"] },
                        "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                        "project_id": { "type": "string" },
                        "tag": { "type": "string" },
                        "with_discussion": { "type": "boolean" },
                        "cursor": { "type": "integer", "minimum": 0 },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                    },
                    "required": []
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_get",
                "description": "Read one full Planning task by KT reference or UUID, including DoD, blockers and history.",
                "parameters": {
                    "type": "object",
                    "properties": { "task_id": { "type": "string" } },
                    "required": ["task_id"]
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_create",
                "description": "Create one task and atomically add it to this discussion's plan. Call plan_get first. Use one distinct idempotency_key per logical task when creating several.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "idempotency_key": { "type": "string" },
                        "description": { "type": "string" },
                        "status": { "type": "string", "enum": ["idea", "todo", "in_progress", "blocked", "done", "archived"] },
                        "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                        "parent_id": { "type": "string" },
                        "project_ids": { "type": "array", "items": { "type": "string" } },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "definition_of_done": { "type": "array", "items": { "type": "object" } },
                        "links": { "type": "array", "items": { "type": "object" } }
                    },
                    "required": ["title"]
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_update",
                "description": "Patch one Planning task by KT reference or UUID. Only supplied fields change.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "status": { "type": "string", "enum": ["idea", "todo", "in_progress", "blocked", "done", "archived"] },
                        "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                        "parent_id": { "type": ["string", "null"] },
                        "blocked_reason": { "type": ["string", "null"] },
                        "rank": { "type": "integer" },
                        "project_ids": { "type": "array", "items": { "type": "string" } },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "definition_of_done": { "type": "array", "items": { "type": "object" } },
                        "links": { "type": "array", "items": { "type": "object" } }
                    },
                    "required": ["task_id"]
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_update_dod",
                "description": "Check or uncheck one DoD item atomically, using ids returned by task_get.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string" },
                        "dod_id": { "type": "string" },
                        "completed": { "type": "boolean" }
                    },
                    "required": ["task_id", "dod_id", "completed"]
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_link_discussion",
                "description": "Link an existing task to this discussion as active/later or as its primary objective.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string" },
                        "placement": { "type": "string", "enum": ["active", "later"] },
                        "is_primary": { "type": "boolean" },
                        "position": { "type": "integer" }
                    },
                    "required": ["task_id"]
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_add_blocker",
                "description": "Declare that one task is blocked by another. Cycles are rejected.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string" },
                        "blocker_task_id": { "type": "string" }
                    },
                    "required": ["task_id", "blocker_task_id"]
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "task_remove_blocker",
                "description": "Remove one dependency edge without changing task status or blocked reason. Safe to retry.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string" },
                        "blocker_task_id": { "type": "string" }
                    },
                    "required": ["task_id", "blocker_task_id"]
                },
            },
        }),
    ];
    catalogue.extend(workflow_tools::declarations());
    catalogue
}

/// Discussion-bound task execution lifecycle for native HTTP agents. These are
/// intentionally separate from the general catalogue: workflow Agent steps have no
/// principal/worker room identity, and every schema omits caller ids because Kronn
/// derives them from the trusted executor.
/// Whether this name belongs to the orchestration catalogue.
///
/// Derived from the catalogue itself so a tool cannot be declared without being
/// routable. Declaring one and forgetting this step is invisible: the model is
/// told it has the tool, calls it, and is answered "unknown tool".
/// A stable idempotency key for a media generation the agent did not key
/// itself.
///
/// The server keys a job by whatever it is handed and falls back to a random
/// id, so an agent retrying the same generation creates a second job and pays
/// for the asset twice. Deriving the key from the request makes the retry
/// collapse onto the first job instead — the safe behaviour has to be the one
/// you get by saying nothing, because this one costs money.
///
/// The discussion is in the digest so two rooms asking for the same picture
/// still get their own asset; the tool arguments are, so changing anything the
/// operator would see in the result is a different job.
fn derived_media_idempotency_key(discussion_id: &str, arguments: &Value) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    // v2: the connection is no longer part of the key. The server binds the
    // job to the connection it resolved, so an alias and the id it stands for
    // are one generation, and two connections are still two jobs.
    digest.update(b"kronn-agent-media-v2\0");
    digest.update(discussion_id.as_bytes());
    for field in [
        "modality",
        "prompt",
        "duration_secs",
        "resolution",
        "aspect_ratio",
        "generate_audio",
        "reference_asset_ids",
        "reference_mode",
    ] {
        digest.update(b"\0");
        digest.update(field.as_bytes());
        digest.update(b"=");
        // Serialised rather than stringified so `null`, `"1"` and `1` stay
        // distinguishable — a collision here is a generation silently skipped.
        digest.update(
            serde_json::to_string(arguments.get(field).unwrap_or(&Value::Null))
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn is_orchestration_tool(name: &str) -> bool {
    orchestration_tool_catalogue()
        .iter()
        .any(|tool| tool["function"]["name"] == name)
}

enum ToolDispatch {
    DiscussionRead,
    DiscussionList,
    Workspace,
    Orchestration,
    Resume,
    Core,
}

fn tool_dispatch(name: &str) -> ToolDispatch {
    // A declared tool still needs to reach its handler. Media generation and
    // family loading both previously fell through at this boundary.
    match name {
        "disc_read" => ToolDispatch::DiscussionRead,
        "disc_list" => ToolDispatch::DiscussionList,
        // The family loader is declared by tiered(), outside the orchestration list.
        "tools_load" => ToolDispatch::Orchestration,
        _ if crate::api::agent_workspace_tools::TOOL_NAMES.contains(&name) => {
            ToolDispatch::Workspace
        }
        _ if is_orchestration_tool(name) => ToolDispatch::Orchestration,
        "agent_job_start"
        | "agent_schedule_wake"
        | "agent_resume_status"
        | "agent_resume_cancel" => ToolDispatch::Resume,
        _ => ToolDispatch::Core,
    }
}

fn orchestration_tool_catalogue() -> Vec<Value> {
    let tool = |name: &str, description: &str, properties: Value, required: Value| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                },
            },
        })
    };
    let mut delivery_manifest_schema = crate::models::delivery_manifest_v1_schema();
    delivery_manifest_schema["description"] = json!(
        "DeliveryManifest v1. Every listed field is required, including empty arrays; use the exact task and DoD ids from the pinned worker brief."
    );
    let worker_scope_schema = json!({
        "type": "object",
        "description": "Optional mechanical scope for a tiny HTTP worker mutation. Kronn validates the closed shape in the pinned worktree and obtains a fresh CAS receipt there.",
        "properties": {
            "mode": {"type": "string", "enum": ["prelocalized_edit", "prelocalized_insert_after"]},
            "path": {"type": "string", "minLength": 1},
            "start_line": {"type": "integer", "minimum": 1, "description": "Required only for prelocalized_edit."},
            "end_line": {"type": "integer", "minimum": 1, "description": "Required only for prelocalized_edit."},
            "anchor_line": {"type": "integer", "minimum": 1, "description": "Required only for prelocalized_insert_after."}
        },
        "required": ["mode", "path"],
        "additionalProperties": false
    });
    vec![
        tool(
            "agent_list",
            "List the worker identities this principal room can pass verbatim to task_exec_prepare, and the media connections `media_generate` can be billed on. Separates configured, reachable and available with stable secret-free reason codes; availability proves transport readiness only, never task or model success.",
            json!({}),
            json!([]),
        ),
        tool(
            "media_generate",
            "Generate an image or a video on a configured HTTP connection; returns {job_id, status, model, connection_id}. Never ask a human for the connection: omit it and Kronn uses the only one configured for this modality, or names the candidates when there are several. The configured slot fixes the model — you do not choose it; a modality with no slot is refused. `agent_list`'s `media` lists one entry per configured modality, with the `connection_id` and the `capabilities` advertised: take duration, resolution and ratio from there, never from habit. A video can start from a picture: pass the context file id of an image from this room in `reference_asset_ids` — generate the image first, then feed it in. Billed: video per second, image per picture. Keep the clip short. The asset lands in the discussion on its own as a context file, so poll `media_job_status` only when you need it inside this very answer.",
            json!({
                "connection_id": {
                    "type": "string",
                    "description": "Optional. Omit it and Kronn uses the only connection configured for this modality; pass an id or an alias, from `agent_list`, only when several can serve it."
                },
                "modality": {"type": "string", "enum": ["image", "video"]},
                "prompt": {"type": "string"},
                "duration_secs": {"type": "integer"},
                "resolution": {"type": "string"},
                "aspect_ratio": {"type": "string"},
                "generate_audio": {"type": "boolean"},
                "reference_asset_ids": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Context file ids of pictures the generation starts from — typically an image you just generated in this room. A video can be driven by one; an image can be varied from one. That picture may also be the LAST IMAGE OF AN EARLIER CLIP: that is how several videos are chained into one continuous sequence. Kronn cannot cut that image out of a clip itself — ask the human to open the clip in the discussion's Assets carousel and keep its last image, which arrives here as an ordinary context file you then pass in this list."
                },
                "wake_when_ready": {
                    "type": "boolean",
                    "description": "Wake this room when the generation settles, success or failure, instead of you polling for it. Use it when the result drives your next step — feeding the image into a video, or reporting what came back. Without it the asset still arrives in the room on its own; you simply are not told."
                },
                "reference_mode": {
                    "type": "string",
                    "enum": ["first_frame", "last_frame", "reference"],
                    "description": "What the referenced picture is to a video: the exact opening frame, the exact closing frame, or a visual the model draws from. Defaults to first_frame for a video."
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "Reuse the SAME value when you retry a generation you already asked for: it is what stops the asset being bought, and billed, a second time. Omit it and Kronn derives one from this request, so an identical retry collapses onto the first job rather than paying twice. Pass a new value only when you genuinely want another, separate take."
                }
            }),
            json!(["modality", "prompt"]),
        ),
        tool(
            "media_job_status",
            "Read one media generation job by id: status, model and the asset once it is ready. The asset reaches the discussion by itself; call this only when the result is needed within the current answer.",
            json!({"job_id": {"type": "string", "description": "From `media_generate`."}}),
            json!(["job_id"]),
        ),
        tool(
            "task_exec_prepare",
            "Preflight a Todo task from this principal room. Returns launchable plus stable reasons; creates nothing. Call before launch. Delegate to Ollama only for one atomic unit with explicit scope and principal-owned mechanical validations. Escalate immediately for trust or protocol boundaries, concurrency, migrations, architecture, or cross-cutting parity. The principal reviews the delivered SHA and runs its validations. Allow at most one targeted local rework, then reassign to a stronger worker.",
            json!({
                "task_reference": {"type": "string"},
                "worker": {"type": "object", "description": "Typed MessageTarget: kind, agent_type, optional exact cli_session_id and tier."},
                "worker_scope_intent": {"type": "string", "enum": ["generic", "scoped"], "description": "Required sentinel proving the current tool contract was transported. scoped requires worker_scope; generic forbids it."},
                "worker_scope": worker_scope_schema.clone()
            }),
            json!(["task_reference", "worker", "worker_scope_intent"]),
        ),
        tool(
            "task_exec_launch",
            "Launch a task accepted by preflight from this principal room. Creates a durable execution, child discussion and worktree. Use one stable idempotency_key for retries. Delegate to Ollama only for one atomic unit with explicit scope and principal-owned mechanical validations. Escalate immediately for trust or protocol boundaries, concurrency, migrations, architecture, or cross-cutting parity. The principal reviews the delivered SHA and runs its validations. Allow at most one targeted local rework, then reassign to a stronger worker.",
            json!({
                "task_reference": {"type": "string"},
                "worker": {"type": "object"},
                "worker_scope_intent": {"type": "string", "enum": ["generic", "scoped"], "description": "Must exactly match the preflighted scope intent."},
                "worker_scope": worker_scope_schema,
                "base_rev": {"type": "string"},
                "idempotency_key": {"type": "string"},
                "validations": {
                    "type": "array",
                    "description": "Principal-owned mechanical gates run on the candidate before integration. Never read from the worker's own manifest.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "command": {"type": "string", "minLength": 1},
                            "quick_exec_id": {"type": "string"},
                            "timeout_secs": {"type": "integer", "minimum": 1}
                        },
                        "required": ["command"],
                        "additionalProperties": false
                    }
                }
            }),
            json!(["task_reference", "worker", "worker_scope_intent"]),
        ),
        tool(
            "task_exec_status",
            "Read one execution you are a party to, including state, lineage, DoD, attempts, validation, recovery, duration and honest token telemetry. After reconnect, a KT task_reference recovers its active/latest execution.",
            json!({
                "task_execution_id": {"type": "string"},
                "task_reference": {"type": "string"}
            }),
            json!([]),
        ),
        tool(
            "task_exec_deliver",
            "As the exact native worker in its child room, submit a DeliveryManifest v1 and durably request principal review.",
            json!({
                "task_execution_id": {"type": "string"},
                "manifest": delivery_manifest_schema
            }),
            json!(["task_execution_id", "manifest"]),
        ),
        tool(
            "task_exec_review",
            "As the parent-room principal, submit a ReviewDecision v1. Approval is guarded and must name the exact reviewed_head_sha. If an HTTP worker honestly left a DoD unmet because it lacks a shell, include attempt-scoped dod_verifications with non-empty evidence from your own validation; a mutable Planning checkbox is not review evidence. request_changes requires an actionable comment.",
            json!({
                "task_execution_id": {"type": "string"},
                "decision": {"type": "object", "description": "ReviewDecision v1: approve or request_changes, optional findings, reviewed_head_sha for approve, and principal dod_verifications when needed."}
            }),
            json!(["task_execution_id", "decision"]),
        ),
        tool(
            "task_exec_cancel",
            "Cancel an execution as its parent-room principal. preserve is the safe default; remove_if_clean refuses dirty or unproven worktrees.",
            json!({
                "task_execution_id": {"type": "string"},
                "reason": {"type": "string"},
                "cleanup_policy": {"type": "string", "enum": ["preserve", "remove_if_clean"]}
            }),
            json!(["task_execution_id", "reason"]),
        ),
        tool(
            "task_exec_reassign",
            "Reassign a blocked, interrupted or awaiting-review execution (a pending delivery is rejected) as its parent-room principal while preserving durable child/worktree/checkpoints.",
            json!({
                "task_execution_id": {"type": "string"},
                "worker": {"type": "object", "description": "Typed MessageTarget: kind, agent_type, optional exact cli_session_id and tier — the same object agent_list hands back and task_exec_prepare/task_exec_launch accept as worker."},
                "reason": {"type": "string"}
            }),
            json!(["task_execution_id", "worker", "reason"]),
        ),
    ]
}

/// Backend-owned continuations for native HTTP agents. A command can only be
/// a user-saved Quick Exec: the model never submits a shell string or cwd.
fn agent_resume_tool_catalogue() -> Vec<Value> {
    let tool = |name: &str, description: &str, properties: Value, required: Value| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                },
            },
        })
    };
    vec![
        tool(
            "agent_job_start",
            "Start a backend-owned saved Quick Exec that survives this turn. Completion re-dispatches you exactly once with the durable job/result ids.",
            json!({
                "quick_exec_id": {"type": "string", "description": "id from qe_list."},
                "variables": {"type": "object", "additionalProperties": {"type": "string"}},
                "reason": {"type": "string"},
                "dedupe_key": {"type": "string", "description": "Stable logical key; reuse it on retry."},
                "task_execution_id": {"type": "string"}
            }),
            json!(["quick_exec_id", "reason", "dedupe_key"]),
        ),
        tool(
            "agent_schedule_wake",
            "Schedule one bounded backend-owned wake for external state that cannot notify Kronn. Use command jobs instead of guessed timers for local commands.",
            json!({
                "delay_seconds": {"type": "integer", "minimum": 1, "maximum": 604800},
                "reason": {"type": "string"},
                "dedupe_key": {"type": "string", "description": "Stable logical key; reuse it on retry."},
                "task_execution_id": {"type": "string"}
            }),
            json!(["delay_seconds", "reason", "dedupe_key"]),
        ),
        tool(
            "agent_resume_status",
            "Read durable command/wake state in this room. Omit job_id for the recent list.",
            json!({"job_id": {"type": "string"}}),
            json!([]),
        ),
        tool(
            "agent_resume_cancel",
            "Cancel one of your active durable command/wake jobs in this room.",
            json!({"job_id": {"type": "string"}}),
            json!(["job_id"]),
        ),
    ]
}

/// Web + workspace tools (KT-338). Offered only when the run has a discussion:
/// they are scoped to *its* workspace, and a workflow step has no discussion to
/// scope to — handing them out there would mean an unbounded filesystem.
fn workspace_tool_catalogue() -> Vec<Value> {
    let mut tools = crate::api::agent_workspace_tools::tool_definitions();
    // KT-340 — reading sibling rooms of the same project. Scoped there on purpose:
    // what an HTTP agent reads leaves for its host, so the boundary is the grouping
    // the user made, not the whole instance.
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "disc_list",
            "description": "List the other discussions of THIS project (id, title, agent). Use it \
                            to find a room by name instead of needing its UUID.",
            "parameters": { "type": "object", "properties": {}, "required": [] },
        },
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "disc_read",
            "description": "Read recent messages of a discussion of THIS project. Omit \
                            `discussion_id` to re-read THIS one: that is how you recover what \
                            a long run shortened out of your own context. Returns the tail \
                            (most recent first requested `limit`, default 40, max 200) with \
                            `truncated` set when the room is longer. A room from another \
                            project is refused. Read-only.",
            "parameters": {
                "type": "object",
                "properties": {
                    "discussion_id": { "type": "string", "description": "UUID from disc_list. Omitted, this discussion." },
                    "limit": { "type": "integer", "description": "How many recent messages (1-200)." },
                },
                "required": [],
            },
        },
    }));
    tools
}

/// Workspace tools safe for a project-scoped workflow Agent step. A workflow
/// has no discussion identity, so it cannot read sibling rooms or prove that it
/// is the exact worker owning a managed orchestration worktree.
fn workflow_workspace_tool_catalogue() -> Vec<Value> {
    const DISCUSSION_ONLY: &[&str] = &["disc_list", "disc_read", "git_commit"];
    workspace_tool_catalogue()
        .into_iter()
        .filter(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_some_and(|name| !DISCUSSION_ONLY.contains(&name))
        })
        .collect()
}

fn workflow_tool_catalogue(has_project: bool) -> Vec<Value> {
    const WORKFLOW_TOOLS: &[&str] = &["mcp_list", "api_endpoints", "qa_list", "qa_run", "api_call"];
    tool_catalogue()
        .into_iter()
        .filter(|tool| {
            tool["function"]["name"].as_str().is_some_and(|name| {
                WORKFLOW_TOOLS.contains(&name)
                    || (has_project && matches!(name, "task_list" | "task_get"))
            })
        })
        .collect()
}

/// Narrow the catalogue to what a worker needs to deliver its one task.
///
/// KT-398 — found by delegating a real task to a local model. The worker was
/// handed 22 tools, nine of them planning-management. Its first call failed on
/// a missing argument, and it fell back to what was on offer: `task_list`,
/// twelve times, until the per-tool budget cut it off. It never opened a file.
///
/// The brief is already in the prompt, and the task is already chosen — reading
/// or reshaping the backlog is the principal's job. Removing those tools is not
/// a guard against a bad model; it is refusing to offer a wrong turn. `task_get`
/// stays: reading its own task's DoD is legitimate.
fn worker_room_catalogue(catalogue: Vec<Value>) -> Vec<Value> {
    const NOT_FOR_A_WORKER: &[&str] = &[
        // Backlog browsing and shaping — the principal owns the plan.
        "task_list",
        "task_create",
        "plan_get",
        "task_update",
        "task_update_dod",
        "task_link_discussion",
        "task_add_blocker",
        "task_remove_blocker",
        // Launching or steering OTHER executions from inside one.
        "agent_list",
        "task_exec_prepare",
        "task_exec_launch",
        "task_exec_cancel",
        "task_exec_reassign",
        "task_exec_review",
        // Durable jobs are a principal/recovery capability. A bounded worker
        // must finish this attempt or report what its no-shell boundary
        // prevented; spawning work behind the principal's back makes the
        // attempt and its review evidence impossible to reason about.
        "agent_job_start",
        // Its catalogue goes with it: offering the list without the only tool
        // that consumes it is precisely the wrong turn this filter removes.
        "qe_list",
        // And running one directly is the same capability by another route. A
        // worker has one task and is already briefed; choosing among the saved
        // commands of a project it is visiting is not part of that contract.
        // `qe_run` without `qe_list` would also be exactly the runnable-but-
        // undiscoverable shape this pair was just fixed for.
        "qe_run",
        // Authoring automations is a principal capability outright: a worker
        // that leaves a new Quick API behind has changed the instance, which
        // is not something its delivery can be reviewed against.
        "qa_create_draft",
        "qa_update",
        "qe_create_draft",
        "qe_update",
        "qp_list",
        "qp_run",
        "qp_create_draft",
        "qp_update",
        "agent_schedule_wake",
        "agent_resume_status",
        "agent_resume_cancel",
        "workflow_list",
        "workflow_get",
        "workflow_step_schema",
        "workflow_create_draft",
        "workflow_update",
    ];
    catalogue
        .into_iter()
        .filter_map(|mut tool| {
            let name = tool["function"]["name"].as_str()?;
            if NOT_FOR_A_WORKER.contains(&name) {
                return None;
            }
            if name == "task_exec_deliver" {
                // The model cannot know, and must never choose, the execution
                // capability that this trusted worker run already owns. The
                // task identity, Git facts and opaque DoD ids are equally
                // mechanical: Kronn owns them and injects them after authz.
                // Keep the public/principal schema unchanged, but let a native
                // HTTP worker author only the semantic delivery assertions.
                tool["function"]["description"] = json!(
                    "Submit the semantic delivery assertions for this exact worker run. Pass only `manifest`; Kronn derives the execution, task reference, Git HEAD, committed file inventory and DoD ids from trusted state. `dod_status` must contain exactly one `{met, evidence}` item per DoD, in the brief's order."
                );
                let mut manifest_schema =
                    tool["function"]["parameters"]["properties"]["manifest"].clone();
                let properties = manifest_schema["properties"]
                    .as_object_mut()
                    .expect("DeliveryManifest schema properties");
                for mechanical in [
                    "version",
                    "task_ref",
                    "head_sha",
                    "files_touched",
                ] {
                    properties.remove(mechanical);
                }
                let dod_items = properties["dod_status"]["items"]
                    .as_object_mut()
                    .expect("DeliveryManifest dod_status item schema");
                dod_items["required"] = json!(["met", "evidence"]);
                dod_items["properties"]
                    .as_object_mut()
                    .expect("DeliveryManifest dod_status properties")
                    .remove("dod_id");
                properties["tests"]["items"]["required"] =
                    json!(["name", "status", "evidence"]);
                properties["tests"]["items"]["additionalProperties"] = json!(false);
                properties["dod_status"]["items"]["additionalProperties"] = json!(false);
                manifest_schema["additionalProperties"] = json!(false);
                manifest_schema["required"] = json!([
                    "tests",
                    "dod_status",
                    "docs",
                    "migrations",
                    "risks",
                    "limitations",
                    "summary"
                ]);
                tool["function"]["parameters"]["properties"] = json!({
                    "manifest": manifest_schema
                });
                tool["function"]["parameters"]["required"] = json!(["manifest"]);
            }
            Some(tool)
        })
        .collect()
}

#[async_trait::async_trait]
impl ToolExecutor for KronnToolExecutor {
    fn catalogue(&self) -> Vec<Value> {
        let mut catalogue = tool_catalogue();
        if self.workflow_run_id.is_none() {
            // A discussion run gets the web and its workspace: without them an
            // HTTP agent can discuss work but never read a page, open a file or
            // produce one (KT-338).
            catalogue.extend(workspace_tool_catalogue());
            if self.disc_id.is_some() && self.actor_type.is_some() {
                catalogue.extend(orchestration_tool_catalogue());
                catalogue.extend(agent_resume_tool_catalogue());
            }
            if self.worker_room {
                return worker_room_catalogue(catalogue);
            }
            // a discussion run may start on the core plus the index.
            return if tiered_tools_enabled() {
                tiered(catalogue)
            } else {
                catalogue
            };
        }
        let mut catalogue = workflow_tool_catalogue(self.project_id.is_some());
        // A workflow scoped to a project gets the same file, web and git tools as a
        // discussion: an Agent step asked to review or summarise a repository needs
        // to read it, and the guards (workspace-bounded paths, SSRF refusal, read-only
        // git) do not depend on a human watching. Cross-room reads stay out — they are
        // scoped to the run's own discussion, which a workflow does not have.
        if self.project_id.is_some() {
            catalogue.extend(workflow_workspace_tool_catalogue());
        }
        catalogue
    }

    fn run_mode(&self) -> crate::agents::tools::ToolRunMode {
        if self.worker_room {
            crate::agents::tools::ToolRunMode::Worker
        } else {
            crate::agents::tools::ToolRunMode::General
        }
    }

    /// only a discussion run has someone to ask when a ceiling is
    /// reached; a workflow step or a worker keeps the plain ceilings.
    async fn ceiling_allowance(&self) -> crate::agents::tools::CeilingAllowance {
        let (Some(disc_id), Some(actor_type)) = (self.disc_id.clone(), self.actor_type.as_ref())
        else {
            return crate::agents::tools::CeilingAllowance::default();
        };
        if self.worker_room || self.workflow_run_id.is_some() {
            return crate::agents::tools::CeilingAllowance::default();
        }
        let agent = crate::db::discussions::format_agent_type(actor_type);
        match self
            .state
            .db
            .with_conn(move |conn| {
                crate::db::discussion_ceiling_requests::take_allowance(conn, &disc_id, &agent)
            })
            .await
        {
            Ok(allowance) => allowance,
            Err(error) => {
                // An unreadable grant is no grant, but the ceiling can still
                // be put to the human.
                tracing::warn!(target: "kronn::agent::tools", %error, "ceiling allowance unreadable");
                crate::agents::tools::CeilingAllowance {
                    ask_on_ceiling: true,
                    ..Default::default()
                }
            }
        }
    }

    fn worker_scope(&self) -> Option<crate::models::TaskWorkerScope> {
        self.worker_scope.clone()
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        if self.workflow_run_id.is_some()
            && !self.catalogue().iter().any(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .is_some_and(|name| name == call.name)
            })
        {
            return fail(
                call,
                format!(
                    "tool `{}` is not allowed in workflow Agent steps; use only the bounded catalogue declared on this request",
                    call.name
                ),
            );
        }
        use axum::extract::{Path, State};
        use axum::Json;

        // Handlers are invoked directly rather than over loopback HTTP: same
        // code path as the bridge's calls, minus a round-trip and an auth hop.
        match tool_dispatch(&call.name) {
            ToolDispatch::DiscussionRead => return self.read_other_discussion(call).await,
            ToolDispatch::DiscussionList => return self.list_project_discussions(call).await,
            ToolDispatch::Workspace => return self.execute_workspace_tool(call).await,
            ToolDispatch::Orchestration => return self.execute_orchestration_tool(call).await,
            ToolDispatch::Resume => return self.execute_agent_resume_tool(call).await,
            ToolDispatch::Core => {}
        }
        match call.name.as_str() {
            "mcp_list" => {
                let Json(res) = crate::api::mcps::overview(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(d)) => match serde_json::to_value(d) {
                        Ok(v) => {
                            let project_id = self.effective_project_id().await;
                            ok(call, compact_plugin_list(&v, project_id.as_deref()))
                        }
                        Err(e) => fail(call, format!("could not serialise result: {e}")),
                    },
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "api_endpoints" => {
                let Some(slug) = call.arguments["api_plugin_slug"].as_str() else {
                    return fail(call, "missing required field `api_plugin_slug`");
                };
                let Json(res) = crate::api::mcps::overview(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(d)) => {
                        let project_id = self.effective_project_id().await;
                        match serde_json::to_value(d).ok().and_then(|v| {
                            let listed = compact_plugin_list(&v, project_id.as_deref());
                            listed["plugins"]
                                .as_array()
                                .is_some_and(|plugins| plugins.iter().any(|p| p["slug"] == slug))
                                .then(|| compact_endpoints(&v, slug))
                                .flatten()
                        }) {
                            Some(v) => ok(call, v),
                            None => fail(
                                call,
                                format!("no in-scope plugin `{slug}` with an API spec — call mcp_list first"),
                            ),
                        }
                    }
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "qa_list" => {
                let Json(res) = crate::api::quick_apis::list(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(d)) => match serde_json::to_value(d) {
                        Ok(v) => {
                            let project_id = self.effective_project_id().await;
                            ok(call, compact_quick_apis(&v, project_id.as_deref()))
                        }
                        Err(e) => fail(call, format!("could not serialise result: {e}")),
                    },
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "tool_manual" => ok(call, tool_manual(call.arguments["tool"].as_str())),
            "workflow_list"
            | "workflow_get"
            | "workflow_step_schema"
            | "workflow_create_draft"
            | "workflow_update" => self.execute_workflow_tool(call).await,
            "qp_list" => {
                let Json(res) = crate::api::quick_prompts::list(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(items)) => {
                        let project_id = self.effective_project_id().await;
                        ok(call, compact_quick_prompts(&items, project_id.as_deref()))
                    }
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "qp_create_draft" => {
                let project_id = self.effective_project_id().await;
                let request = match quick_prompt_write_request(
                    None,
                    &call.arguments,
                    project_id.as_deref(),
                ) {
                    Ok(request) => request,
                    Err(error) => return fail(call, error),
                };
                let Json(res) =
                    crate::api::quick_prompts::create(State(self.state.clone()), Json(request))
                        .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "qp_update" | "qp_run" => {
                let Some(id) = call.arguments["qp_id"].as_str() else {
                    return fail(call, "missing required field `qp_id`");
                };
                let lookup = id.to_owned();
                let saved = self
                    .state
                    .db
                    .with_conn(move |conn| {
                        crate::db::quick_prompts::get_quick_prompt(conn, &lookup)
                    })
                    .await;
                let saved = match saved {
                    Ok(saved) => saved,
                    Err(error) => return fail(call, format!("DB error: {error}")),
                };
                let project_id = self.effective_project_id().await;
                let Some(saved) = saved.filter(|qp| {
                    project_is_in_scope(qp.project_id.as_deref(), project_id.as_deref())
                }) else {
                    return fail(call, "Quick Prompt is not available in this discussion's scope — call qp_list again");
                };
                if call.name == "qp_update" {
                    let request = match quick_prompt_write_request(
                        Some(&saved),
                        &call.arguments,
                        project_id.as_deref(),
                    ) {
                        Ok(request) => request,
                        Err(error) => return fail(call, error),
                    };
                    let Json(res) = crate::api::quick_prompts::update(
                        State(self.state.clone()),
                        Path(saved.id),
                        Json(request),
                    )
                    .await;
                    unwrap_api(call, res.success, res.data, res.error)
                } else {
                    let request = crate::api::mcp_remote::McpQpRunRequest {
                        qp_id: saved.id,
                        vars: call.arguments["vars"]
                            .as_object()
                            .map(|values| {
                                values
                                    .iter()
                                    .map(|(name, value)| (name.clone(), as_plain_string(value)))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        agent: None,
                        project_id: None,
                        title: call.arguments["title"].as_str().map(str::to_owned),
                        launch: Some(crate::core::launch_context::LaunchContext {
                            discussion_id: self.disc_id.clone(),
                            project_id,
                            context: Default::default(),
                        }),
                    };
                    let Json(res) =
                        crate::api::mcp_remote::qp_run(State(self.state.clone()), Json(request))
                            .await;
                    unwrap_api(call, res.success, res.data, res.error)
                }
            }
            "qe_run" => {
                let Some(id) = call.arguments["quick_exec_id"].as_str() else {
                    return fail(call, "missing required field `quick_exec_id`");
                };
                // Scoped exactly like agent_job_start, and for the same reason:
                // a Quick Exec belongs to a project, and a room of another one
                // must not be able to start it by naming its id.
                let project_id = self.effective_project_id().await;
                let lookup = id.to_string();
                let saved = self
                    .state
                    .db
                    .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup))
                    .await
                    .ok()
                    .flatten();
                let Some(saved) = saved.filter(|saved| match project_id.as_deref() {
                    Some(project_id) => saved
                        .project_id
                        .as_deref()
                        .is_none_or(|id| id == project_id),
                    None => saved.project_id.is_none(),
                }) else {
                    return fail(
                        call,
                        "Quick Exec is not available in this discussion's scope — call qe_list again",
                    );
                };
                let request = crate::models::RunQuickExecRequest {
                    variables: call.arguments["variables"]
                        .as_object()
                        .map(|values| {
                            values
                                .iter()
                                .map(|(name, value)| (name.clone(), as_plain_string(value)))
                                .collect()
                        })
                        .unwrap_or_default(),
                    launch: None,
                };
                let Json(res) = crate::api::quick_execs::run(
                    State(self.state.clone()),
                    Path(saved.id.clone()),
                    Json(request),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "qa_create_draft" => {
                let request = match serde_json::from_value::<crate::models::CreateQuickApiRequest>(
                    call.arguments.clone(),
                ) {
                    Ok(request) => request,
                    Err(error) => return fail(call, format!("invalid Quick API: {error}")),
                };
                let Json(res) =
                    crate::api::quick_apis::create(State(self.state.clone()), Json(request)).await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "qa_update" => {
                let Some(id) = call.arguments["quick_api_id"].as_str() else {
                    return fail(call, "missing required field `quick_api_id`");
                };
                let lookup = id.to_string();
                let existing = self
                    .state
                    .db
                    .with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &lookup))
                    .await
                    .ok()
                    .flatten();
                let project_id = self.effective_project_id().await;
                let Some(existing) =
                    existing.filter(|qa| quick_api_is_in_scope(qa, project_id.as_deref()))
                else {
                    return fail(
                        call,
                        "Quick API is not available in this discussion's scope — call qa_list again",
                    );
                };
                let merged = match merged_definition(&existing, &call.arguments) {
                    Ok(merged) => merged,
                    Err(error) => return fail(call, format!("invalid Quick API: {error}")),
                };
                let Json(res) = crate::api::quick_apis::update(
                    State(self.state.clone()),
                    Path(existing.id.clone()),
                    Json(merged),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "qe_create_draft" => {
                let request = match serde_json::from_value::<crate::models::CreateQuickExecRequest>(
                    call.arguments.clone(),
                ) {
                    Ok(request) => request,
                    Err(error) => return fail(call, format!("invalid Quick Exec: {error}")),
                };
                let Json(res) =
                    crate::api::quick_execs::create(State(self.state.clone()), Json(request)).await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "qe_update" => {
                let Some(id) = call.arguments["quick_exec_id"].as_str() else {
                    return fail(call, "missing required field `quick_exec_id`");
                };
                let lookup = id.to_string();
                let existing = self
                    .state
                    .db
                    .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup))
                    .await
                    .ok()
                    .flatten();
                let project_id = self.effective_project_id().await;
                let Some(existing) = existing.filter(|saved| match project_id.as_deref() {
                    Some(project_id) => saved
                        .project_id
                        .as_deref()
                        .is_none_or(|id| id == project_id),
                    None => saved.project_id.is_none(),
                }) else {
                    return fail(
                        call,
                        "Quick Exec is not available in this discussion's scope — call qe_list again",
                    );
                };
                let merged = match merged_definition(&existing, &call.arguments) {
                    Ok(merged) => merged,
                    Err(error) => return fail(call, format!("invalid Quick Exec: {error}")),
                };
                let Json(res) = crate::api::quick_execs::update(
                    State(self.state.clone()),
                    Path(existing.id.clone()),
                    Json(merged),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "qe_list" => {
                let Json(res) = crate::api::quick_execs::list(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(items)) => {
                        let project_id = self.effective_project_id().await;
                        ok(call, compact_quick_execs(&items, project_id.as_deref()))
                    }
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "qa_run" => {
                let Some(id) = call.arguments["quick_api_id"].as_str() else {
                    return fail(call, "missing required field `quick_api_id`");
                };
                let project_id = self.effective_project_id().await;
                let qa_id = id.to_string();
                let qa = self
                    .state
                    .db
                    .with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &qa_id))
                    .await
                    .ok()
                    .flatten();
                let Some(qa) = qa.filter(|qa| quick_api_is_in_scope(qa, project_id.as_deref()))
                else {
                    return fail(call, "Quick API is not available in this workflow/discussion scope — call qa_list again");
                };
                let variables = call.arguments["variables"]
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .map(|(k, v)| (k.clone(), as_plain_string(v)))
                            .collect()
                    })
                    .unwrap_or_default();
                let Json(res) = crate::api::quick_apis::run_qa(
                    State(self.state.clone()),
                    Path(qa.id),
                    Json(crate::models::RunQuickApiRequest {
                        variables,
                        workflow_run_id: self.workflow_run_id.clone(),
                        agent: Some(self.actor_id.clone()),
                        launch: None,
                    }),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "api_call" => {
                let (Some(slug), Some(path)) = (
                    call.arguments["api_plugin_slug"].as_str(),
                    call.arguments["endpoint_path"].as_str(),
                ) else {
                    return fail(
                        call,
                        "missing required fields `api_plugin_slug` and/or `endpoint_path`",
                    );
                };
                // Making the model carry a UUID across turns is a reliability
                // tax it fails to pay: a 4B model paired `api-speedcurve` with
                // Resend's config id (2026-08-09). Kronn owns that mapping, so
                // resolve it here and treat an explicit value as a
                // disambiguation hint, not a requirement.
                let config_id = match call.arguments["api_config_id"].as_str() {
                    Some(explicit) if !explicit.trim().is_empty() => self
                        .config_id_in_scope(slug, explicit)
                        .await
                        .then(|| explicit.to_string()),
                    _ => self.resolve_config_id(slug).await,
                };
                if config_id.is_none() {
                    return fail(
                        call,
                        format!("no API configuration wired for plugin `{slug}` — call mcp_list to see what is available"),
                    );
                }
                let req = crate::api::agent_api::AgentApiCallRequest {
                    disc_id: self.disc_id.clone(),
                    project_id: self.project_id.clone(),
                    api_plugin_slug: Some(slug.to_string()),
                    api_config_id: config_id,
                    quick_api_id: None,
                    endpoint_path: path.to_string(),
                    method: call.arguments["method"].as_str().map(str::to_string),
                    path_params: None,
                    query: call.arguments["query"].as_object().map(|o| {
                        o.iter()
                            .map(|(k, v)| (k.clone(), as_plain_string(v)))
                            .collect()
                    }),
                    body: call.arguments.get("body").cloned(),
                    headers: None,
                    extract: None,
                    workflow_run_id: self.workflow_run_id.clone(),
                    agent: Some(self.actor_id.clone()),
                };
                let Json(res) =
                    crate::api::agent_api::agent_api_call(State(self.state.clone()), Json(req))
                        .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "plan_get" => {
                let Some(disc_id) = self.discussion_id(call) else {
                    return fail(call, "plan_get is available only inside a Kronn discussion");
                };
                let Json(res) = crate::api::planning::get_discussion_plan(
                    State(self.state.clone()),
                    Path(disc_id),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "task_list" => {
                let mut query =
                    match serde_json::from_value::<PlanningTaskListQuery>(call.arguments.clone()) {
                        Ok(query) => query,
                        Err(error) => {
                            return fail(call, format!("invalid task_list fields: {error}"))
                        }
                    };
                // HTTP agents are discussion-scoped. Do not let a model-supplied
                // filter silently widen native Planning reads to another room.
                if let Some(discussion_id) = &self.disc_id {
                    query.discussion_id = Some(discussion_id.clone());
                } else if let Some(project_id) = &self.project_id {
                    query.project_id = Some(project_id.clone());
                }
                let Json(res) = crate::api::planning::list_tasks(
                    State(self.state.clone()),
                    axum::extract::Query(query),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "task_get" => {
                let Some(task_id) = required_string(call, "task_id") else {
                    return fail(call, "missing required field `task_id`");
                };
                let Json(mut res) =
                    crate::api::planning::get_task(State(self.state.clone()), Path(task_id)).await;
                let effective_project_id = self.effective_project_id().await;
                if let (Some(project_id), Some(task)) = (effective_project_id, res.data.as_ref()) {
                    if !task.summary.project_ids.iter().any(|id| id == &project_id) {
                        res.success = false;
                        res.data = None;
                        res.error = Some(format!(
                            "task is outside workflow project `{project_id}` — call task_list again"
                        ));
                    }
                }
                unwrap_api(call, res.success, res.data, res.error)
            }
            "task_create" => {
                let Some(discussion_id) = self.discussion_id(call) else {
                    return fail(
                        call,
                        "task_create is available only inside a Kronn discussion",
                    );
                };
                let mut value = call.arguments.clone();
                let Some(fields) = value.as_object_mut() else {
                    return fail(call, "task_create arguments must be an object");
                };
                fields.insert("discussion_id".into(), json!(discussion_id));
                fields.insert("actor".into(), self.actor_json());
                if let Some(key) = fields.get("idempotency_key").and_then(Value::as_str) {
                    let digest =
                        Sha256::digest(format!("{discussion_id}\0explicit\0{key}").as_bytes());
                    fields.insert(
                        "idempotency_key".into(),
                        json!(format!("http-agent-task-create:{}", hex_digest(&digest))),
                    );
                }
                let request = match serde_json::from_value::<CreatePlanningTaskRequest>(value) {
                    Ok(request) => request,
                    Err(error) => {
                        return fail(call, format!("invalid task_create fields: {error}"))
                    }
                };
                let Json(res) =
                    crate::api::planning::create_task(State(self.state.clone()), Json(request))
                        .await;
                unwrap_api_compact_task(call, res.success, res.data, res.error)
            }
            "task_update" => {
                let Some(task_id) = required_string(call, "task_id") else {
                    return fail(call, "missing required field `task_id`");
                };
                let mut value = call.arguments.clone();
                let Some(fields) = value.as_object_mut() else {
                    return fail(call, "task_update arguments must be an object");
                };
                fields.remove("task_id");
                fields.insert("actor".into(), self.actor_json());
                let request = match serde_json::from_value::<UpdatePlanningTaskRequest>(value) {
                    Ok(request) => request,
                    Err(error) => {
                        return fail(call, format!("invalid task_update fields: {error}"))
                    }
                };
                let Json(res) = crate::api::planning::update_task(
                    State(self.state.clone()),
                    Path(task_id),
                    Json(request),
                )
                .await;
                unwrap_api_compact_task(call, res.success, res.data, res.error)
            }
            "task_update_dod" => {
                let (Some(task_id), Some(dod_id)) = (
                    required_string(call, "task_id"),
                    required_string(call, "dod_id"),
                ) else {
                    return fail(call, "missing required fields `task_id` and/or `dod_id`");
                };
                let Some(completed) = flag_arg(call, "completed") else {
                    return fail(call, "missing boolean field `completed`");
                };
                let request = UpdatePlanningDodItemRequest {
                    completed,
                    actor: self.actor(),
                };
                let Json(res) = crate::api::planning::update_dod_item(
                    State(self.state.clone()),
                    Path((task_id, dod_id)),
                    Json(request),
                )
                .await;
                unwrap_api_compact_task(call, res.success, res.data, res.error)
            }
            "task_link_discussion" => {
                let Some(task_id) = required_string(call, "task_id") else {
                    return fail(call, "missing required field `task_id`");
                };
                let Some(discussion_id) = self.discussion_id(call) else {
                    return fail(
                        call,
                        "task_link_discussion is available only inside a Kronn discussion",
                    );
                };
                let mut value = call.arguments.clone();
                let Some(fields) = value.as_object_mut() else {
                    return fail(call, "task_link_discussion arguments must be an object");
                };
                fields.remove("task_id");
                fields.insert("discussion_id".into(), json!(discussion_id));
                fields.insert("actor".into(), self.actor_json());
                let request = match serde_json::from_value::<LinkPlanningDiscussionRequest>(value) {
                    Ok(request) => request,
                    Err(error) => {
                        return fail(
                            call,
                            format!("invalid task_link_discussion fields: {error}"),
                        )
                    }
                };
                let Json(res) = crate::api::planning::link_discussion(
                    State(self.state.clone()),
                    Path(task_id),
                    Json(request),
                )
                .await;
                unwrap_api(call, res.success, res.data, res.error)
            }
            "task_add_blocker" => {
                let (Some(task_id), Some(blocker_task_id)) = (
                    required_string(call, "task_id"),
                    required_string(call, "blocker_task_id"),
                ) else {
                    return fail(
                        call,
                        "missing required fields `task_id` and/or `blocker_task_id`",
                    );
                };
                let request = AddPlanningBlockerRequest {
                    blocker_task_id,
                    actor: self.actor(),
                };
                let Json(res) = crate::api::planning::add_blocker(
                    State(self.state.clone()),
                    Path(task_id),
                    Json(request),
                )
                .await;
                unwrap_api_compact_task(call, res.success, res.data, res.error)
            }
            "task_remove_blocker" => {
                let (Some(task_id), Some(blocker_task_id)) = (
                    required_string(call, "task_id"),
                    required_string(call, "blocker_task_id"),
                ) else {
                    return fail(
                        call,
                        "missing required fields `task_id` and/or `blocker_task_id`",
                    );
                };
                let request = RemovePlanningBlockerRequest {
                    actor: self.actor(),
                };
                let Json(res) = crate::api::planning::remove_blocker(
                    State(self.state.clone()),
                    Path((task_id, blocker_task_id)),
                    Json(request),
                )
                .await;
                unwrap_api_compact_task(call, res.success, res.data, res.error)
            }
            other => fail(
                call,
                format!("unknown tool `{other}` — call only the tools you were given"),
            ),
        }
    }
}

/// A JSON string stays as-is; anything else is rendered compactly. Wrapping a
/// string in quotes here would put `"\"42\""` in a query parameter.
fn as_plain_string(v: &Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

impl KronnToolExecutor {
    /// Native counterpart of the stdio task-execution tools. Caller discussion and
    /// provider come from this trusted executor; model arguments contain only the
    /// business payload. Unknown executions and foreign callers stay fused.
    async fn execute_orchestration_tool(&self, call: &ToolCall) -> ToolOutcome {
        use axum::extract::{Path, State};
        use axum::Json;

        let Some(discussion_id) = self.disc_id.clone() else {
            return fail(
                call,
                "task execution tools require a discussion-bound native agent; workflow Agent steps cannot act as a principal or worker",
            );
        };
        let Some(actor_type) = self.actor_type.clone() else {
            return fail(
                call,
                "task execution tools require a typed native provider identity supplied by Kronn",
            );
        };

        match call.name.as_str() {
            "agent_list" => {
                match crate::api::orchestration::target_aware_task_worker_catalogue_for_discussion(
                    &self.state,
                    &discussion_id,
                )
                .await
                {
                    Ok(catalogue) => ok(
                        call,
                        serde_json::to_value(catalogue).unwrap_or_else(
                            |error| json!({"serialization_error": error.to_string()}),
                        ),
                    ),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            // the family's declarations go back to the runner, which
            // adds them to the catalogue for the rest of the run.
            "tools_load" => {
                if self.worker_room {
                    return fail(
                        call,
                        "tool families can only be loaded by a principal discussion agent",
                    );
                }
                // Missing or unknown families return the discovery index so the model can recover.
                let family = required_string(call, "family").unwrap_or_default();
                let declarations = declarations_for_family(&family);
                if declarations.is_empty() {
                    return ToolOutcome {
                        call: call.clone(),
                        content: json!({
                            "families": TOOL_FAMILIES
                                .iter()
                                .map(|(id, what, _)| json!({"family": id, "brings": what}))
                                .collect::<Vec<_>>(),
                            "note": "Call `tools_load` again with one of these as `family`,                                      then use the tools it declares. Load a second family the                                      same way once the first part is done.",
                        }),
                        ok: true,
                    };
                }
                let loaded: Vec<Value> = declarations
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool["function"]["name"],
                            "what": first_sentence(
                                tool["function"]["description"].as_str().unwrap_or_default(),
                            ),
                        })
                    })
                    .collect();
                let first = loaded
                    .first()
                    .and_then(|tool| tool["name"].as_str())
                    .unwrap_or("the one you need")
                    .to_string();
                ok(
                    call,
                    json!({
                        "family": family,
                        "loaded": loaded,
                        "note": format!(
                            "These tools are declared from now on. Call the one the request needs, starting with `{first}` if it fits: they are ready, and no other route is required."
                        ),
                        // Read by the runner, never shown to the model: it sees
                        // the declarations themselves in the next turn's catalogue.
                        "__kronn_tools_add": declarations,
                    }),
                )
            }
            "media_generate" => {
                // Let the media handler resolve an absent connection or report ambiguous providers.
                let connection_id = required_string(call, "connection_id");
                let Some(prompt) = required_string(call, "prompt") else {
                    return fail(call, "missing required field `prompt`");
                };
                let modality = match call
                    .arguments
                    .get("modality")
                    .and_then(serde_json::Value::as_str)
                {
                    Some("image") => crate::models::MediaModality::Image,
                    Some("video") => crate::models::MediaModality::Video,
                    // Named rather than defaulted: generating the wrong modality
                    // is a billed mistake the operator did not ask for.
                    other => {
                        return fail(
                            call,
                            format!(
                                "`modality` must be \"image\" or \"video\", got {}",
                                other.unwrap_or("nothing")
                            ),
                        )
                    }
                };
                let request = crate::api::media::GenerateMediaRequest {
                    // Derived from the request when the agent gives none.
                    // Without this the server falls back to a random id, so two
                    // identical asks are two jobs and two charges — which is
                    // what a wake-and-retry loop produced: the same picture
                    // generated twice, six minutes apart, billed twice.
                    //
                    // An agent that wants a genuinely different take says so by
                    // passing its own key, or by changing the prompt.
                    idempotency_key: Some(
                        call.arguments
                            .get("idempotency_key")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                derived_media_idempotency_key(&discussion_id, &call.arguments)
                            }),
                    ),
                    connection_id,
                    modality,
                    prompt,
                    // The asset belongs to the room the agent is answering in.
                    discussion_id: Some(discussion_id.clone()),
                    message_id: None,
                    duration_secs: call
                        .arguments
                        .get("duration_secs")
                        .and_then(serde_json::Value::as_u64)
                        .map(|value| value as u32),
                    resolution: call
                        .arguments
                        .get("resolution")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    aspect_ratio: call
                        .arguments
                        .get("aspect_ratio")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    generate_audio: call
                        .arguments
                        .get("generate_audio")
                        .and_then(serde_json::Value::as_bool),
                    // Hardcoded to None until 0.13.0, which is why an agent
                    // could generate an image and a video but never join them.
                    reference_asset_ids: call
                        .arguments
                        .get("reference_asset_ids")
                        .and_then(serde_json::Value::as_array)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_string)
                                .collect()
                        }),
                    reference_asset_id: None,
                    reference_mode: match call
                        .arguments
                        .get("reference_mode")
                        .and_then(serde_json::Value::as_str)
                    {
                        Some("first_frame") => Some(crate::models::MediaReferenceMode::FirstFrame),
                        Some("last_frame") => Some(crate::models::MediaReferenceMode::LastFrame),
                        Some("reference") => Some(crate::models::MediaReferenceMode::Reference),
                        // Left to the server's own default rather than guessed
                        // here, so the agent path and the UI agree.
                        _ => None,
                    },
                };
                let response = crate::api::media::generate(
                    axum::extract::State(self.state.clone()),
                    axum::Json(request),
                )
                .await;

                // Recorded AFTER the job exists, and only when asked. Writing it
                // into the generation request would carry caller identity into
                // the body sent to the provider.
                if call
                    .arguments
                    .get("wake_when_ready")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    if let Some(job_id) =
                        response.0.data.as_ref().map(|queued| queued.job_id.clone())
                    {
                        let payload = serde_json::json!({
                            "agent_type": actor_type,
                            "source_dispatch_job_id": self.source_dispatch_job_id,
                        })
                        .to_string();
                        let job = job_id.clone();
                        if let Err(error) = self
                            .state
                            .db
                            .with_conn(move |conn| {
                                crate::db::media_jobs::set_wake_request(conn, &job, &payload)
                            })
                            .await
                        {
                            // The generation is already queued and billed. Say
                            // the wake was not armed rather than let the agent
                            // wait for a call that will never come.
                            return fail(
                                call,
                                format!(
                                    "the generation started ({job_id}) but the wake could not be \
                                     armed: {error}. Poll media_job_status instead."
                                ),
                            );
                        }
                    }
                }

                match serde_json::to_value(response.0) {
                    Ok(value) => ok(call, value),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "media_job_status" => {
                let Some(job_id) = required_string(call, "job_id") else {
                    return fail(call, "missing required field `job_id`");
                };
                let response = crate::api::media::get_job(
                    axum::extract::State(self.state.clone()),
                    axum::extract::Path(job_id),
                )
                .await;
                match serde_json::to_value(response.0) {
                    Ok(value) => ok(call, value),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "task_exec_prepare" => {
                let Some(task_reference) = required_string(call, "task_reference") else {
                    return fail(call, "missing required field `task_reference`");
                };
                let worker = match serde_json::from_value::<crate::models::MessageTarget>(
                    call.arguments["worker"].clone(),
                ) {
                    Ok(worker) => worker,
                    Err(error) => return fail(call, format!("invalid typed worker: {error}")),
                };
                let (_scope_intent, worker_scope) = match task_worker_scope_contract(call) {
                    Ok(contract) => contract,
                    Err(error) => return fail(call, error),
                };
                let scope_refusal =
                    crate::api::orchestration::worker_scope_refusal(&worker, worker_scope.as_ref());
                let parent = discussion_id;
                match self
                    .state
                    .db
                    .with_conn(move |conn| {
                        let mut preparation = crate::api::orchestration::prepare_task_execution(
                            conn,
                            &task_reference,
                            &parent,
                            &worker,
                        )?;
                        if let Some(reason) = scope_refusal {
                            preparation.launchable = false;
                            preparation.reasons.push(reason);
                        }
                        Ok(preparation)
                    })
                    .await
                {
                    Ok(preparation) => ok(
                        call,
                        serde_json::to_value(preparation).unwrap_or_else(
                            |error| json!({"serialization_error": error.to_string()}),
                        ),
                    ),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "task_exec_launch" => {
                let Some(task_reference) = required_string(call, "task_reference") else {
                    return fail(call, "missing required field `task_reference`");
                };
                let worker = match serde_json::from_value::<crate::models::MessageTarget>(
                    call.arguments["worker"].clone(),
                ) {
                    Ok(worker) => worker,
                    Err(error) => return fail(call, format!("invalid typed worker: {error}")),
                };
                let (_scope_intent, worker_scope) = match task_worker_scope_contract(call) {
                    Ok(contract) => contract,
                    Err(error) => return fail(call, error),
                };
                if let Some(reason) =
                    crate::api::orchestration::worker_scope_refusal(&worker, worker_scope.as_ref())
                {
                    return fail(call, format!("{}: {}", reason.code, reason.detail));
                }
                let preflight = {
                    let task = task_reference.clone();
                    let parent = discussion_id.clone();
                    let selected = worker.clone();
                    self.state
                        .db
                        .with_conn(move |conn| {
                            crate::api::orchestration::prepare_task_execution(
                                conn, &task, &parent, &selected,
                            )
                        })
                        .await
                };
                let preparation = match preflight {
                    Ok(preparation) => preparation,
                    Err(error) => return fail(call, error.to_string()),
                };
                if !preparation.launchable {
                    return fail(
                        call,
                        format!(
                            "task is not launchable: {}",
                            serde_json::to_string(&preparation.reasons)
                                .unwrap_or_else(|_| "preflight reasons unavailable".into())
                        ),
                    );
                }
                // The principal's mechanical gates are opt-in but never silently
                // dropped: only a genuinely ABSENT field defaults to no gates.
                // An explicit `null`, an unknown field (ValidationSpec denies
                // them), or a structurally-valid-but-empty command/timeout is
                // refused explicitly rather than folded into an ungated run.
                let validations = match call.arguments.get("validations") {
                    None => Vec::new(),
                    Some(Value::Null) => {
                        return fail(call, "invalid validations: must be an array, not null")
                    }
                    Some(value) => {
                        let parsed = match serde_json::from_value::<
                            Vec<crate::models::ValidationSpec>,
                        >(value.clone())
                        {
                            Ok(validations) => validations,
                            Err(error) => {
                                return fail(call, format!("invalid validations: {error}"))
                            }
                        };
                        if let Err(reason) =
                            crate::api::orchestration::validate_new_validation_specs(&parsed)
                        {
                            return fail(call, format!("invalid validations: {reason}"));
                        }
                        parsed
                    }
                };
                match crate::api::orchestration::provision_single_task_execution_with_scope_and_validations(
                    &self.state.db,
                    crate::api::orchestration::ProvisionInput {
                        task_reference,
                        parent_discussion_id: discussion_id,
                        worker,
                        base_rev: required_string(call, "base_rev"),
                        idempotency_key: required_string(call, "idempotency_key"),
                    },
                    worker_scope,
                    validations,
                )
                .await
                {
                    Ok(execution) => ok(call, json!(execution)),
                    Err(error) => {
                        let (_, message) = crate::api::orchestration::provision_error_parts(&error);
                        fail(call, message)
                    }
                }
            }
            "task_exec_status" => {
                let Some(execution_id) = required_string(call, "task_execution_id")
                    .or_else(|| required_string(call, "task_reference"))
                else {
                    return fail(
                        call,
                        "missing required field `task_execution_id` or `task_reference`",
                    );
                };
                let source_message_id = self.source_message_id.clone();
                let result = self
                    .state
                    .db
                    .with_conn(move |conn| {
                        let execution = native_execution_for_caller(
                            conn,
                            &execution_id,
                            &discussion_id,
                            &actor_type,
                            source_message_id.as_deref(),
                            false,
                        )?;
                        crate::api::orchestration::execution_detail(conn, &execution.id)
                    })
                    .await;
                match result {
                    Ok(detail) => ok(call, json!(detail)),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "task_exec_deliver" => {
                let execution_id = if self.worker_room {
                    let discussion_id = discussion_id.clone();
                    let actor_type = actor_type.clone();
                    let source_message_id = self.source_message_id.clone();
                    let source_dispatch_job_id = self.source_dispatch_job_id.clone();
                    match self
                        .state
                        .db
                        .with_read_conn(move |conn| {
                            native_worker_execution_for_trusted_dispatch(
                                conn,
                                &discussion_id,
                                &actor_type,
                                source_message_id.as_deref(),
                                source_dispatch_job_id.as_deref(),
                            )
                            .map(|execution| execution.id)
                        })
                        .await
                    {
                        Ok(execution_id) => execution_id,
                        Err(error) => return fail(call, error.to_string()),
                    }
                } else {
                    let Some(execution_id) = required_string(call, "task_execution_id") else {
                        return fail(call, "missing required field `task_execution_id`");
                    };
                    execution_id
                };
                let Some(manifest) = call
                    .arguments
                    .get("manifest")
                    .filter(|value| value.is_object())
                else {
                    return fail(call, "missing object field `manifest`");
                };
                let manifest_json = match serde_json::to_string(manifest) {
                    Ok(value) => value,
                    Err(error) => return fail(call, format!("manifest is not JSON: {error}")),
                };
                let actor_session_id = self.actor_session_id();
                let Some(source_dispatch_job_id) = self.source_dispatch_job_id.as_deref() else {
                    return fail(call, "execution not found or caller is not a party");
                };
                match crate::api::orchestration::deliver_native_worker_manifest(
                    &self.state.db,
                    &execution_id,
                    crate::api::orchestration::NativeExecutionCaller {
                        discussion_id: &discussion_id,
                        agent_type: &actor_type,
                        source_message_id: self.source_message_id.as_deref(),
                        alias: &self.actor_id,
                        actor_session_id: actor_session_id.as_deref(),
                    },
                    source_dispatch_job_id,
                    &manifest_json,
                )
                .await
                {
                    Ok(outcome) => {
                        let response =
                            crate::api::orchestration::deliver_outcome_to_response(outcome);
                        unwrap_api(call, response.success, response.data, response.error)
                    }
                    Err(error) => {
                        let (_, message) = crate::api::orchestration::provision_error_parts(&error);
                        fail(call, message)
                    }
                }
            }
            "task_exec_review" => {
                let Some(execution_id) = required_string(call, "task_execution_id") else {
                    return fail(call, "missing required field `task_execution_id`");
                };
                let Some(decision) = call
                    .arguments
                    .get("decision")
                    .filter(|value| value.is_object())
                else {
                    return fail(call, "missing object field `decision`");
                };
                let decision_json = match serde_json::to_string(decision) {
                    Ok(value) => value,
                    Err(error) => return fail(call, format!("decision is not JSON: {error}")),
                };
                let actor_session_id = self.actor_session_id();
                match crate::api::orchestration::decide_native_review(
                    &self.state.db,
                    &execution_id,
                    &decision_json,
                    crate::api::orchestration::NativeExecutionCaller {
                        discussion_id: &discussion_id,
                        agent_type: &actor_type,
                        source_message_id: self.source_message_id.as_deref(),
                        alias: &self.actor_id,
                        actor_session_id: actor_session_id.as_deref(),
                    },
                )
                .await
                {
                    Ok(outcome) => match crate::api::orchestration::continue_approved_review(
                        &self.state.db,
                        outcome,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            let response =
                                crate::api::orchestration::review_outcome_to_response(outcome);
                            unwrap_api(call, response.success, response.data, response.error)
                        }
                        Err(error) => {
                            let (_, message) =
                                crate::api::orchestration::provision_error_parts(&error);
                            fail(call, message)
                        }
                    },
                    Err(error) => {
                        let (_, message) = crate::api::orchestration::provision_error_parts(&error);
                        fail(call, message)
                    }
                }
            }
            "task_exec_cancel" => {
                let Some(execution_id) = required_string(call, "task_execution_id") else {
                    return fail(call, "missing required field `task_execution_id`");
                };
                let authorized = {
                    let execution_id = execution_id.clone();
                    let discussion_id = discussion_id.clone();
                    let actor_type = actor_type.clone();
                    let source_message_id = self.source_message_id.clone();
                    self.state
                        .db
                        .with_conn(move |conn| {
                            native_execution_for_caller(
                                conn,
                                &execution_id,
                                &discussion_id,
                                &actor_type,
                                source_message_id.as_deref(),
                                true,
                            )
                            .map(|_| ())
                        })
                        .await
                };
                if let Err(error) = authorized {
                    return fail(call, error.to_string());
                }
                let cleanup_policy = match call.arguments.get("cleanup_policy") {
                    None | Some(Value::Null) => None,
                    Some(value) => match serde_json::from_value(value.clone()) {
                        Ok(policy) => Some(policy),
                        Err(error) => {
                            return fail(call, format!("invalid cleanup_policy: {error}"))
                        }
                    },
                };
                let Json(response) = crate::api::orchestration::cancel_execution(
                    State(self.state.clone()),
                    Path(execution_id),
                    Json(crate::api::orchestration::CancelExecutionRequest {
                        reason: required_string(call, "reason")
                            .unwrap_or_else(|| "cancelled by native principal".into()),
                        cleanup_policy,
                    }),
                )
                .await;
                unwrap_api(call, response.success, response.data, response.error)
            }
            "task_exec_reassign" => {
                let Some(execution_id) = required_string(call, "task_execution_id") else {
                    return fail(call, "missing required field `task_execution_id`");
                };
                let Some(reason) = required_string(call, "reason") else {
                    return fail(call, "missing required field `reason`");
                };
                let target = match serde_json::from_value::<crate::models::MessageTarget>(
                    call.arguments["worker"].clone(),
                ) {
                    Ok(target) => target,
                    Err(error) => {
                        return fail(
                            call,
                            format!(
                                "worker must be the typed MessageTarget object copied verbatim \
                                 from agent_list (kind/agent_type/...), not the internal \
                                 CampaignWorkerSelection envelope: {error}"
                            ),
                        )
                    }
                };
                let worker = crate::models::CampaignWorkerSelection {
                    target,
                    model: None,
                    profile_id: None,
                };
                let authorized = {
                    let execution_id = execution_id.clone();
                    let discussion_id = discussion_id.clone();
                    let actor_type = actor_type.clone();
                    let source_message_id = self.source_message_id.clone();
                    self.state
                        .db
                        .with_conn(move |conn| {
                            native_execution_for_caller(
                                conn,
                                &execution_id,
                                &discussion_id,
                                &actor_type,
                                source_message_id.as_deref(),
                                true,
                            )
                            .map(|_| ())
                        })
                        .await
                };
                if let Err(error) = authorized {
                    return fail(call, error.to_string());
                }
                let Json(response) = crate::api::orchestration::reassign_execution(
                    State(self.state.clone()),
                    Path(execution_id),
                    Json(crate::api::orchestration::ReassignExecutionRequest { worker, reason }),
                )
                .await;
                unwrap_api(call, response.success, response.data, response.error)
            }
            other => fail(call, format!("unknown task execution tool `{other}`")),
        }
    }

    async fn execute_agent_resume_tool(&self, call: &ToolCall) -> ToolOutcome {
        let Some(discussion_id) = self.disc_id.as_deref() else {
            return fail(
                call,
                "agent resume tools require a discussion-bound native agent",
            );
        };
        let Some(actor_type) = self.actor_type.as_ref() else {
            return fail(
                call,
                "agent resume tools require a typed native provider identity",
            );
        };

        match call.name.as_str() {
            "agent_job_start" => {
                let request = match serde_json::from_value::<
                    crate::models::StartAgentBackgroundJobRequest,
                >(call.arguments.clone())
                {
                    Ok(request) => request,
                    Err(error) => return fail(call, format!("invalid job request: {error}")),
                };
                let Some(workspace_root) = self.workspace_root().await else {
                    return fail(
                        call,
                        "this discussion has no bounded workspace; attach it to a project before starting a command job",
                    );
                };
                let caller = crate::api::agent_jobs::NativeAgentJobCaller {
                    discussion_id,
                    agent_type: actor_type,
                    source_dispatch_job_id: self.source_dispatch_job_id.as_deref(),
                    workspace_root: Some(&workspace_root),
                };
                match crate::api::agent_jobs::start_background_job(&self.state, caller, request)
                    .await
                {
                    Ok(job) => ok(call, json!(job)),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "agent_schedule_wake" => {
                let request = match serde_json::from_value::<crate::models::ScheduleAgentWakeRequest>(
                    call.arguments.clone(),
                ) {
                    Ok(request) => request,
                    Err(error) => return fail(call, format!("invalid wake request: {error}")),
                };
                let caller = crate::api::agent_jobs::NativeAgentJobCaller {
                    discussion_id,
                    agent_type: actor_type,
                    source_dispatch_job_id: self.source_dispatch_job_id.as_deref(),
                    workspace_root: None,
                };
                match crate::api::agent_jobs::schedule_wake(&self.state, caller, request).await {
                    Ok(job) => ok(call, json!(job)),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "agent_resume_status" => {
                let discussion_id = discussion_id.to_string();
                let actor_type = actor_type.clone();
                let job_id = required_string(call, "job_id");
                let result = self
                    .state
                    .db
                    .with_conn(move |conn| {
                        if let Some(job_id) = job_id {
                            let job = crate::db::agent_jobs::get(conn, &job_id)?
                                .filter(|job| {
                                    job.view.discussion_id == discussion_id
                                        && job.view.target_agent == actor_type
                                })
                                .map(|job| job.view);
                            Ok(job.into_iter().collect::<Vec<_>>())
                        } else {
                            Ok(
                                crate::db::agent_jobs::list_for_discussion(conn, &discussion_id)?
                                    .into_iter()
                                    .filter(|job| job.target_agent == actor_type)
                                    .collect(),
                            )
                        }
                    })
                    .await;
                match result {
                    Ok(jobs) => ok(call, json!({"jobs": jobs})),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            "agent_resume_cancel" => {
                let Some(job_id) = required_string(call, "job_id") else {
                    return fail(call, "missing required field `job_id`");
                };
                let discussion_id = discussion_id.to_string();
                let actor_type = actor_type.clone();
                let lookup_id = job_id.clone();
                let cancelled = self
                    .state
                    .db
                    .with_conn(move |conn| {
                        let Some(job) = crate::db::agent_jobs::get(conn, &lookup_id)? else {
                            return Ok(false);
                        };
                        if job.view.discussion_id != discussion_id
                            || job.view.target_agent != actor_type
                        {
                            return Ok(false);
                        }
                        crate::db::agent_jobs::cancel(conn, &lookup_id, &discussion_id)
                    })
                    .await;
                match cancelled {
                    Ok(true) => {
                        if let Ok(mut registry) = self.state.cancel_registry.lock() {
                            if let Some(token) = registry.remove(&format!("agent-job:{job_id}")) {
                                token.cancel();
                            }
                        }
                        ok(call, json!({"cancelled": true, "job_id": job_id}))
                    }
                    Ok(false) => fail(
                        call,
                        "job not found, not active, or caller is not its owner",
                    ),
                    Err(error) => fail(call, error.to_string()),
                }
            }
            other => fail(call, format!("unknown agent resume tool `{other}`")),
        }
    }

    /// The directory the file tools are scoped to, resolved the way the rest of
    /// Kronn resolves a discussion's directory — most specific first:
    ///
    ///  1. a **managed** worktree, when the room is an orchestrated task's
    ///     sub-discussion (that checkout is the one the execution owns);
    ///  2. a workspace a CLI **declared** for the room;
    ///  3. the **project's path**, which is what nearly every room actually has.
    ///
    /// Step 3 was missing at first and it made the tools useless in practice: only
    /// 16 of 393 rooms carry a `discussion_workspaces` row, and all 16 come from
    /// orchestration — a `discussion_workspaces` row is not something a user
    /// creates. An agent in a normal project room was told "no workspace declared"
    /// while the project path sat right there on the discussion.
    ///
    /// `None` stays a readable refusal (a room attached to nothing): never a
    /// fallback to the server's cwd, which would hand the model the whole host.
    async fn workspace_root(&self) -> Option<std::path::PathBuf> {
        // A workflow step has no discussion but does carry a project, and a project
        // is a directory. Refusing the file tools there was the same mistake as
        // demanding a `discussion_workspaces` row from a discussion: the path was
        // available all along, just reached differently.
        let Some(disc_id) = self.disc_id.clone() else {
            let project_id = self.project_id.clone()?;
            return self
                .state
                .db
                .with_read_conn(move |conn| {
                    Ok(crate::db::projects::get_project(conn, &project_id)?
                        .map(|project| project.path)
                        .filter(|path| !path.trim().is_empty()))
                })
                .await
                .ok()?
                .map(std::path::PathBuf::from);
        };
        let resolved = self
            .state
            .db
            .with_read_conn(move |conn| {
                let rows = crate::db::discussion_workspaces::list_for_discussion(conn, &disc_id)?;
                let declared = rows
                    .iter()
                    .find(|row| row.ownership == "managed")
                    .or_else(|| rows.first())
                    .and_then(|row| row.canonical_path.clone())
                    .filter(|path| !path.trim().is_empty());
                if declared.is_some() {
                    return Ok(declared);
                }
                // Fall back to the project the discussion belongs to.
                let disc = crate::db::discussions::get_discussion(conn, &disc_id)?;
                let Some(project_id) = disc.and_then(|d| d.project_id) else {
                    return Ok(None);
                };
                let project = crate::db::projects::get_project(conn, &project_id)?;
                Ok(project
                    .map(|p| p.path)
                    .filter(|path| !path.trim().is_empty()))
            })
            .await
            .ok()??;
        Some(std::path::PathBuf::from(resolved))
    }

    /// KT-340 — read another discussion of the SAME project.
    ///
    /// Scope is the project, not the instance, and that is a confidentiality
    /// decision rather than a technical one: everything an HTTP agent reads is sent
    /// to whoever hosts it, so a remote provider must not be able to sweep rooms
    /// the user never associated with this work. A project is a grouping the user
    /// made themselves, which makes it a defensible boundary.
    ///
    /// Reading only — no tool here can write to another room.
    async fn read_other_discussion(&self, call: &ToolCall) -> ToolOutcome {
        let Some(disc_id) = self.disc_id.clone() else {
            return fail(
                call,
                "this run has no discussion, so it has no project to read within",
            );
        };
        // Default to the executor room so the model can read its own discussion history.
        let target = call.arguments["discussion_id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| disc_id.clone());
        let limit = count_arg(call, "limit").unwrap_or(40).clamp(1, 200) as usize;
        let target_for_db = target.clone();
        let result = self
            .state
            .db
            .with_read_conn(move |conn| {
                let here = crate::db::discussions::get_discussion(conn, &disc_id)?;
                let there = crate::db::discussions::get_discussion(conn, &target_for_db)?;
                let (Some(here), Some(there)) = (here, there) else {
                    return Ok(None);
                };
                // Same project, and a project that exists: two rooms with no project
                // are not "the same project", they are both unattached.
                let same_project = match (&here.project_id, &there.project_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
                if here.id != there.id && !same_project {
                    return Ok(Some((there.title, Vec::new(), false)));
                }
                let messages = crate::db::discussions::list_messages(conn, &target_for_db)?;
                Ok(Some((there.title, messages, true)))
            })
            .await;
        let Ok(Some((title, messages, allowed))) = result else {
            return fail(
                call,
                format!("discussion `{target}` not found or unreadable"),
            );
        };
        if !allowed {
            return fail(
                call,
                format!(
                    "refused: `{title}` belongs to another project (or none). An agent reads only \
                     within the project of its own discussion — what you read is sent to whoever \
                     hosts this model."
                ),
            );
        }
        // Keep the tail: the recent exchange is what a reader needs, and the whole
        // history would blow the window on a long room.
        let total = messages.len();
        let tail: Vec<Value> = messages
            .into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|message| {
                let body = message.content.chars().take(4_000).collect::<String>();
                serde_json::json!({
                    "role": format!("{:?}", message.role),
                    "agent_type": message.agent_type.map(|a| format!("{a:?}")),
                    "content": body,
                })
            })
            .collect();
        ok(
            call,
            serde_json::json!({
                "discussion_id": target,
                "title": title,
                "total_messages": total,
                "returned": tail.len(),
                "truncated": total > tail.len(),
                "messages": tail,
            }),
        )
    }

    /// KT-340 — the other discussions of this project, so an agent can find the
    /// room it needs instead of being handed a UUID.
    async fn list_project_discussions(&self, call: &ToolCall) -> ToolOutcome {
        let Some(disc_id) = self.disc_id.clone() else {
            return fail(
                call,
                "this run has no discussion, so it has no project to list",
            );
        };
        let rows = self
            .state
            .db
            .with_read_conn(move |conn| {
                let Some(here) = crate::db::discussions::get_discussion(conn, &disc_id)? else {
                    return Ok(Vec::new());
                };
                let Some(project_id) = here.project_id else {
                    return Ok(Vec::new());
                };
                Ok(crate::db::discussions::list_discussions(conn)?
                    .into_iter()
                    .filter(|d| d.project_id.as_deref() == Some(project_id.as_str()))
                    .map(|d| {
                        serde_json::json!({
                            "discussion_id": d.id,
                            "title": d.title,
                            "agent": format!("{:?}", d.agent),
                        })
                    })
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap_or_default();
        ok(
            call,
            serde_json::json!({ "count": rows.len(), "discussions": rows }),
        )
    }

    /// KT-338 — web and workspace tools. Every refusal is a readable `fail`, never
    /// an opaque error: the model must be able to correct its own call.
    async fn execute_workspace_tool(&self, call: &ToolCall) -> ToolOutcome {
        use crate::api::agent_workspace_tools as ws;
        if call.name == "web_fetch" {
            let Some(url) = call.arguments["url"].as_str() else {
                return fail(call, "missing required field `url`");
            };
            return match ws::check_fetch_url(url).await {
                Err(refusal) => fail(call, refusal.message()),
                Ok(validated) => match ws::fetch_text(validated).await {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                },
            };
        }
        // The remaining tools are workspace-scoped.
        let Some(root) = self.workspace_root().await else {
            return fail(call, ws::Refusal::NoWorkspace.message());
        };
        match call.name.as_str() {
            "read_file" => match call.arguments["path"].as_str() {
                None => fail(call, "missing required field `path`"),
                Some(path) => {
                    let as_count = |field: &str| count_arg(call, field).map(|value| value as usize);
                    match ws::read_file_payload(&root, path, as_count("offset"), as_count("limit"))
                    {
                        Ok(payload) => ok(call, payload),
                        Err(message) => fail(call, message),
                    }
                }
            },
            "write_file" => {
                let Some(path) = call.arguments["path"].as_str() else {
                    return fail(call, "missing required field `path`");
                };
                let Some(content) = call.arguments["content"].as_str() else {
                    return fail(call, "missing required field `content`");
                };
                match ws::write_file_payload_with_receipt(
                    &root,
                    path,
                    content,
                    call.arguments["expected_sha256"].as_str(),
                ) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                }
            }
            "list_files" => match ws::list_files_payload(
                &root,
                call.arguments["path"].as_str(),
                flag_arg(call, "recursive").unwrap_or(false),
            ) {
                Ok(payload) => ok(call, payload),
                Err(message) => fail(call, message),
            },
            "git_status" => match ws::git_status_payload(&root) {
                Ok(payload) => ok(call, payload),
                Err(message) => fail(call, message),
            },
            "git_diff" => match ws::git_diff_payload(
                &root,
                call.arguments["revision_range"].as_str(),
                call.arguments["path"].as_str(),
            ) {
                Ok(payload) => ok(call, payload),
                Err(message) => fail(call, message),
            },
            "git_log" => {
                match ws::git_log_payload(
                    &root,
                    count_arg(call, "limit").map(|v| v as u32),
                    call.arguments["path"].as_str(),
                ) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                }
            }
            "git_commit" => {
                let Some(discussion_id) = self.disc_id.clone() else {
                    return fail(
                        call,
                        "git_commit is available only to a native worker in its managed task worktree",
                    );
                };
                let Some(actor_type) = self.actor_type.clone() else {
                    return fail(
                        call,
                        "git_commit requires a typed native worker identity supplied by Kronn",
                    );
                };
                let canonical_root = match root.canonicalize() {
                    Ok(root) => root,
                    Err(_) => return fail(call, ws::Refusal::NoWorkspace.message()),
                };
                let canonical_path = canonical_root.to_string_lossy().to_string();
                let source_message_id = self.source_message_id.clone();
                let authorised = self
                    .state
                    .db
                    .with_read_conn(move |conn| {
                        let Some(execution) =
                            crate::db::orchestration::managed_working_execution_for_workspace(
                                conn,
                                &discussion_id,
                                &canonical_path,
                            )?
                        else {
                            return Ok(false);
                        };
                        native_execution_for_caller(
                            conn,
                            &execution.id,
                            &discussion_id,
                            &actor_type,
                            source_message_id.as_deref(),
                            false,
                        )?;
                        Ok(true)
                    })
                    .await;
                if !matches!(authorised, Ok(true)) {
                    return fail(
                        call,
                        "git_commit is available only to the exact active native worker in its managed task worktree",
                    );
                }
                let Some(files) = call.arguments["files"].as_array() else {
                    return fail(call, "missing array field `files`");
                };
                let files: Result<Vec<String>, _> = files
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .ok_or("every `files` entry must be a string")
                    })
                    .collect();
                let files = match files {
                    Ok(files) => files,
                    Err(message) => return fail(call, message),
                };
                let Some(message) = call.arguments["message"].as_str() else {
                    return fail(call, "missing required field `message`");
                };
                match ws::git_commit_payload(&canonical_root, &files, message) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                }
            }
            "find_files" => match call.arguments["pattern"].as_str() {
                None => fail(call, "missing required field `pattern`"),
                Some(pattern) => match ws::find_files_payload(&root, pattern) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                },
            },
            "edit_file" => {
                let Some(path) = call.arguments["path"].as_str() else {
                    return fail(call, "missing required field `path`");
                };
                // Never trimmed: leading whitespace IS the anchor in an
                // indentation-significant file.
                let Some(old_string) = call.arguments["old_string"].as_str() else {
                    return fail(call, "missing required field `old_string`");
                };
                let Some(new_string) = call.arguments["new_string"].as_str() else {
                    return fail(
                        call,
                        "missing required field `new_string` (may be empty to delete)",
                    );
                };
                let Some(expected_sha256) = call.arguments["expected_sha256"].as_str() else {
                    return fail(
                        call,
                        "missing required field `expected_sha256`; copy `content_sha256` from the read/search result used for this edit",
                    );
                };
                match ws::edit_file_payload(
                    &root,
                    path,
                    old_string,
                    new_string,
                    flag_arg(call, "replace_all").unwrap_or(false),
                    expected_sha256,
                ) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                }
            }
            "edit_lines" => {
                let Some(path) = call.arguments["path"].as_str() else {
                    return fail(call, "missing required field `path`");
                };
                if call.arguments.get("start_line").is_none() {
                    return fail(call, "missing required field `start_line`");
                }
                let Some(start_line) =
                    count_arg(call, "start_line").and_then(|value| usize::try_from(value).ok())
                else {
                    return fail(call, "`start_line` must be a positive integer");
                };
                if call.arguments.get("end_line").is_none() {
                    return fail(call, "missing required field `end_line`");
                }
                let Some(end_line) =
                    count_arg(call, "end_line").and_then(|value| usize::try_from(value).ok())
                else {
                    return fail(call, "`end_line` must be a positive integer");
                };
                let Some(new_string) = call.arguments["new_string"].as_str() else {
                    return fail(
                        call,
                        "missing required field `new_string` (may be empty to delete)",
                    );
                };
                let Some(expected_sha256) = call.arguments["expected_sha256"].as_str() else {
                    return fail(
                        call,
                        "missing required field `expected_sha256`; copy `content_sha256` from the read/search result used for this edit",
                    );
                };
                match ws::edit_lines_payload(
                    &root,
                    path,
                    start_line,
                    end_line,
                    new_string,
                    expected_sha256,
                ) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                }
            }
            "insert_after_line" => {
                let Some(path) = call.arguments["path"].as_str() else {
                    return fail(call, "missing required field `path`");
                };
                if call.arguments.get("anchor_line").is_none() {
                    return fail(call, "missing required field `anchor_line`");
                }
                let Some(anchor_line) = count_arg(call, "anchor_line")
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|value| *value > 0)
                else {
                    return fail(call, "`anchor_line` must be a positive integer");
                };
                let Some(new_string) = call.arguments["new_string"].as_str() else {
                    return fail(call, "missing required field `new_string`");
                };
                let Some(expected_sha256) = call.arguments["expected_sha256"].as_str() else {
                    return fail(
                        call,
                        "missing required field `expected_sha256`; copy `content_sha256` from the authoritative read",
                    );
                };
                match ws::insert_after_line_payload(
                    &root,
                    path,
                    anchor_line,
                    new_string,
                    expected_sha256,
                ) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                }
            }
            "search_text" => match call.arguments["query"].as_str() {
                None => fail(call, "missing required field `query`"),
                Some(query) => match ws::search_text_payload(
                    &root,
                    query,
                    call.arguments["path_glob"].as_str(),
                    flag_arg(call, "case_sensitive").unwrap_or(false),
                ) {
                    Ok(payload) => ok(call, payload),
                    Err(message) => fail(call, message),
                },
            },
            other => fail(call, format!("unknown workspace tool `{other}`")),
        }
    }

    async fn effective_project_id(&self) -> Option<String> {
        if self.project_id.is_some() {
            return self.project_id.clone();
        }
        let disc_id = self.disc_id.clone()?.trim().to_string();
        if disc_id.is_empty() {
            return None;
        }
        self.state
            .db
            .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &disc_id))
            .await
            .ok()
            .flatten()
            .and_then(|discussion| discussion.project_id)
    }

    fn discussion_id(&self, call: &ToolCall) -> Option<String> {
        self.disc_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
            .or_else(|| required_string(call, "discussion_id"))
    }

    fn actor(&self) -> PlanningActor {
        PlanningActor {
            kind: PlanningActorKind::Agent,
            id: Some(self.actor_id.clone()),
            session_id: self.actor_session_id(),
            source_message_id: self.source_message_id.clone(),
        }
    }

    fn actor_session_id(&self) -> Option<String> {
        let discussion_id = self
            .disc_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let source_message_id = self
            .source_message_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        Some(format!("native:{discussion_id}:{source_message_id}"))
    }

    fn actor_json(&self) -> Value {
        serde_json::to_value(self.actor()).expect("PlanningActor is serializable")
    }

    /// Test seam for `resolve_config_id`, which is otherwise only reachable
    /// through a live tool call.
    #[cfg(test)]
    pub async fn resolve_config_id_pub(&self, slug: &str) -> Option<String> {
        self.resolve_config_id(slug).await
    }

    /// The config id Kronn would use for a plugin slug. `None` when the plugin
    /// has no configuration, which is a real answer, not an error to hide.
    async fn resolve_config_id(&self, slug: &str) -> Option<String> {
        use axum::extract::State;
        use axum::Json;
        let Json(res) = crate::api::mcps::overview(State(self.state.clone())).await;
        let v = serde_json::to_value(res.data?).ok()?;
        let project_id = self.effective_project_id().await;
        v["configs"]
            .as_array()?
            .iter()
            .find(|c| c["server_id"] == slug && config_is_in_scope(c, project_id.as_deref()))
            .and_then(|c| c["id"].as_str().map(str::to_string))
    }

    async fn config_id_in_scope(&self, slug: &str, config_id: &str) -> bool {
        use axum::extract::State;
        let axum::Json(res) = crate::api::mcps::overview(State(self.state.clone())).await;
        let Some(v) = res.data.and_then(|data| serde_json::to_value(data).ok()) else {
            return false;
        };
        let project_id = self.effective_project_id().await;
        v["configs"].as_array().is_some_and(|configs| {
            configs.iter().any(|config| {
                config["id"] == config_id
                    && config["server_id"] == slug
                    && config_is_in_scope(config, project_id.as_deref())
            })
        })
    }
}

fn native_execution_for_caller(
    conn: &rusqlite::Connection,
    execution_id: &str,
    discussion_id: &str,
    actor_type: &AgentType,
    source_message_id: Option<&str>,
    principal_only: bool,
) -> anyhow::Result<crate::models::TaskExecution> {
    let execution =
        crate::api::orchestration::resolve_task_execution_reference(conn, execution_id)?
            .ok_or_else(|| anyhow::anyhow!("execution not found or caller is not a party"))?;
    let is_principal = execution.parent_discussion_id == discussion_id;
    let provider_matches = execution
        .worker_agent_type
        .as_deref()
        .map(crate::db::orchestration::agent_type_from_db)
        .transpose()?
        .as_ref()
        == Some(actor_type);
    let is_native_worker = execution.worker_cli_session_id.is_none()
        && execution.sub_discussion_id.as_deref() == Some(discussion_id)
        && provider_matches
        && crate::api::orchestration::native_worker_dispatch_matches(
            conn,
            &execution,
            source_message_id,
        )?;
    let authorized = is_principal || (!principal_only && is_native_worker);
    if !authorized {
        anyhow::bail!(if principal_only {
            "execution not found or caller is not its principal"
        } else {
            "execution not found or caller is not a party"
        });
    }
    Ok(execution)
}

/// Resolve a worker's delivery target solely from context Kronn attached to
/// this executor. Model arguments are deliberately absent from this boundary:
/// even a valid execution id for a concurrent worker cannot redirect delivery.
fn native_worker_execution_for_trusted_dispatch(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    actor_type: &AgentType,
    source_message_id: Option<&str>,
    source_dispatch_job_id: Option<&str>,
) -> anyhow::Result<crate::models::TaskExecution> {
    let dispatch_job_id = source_dispatch_job_id
        .ok_or_else(|| anyhow::anyhow!("execution not found or caller is not a party"))?;
    let execution = crate::db::orchestration::get_execution_for_dispatch(conn, dispatch_job_id)?
        .ok_or_else(|| anyhow::anyhow!("execution not found or caller is not a party"))?;
    native_execution_for_caller(
        conn,
        &execution.id,
        discussion_id,
        actor_type,
        source_message_id,
        false,
    )
}

/// Read a whole-number argument, whether the model typed it as a number or as
/// a string.
///
/// Measured, not defensive: a worker asked for `read_file` with
/// `{"limit":"120","offset":"1995"}`. `as_u64` said None, the slice was
/// SILENTLY dropped, the whole file came back, and the model — having received
/// something other than what it asked for — asked again, identically, until the
/// loop guard refused it. A quoted number is the same number; refusing it would
/// at least have been honest, but accepting it is what the caller meant.
fn count_arg(call: &ToolCall, field: &str) -> Option<u64> {
    let value = &call.arguments[field];
    value.as_u64().or_else(|| {
        let text = value.as_str()?.trim();
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    })
}

/// Same for booleans: `"true"` from a model that quotes everything means true.
fn flag_arg(call: &ToolCall, field: &str) -> Option<bool> {
    let value = &call.arguments[field];
    value.as_bool().or_else(|| match value.as_str()?.trim() {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    })
}

fn required_string(call: &ToolCall, field: &str) -> Option<String> {
    call.arguments[field]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn task_worker_scope_contract(
    call: &ToolCall,
) -> Result<
    (
        crate::api::orchestration::TaskWorkerScopeIntent,
        Option<crate::models::TaskWorkerScope>,
    ),
    String,
> {
    use crate::api::orchestration::TaskWorkerScopeIntent;

    let intent = match call
        .arguments
        .get("worker_scope_intent")
        .and_then(Value::as_str)
    {
        Some("generic") => TaskWorkerScopeIntent::Generic,
        Some("scoped") => TaskWorkerScopeIntent::Scoped,
        _ => {
            return Err(
                "missing or invalid `worker_scope_intent`; the tool schema may be stale — reconnect before retrying"
                    .into(),
            )
        }
    };
    let worker_scope = match call.arguments.get("worker_scope") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            serde_json::from_value::<crate::models::TaskWorkerScope>(value.clone())
                .map_err(|error| format!("invalid worker_scope: {error}"))?,
        ),
    };
    if let Some(reason) = crate::api::orchestration::worker_scope_contract_refusal(
        Some(intent),
        worker_scope.as_ref(),
    ) {
        return Err(format!("{}: {}", reason.code, reason.detail));
    }
    Ok((intent, worker_scope))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Trim a description to something a model can scan without paying for prose.
fn brief(v: &Value, max: usize) -> String {
    let s = v.as_str().unwrap_or("").trim();
    match s.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &s[..cut]),
        None => s.to_string(),
    }
}

/// What a model needs to pick a plugin: slug and purpose. **Not** the API
/// specs — the raw overview is ~52 KB (~13k tokens), which would swallow a
/// local model's whole context window on the first tool call. Specs are
/// fetched one plugin at a time via `api_endpoints`.
fn config_is_in_scope(config: &Value, project_id: Option<&str>) -> bool {
    if config["is_global"].as_bool().unwrap_or(false) {
        return true;
    }
    match project_id {
        Some(project_id) => config["project_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == project_id)),
        None => config["include_general"].as_bool().unwrap_or(false),
    }
}

fn compact_plugin_list(overview: &Value, project_id: Option<&str>) -> Value {
    let configs = overview["configs"].as_array();
    // `api_call` needs the CONFIG id, not just the plugin slug — a plugin can
    // be wired more than once (different accounts/projects) and the broker
    // resolves credentials per config. Omitting it made every api_call fail
    // with "Either (api_plugin_slug + api_config_id) OR quick_api_id is
    // required" and left the model with no way to discover it.
    let config_for = |slug: &Value| -> Option<Value> {
        configs?
            .iter()
            .find(|c| c["server_id"] == *slug && config_is_in_scope(c, project_id))
            .map(|c| c["id"].clone())
    };
    let plugins: Vec<Value> = overview["servers"]
        .as_array()
        .map(|servers| {
            servers
                .iter()
                .filter(|s| !s["api_spec"].is_null())
                // A plugin with no config has no credentials, so it cannot be
                // called; listing it would only invite a failing api_call.
                .filter_map(|s| {
                    let config_id = config_for(&s["id"])?;
                    Some(json!({
                        "slug": s["id"],
                        "api_config_id": config_id,
                        "name": s["name"],
                        "purpose": brief(&s["description"], 160),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "plugins": plugins,
        "next": "call api_endpoints with a slug to see its paths, then api_call with BOTH \
                 the slug and its api_config_id",
    })
}

/// Endpoint paths for a single plugin. Method + path + a short summary is
/// what `api_call` actually needs; the rest of the spec is noise to a model.
fn compact_endpoints(overview: &Value, slug: &str) -> Option<Value> {
    let server = overview["servers"]
        .as_array()?
        .iter()
        .find(|s| s["id"] == slug && !s["api_spec"].is_null())?;
    let endpoints: Vec<Value> = server["api_spec"]["endpoints"]
        .as_array()
        .map(|eps| {
            eps.iter()
                .map(|e| {
                    json!({
                        "method": e["method"].as_str().unwrap_or("GET"),
                        "path": e["path"],
                        "summary": brief(&e["description"], 120),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(json!({ "slug": slug, "endpoints": endpoints }))
}

/// Quick APIs, minus the machinery. `variables`, extraction specs and
/// timestamps account for most of the raw payload and none of the decision.
/// Overlay what the caller named onto the definition already stored.
///
/// The HTTP handler behind an update is a full replacement, and the agent has
/// no way to read back a full definition — `qa_list` returns a compact view by
/// design, since it is paid on every turn. Requiring the whole object would be
/// the same defect as an id no tool can produce: declared, and unsatisfiable.
///
/// So the agent names only what changes. Serialising the stored record and
/// overlaying the call's own keys keeps one source of truth for the shape —
/// there is no second list of fields here to fall out of step with the model.
fn merged_definition<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    existing: &T,
    arguments: &Value,
) -> Result<R, String> {
    let mut base = serde_json::to_value(existing).map_err(|error| error.to_string())?;
    let (Some(base_fields), Some(named)) = (base.as_object_mut(), arguments.as_object()) else {
        return Err("definition is not an object".to_string());
    };
    for (field, value) in named {
        // The id addresses the record; it is not part of its definition, and
        // letting one through would rename what the update is pointing at.
        if field.ends_with("_id") && field != "project_id" {
            continue;
        }
        base_fields.insert(field.clone(), value.clone());
    }
    serde_json::from_value(base).map_err(|error| error.to_string())
}

/// What the declarations deliberately leave out.
///
/// The catalogue is re-sent on every turn, so a description that explains an
/// argument shape is paid on every message of every room. The bridge solved
/// this with `tool_manual` and so does this surface: the declaration carries
/// the contract, the manual carries the detail, and only an agent that is
/// about to author something pays for it.
fn tool_manual(name: Option<&str>) -> Value {
    const MANUALS: &[(&str, &str)] = &[
        (
            "workflow_create_draft",
            "Read workflow_step_schema for the canonical step contracts; step_type narrows the result. \
             Shared templating and data pipeline contracts are available by section. \
             Read workflow_get on an accessible example. Send name, trigger (e.g. {type: Manual}) \
             and 1–20 steps; each has name and step_type:{type: ...}. Optional authoring fields: \
             actions, safety, workspace_config, guards, artifacts, on_failure (0–20 steps), \
             exec_allowlist, concurrency_limit, variables ({name,label,placeholder,required}). \
             Exec binaries must be allowlisted. The workflow is always disabled for human review; \
             no native tool here enables, triggers or deletes it. Cron/Tracker drafts default \
             concurrency_limit to 1. Omitted project_id inherits the room; null means general; \
             other projects are refused. Unlisted top-level fields are ignored.",
        ),
        (
            "workflow_update",
            "Read workflow_get first. Pass workflow_id and only changed authoring fields from \
             workflow_create_draft. Omitted fields survive; supplied arrays replace the whole \
             array, so include all intended steps when editing steps. The project never moves \
             and enabled cannot be set. Only disabled workflows can be updated; disable an \
             enabled workflow in the Workflows UI first, or author a new disabled draft. \
             No workflow runs from this call.",
        ),
        (
            "qp_create_draft",
            "Save a prompt_template with {{name}} placeholders. Declare each variable as \
             {\"name\":\"topic\",\"label\":\"Topic\",\"placeholder\":\"Release notes\",\"required\":true}. \
             Both label and placeholder are required. Allowed fields: name, description, icon, \
             prompt_template, variables, project_id. Other fields are ignored. A draft does not run; \
             it uses Kronn's creation defaults (Claude Code, default tier, no persona or rules). \
             The human can configure its model and bindings in the automation library. \
             project_id defaults to this discussion's project; an explicit null creates a general \
             prompt. Another project's id is refused. qp_id and vars follow the MCP Quick Prompt contract.",
        ),
        (
            "qp_update",
            "Pass qp_id from qp_list and only the fields that change: name, description, icon, \
             prompt_template, variables. Omitted fields keep their stored values; \
             variables replaces the whole list. Use the variable shape in qp_create_draft. \
             The project scope stays unchanged, including if project_id is supplied. \
             The saved agent, connection, tier, settings, skills, profiles and directives are \
             preserved. Fields outside the allowed list are ignored.",
        ),
        (
            "qp_run",
            "Pass qp_id from qp_list and vars mapping the required variable names to values. \
             This launches exactly one new discussion with the saved agent, connection, tier, \
             settings, skills, profiles and directives. Model or project overrides are ignored. \
             A general prompt inherits the source discussion's project. The result includes \
             disc_id and next_check; within a project, wait for that interval then call \
             disc_read({discussion_id: disc_id}). Without a project the result is available \
             in the new discussion's UI; cross-discussion reading is not available. \
             An optional title names the new discussion. Batch launch and deletion are not available.",
        ),
        (
            "qa_create_draft",
            "A Quick API is a saved, replayable call through a configured plugin.              Kronn injects the credential server-side at run time — never put a key,              a token or an Authorization header in the definition.\n\n             Discovery order: `mcp_list` gives `api_plugin_slug` and `api_config_id`;              `api_endpoints` gives the exact `api_endpoint_path` for that plugin. Both              must come from those calls; a guessed path is a 404 at run time, not a              validation error here.\n\n             Variables are `{{name}}` placeholders anywhere in the path, query, headers              or body. Declare each one as `{\"name\": \"service_id\", \"label\":              \"Service\", \"placeholder\": \"abc123\", \"required\": true}`; `label`              and `placeholder` are what a human sees in the launcher, so write them for              a human. A variable used but not declared fails the launch.\n\n             `project_id` scopes it: set it and only that project's rooms see it, omit it              and every room does. A draft is saved disabled-safe — it runs only when              someone calls `qa_run` with it.",
        ),
        (
            "qe_create_draft",
            "A Quick Exec is one saved command. There is NO shell: `command` is the              binary alone and every argument is its own element of `args`. A pipe, a              redirection, a `&&`, a glob or a `$VAR` written inside a string reaches the              binary as literal text — it is not interpreted, and that is the point.\n\n             So `aws logs tail /my/group --since 1h` is              `command: \"aws\"`, `args: [\"logs\", \"tail\", \"/my/group\",              \"--since\", \"1h\"]`. Never `command: \"aws logs tail ...\"`.\n\n             `{{var}}` is substituted inside an element before execution, so              `args: [\"logs\", \"tail\", \"{{group}}\"]` with a declared `group`              variable is the way to parameterise it. Substitution happens per element,              which is what stops a value containing a space from becoming two arguments.\n\n             `output_format` decides what a caller gets back: `json` parsed as-is, `csv`              as an array of objects, `lines` as an array, `text` as a string. Pick the              one the command actually produces — a mismatch is returned as an error, not              silently coerced.\n\n             `timeout_secs` is a ceiling, not a hint: the process is killed at it. The              working directory is the project's, and a Quick Exec scoped to a project              cannot be started from a room belonging to another one.",
        ),
        (
            "qe_run",
            "Runs the saved argv now and returns its output, parsed per the Quick Exec's              `output_format`. The room must belong to the same project as the Quick Exec,              or to none — the refusal names which.\n\n             This blocks until the command finishes or its timeout kills it. For anything              slow, `agent_job_start` takes the same id, survives the end of your turn and              wakes the room once with the result — which is cheaper than holding a turn              open and is the only option if the command outlives the request.",
        ),
        (
            "qa_update",
            "Name only what changes. Kronn reads the stored definition and overlays your \
             fields onto it, so an omitted field keeps its current value rather than being \
             cleared — `qa_list` returns a compact view and could never have handed you the \
             whole object to send back.\n\n\
             A field you DO send replaces its value outright: sending `variables` replaces \
             the whole list, it does not append to it.\n\n\
             The id keeps its run history, so an edit that changes what the call does makes \
             the older runs misleading rather than wrong. When the intent changes, a new \
             draft is more honest than an update.",
        ),
        (
            "qe_update",
            "Name only what changes; Kronn overlays your fields onto the stored definition \
             and keeps the rest. A field you do send replaces its value outright — sending \
             `args` replaces the whole argv, it does not append to it.\n\n\
             The argv rules are the same as for `qe_create_draft`: no shell, one element \
             per argument.",
        ),
    ];

    match name.map(str::trim).filter(|name| !name.is_empty()) {
        None => json!({
            "available": MANUALS.iter().map(|(name, _)| *name).chain(std::iter::once("signals")).collect::<Vec<_>>(),
            "hint": "Pass `tool` to read one. signals lists structured proposal contracts; other entries document the named tool.",
        }),
        Some("signals") => {
            json!({"tool": "signals", "catalogue": crate::api::signal_catalog::catalogue()})
        }
        Some(wanted) => match MANUALS.iter().find(|(name, _)| *name == wanted) {
            Some((name, manual)) => json!({ "tool": name, "manual": manual }),
            None => json!({
                "error": format!("no manual for `{wanted}`"),
                "available": MANUALS.iter().map(|(name, _)| *name).chain(std::iter::once("signals")).collect::<Vec<_>>(),
            }),
        },
    }
}

fn quick_api_is_in_scope(quick_api: &crate::models::QuickApi, project_id: Option<&str>) -> bool {
    match project_id {
        Some(project_id) => quick_api
            .project_id
            .as_deref()
            .is_none_or(|id| id == project_id),
        None => quick_api.project_id.is_none(),
    }
}

fn project_is_in_scope(target: Option<&str>, project_id: Option<&str>) -> bool {
    target.is_none() || target == project_id
}

fn quick_prompt_write_request(
    existing: Option<&crate::models::QuickPrompt>,
    arguments: &Value,
    project_id: Option<&str>,
) -> Result<crate::models::CreateQuickPromptRequest, String> {
    let Some(fields) = arguments.as_object() else {
        return Err("invalid Quick Prompt: definition is not an object".into());
    };
    // These tools may edit a template, never choose its model or bindings.
    // Schemas alone do not constrain arguments actually sent by a provider.
    let allowed = [
        "name",
        "description",
        "icon",
        "prompt_template",
        "variables",
        "project_id",
    ];
    let mut filtered = Value::Object(
        fields
            .iter()
            .filter(|(key, _)| {
                allowed.contains(&key.as_str())
                    && !(existing.is_some() && key.as_str() == "project_id")
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    if existing.is_none() && !fields.contains_key("project_id") {
        filtered["project_id"] = json!(project_id);
    }
    let request: crate::models::CreateQuickPromptRequest = match existing {
        Some(saved) => merged_definition(saved, &filtered)?,
        None => serde_json::from_value(filtered)
            .map_err(|error| format!("invalid Quick Prompt: {error}"))?,
    };
    if !project_is_in_scope(request.project_id.as_deref(), project_id) {
        return Err("Quick Prompt project is outside this discussion's scope".into());
    }
    Ok(request)
}

fn compact_quick_prompts(items: &[crate::models::QuickPrompt], project_id: Option<&str>) -> Value {
    json!({"quick_prompts": items.iter()
        .filter(|qp| project_is_in_scope(qp.project_id.as_deref(), project_id))
        .map(|qp| json!({
            "id": qp.id,
            "name": qp.name,
            "does": brief(&json!(qp.description), 140),
            "agent": qp.agent,
            "required_variables": qp.variables.iter().filter(|variable| variable.required)
                .map(|variable| &variable.name).collect::<Vec<_>>(),
        })).collect::<Vec<_>>()})
}

/// The Quick Execs this discussion could actually start, compacted.
///
/// Filtered on exactly the rule `start_background_job` enforces: an id shown
/// here and refused there would be worse than not listing it. The command is
/// carried because a Quick Exec is often saved without a description, and its
/// name alone does not say what running it would do.
fn compact_quick_execs(items: &[crate::models::QuickExec], project_id: Option<&str>) -> Value {
    let list: Vec<Value> = items
        .iter()
        .filter(|quick| match project_id {
            Some(project_id) => quick
                .project_id
                .as_deref()
                .is_none_or(|id| id == project_id),
            None => quick.project_id.is_none(),
        })
        .map(|quick| {
            json!({
                "id": quick.id,
                "name": quick.name,
                "does": brief(&json!(quick.description), 140),
                "command": brief(&json!(quick.command), 120),
                "required_variables": quick
                    .variables
                    .iter()
                    .filter(|variable| variable.required)
                    .map(|variable| variable.name.clone())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "quick_execs": list })
}

fn compact_quick_apis(items: &Value, project_id: Option<&str>) -> Value {
    let list: Vec<Value> = items
        .as_array()
        .map(|qs| {
            qs.iter()
                .filter(|q| match project_id {
                    Some(project_id) => q["project_id"].is_null() || q["project_id"] == project_id,
                    None => q["project_id"].is_null(),
                })
                .map(|q| {
                    json!({
                        "id": q["id"],
                        "name": q["name"],
                        "does": brief(&q["description"], 140),
                        "required_variables": q["variables"]
                            .as_array()
                            .map(|vs| vs.iter()
                                .filter(|v| v["required"].as_bool().unwrap_or(false))
                                .filter_map(|v| v["name"].as_str().map(str::to_string))
                                .collect::<Vec<_>>())
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "quick_apis": list })
}

/// Collapse a handler's `ApiResponse` into the payload the model sees.
fn unwrap_api<T: serde::Serialize>(
    call: &ToolCall,
    success: bool,
    data: Option<T>,
    error: Option<String>,
) -> ToolOutcome {
    match (success, data) {
        (true, Some(d)) => match serde_json::to_value(d) {
            Ok(v) => ok(call, v),
            Err(e) => fail(call, format!("could not serialise result: {e}")),
        },
        _ => fail(call, error.unwrap_or_else(|| "call failed".into())),
    }
}

/// Planning writes return a full task for the UI, including descriptions and
/// history. The agent already knows what it wrote; keep only the receipt it
/// needs to verify the mutation, matching the stdio bridge's `_task_ack`.
fn unwrap_api_compact_task(
    call: &ToolCall,
    success: bool,
    data: Option<crate::models::PlanningTaskDetail>,
    error: Option<String>,
) -> ToolOutcome {
    match (success, data) {
        (true, Some(task)) => ok(
            call,
            json!({
                "id": task.summary.id,
                "reference": task.summary.reference,
                "title": task.summary.title,
                "status": task.summary.status,
                "priority": task.summary.priority,
                "parent_reference": task.summary.parent_reference,
                "blocker_count": task.summary.blocker_count,
                "dod_progress": format!(
                    "{}/{}",
                    task.definition_of_done.iter().filter(|item| item.completed).count(),
                    task.definition_of_done.len()
                ),
                "omitted": "description, full definition_of_done, events — call task_get for them"
            }),
        ),
        _ => fail(call, error.unwrap_or_else(|| "call failed".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progressive_tool_loading_requires_an_explicit_opt_in() {
        for raw in [None, Some(""), Some("0"), Some("true"), Some("unexpected")] {
            assert!(
                !explicit_tiering(raw),
                "{raw:?} must keep the full catalogue"
            );
        }
        for raw in [Some("1"), Some(" 1 ")] {
            assert!(explicit_tiering(raw));
        }
    }

    #[tokio::test]
    async fn a_projectless_room_can_read_itself_but_not_other_unattached_rooms() {
        let state = super::quick_prompt_tests::state_with_prompts().await;
        state.db.with_conn(|conn| {
            for (id, project) in [("other-general", None), ("other-a", Some("a")), ("room-b", Some("b"))] {
                conn.execute("INSERT INTO discussions (id,title,project_id,created_at,updated_at) VALUES (?1,?1,?2,'2026-09-22T00:00:00Z','2026-09-22T00:00:00Z')", rusqlite::params![id, project])?;
            }
            conn.execute("INSERT INTO messages (id,discussion_id,role,content,timestamp) VALUES ('self-message','general','User','Own room history','2026-09-22T00:00:00Z')", [])?;
            Ok(())
        }).await.unwrap();
        for (room, target, allowed) in [
            ("general", None, true),
            ("general", Some("general"), true),
            ("general", Some("other-general"), false),
            ("general", Some("room-a"), false),
            ("room-a", None, true),
            ("room-a", Some("other-a"), true),
            ("room-a", Some("room-b"), false),
            ("room-a", Some("general"), false),
            ("general", Some("missing"), false),
        ] {
            let executor = KronnToolExecutor::new(state.clone(), Some(room.into()));
            let result = executor
                .execute(&ToolCall {
                    id: "read".into(),
                    name: "disc_read".into(),
                    arguments: target.map_or_else(|| json!({}), |id| json!({"discussion_id":id})),
                })
                .await;
            assert_eq!(
                result.ok, allowed,
                "{room} -> {target:?}: {}",
                result.content
            );
            if allowed && room == "general" {
                assert_eq!(result.content["discussion_id"], "general");
                assert_eq!(result.content["messages"][0]["content"], "Own room history");
            }
        }
    }

    fn call_with(arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments,
        }
    }

    /// A model that quotes its numbers meant the numbers. Measured on a real
    /// delegation: `{"limit":"120","offset":"1995"}` silently lost its slice,
    /// returned the whole file, and the worker asked again — identically — until
    /// the loop guard refused it.
    #[test]
    fn a_quoted_number_is_still_a_number() {
        let typed = call_with(serde_json::json!({ "offset": 1995, "limit": 120 }));
        assert_eq!(count_arg(&typed, "offset"), Some(1995));
        assert_eq!(count_arg(&typed, "limit"), Some(120));

        let quoted = call_with(serde_json::json!({ "offset": "1995", "limit": " 120 " }));
        assert_eq!(
            count_arg(&quoted, "offset"),
            Some(1995),
            "a quoted offset is an offset"
        );
        assert_eq!(
            count_arg(&quoted, "limit"),
            Some(120),
            "and whitespace is not an argument"
        );

        let nonsense = call_with(serde_json::json!({
            "offset": "the top",
            "limit": -3,
            "signed": "+3",
            "fraction": "3.0",
            "scientific": "3e0",
            "empty": "   ",
        }));
        assert_eq!(count_arg(&nonsense, "offset"), None, "prose is not a count");
        assert_eq!(count_arg(&nonsense, "limit"), None, "nor is a negative one");
        assert_eq!(count_arg(&nonsense, "signed"), None, "nor a signed string");
        assert_eq!(count_arg(&nonsense, "fraction"), None, "nor a fraction");
        assert_eq!(
            count_arg(&nonsense, "scientific"),
            None,
            "nor exponent notation"
        );
        assert_eq!(count_arg(&nonsense, "empty"), None, "nor an empty string");
        assert_eq!(count_arg(&nonsense, "absent"), None);
    }

    #[test]
    fn a_quoted_boolean_is_still_a_boolean() {
        let typed = call_with(serde_json::json!({ "recursive": true }));
        assert_eq!(flag_arg(&typed, "recursive"), Some(true));

        let quoted = call_with(serde_json::json!({ "a": "true", "b": "False", "c": "TRUE" }));
        assert_eq!(flag_arg(&quoted, "a"), Some(true));
        assert_eq!(flag_arg(&quoted, "b"), Some(false));
        assert_eq!(flag_arg(&quoted, "c"), Some(true));

        let nonsense = call_with(serde_json::json!({ "a": "yes", "b": 1 }));
        assert_eq!(
            flag_arg(&nonsense, "a"),
            None,
            "only a boolean spelling counts"
        );
        assert_eq!(flag_arg(&nonsense, "b"), None, "a number is not a flag");
        assert_eq!(flag_arg(&nonsense, "absent"), None);
    }

    #[test]
    fn the_three_http_providers_get_the_same_catalogue() {
        // Romu's question in one assertion: NVIDIA, LiteLLM and Ollama must not drift
        // apart. They share one execution path, so the catalogue is decided by
        // is_http_chat_agent, never by naming a provider — this pins that.
        for agent in [
            crate::models::AgentType::Ollama,
            crate::models::AgentType::LiteLlm,
            crate::models::AgentType::Nvidia,
        ] {
            assert!(
                crate::agents::runner::is_http_chat_agent(&agent),
                "{agent:?} must take the shared HTTP path, or it silently loses every tool"
            );
            assert!(
                crate::api::disc_prompts::agent_has_native_planning(&agent),
                "{agent:?} must be told it has the plan/task tools, or it will not call them"
            );
        }
    }

    #[test]
    fn an_identical_generation_is_not_bought_twice() {
        // Measured on a real room: the same prompt generated twice, six minutes
        // apart, $0.067 each. The agent DID pass an idempotency key — a
        // different one each attempt, because the schema declared the field
        // with no description at all and it had no way to know that reusing it
        // is the whole point.
        //
        // Describing it is necessary but not sufficient: the safe behaviour has
        // to be what you get by saying nothing, because this one costs money.
        let args = serde_json::json!({
            "connection_id": "conn-1",
            "modality": "image",
            "prompt": "a golden robot bowing on stage",
        });

        let first = derived_media_idempotency_key("disc-1", &args);
        let retry = derived_media_idempotency_key("disc-1", &args);
        assert_eq!(
            first, retry,
            "a retry of the same ask must collapse onto it"
        );

        // Anything the operator would see change in the result is a new job.
        for (field, value) in [
            (
                "prompt",
                serde_json::json!("a silver robot bowing on stage"),
            ),
            ("modality", serde_json::json!("video")),
            ("aspect_ratio", serde_json::json!("16:9")),
        ] {
            let mut different = args.clone();
            different[field] = value;
            assert_ne!(
                first,
                derived_media_idempotency_key("disc-1", &different),
                "changing `{field}` must not reuse the previous asset"
            );
        }

        // The connection is named however the agent likes: its id and its
        // alias are one generation. Two connections stay two jobs, because
        // the server binds the job id to the connection it resolved.
        let mut by_alias = args.clone();
        by_alias["connection_id"] = serde_json::json!("openrouter");
        assert_eq!(first, derived_media_idempotency_key("disc-1", &by_alias));
        assert_ne!(
            crate::api::media::idempotent_job_id("conn-1", &first),
            crate::api::media::idempotent_job_id("conn-2", &first),
        );

        // And two rooms asking for the same picture each get their own.
        assert_ne!(first, derived_media_idempotency_key("disc-2", &args));

        // `null`, `1` and `"1"` must not collide — a collision here is a
        // generation silently skipped, which reads as the feature being broken.
        let mut as_number = args.clone();
        as_number["duration_secs"] = serde_json::json!(1);
        let mut as_string = args.clone();
        as_string["duration_secs"] = serde_json::json!("1");
        assert_ne!(
            derived_media_idempotency_key("disc-1", &as_number),
            derived_media_idempotency_key("disc-1", &as_string),
        );
    }

    #[test]
    fn the_media_tool_explains_what_the_idempotency_key_is_for() {
        let tool = orchestration_tool_catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "media_generate")
            .expect("declared");
        let key = &tool["function"]["parameters"]["properties"]["idempotency_key"];
        let description = key["description"].as_str().unwrap_or_default();
        assert!(
            description.contains("retry") && description.contains("billed"),
            "an agent cannot reuse a key whose purpose it was never told: {description}"
        );
    }

    #[test]
    fn an_agent_can_ask_to_be_called_back_when_the_media_settles() {
        // Without this an agent had to poll `media_job_status` and then
        // schedule its own wake to act on a result — measured on a live room as
        // three agent turns spent waiting for one image.
        let tool = orchestration_tool_catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "media_generate")
            .expect("declared");
        let properties = &tool["function"]["parameters"]["properties"];

        assert_eq!(properties["wake_when_ready"]["type"], "boolean");
        // Opt-in, never required: a generation nobody is waiting on must not
        // hand its room an extra turn.
        let required: Vec<&str> = tool["function"]["parameters"]["required"]
            .as_array()
            .expect("required list")
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        assert!(!required.contains(&"wake_when_ready"));

        let description = properties["wake_when_ready"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(
            description.contains("failure"),
            "an agent told only about successes waits for ever on a refused \
             generation: {description}"
        );
    }

    #[test]
    fn an_agent_can_drive_a_video_from_a_picture_it_generated() {
        // The API has carried reference images since KT-551, and the UI form
        // offers them. The agent-facing tool hardcoded `reference_asset_ids:
        // None`, so an agent could generate an image and generate a video but
        // never join the two — with nothing in the declaration to suggest the
        // capability existed.
        let tool = orchestration_tool_catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "media_generate")
            .expect("media_generate is declared");
        let properties = &tool["function"]["parameters"]["properties"];

        assert_eq!(
            properties["reference_asset_ids"]["type"], "array",
            "an agent must be able to pass the picture the clip starts from"
        );
        assert_eq!(properties["reference_asset_ids"]["items"]["type"], "string");

        // Declared is not enough — it has to be discoverable. A parameter an
        // agent never reads about is a parameter it never uses.
        let description = tool["function"]["description"]
            .as_str()
            .expect("description");
        assert!(
            description.contains("reference_asset_ids"),
            "the description must say a video can start from a picture: {description}"
        );

        // The mode is offered, never invented: a clip that opens on the picture
        // and one that closes on it are different clips, and both are billed.
        let modes = properties["reference_mode"]["enum"]
            .as_array()
            .expect("the reference mode is an enum");
        assert_eq!(modes.len(), 3, "first_frame, last_frame, reference");
    }

    #[test]
    fn every_declared_orchestration_tool_is_actually_routed() {
        // The defect this pins shipped twice over: `media_generate` and
        // `media_job_status` were declared in the catalogue and implemented in
        // the orchestration dispatcher, while the router between them still
        // listed only `agent_list` and `task_exec_*`. An agent was told it had
        // the tool, called it, and was answered "unknown tool" — twice, on a
        // real room, with both models correctly configured.
        //
        // Nothing failed at compile time and nothing failed in any test: the
        // two halves were each correct on their own.
        for tool in orchestration_tool_catalogue() {
            let name = tool["function"]["name"].as_str().expect("declared name");
            assert!(
                is_orchestration_tool(name),
                "`{name}` is declared to agents but the router will not send it \
                 to the orchestration dispatcher",
            );
        }
    }

    #[test]
    fn native_orchestration_catalogue_is_complete_and_identity_free() {
        let catalogue = orchestration_tool_catalogue();
        let names: Vec<&str> = catalogue
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "agent_list",
                "media_generate",
                "media_job_status",
                "task_exec_prepare",
                "task_exec_launch",
                "task_exec_status",
                "task_exec_deliver",
                "task_exec_review",
                "task_exec_cancel",
                "task_exec_reassign",
            ]
        );
        for tool in catalogue {
            let properties = &tool["function"]["parameters"]["properties"];
            assert!(properties.get("source_agent").is_none());
            assert!(properties.get("source_session_id").is_none());
            assert!(properties.get("parent_discussion_id").is_none());
        }
        for tool_name in ["task_exec_prepare", "task_exec_launch"] {
            let tool = orchestration_tool_catalogue()
                .into_iter()
                .find(|tool| tool["function"]["name"] == tool_name)
                .expect("scope-aware principal tool");
            assert_eq!(
                tool["function"]["parameters"]["properties"]["worker_scope_intent"]["enum"],
                json!(["generic", "scoped"])
            );
            assert!(tool["function"]["parameters"]["required"]
                .as_array()
                .expect("required array")
                .iter()
                .any(|field| field == "worker_scope_intent"));
        }
    }

    /// Parity with a CLI agent: an HTTP agent in a plain discussion finds the
    /// media connection in `agent_list` and generates on it by its alias.
    #[tokio::test]
    async fn an_http_agent_in_a_plain_discussion_finds_and_uses_a_media_connection() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('disc-plain', 'Clip', '2026-09-18T00:00:00Z', '2026-09-18T00:00:00Z')",
                [],
            )?;
            crate::db::external_api_connections::insert(
                conn,
                &crate::models::ExternalApiConnection {
                    id: "731fe83a-3082-4b45-8ebc-c16af38406f0".into(),
                    display_name: "OpenRouter".into(),
                    mention_alias: "openrouter".into(),
                    // Closed on purpose: nothing here may reach a provider.
                    endpoint: Some("http://127.0.0.1:1".into()),
                    credential_slug: "openrouter".into(),
                    origin_preset: crate::models::ExternalApiConnectionPreset::OpenRouter,
                    economy_model: Some("qwen/qwen3.8-max".into()),
                    default_model: None,
                    reasoning_model: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    image_model: None,
                    video_model: Some("google/veo-3.1-lite".into()),
                    media_endpoint: None,
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let state = crate::AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let executor = KronnToolExecutor::arc(
            state,
            Some("disc-plain".into()),
            crate::models::AgentType::Ollama,
            None,
            None,
        );

        let listed = executor
            .execute(&ToolCall {
                id: "list".into(),
                name: "agent_list".into(),
                arguments: json!({}),
            })
            .await;
        assert!(listed.ok, "{}", listed.content);
        let listed = listed.content.to_string();
        assert!(
            listed.contains("731fe83a-3082-4b45-8ebc-c16af38406f0"),
            "{listed}"
        );
        assert!(listed.contains("google/veo-3.1-lite"), "{listed}");

        let generated = executor
            .execute(&ToolCall {
                id: "clip".into(),
                name: "media_generate".into(),
                arguments: json!({
                    "connection_id": "openrouter",
                    "modality": "video",
                    "prompt": "a man is handed a pair of earrings in the street",
                }),
            })
            .await;
        assert!(generated.ok, "{}", generated.content);
        assert!(generated.content.to_string().contains("job_id"));
    }

    #[tokio::test]
    async fn fresh_native_executor_refuses_missing_scope_intent_before_provisioning() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let state = crate::AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let executor = KronnToolExecutor::arc(
            state,
            Some("disc-parent".into()),
            crate::models::AgentType::Codex,
            None,
            None,
        );
        let outcome = executor
            .execute(&ToolCall {
                id: "stale-host-schema".into(),
                name: "task_exec_launch".into(),
                arguments: json!({
                    "task_reference": "KT-466",
                    "worker": {
                        "kind": "discussion_agent",
                        "agent_type": "Ollama"
                    },
                    "worker_scope": {
                        "mode": "prelocalized_insert_after",
                        "path": "docs/operations/ollama-local-models.md",
                        "anchor_line": 168
                    }
                }),
            })
            .await;
        assert!(!outcome.ok);
        assert!(outcome.content.to_string().contains("worker_scope_intent"));
        assert!(outcome.content.to_string().contains("reconnect"));

        let counts = db
            .with_conn(|conn| {
                Ok((
                    conn.query_row("SELECT COUNT(*) FROM orchestration_runs", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                    conn.query_row("SELECT COUNT(*) FROM task_executions", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                    conn.query_row("SELECT COUNT(*) FROM discussion_workspaces", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                    conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0));
    }

    #[test]
    fn worker_delivery_schema_projects_mechanics_but_principal_contract_stays_full() {
        let principal = orchestration_tool_catalogue();
        let principal_delivery = principal
            .iter()
            .find(|tool| tool["function"]["name"] == "task_exec_deliver")
            .expect("principal delivery schema");
        assert!(principal_delivery["function"]["parameters"]["properties"]
            .get("task_execution_id")
            .is_some());
        assert_eq!(
            principal_delivery["function"]["parameters"]["required"],
            json!(["task_execution_id", "manifest"])
        );
        let principal_manifest_schema =
            principal_delivery["function"]["parameters"]["properties"]["manifest"].clone();
        assert_eq!(
            principal_manifest_schema["required"],
            json!([
                "version",
                "task_ref",
                "head_sha",
                "files_touched",
                "tests",
                "dod_status",
                "docs",
                "migrations",
                "risks",
                "limitations",
                "summary"
            ])
        );

        let worker = worker_room_catalogue(principal);
        let worker_delivery = worker
            .iter()
            .find(|tool| tool["function"]["name"] == "task_exec_deliver")
            .expect("worker delivery schema");
        assert!(worker_delivery["function"]["parameters"]["properties"]
            .get("task_execution_id")
            .is_none());
        assert_eq!(
            worker_delivery["function"]["parameters"]["required"],
            json!(["manifest"])
        );
        let projected = &worker_delivery["function"]["parameters"]["properties"]["manifest"];
        assert_eq!(
            projected["required"],
            json!([
                "tests",
                "dod_status",
                "docs",
                "migrations",
                "risks",
                "limitations",
                "summary"
            ])
        );
        for mechanical in ["version", "task_ref", "head_sha", "files_touched"] {
            assert!(projected["properties"].get(mechanical).is_none());
            assert!(principal_manifest_schema["properties"]
                .get(mechanical)
                .is_some());
        }
        assert!(projected["properties"]["dod_status"]["items"]["properties"]
            .get("dod_id")
            .is_none());
        assert_eq!(
            projected["properties"]["dod_status"]["items"]["required"],
            json!(["met", "evidence"])
        );
        assert_eq!(
            projected["properties"]["tests"]["items"]["required"],
            json!(["name", "status", "evidence"])
        );
        assert_eq!(projected["additionalProperties"], json!(false));
        assert_eq!(
            projected["properties"]["dod_status"]["items"]["additionalProperties"],
            json!(false)
        );
        assert!(worker.iter().any(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_some_and(|name| name == "task_exec_status")
        }));
    }

    #[test]
    fn a_project_scoped_workflow_gets_the_file_tools_but_not_cross_room_reads() {
        // Romu's question: is everything available in workflows too? It was not —
        // a workflow step got only the bounded five, because it has no discussion.
        // But it does have a project, and a project is a directory, so an Agent step
        // asked to review a repository can now read it. Cross-room reads stay out:
        // they are scoped to the run's own discussion, which a workflow lacks.
        let names: Vec<String> = workspace_tool_catalogue()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        for expected in [
            "web_fetch",
            "read_file",
            "write_file",
            "edit_file",
            "edit_lines",
            "list_files",
            "find_files",
            "search_text",
            "git_status",
            "git_diff",
            "git_log",
            "git_commit",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} missing from the workspace catalogue: {names:?}"
            );
        }
        // And the bounded workflow list itself never carried them, which is what
        // made the gap invisible.
        let bounded: Vec<String> = workflow_tool_catalogue(true)
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        assert!(
            !bounded.iter().any(|n| n == "read_file"),
            "the bounded list is the pre-existing one; the file tools are added on top"
        );
        let workflow_workspace: Vec<String> = workflow_workspace_tool_catalogue()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        assert!(workflow_workspace.iter().any(|name| name == "git_status"));
        assert!(
            !workflow_workspace.iter().any(|name| name == "git_commit"),
            "a workflow has no worker discussion/dispatch identity and must never receive git_commit"
        );
    }

    /// Not a test: dumps the real principal catalogue for the local tool-cliff
    /// measurement. Ignored so it never runs in CI.
    #[test]
    #[ignore]
    fn dump_principal_catalogue() {
        let mut catalogue = tool_catalogue();
        catalogue.extend(workspace_tool_catalogue());
        catalogue.extend(orchestration_tool_catalogue());
        catalogue.extend(agent_resume_tool_catalogue());
        let out = std::env::var("KRONN_CATALOGUE_DUMP").expect("KRONN_CATALOGUE_DUMP");
        std::fs::write(&out, serde_json::to_string_pretty(&catalogue).unwrap()).unwrap();
        eprintln!("wrote {} tools to {out}", catalogue.len());
    }

    #[test]
    fn no_declaration_uses_a_keyword_the_openai_wire_rejects() {
        // Measured against gpt-5.1 through LiteLLM on 2026-09-17: a top-level
        // `allOf` on ONE declaration answered HTTP 400 to the WHOLE request,
        // so the agent could not make a single call — not a degraded tool, a
        // dead surface. `enum` is fine on a property and refused at the root.
        //
        // Every HTTP provider Kronn ships reaches its models this way, so this
        // is the one shape rule the native catalogue cannot drift on.
        const REFUSED_AT_ROOT: [&str; 6] = ["oneOf", "anyOf", "allOf", "enum", "const", "not"];
        let surfaces = [
            ("discussion", tool_catalogue()),
            ("orchestration", orchestration_tool_catalogue()),
            ("resume", agent_resume_tool_catalogue()),
            ("workspace", workspace_tool_catalogue()),
        ];
        for (surface, catalogue) in surfaces {
            for tool in catalogue {
                let name = tool["function"]["name"].as_str().unwrap_or("?");
                let parameters = &tool["function"]["parameters"];
                assert_eq!(
                    parameters["type"], "object",
                    "{surface}/{name}: the wire requires a top-level object schema"
                );
                for keyword in REFUSED_AT_ROOT {
                    assert!(
                        parameters.get(keyword).is_none(),
                        "{surface}/{name}: `{keyword}` at the top level is refused by the \
                         OpenAI wire, and it refuses the whole request — say the rule in the \
                         field descriptions instead"
                    );
                }
            }
        }
    }

    #[test]
    fn catalogue_shape_is_what_both_providers_expect() {
        // Ollama and OpenAI both read `type: function` + `function.parameters`
        // as a JSON Schema object; a malformed entry is silently ignored by the
        // model, which surfaces as "the tool never worked".
        let expected = [
            "mcp_list",
            "api_endpoints",
            "qa_list",
            "qe_list",
            "qa_run",
            "qe_run",
            "qa_create_draft",
            "qa_update",
            "qp_list",
            "qp_run",
            "qp_create_draft",
            "qp_update",
            "qe_create_draft",
            "qe_update",
            "tool_manual",
            "api_call",
            "plan_get",
            "task_list",
            "task_get",
            "task_create",
            "task_update",
            "task_update_dod",
            "task_link_discussion",
            "task_add_blocker",
            "task_remove_blocker",
            "workflow_list",
            "workflow_get",
            "workflow_step_schema",
            "workflow_create_draft",
            "workflow_update",
        ];
        let items = tool_catalogue();
        assert_eq!(items.len(), expected.len());
        for (item, name) in items.iter().zip(expected) {
            assert_eq!(item["type"], "function");
            assert_eq!(item["function"]["name"], name);
            assert_eq!(item["function"]["parameters"]["type"], "object");
            assert!(
                item["function"]["description"]
                    .as_str()
                    .is_some_and(|d| d.len() > 20),
                "{name} needs a description the model can act on"
            );
        }
    }

    #[test]
    fn workflow_catalogue_is_bounded_and_contains_no_planning_mutation() {
        let catalogue = workflow_tool_catalogue(true);
        let names: Vec<&str> = catalogue
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "mcp_list",
                "api_endpoints",
                "qa_list",
                "qa_run",
                "api_call",
                "task_list",
                "task_get"
            ]
        );
        assert!(!names.iter().any(|name| {
            name.contains("create")
                || name.contains("update")
                || name.contains("remove")
                || name.contains("link")
        }));
        let projectless_names: Vec<String> = workflow_tool_catalogue(false)
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        assert!(!projectless_names
            .iter()
            .any(|name| name.starts_with("task_")));
    }

    #[test]
    fn plugin_list_drops_the_api_specs_that_would_swallow_the_context() {
        // Measured on the real instance: the raw overview is ~52 KB (~13k
        // tokens), almost all of it `api_spec`. A local model's window cannot
        // absorb that on the first tool call, so the list carries only what
        // is needed to CHOOSE, and specs are fetched one plugin at a time.
        let overview = json!({
            "servers": [
                { "id": "mcp-resend", "name": "Resend", "description": "Send email",
                  "api_spec": { "endpoints": [{ "method": "POST", "path": "/emails" }] } },
                { "id": "plain-mcp", "name": "No API", "description": "stdio only" },
                { "id": "orphan", "name": "Unconfigured", "description": "no credentials",
                  "api_spec": { "endpoints": [] } },
            ],
            "configs": [{ "id": "cfg-1", "server_id": "mcp-resend", "label": "Resend", "include_general": true, "is_global": false, "project_ids": [] }],
        });
        let out = compact_plugin_list(&overview, None);
        let plugins = out["plugins"].as_array().unwrap();
        assert_eq!(
            plugins.len(),
            1,
            "needs an API spec AND a config to be callable"
        );
        assert_eq!(plugins[0]["slug"], "mcp-resend");
        assert_eq!(
            plugins[0]["api_config_id"], "cfg-1",
            "without this api_call fails: the broker resolves credentials per config"
        );
        let rendered = out.to_string();
        assert!(!rendered.contains("api_spec"), "specs must not ride along");
        assert!(
            !rendered.contains("/emails"),
            "endpoints belong to api_endpoints"
        );
    }

    #[test]
    fn native_api_and_quick_api_discovery_never_cross_project_scope() {
        let overview = json!({
            "servers": [{
                "id": "api-adobe", "name": "Adobe", "description": "Analytics",
                "api_spec": { "endpoints": [] }
            }],
            "configs": [
                { "id": "cfg-a", "server_id": "api-adobe", "is_global": false,
                  "include_general": false, "project_ids": ["project-a"] },
                { "id": "cfg-b", "server_id": "api-adobe", "is_global": false,
                  "include_general": true, "project_ids": ["project-b"] }
            ]
        });
        let project = compact_plugin_list(&overview, Some("project-a"));
        assert_eq!(project["plugins"][0]["api_config_id"], "cfg-a");
        let general = compact_plugin_list(&overview, None);
        assert_eq!(general["plugins"][0]["api_config_id"], "cfg-b");

        let quick_apis = json!([
            { "id": "global", "name": "Global", "description": "", "project_id": null, "variables": [] },
            { "id": "a", "name": "A", "description": "", "project_id": "project-a", "variables": [] },
            { "id": "b", "name": "B", "description": "", "project_id": "project-b", "variables": [] }
        ]);
        let scoped = compact_quick_apis(&quick_apis, Some("project-a"));
        let ids: Vec<&str> = scoped["quick_apis"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|qa| qa["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["global", "a"]);
        let general = compact_quick_apis(&quick_apis, None);
        let general_ids: Vec<&str> = general["quick_apis"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|qa| qa["id"].as_str())
            .collect();
        assert_eq!(general_ids, vec!["global"]);
    }

    fn saved_exec(id: &str, project_id: Option<&str>) -> crate::models::QuickExec {
        crate::models::QuickExec {
            id: id.to_string(),
            name: id.to_uppercase(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: project_id.map(str::to_string),
            command: "aws".into(),
            args: vec!["logs".into()],
            timeout_secs: 60,
            output_format: crate::models::CollectQuickExecOutputFormat::Text,
            variables: Vec::new(),
            pinned: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn a_listed_quick_exec_is_one_the_room_could_actually_start() {
        // An HTTP agent could start a Quick Exec and had no tool to discover
        // one: `agent_job_start` asked for an id nothing on its surface could
        // produce. Listing is the fix, and it must filter on exactly the rule
        // `start_background_job` enforces — an id shown here and refused there
        // would be worse than no listing at all.
        let saved = [
            saved_exec("global", None),
            saved_exec("a", Some("project-a")),
            saved_exec("b", Some("project-b")),
        ];
        let scoped = compact_quick_execs(&saved, Some("project-a"));
        let ids: Vec<&str> = scoped["quick_execs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|quick| quick["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["global", "a"]);

        let general = compact_quick_execs(&saved, None);
        let general_ids: Vec<&str> = general["quick_execs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|quick| quick["id"].as_str())
            .collect();
        assert_eq!(general_ids, vec!["global"]);
    }

    #[test]
    fn a_worker_is_offered_neither_the_job_nor_its_catalogue() {
        let worker = worker_room_catalogue(tool_catalogue());
        let names: Vec<&str> = worker
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert!(
            !names.contains(&"qe_list"),
            "listing what only agent_job_start consumes, without it, is the wrong turn"
        );
    }

    #[test]
    fn authoring_an_automation_is_a_principal_capability() {
        // A worker has one task and is already briefed. Leaving a new Quick API
        // behind changes the instance, which its delivery cannot be reviewed
        // against — and picking among a visited project's saved commands is not
        // part of its contract either.
        let worker: Vec<String> = worker_room_catalogue(tool_catalogue())
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        let has = |name: &str| worker.iter().any(|tool| tool == name);
        for withheld in [
            "qp_list",
            "qp_run",
            "qp_create_draft",
            "qp_update",
            "qe_list",
            "qe_run",
            "qa_create_draft",
            "qa_update",
            "qe_create_draft",
            "qe_update",
        ] {
            assert!(!has(withheld), "{withheld} reached a worker");
        }
        // Consuming one still works: a worker may read and run a saved API call.
        assert!(has("qa_list") && has("qa_run"));
    }

    #[test]
    fn an_update_names_only_what_changes() {
        // Found by running the real loop: `qa_update` demanded the plugin slug,
        // config id and endpoint path, and `qa_list` — compact by design, since
        // it is paid on every turn — returns none of them. The model could not
        // satisfy it and fell back to hand-built calls. Declared and
        // unsatisfiable is the same defect as an id no tool can produce.
        let stored = crate::models::QuickExec {
            id: "qe-1".into(),
            name: "AWS logs".into(),
            icon: "⌘".into(),
            description: "tail a log group".into(),
            project_id: Some("project-a".into()),
            command: "aws".into(),
            args: vec!["logs".into(), "tail".into()],
            timeout_secs: 30,
            output_format: crate::models::CollectQuickExecOutputFormat::Text,
            variables: Vec::new(),
            pinned: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let merged: crate::models::CreateQuickExecRequest = merged_definition(
            &stored,
            &json!({ "quick_exec_id": "qe-1", "timeout_secs": 60 }),
        )
        .expect("a lone changed field is enough");
        assert_eq!(merged.timeout_secs, Some(60), "what was named changes");
        assert_eq!(merged.command, "aws", "what was not named is kept");
        assert_eq!(merged.args, vec!["logs".to_string(), "tail".to_string()]);
        assert_eq!(merged.project_id.as_deref(), Some("project-a"));

        // A named field replaces outright — it never appends.
        let merged: crate::models::CreateQuickExecRequest =
            merged_definition(&stored, &json!({ "args": ["logs", "get"] })).expect("merge");
        assert_eq!(merged.args, vec!["logs".to_string(), "get".to_string()]);

        // The id addresses the record and is not part of its definition:
        // letting one through would rename what the update points at.
        let merged: crate::models::CreateQuickExecRequest = merged_definition(
            &stored,
            &json!({ "id": "somebody-elses", "name": "renamed" }),
        )
        .expect("merge");
        assert_eq!(merged.name, "renamed");
    }

    #[test]
    fn the_manual_answers_only_for_tools_that_point_at_it() {
        // The catalogue is re-sent every turn, so the contract stays in the
        // declaration and the detail moves here — paid only by an agent that
        // is about to author something.
        let listed = tool_manual(None);
        let available = listed["available"].as_array().expect("available");
        assert!(!available.is_empty());

        let page = tool_manual(Some("qe_create_draft"));
        let text = page["manual"].as_str().expect("manual text");
        assert!(
            text.contains("NO shell"),
            "the argv rule is the whole point"
        );
        assert!(
            text.contains("aws"),
            "and it shows the shape, not just the rule"
        );

        assert!(
            tool_manual(Some("task_list"))["error"].is_string(),
            "a tool whose description is its whole contract has no page"
        );
        assert_eq!(
            tool_manual(Some("  "))["available"],
            listed["available"],
            "a blank name lists rather than erroring"
        );
    }

    #[test]
    fn every_manual_entry_belongs_to_a_declared_tool_or_the_signal_registry() {
        // A page for a tool nobody can call is documentation of a capability
        // that does not exist — the exact shape that taught models to
        // hallucinate calls (tools.rs).
        let declared: Vec<String> = tool_catalogue()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        for page in tool_manual(None)["available"].as_array().unwrap() {
            let name = page.as_str().unwrap();
            if name == "signals" {
                // This entry documents response fences, not a callable tool.
                assert!(!declared.iter().any(|tool| tool == name));
                assert!(tool_manual(Some(name))["catalogue"]["signals"].is_array());
                continue;
            }
            assert!(
                declared.iter().any(|tool| tool == name),
                "manual page `{name}` has no declared tool"
            );
        }
    }

    #[test]
    fn qa_run_declares_the_variables_its_handler_reads() {
        // The handler read `variables` all along; the declaration hid them, so
        // any Quick API with a required variable was unusable by an HTTP agent.
        let qa_run = tool_catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "qa_run")
            .expect("qa_run");
        assert_eq!(
            qa_run["function"]["parameters"]["properties"]["variables"]["type"],
            "object"
        );
    }

    #[test]
    fn endpoints_are_scoped_to_one_plugin_and_reject_unknown_slugs() {
        let overview = json!({
            "servers": [{
                "id": "mcp-resend", "name": "Resend", "description": "Send email",
                "api_spec": { "endpoints": [
                    { "method": "POST", "path": "/emails", "description": "Send one email" },
                ] },
            }]
        });
        let out = compact_endpoints(&overview, "mcp-resend").expect("known slug");
        assert_eq!(out["endpoints"][0]["path"], "/emails");
        assert_eq!(out["endpoints"][0]["method"], "POST");
        // An unknown slug must fail loudly, not return an empty list the model
        // would read as "this plugin has no endpoints".
        assert!(compact_endpoints(&overview, "nope").is_none());
    }

    #[test]
    fn quick_apis_keep_the_decision_fields_and_drop_the_machinery() {
        let items = json!([{
            "id": "qa-1", "name": "Daily report",
            "description": "Fetch yesterday's numbers",
            "api_extract": { "heavy": "spec" },
            "created_at": "2026-01-01", "updated_at": "2026-01-02",
            "variables": [
                { "name": "site", "required": true },
                { "name": "format", "required": false },
            ],
        }]);
        let out = compact_quick_apis(&items, None);
        let qa = &out["quick_apis"][0];
        assert_eq!(qa["id"], "qa-1");
        assert_eq!(
            qa["required_variables"],
            json!(["site"]),
            "optional ones are noise"
        );
        let rendered = out.to_string();
        assert!(!rendered.contains("api_extract"));
        assert!(!rendered.contains("created_at"));
    }

    #[test]
    fn brief_truncates_on_a_char_boundary() {
        // Descriptions are user data and routinely contain accents; slicing by
        // byte would panic mid-character.
        assert_eq!(brief(&json!("éééééé"), 3), "ééé…");
        assert_eq!(brief(&json!("short"), 50), "short");
        assert_eq!(brief(&Value::Null, 10), "");
    }

    #[test]
    fn required_fields_are_declared_so_the_model_sends_them() {
        let items = tool_catalogue();
        let by_name = |n: &str| {
            items
                .iter()
                .find(|i| i["function"]["name"] == n)
                .unwrap()
                .clone()
        };
        assert_eq!(
            by_name("qa_run")["function"]["parameters"]["required"],
            serde_json::json!(["quick_api_id"])
        );
        assert_eq!(
            by_name("api_call")["function"]["parameters"]["required"],
            serde_json::json!(["api_plugin_slug", "endpoint_path"]),
            "the config id is resolved server-side — asking the model to carry a UUID proved unreliable"
        );
        // Argument-less tools must still declare an empty object schema.
        assert_eq!(
            by_name("mcp_list")["function"]["parameters"]["required"],
            serde_json::json!([])
        );
        assert_eq!(
            by_name("plan_get")["function"]["parameters"]["required"],
            serde_json::json!([])
        );
        assert_eq!(
            by_name("task_create")["function"]["parameters"]["required"],
            serde_json::json!(["title"])
        );
        assert_eq!(
            by_name("task_update_dod")["function"]["parameters"]["required"],
            serde_json::json!(["task_id", "dod_id", "completed"])
        );
    }

    #[test]
    fn idempotency_digest_is_fixed_width_lowercase_hex() {
        let digest = Sha256::digest(b"disc\0explicit\0one-logical-task");
        let encoded = hex_digest(&digest);
        assert_eq!(encoded.len(), 64);
        assert!(encoded.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(encoded, encoded.to_ascii_lowercase());
    }

    #[test]
    fn orchestration_catalogue_exposes_local_delegation_policy() {
        let catalogue = orchestration_tool_catalogue();
        for tool_name in ["task_exec_prepare", "task_exec_launch"] {
            let tool = catalogue
                .iter()
                .find(|tool| tool["function"]["name"] == tool_name)
                .unwrap_or_else(|| panic!("{tool_name} must exist in catalogue"));
            let description = tool["function"]["description"]
                .as_str()
                .expect("orchestration tool description must be text");
            for invariant in [
                "Ollama only for one atomic unit",
                "explicit scope",
                "principal-owned mechanical validations",
                "trust or protocol boundaries",
                "concurrency",
                "migrations",
                "architecture",
                "cross-cutting parity",
                "principal reviews the delivered SHA",
                "runs its validations",
                "at most one targeted local rework",
                "reassign to a stronger worker",
            ] {
                assert!(
                    description.contains(invariant),
                    "{tool_name} description must contain {invariant:?}"
                );
            }
        }
    }

    // ── the catalogue in tiers ──

    fn names(catalogue: &[Value]) -> std::collections::HashSet<String> {
        catalogue
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .map(str::to_string)
            .collect()
    }

    /// Omitting the discussion id must read the executor room, and the schema must say so.
    #[test]
    fn an_agent_can_ask_for_its_own_room_without_knowing_its_id() {
        let disc_read = full_discussion_catalogue()
            .into_iter()
            .find(|tool| tool["function"]["name"] == "disc_read")
            .expect("disc_read is declared");
        let function = &disc_read["function"];
        assert_eq!(
            function["parameters"]["required"],
            serde_json::json!([]),
            "requiring an id it cannot know makes the tool unusable on its own room"
        );
        let description = function["description"].as_str().unwrap_or_default();
        assert!(
            description.contains("Omit `discussion_id` to re-read THIS one"),
            "say that omitting it reads this room: {description}"
        );
        assert!(
            description.contains("shortened"),
            "name what it is for, or it reads as a way to snoop on other rooms: {description}"
        );
    }

    #[test]
    fn every_tool_is_either_in_the_core_or_in_exactly_one_family() {
        let full = names(&full_discussion_catalogue());
        let core = names(&tiered(full_discussion_catalogue()));
        let mut in_families: Vec<&str> = Vec::new();
        for (_, _, tools) in TOOL_FAMILIES {
            in_families.extend(*tools);
        }
        for (index, tool) in in_families.iter().enumerate() {
            assert!(
                full.contains(*tool),
                "family names `{tool}`, which the catalogue does not declare"
            );
            assert!(
                !in_families[index + 1..].contains(tool),
                "`{tool}` is in two families"
            );
        }
        // Nothing may fall between the two: a capability declared nowhere is
        // a capability an agent can never reach.
        for tool in &full {
            assert!(
                core.contains(tool) || in_families.contains(&tool.as_str()),
                "`{tool}` is neither in the core nor in a family"
            );
        }
        assert_eq!(
            core.len(),
            full.len() - in_families.len() + 1,
            "plus tools_load"
        );
        assert!(core.contains("tools_load"));
    }

    #[test]
    fn discovery_tools_stay_in_the_core() {
        let core = names(&tiered(full_discussion_catalogue()));
        // Provider discovery must not require a preliminary family load.
        for tool in [
            "agent_list",
            "plan_get",
            "task_list",
            "api_call",
            "mcp_list",
            "read_file",
            "search_text",
            "web_fetch",
            "tool_manual",
            "tools_load",
        ] {
            assert!(
                core.contains(tool),
                "`{tool}` must be declared from the start"
            );
        }
    }

    #[test]
    fn the_index_names_every_family_and_the_tools_it_brings() {
        let declaration = tools_load_declaration();
        let description = declaration["function"]["description"].as_str().unwrap();
        let options = declaration["function"]["parameters"]["properties"]["family"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        for (family, what, tools) in TOOL_FAMILIES {
            assert!(options.contains(family), "`{family}` is not selectable");
            assert!(description.contains(family), "the index omits `{family}`");
            // A family that only says what it is for sent a 12B model looking
            // elsewhere; naming its tools is what stopped that.
            assert!(
                tools.iter().any(|tool| what.contains(tool)),
                "`{family}` names none of its tools: {what}"
            );
        }
    }

    #[test]
    fn a_family_answers_with_its_declarations_and_what_each_one_does() {
        let declarations = declarations_for_family("media");
        assert_eq!(
            names(&declarations),
            ["media_generate", "media_job_status"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert!(declarations_for_family("nope").is_empty());
        let summary = first_sentence(
            "Generate an image or a video. The rest of the description explains the envelope.",
        );
        assert_eq!(summary, "Generate an image or a video.");
    }
}
