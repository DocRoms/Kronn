//! KT-1346 — an HTTP agent's tools cross the same gate as a CLI agent's
//! bridge calls: each tool that reads or writes a Kronn object is decided as
//! the HTTP route it mirrors, with a bridge grant minted for the run, and its
//! result is scoped the same way.
use std::sync::Arc;

use serde_json::{json, Value};

use super::KronnToolExecutor;
use crate::agents::tools::{ToolCall, ToolOutcome};
use crate::core::bridge_admit::{self, Admitted};
use crate::core::bridge_token::{self, BridgeGrant, BridgeScope};

/// The run id of the workflow editor's "Test step": no run row exists behind it.
pub(crate) const TEST_STEP_RUN_ID: &str = "test-step";

/// One tool call, as the route the gate decides it on.
struct GateRequest {
    method: &'static str,
    pattern: &'static str,
    params: Vec<(String, String)>,
    query: Option<String>,
    body: Option<Value>,
}

/// A call the gate let through: the arguments the tool must run with (the
/// project forced or kept by the gate) and what scoping its result needs.
pub(super) struct Gated {
    pub call: Option<ToolCall>,
    pub admitted: Option<Admitted>,
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn query_of(pairs: &[(&str, Option<String>)]) -> Option<String> {
    let query = pairs
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_ref()
                .map(|value| format!("{key}={}", encode(value)))
        })
        .collect::<Vec<_>>()
        .join("&");
    (!query.is_empty()).then_some(query)
}

fn text(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The arguments minus the fields the route carries in its path.
fn body_without(arguments: &Value, path_keys: &[&str]) -> Value {
    let mut body = arguments.clone();
    if let Some(fields) = body.as_object_mut() {
        for key in path_keys {
            fields.remove(*key);
        }
    }
    body
}

/// The Quick Prompt fields an agent may write; `project_id` last.
const QUICK_PROMPT_FIELDS: &[&str] = &[
    "name",
    "description",
    "icon",
    "prompt_template",
    "variables",
    "project_id",
];

fn only(arguments: &Value, keys: &[&str]) -> Value {
    Value::Object(
        arguments
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| keys.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// Tools that touch no Kronn object, or keep a rule of their own at least as
/// strict as the gate's; every other tool is decided by the gate.
pub(super) const UNGATED_TOOLS: &[(&str, &str)] = &[
    (
        "read_file",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "write_file",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "edit_file",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "edit_lines",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "insert_after_line",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "list_files",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "find_files",
        "workspace files, bounded to the run's workspace",
    ),
    (
        "search_text",
        "workspace files, bounded to the run's workspace",
    ),
    ("git_status", "the workspace's git, read-only"),
    ("git_diff", "the workspace's git, read-only"),
    ("git_log", "the workspace's git, read-only"),
    ("git_commit", "the workspace's git, discussion runs only"),
    ("web_fetch", "the public web, behind the SSRF check"),
    ("tool_manual", "static manuals"),
    ("workflow_step_schema", "static schema"),
    ("tools_load", "tool declarations"),
    (
        "disc_read",
        "rooms of the same project only, stricter than the gate",
    ),
    (
        "disc_list",
        "rooms of the same project only, stricter than the gate",
    ),
    (
        "page_list",
        "filtered to the run's project; the bridge has no list route",
    ),
    ("agent_list", "the room's own agents and connections"),
    (
        "task_exec_status",
        "the execution's principal or worker only",
    ),
    (
        "task_exec_cancel",
        "the execution's principal or worker only",
    ),
    (
        "task_exec_reassign",
        "the execution's principal or worker only",
    ),
    ("task_exec_deliver", "the worker's own dispatch only"),
    ("task_exec_review", "the execution's principal only"),
    ("agent_job_start", "the room's own job, in its workspace"),
    ("agent_schedule_wake", "the room's own wake-up"),
    ("agent_resume_status", "the room's own jobs only"),
    ("agent_resume_cancel", "the room's own jobs only"),
];

/// The fields the gate may force or keep in a body, written back to the call.
const GATE_OWNED_FIELDS: &[&str] = &["project_id", "project_ids"];

impl KronnToolExecutor {
    /// The scope a CLI launch in the same place would get (see
    /// `mint_launch_bridge_token`): a step or a worker posts in a room without
    /// being its agent, so the room's grants never reach it.
    fn bridge_scope(&self) -> BridgeScope {
        let disc = self
            .disc_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let run = self
            .workflow_run_id
            .clone()
            .filter(|run| run != TEST_STEP_RUN_ID);
        let is_room_agent = !self.worker_room && self.workflow_run_id.is_none();
        BridgeScope {
            discussion_ids: disc.clone().into_iter().collect(),
            room_grant_discussions: disc.filter(|_| is_room_agent).into_iter().collect(),
            task_execution_id: None,
            workflow_run_id: run,
            project_id: self.project_id.clone(),
            agent: self
                .launch_identity
                .lock()
                .ok()
                .and_then(|identity| identity.clone()),
        }
    }

    /// This run's grant, minted on first use and held as long as the executor:
    /// a token that cannot be minted refuses, never runs ungated.
    fn bridge_grant(&self) -> Result<Arc<BridgeGrant>, String> {
        let mut guard = self
            .bridge_token
            .lock()
            .map_err(|_| "bridge token unavailable".to_string())?;
        if guard.is_none() {
            *guard = Some(
                bridge_token::mint(self.bridge_scope())
                    .map_err(|error| format!("Kronn access refused: {error}"))?,
            );
        }
        let value = guard.as_ref().map(|token| token.value().to_owned());
        value
            .as_deref()
            .and_then(bridge_token::lookup)
            .ok_or_else(|| "Kronn access refused: this run's bridge token is no longer live".into())
    }

    /// The route a tool mirrors, built from its arguments the way the MCP
    /// bridge builds its request. `None`: the tool touches no Kronn object
    /// (files, web, manuals) or keeps a stricter rule of its own.
    fn gate_request(&self, call: &ToolCall) -> Option<GateRequest> {
        let args = &call.arguments;
        let own_disc = self
            .disc_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let get = |pattern: &'static str, params: Vec<(String, String)>, query: Option<String>| {
            Some(GateRequest {
                method: "GET",
                pattern,
                params,
                query,
                body: None,
            })
        };
        let send = |method: &'static str,
                    pattern: &'static str,
                    params: Vec<(String, String)>,
                    body: Value| {
            Some(GateRequest {
                method,
                pattern,
                params,
                query: None,
                body: Some(body),
            })
        };
        let id = |key: &str| text(args, key).unwrap_or_default();
        let task = || vec![("id".to_string(), id("task_id"))];
        // A Page is named by id or slug; the gate resolves either.
        let page = || {
            text(args, "page_id")
                .or_else(|| text(args, "id"))
                .unwrap_or_default()
        };
        match call.name.as_str() {
            // Reading the room's reach, or filing a request a human decides.
            "room_scope" => {
                let disc = own_disc?;
                match text(args, "project") {
                    None => get(
                        "/api/discussions/{id}/scope",
                        vec![("id".into(), disc)],
                        None,
                    ),
                    Some(project) => send(
                        "POST",
                        "/api/discussions/{id}/scope/requests",
                        vec![("id".into(), disc)],
                        json!({
                            "project": project,
                            "access": args.get("access"),
                            "reason": args.get("reason"),
                        }),
                    ),
                }
            }
            "plan_get" => {
                let disc = own_disc.or_else(|| text(args, "discussion_id"))?;
                get(
                    "/api/discussions/{id}/plan",
                    vec![("id".into(), disc)],
                    None,
                )
            }
            "task_list" => get(
                "/api/planning/tasks",
                Vec::new(),
                query_of(&[
                    ("discussion_id", own_disc),
                    ("project_id", text(args, "project_id")),
                ]),
            ),
            "task_get" => get("/api/planning/tasks/{id}", task(), None),
            "task_create" => {
                let mut body = args.clone();
                if let (Some(fields), Some(disc)) = (body.as_object_mut(), own_disc) {
                    fields.insert("discussion_id".into(), json!(disc));
                }
                send("POST", "/api/planning/tasks", Vec::new(), body)
            }
            "task_update" => send(
                "PATCH",
                "/api/planning/tasks/{id}",
                task(),
                body_without(args, &["task_id"]),
            ),
            "task_update_dod" => send(
                "PATCH",
                "/api/planning/tasks/{id}/dod/{dod_id}",
                vec![
                    ("id".into(), id("task_id")),
                    ("dod_id".into(), id("dod_id")),
                ],
                json!({ "completed": args.get("completed"), "evidence": args.get("evidence") }),
            ),
            "task_link_discussion" => send(
                "POST",
                "/api/planning/tasks/{id}/discussions",
                task(),
                json!({ "discussion_id": own_disc.or_else(|| text(args, "discussion_id")) }),
            ),
            "task_add_blocker" => send(
                "POST",
                "/api/planning/tasks/{id}/blockers",
                task(),
                json!({ "blocker_task_id": id("blocker_task_id") }),
            ),
            "task_remove_blocker" => Some(GateRequest {
                method: "DELETE",
                pattern: "/api/planning/tasks/{id}/blockers/{blocker_id}",
                params: vec![
                    ("id".into(), id("task_id")),
                    ("blocker_id".into(), id("blocker_task_id")),
                ],
                query: None,
                body: None,
            }),
            "qa_list" => get("/api/quick-apis", Vec::new(), None),
            "qa_create_draft" => send("POST", "/api/quick-apis", Vec::new(), args.clone()),
            "qa_update" => send(
                "PUT",
                "/api/quick-apis/{id}",
                vec![("id".into(), id("quick_api_id"))],
                body_without(args, &["quick_api_id"]),
            ),
            "qa_run" => send(
                "POST",
                "/api/quick-apis/{id}/run",
                vec![("id".into(), id("quick_api_id"))],
                json!({}),
            ),
            "qe_list" => get("/api/quick-execs", Vec::new(), None),
            "qe_create_draft" => send("POST", "/api/quick-execs", Vec::new(), args.clone()),
            "qe_update" => send(
                "PUT",
                "/api/quick-execs/{id}",
                vec![("id".into(), id("quick_exec_id"))],
                body_without(args, &["quick_exec_id"]),
            ),
            "qe_run" => send(
                "POST",
                "/api/quick-execs/{id}/run",
                vec![("id".into(), id("quick_exec_id"))],
                json!({}),
            ),
            "qp_list" => get("/api/quick-prompts", Vec::new(), None),
            // Only what `quick_prompt_write_request` keeps: a template, never
            // its model or bindings, and no project change on update.
            "qp_create_draft" => send(
                "POST",
                "/api/quick-prompts",
                Vec::new(),
                only(args, QUICK_PROMPT_FIELDS),
            ),
            "qp_update" => send(
                "PUT",
                "/api/quick-prompts/{id}",
                vec![("id".into(), id("qp_id"))],
                only(args, &QUICK_PROMPT_FIELDS[..QUICK_PROMPT_FIELDS.len() - 1]),
            ),
            "qp_run" => send(
                "POST",
                "/api/mcp/qp-run",
                Vec::new(),
                json!({ "qp_id": id("qp_id") }),
            ),
            "api_call" => send(
                "POST",
                "/api/agent-api/call",
                Vec::new(),
                json!({ "api_config_id": text(args, "api_config_id") }),
            ),
            "mcp_list" | "api_endpoints" => get("/api/mcps", Vec::new(), None),
            "media_job_status" => get(
                "/api/media/jobs/{id}",
                vec![("id".into(), id("job_id"))],
                None,
            ),
            "media_generate" => send(
                "POST",
                "/api/media/generate",
                Vec::new(),
                json!({
                    "connection_id": text(args, "connection_id"),
                    "discussion_id": own_disc,
                }),
            ),
            // A task of another project is refused before its plan is read.
            "task_exec_prepare" | "task_exec_launch" => send(
                "POST",
                if call.name == "task_exec_prepare" {
                    "/api/orchestration/tool/prepare"
                } else {
                    "/api/orchestration/tool/launch"
                },
                Vec::new(),
                json!({
                    "task_reference": text(args, "task_reference"),
                    "parent_discussion_id": own_disc,
                }),
            ),
            "page_create" => {
                let mut body = json!({ "discussion_id": own_disc });
                for field in ["title", "slug", "project_id"] {
                    if let Some(value) = args.get(field) {
                        body[field] = value.clone();
                    }
                }
                send("POST", "/api/pages", Vec::new(), body)
            }
            "workflow_create_draft" => {
                let mut fields: Vec<&str> = super::workflow_tools::AUTHORED_FIELDS.to_vec();
                fields.push("project_id");
                send("POST", "/api/workflows", Vec::new(), only(args, &fields))
            }
            "workflow_update" => send(
                "PUT",
                "/api/workflows/{id}",
                vec![("id".into(), id("workflow_id"))],
                only(args, super::workflow_tools::AUTHORED_FIELDS),
            ),
            "page_get" => get("/api/pages/{id}", vec![("id".into(), page())], None),
            "page_update_html" => send(
                "PUT",
                "/api/pages/{id}/html",
                vec![("id".into(), page())],
                only(args, &["html", "slug"]),
            ),
            "page_add_dataset" => send(
                "POST",
                "/api/pages/{id}/datasets",
                vec![("id".into(), page())],
                only(args, &["name", "kind"]),
            ),
            "workflow_list" => get("/api/workflows", Vec::new(), None),
            "workflow_get" => get(
                "/api/workflows/{id}",
                vec![("id".into(), id("workflow_id"))],
                None,
            ),
            _ => None,
        }
    }

    /// Decide one call before it runs. A tool that is neither routed nor in
    /// [`UNGATED_TOOLS`] is refused: a new tool cannot slip past the gate.
    pub(super) async fn gate(&self, call: &ToolCall) -> Result<Gated, String> {
        // An audit's catalogue is files only, bounded to its workspace.
        if self.audit_workspace.is_some() {
            return Ok(Gated {
                call: None,
                admitted: None,
            });
        }
        if UNGATED_TOOLS.iter().any(|(name, _)| *name == call.name) {
            return Ok(Gated {
                call: None,
                admitted: None,
            });
        }
        let Some(request) = self.gate_request(call) else {
            return Err(format!(
                "Kronn refused `{}`: this tool has no access rule",
                call.name
            ));
        };
        let route = bridge_token::route_for(request.method, request.pattern)
            .ok_or_else(|| format!("`{}` has no Kronn route to be checked against", call.name))?;
        let grant = self.bridge_grant()?;
        let sent = request.body.clone();
        let admitted = bridge_admit::admit(
            &self.state.db,
            grant,
            route,
            request.params,
            request.query,
            request.body,
        )
        .await
        .map_err(|denied| {
            format!(
                "Kronn refused `{}` (outside this run's scope): {}",
                call.name, denied.message
            )
        })?;
        // The project the gate forced or kept is the one the tool runs with.
        let mut rewritten = None;
        if let (Some(prepared), true) = (admitted.body.as_ref(), admitted.body != sent) {
            let mut arguments = call.arguments.clone();
            if let Some(fields) = arguments.as_object_mut() {
                for key in GATE_OWNED_FIELDS {
                    if let Some(value) = prepared.get(*key) {
                        fields.insert((*key).into(), value.clone());
                    }
                }
            }
            if arguments != call.arguments {
                rewritten = Some(ToolCall {
                    arguments,
                    ..call.clone()
                });
            }
        }
        Ok(Gated {
            call: rewritten,
            admitted: Some(admitted),
        })
    }

    /// Scope a tool's result as the bridge scopes the route's response.
    pub(super) async fn scope_outcome(
        &self,
        admitted: Option<Admitted>,
        mut outcome: ToolOutcome,
    ) -> ToolOutcome {
        let Some(admitted) = admitted else {
            return outcome;
        };
        if !outcome.ok {
            return outcome;
        }
        let mut envelope = json!({ "success": true, "data": outcome.content.take() });
        match bridge_admit::scope_json(&self.state.db, &admitted, &mut envelope).await {
            Ok(()) => {
                outcome.content = envelope["data"].take();
                outcome
            }
            Err(denied) => ToolOutcome {
                content: json!({ "error": denied.message }),
                ok: false,
                ..outcome
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::UNGATED_TOOLS;

    fn every_tool() -> Vec<String> {
        let mut all = full_discussion_catalogue();
        all.extend(workflow_tool_catalogue(true));
        all.extend(workflow_workspace_tool_catalogue());
        all.push(tools_load_declaration());
        let mut names: Vec<String> = all
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// A tool an agent can be given is either decided by the gate on a real
    /// bridge route or listed, with its reason, as touching no Kronn object.
    #[tokio::test]
    async fn every_tool_has_an_access_rule() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let state = AppState::new_defaults(
            std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let executor = KronnToolExecutor::new(state, Some("room".into()));
        let mut unclassified = Vec::new();
        for name in every_tool() {
            if UNGATED_TOOLS.iter().any(|(tool, _)| *tool == name) {
                continue;
            }
            let call = ToolCall {
                id: name.clone(),
                name: name.clone(),
                arguments: json!({}),
            };
            match executor.gate_request(&call) {
                Some(request) => assert!(
                    crate::core::bridge_token::route_for(request.method, request.pattern).is_some(),
                    "{name} names {} {}, which is not a bridge route",
                    request.method,
                    request.pattern
                ),
                None => unclassified.push(name),
            }
        }
        assert!(
            unclassified.is_empty(),
            "no access rule for {unclassified:?}"
        );
    }
}
