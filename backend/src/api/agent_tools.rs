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
    AddPlanningBlockerRequest, CreatePlanningTaskRequest, LinkPlanningDiscussionRequest,
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
    /// The User message that caused this run, when known.
    source_message_id: Option<String>,
}

impl KronnToolExecutor {
    pub fn new(state: AppState, disc_id: Option<String>) -> Self {
        Self {
            state,
            disc_id,
            project_id: None,
            workflow_run_id: None,
            actor_id: "Kronn HTTP agent".into(),
            source_message_id: None,
        }
    }

    /// Wrap into the `Arc` the runner takes, or `None` when tools are off.
    pub fn arc(
        state: AppState,
        disc_id: Option<String>,
        actor_id: String,
        source_message_id: Option<String>,
    ) -> std::sync::Arc<dyn ToolExecutor> {
        std::sync::Arc::new(Self {
            state,
            disc_id,
            project_id: None,
            workflow_run_id: None,
            actor_id,
            source_message_id,
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
            workflow_run_id: Some(workflow_run_id),
            source_message_id: None,
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

#[async_trait::async_trait]
impl ToolExecutor for KronnToolExecutor {
    fn catalogue(&self) -> Vec<Value> {
        let catalogue = tool_catalogue();
        if self.workflow_run_id.is_none() {
            return catalogue;
        }
        workflow_tool_catalogue(self.project_id.is_some())
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        if self.workflow_run_id.is_some()
            && !workflow_tool_catalogue(self.project_id.is_some())
                .iter()
                .any(|tool| {
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
                let Some(completed) = call.arguments["completed"].as_bool() else {
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
            source_message_id: self.source_message_id.clone(),
        }
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

fn required_string(call: &ToolCall, field: &str) -> Option<String> {
    call.arguments[field]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
}
