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
        match call.name.as_str() {
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
            "page_create" => {
                let mut body = json!({ "discussion_id": own_disc });
                for field in ["title", "slug", "project_id"] {
                    if let Some(value) = args.get(field) {
                        body[field] = value.clone();
                    }
                }
                send("POST", "/api/pages", Vec::new(), body)
            }
            "workflow_list" => get("/api/workflows", Vec::new(), None),
            "workflow_get" => get(
                "/api/workflows/{id}",
                vec![("id".into(), id("workflow_id"))],
                None,
            ),
            _ => None,
        }
    }

    /// Decide one call before it runs. Tools with no Kronn object pass as is.
    pub(super) async fn gate(&self, call: &ToolCall) -> Result<Gated, String> {
        // An audit's catalogue is files only, bounded to its workspace.
        if self.audit_workspace.is_some() {
            return Ok(Gated {
                call: None,
                admitted: None,
            });
        }
        let Some(request) = self.gate_request(call) else {
            return Ok(Gated {
                call: None,
                admitted: None,
            });
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
