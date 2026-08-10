//! Kronn primitives exposed to HTTP agents (Ollama, LiteLLM).
//!
//! CLI agents reach these through the `kronn-internal` stdio bridge. HTTP
//! agents have no bridge process, so the orchestrator executes on their
//! behalf — in-process, against the same handlers the bridge calls over HTTP.
//!
//! The catalogue is deliberately small. Every tool costs context on a local
//! model with a tight window, and a model given forty options picks worse
//! than one given four. These four cover "what can I reach, and call it",
//! which is what the bridge's own docs push agents towards (`qa_list` before
//! hand-building an `api_call`).

use crate::agents::tools::{ToolCall, ToolExecutor, ToolOutcome};
use crate::AppState;
use serde_json::{json, Value};

pub struct KronnToolExecutor {
    state: AppState,
    /// Scopes `api_call` to the calling conversation when there is one, so the
    /// broker can resolve project-scoped credentials the same way it does for
    /// CLI agents.
    disc_id: Option<String>,
}

impl KronnToolExecutor {
    pub fn new(state: AppState, disc_id: Option<String>) -> Self {
        Self { state, disc_id }
    }

    /// Wrap into the `Arc` the runner takes, or `None` when tools are off.
    pub fn arc(state: AppState, disc_id: Option<String>) -> std::sync::Arc<dyn ToolExecutor> {
        std::sync::Arc::new(Self::new(state, disc_id))
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
    ]
}

#[async_trait::async_trait]
impl ToolExecutor for KronnToolExecutor {
    fn catalogue(&self) -> Vec<Value> {
        tool_catalogue()
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        use axum::extract::{Path, State};
        use axum::Json;

        // Handlers are invoked directly rather than over loopback HTTP: same
        // code path as the bridge's calls, minus a round-trip and an auth hop.
        match call.name.as_str() {
            "mcp_list" => {
                let Json(res) = crate::api::mcps::overview(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(d)) => match serde_json::to_value(d) {
                        Ok(v) => ok(call, compact_plugin_list(&v)),
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
                    (true, Some(d)) => match serde_json::to_value(d)
                        .ok()
                        .and_then(|v| compact_endpoints(&v, slug))
                    {
                        Some(v) => ok(call, v),
                        None => fail(
                            call,
                            format!("no plugin `{slug}` with an API spec — call mcp_list first"),
                        ),
                    },
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "qa_list" => {
                let Json(res) = crate::api::quick_apis::list(State(self.state.clone())).await;
                match (res.success, res.data) {
                    (true, Some(d)) => match serde_json::to_value(d) {
                        Ok(v) => ok(call, compact_quick_apis(&v)),
                        Err(e) => fail(call, format!("could not serialise result: {e}")),
                    },
                    _ => fail(call, res.error.unwrap_or_else(|| "call failed".into())),
                }
            }
            "qa_run" => {
                let Some(id) = call.arguments["quick_api_id"].as_str() else {
                    return fail(call, "missing required field `quick_api_id`");
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
                    Path(id.to_string()),
                    Json(crate::models::RunQuickApiRequest { variables }),
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
                    Some(explicit) if !explicit.trim().is_empty() => Some(explicit.to_string()),
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
                    project_id: None,
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
                };
                let Json(res) =
                    crate::api::agent_api::agent_api_call(State(self.state.clone()), Json(req))
                        .await;
                unwrap_api(call, res.success, res.data, res.error)
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
        v["configs"]
            .as_array()?
            .iter()
            .find(|c| c["server_id"] == slug)
            .and_then(|c| c["id"].as_str().map(str::to_string))
    }
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
fn compact_plugin_list(overview: &Value) -> Value {
    let configs = overview["configs"].as_array();
    // `api_call` needs the CONFIG id, not just the plugin slug — a plugin can
    // be wired more than once (different accounts/projects) and the broker
    // resolves credentials per config. Omitting it made every api_call fail
    // with "Either (api_plugin_slug + api_config_id) OR quick_api_id is
    // required" and left the model with no way to discover it.
    let config_for = |slug: &Value| -> Option<Value> {
        configs?
            .iter()
            .find(|c| c["server_id"] == *slug)
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
fn compact_quick_apis(items: &Value) -> Value {
    let list: Vec<Value> = items
        .as_array()
        .map(|qs| {
            qs.iter()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_shape_is_what_both_providers_expect() {
        // Ollama and OpenAI both read `type: function` + `function.parameters`
        // as a JSON Schema object; a malformed entry is silently ignored by the
        // model, which surfaces as "the tool never worked".
        let expected = ["mcp_list", "api_endpoints", "qa_list", "qa_run", "api_call"];
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
            "configs": [{ "id": "cfg-1", "server_id": "mcp-resend", "label": "Resend" }],
        });
        let out = compact_plugin_list(&overview);
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
        let out = compact_quick_apis(&items);
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
    }
}
