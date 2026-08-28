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
pub fn tool_catalogue() -> Vec<Value> {
    vec![
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
                "name": "qa_run",
                "description": "Execute a saved Quick API by id and return its result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "quick_api_id": { "type": "string", "description": "id from qa_list" },
                    },
                    "required": ["quick_api_id"],
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
    ]
}

/// Discussion-bound task execution lifecycle for native HTTP agents. These are
/// intentionally separate from the general catalogue: workflow Agent steps have no
/// principal/worker room identity, and every schema omits caller ids because Kronn
/// derives them from the trusted executor.
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
            "List the worker identities this principal room can pass verbatim to task_exec_prepare. Separates configured, reachable and available with stable secret-free reason codes; availability proves transport readiness only, never task or model success.",
            json!({}),
            json!([]),
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
            "Reassign a blocked/interrupted execution as its parent-room principal while preserving durable child/worktree/checkpoints.",
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
                "quick_exec_id": {"type": "string", "description": "A user-saved shell-free Quick Exec id."},
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
            "description": "Read the recent messages of another discussion of THIS project. \
                            Returns the tail (most recent first requested `limit`, default 40, \
                            max 200) with `truncated` set when the room is longer. A room from \
                            another project is refused. Read-only.",
            "parameters": {
                "type": "object",
                "properties": {
                    "discussion_id": { "type": "string", "description": "UUID of the discussion, from disc_list." },
                    "limit": { "type": "integer", "description": "How many recent messages (1-200)." },
                },
                "required": ["discussion_id"],
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
        "agent_schedule_wake",
        "agent_resume_status",
        "agent_resume_cancel",
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
            return catalogue;
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
        // KT-340 — cross-room reads, scoped to this discussion's project.
        if call.name == "disc_read" {
            return self.read_other_discussion(call).await;
        }
        if call.name == "disc_list" {
            return self.list_project_discussions(call).await;
        }
        // KT-338 — web and workspace tools. Handled before the Kronn-internal
        // catalogue because they need the discussion's workspace root, not the
        // API handlers.
        if crate::api::agent_workspace_tools::TOOL_NAMES.contains(&call.name.as_str()) {
            return self.execute_workspace_tool(call).await;
        }
        if call.name == "agent_list" || call.name.starts_with("task_exec_") {
            return self.execute_orchestration_tool(call).await;
        }
        if matches!(
            call.name.as_str(),
            "agent_job_start"
                | "agent_schedule_wake"
                | "agent_resume_status"
                | "agent_resume_cancel"
        ) {
            return self.execute_agent_resume_tool(call).await;
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
                match crate::api::orchestration::task_worker_catalogue_for_discussion(
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
                        if let Some(bad) = parsed.iter().find(|spec| {
                            spec.command.trim().is_empty() || spec.timeout_secs == Some(0)
                        }) {
                            return fail(
                                call,
                                format!(
                                    "invalid validations: command must be non-empty and timeout_secs must be at least 1 (got {})",
                                    serde_json::to_string(bad).unwrap_or_default()
                                ),
                            );
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
        let Some(target) = call.arguments["discussion_id"].as_str().map(str::to_string) else {
            return fail(call, "missing required field `discussion_id`");
        };
        let Some(disc_id) = self.disc_id.clone() else {
            return fail(
                call,
                "this run has no discussion, so it has no project to read within",
            );
        };
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
                if !same_project {
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
fn quick_api_is_in_scope(quick_api: &crate::models::QuickApi, project_id: Option<&str>) -> bool {
    match project_id {
        Some(project_id) => quick_api
            .project_id
            .as_deref()
            .is_none_or(|id| id == project_id),
        None => quick_api.project_id.is_none(),
    }
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

    #[test]
    fn catalogue_shape_is_what_both_providers_expect() {
        // Ollama and OpenAI both read `type: function` + `function.parameters`
        // as a JSON Schema object; a malformed entry is silently ignored by the
        // model, which surfaces as "the tool never worked".
        let expected = [
            "mcp_list",
            "api_endpoints",
            "qa_list",
            "qa_run",
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
}
