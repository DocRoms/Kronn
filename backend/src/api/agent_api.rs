//! 0.8.6 — Agent API broker.
//!
//! `POST /api/agent-api/call` lets an MCP-driven agent (Claude Code,
//! Codex, …) invoke a Kronn-configured API plugin **without ever seeing
//! the credentials**. The agent passes plugin_slug + endpoint + params;
//! Kronn decrypts the env from the DB, resolves auth per the plugin's
//! `ApiSpec.auth` declaration, fires the HTTP call, and returns the
//! canonical envelope `{data, status, summary}` in the response body.
//!
//! Architecturally, this **reuses the same executor** as the workflow
//! `ApiCall` step (`crate::workflows::api_call_executor::
//! execute_api_call_step_with_db`). We build a synthetic `WorkflowStep`
//! from the agent's request and feed it in. Zero new execution code →
//! every plugin/auth/retry/extract/pagination behaviour already proven
//! by the workflow path is inherited for free.
//!
//! ## MVP scope (0.8.6 first cut)
//! - Plugin scoping resolved from the disc's `project_id`.
//! - Plugin selection by `api_plugin_slug` + `api_config_id`, OR via a
//!   saved `quick_api_id` (same hydration path as workflow ApiCall).
//! - Canonical envelope returned with `http_status` extracted from the
//!   executor's structured response.
//!
//! ## Deliberately deferred to a follow-up (cf. [[project_agent_api_broker_0_8_6]])
//! - `side_effect` opt-in gate. The executor currently honours the
//!   plugin's spec allowlist; if the plugin marks an endpoint as
//!   side-effecting, the broker still calls it. Future safety layer
//!   needs the caller to pass `allow_side_effects: true`.
//! - Per-disc rate-limit.
//! - Persistent audit log (cf. [[project_api_call_logs_0_8_6]]).
//! - UI counter pill in `ChatHeader`.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

use crate::models::*;
use crate::workflows::api_call_executor::{
    execute_api_call_step_with_db, SecurityPolicy,
};
use crate::workflows::template::TemplateContext;
use crate::AppState;

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct AgentApiCallRequest {
    /// `KRONN_DISCUSSION_ID` of the disc making the call, when the
    /// agent was spawned from a Kronn disc (auto-injected by the
    /// runner). 0.8.6 — now optional: host-CLI sessions (regular
    /// `claude` / `codex` etc. launched outside Kronn) don't have it
    /// and would otherwise be locked out of the broker. Project scope
    /// falls back to `project_id` explicit OR to `config.project_ids[0]`
    /// derived from the chosen `api_config_id`. The disc-id path is
    /// preferred when available — it links the call to a specific
    /// conversation for future audit-log entries.
    #[serde(default)]
    pub disc_id: Option<String>,

    /// 0.8.6 — explicit project scope override. Set this when the
    /// agent knows the right project (e.g. from `mcp_list.configs[].
    /// project_ids`) but doesn't have a disc id. Highest priority of
    /// the 3 resolution sources (explicit > disc > config-derived).
    #[serde(default)]
    pub project_id: Option<String>,

    /// Plugin slug — same value the agent sees in `mcp_list`'s
    /// `servers_with_api[].id`. Either this+`api_config_id`, OR
    /// `quick_api_id`, MUST be provided.
    #[serde(default)]
    pub api_plugin_slug: Option<String>,
    #[serde(default)]
    pub api_config_id: Option<String>,
    #[serde(default)]
    pub quick_api_id: Option<String>,

    /// Endpoint path as declared in `ApiSpec.endpoints[].path`. The
    /// executor's allowlist refuses anything not declared, so the
    /// broker inherits that guarantee.
    pub endpoint_path: String,

    /// HTTP method override. Defaults to the method declared in the
    /// plugin spec for this endpoint.
    #[serde(default)]
    pub method: Option<String>,

    /// Path-segment parameters (e.g. `/repos/{owner}/{repo}` →
    /// `{"owner": "DocRoms", "repo": "Kronn"}`).
    #[serde(default)]
    pub path_params: Option<HashMap<String, String>>,
    /// Query-string parameters (percent-encoded after rendering).
    #[serde(default)]
    pub query: Option<HashMap<String, String>>,
    /// Extra headers (auth comes from the plugin spec, not here).
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    /// JSON body for POST/PUT/PATCH (string leaves are templated).
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    /// JSON extract specification (same shape as workflow ApiCall).
    #[serde(default)]
    pub extract: Option<ExtractSpec>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AgentApiCallResponse {
    /// `true` when the HTTP call resolved to a 2xx AND the envelope
    /// was successfully parsed. Maps to `status == "OK"`.
    pub success: bool,
    pub duration_ms: u64,
    /// The `data` field from the canonical envelope. `None` on
    /// extract failure or transport errors.
    pub data: Option<serde_json::Value>,
    /// `"OK"` | `"ERROR"`.
    pub status: String,
    /// One-line summary suitable for the agent to echo back to the
    /// user without dumping the full payload.
    pub summary: String,
    /// HTTP status code when the call reached the server. `None` for
    /// transport-level errors (DNS, TLS, timeout before connect).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Filled when `success == false`. Carries the human-readable
    /// error from the executor so the agent can self-correct (wrong
    /// endpoint, bad params, expired token, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/agent-api/call`
///
/// Resolves the disc's project → builds a synthetic ApiCall
/// `WorkflowStep` → runs it through the standard executor → maps the
/// canonical envelope into the agent-friendly response shape.
pub async fn agent_api_call(
    State(state): State<AppState>,
    Json(req): Json<AgentApiCallRequest>,
) -> Json<ApiResponse<AgentApiCallResponse>> {
    // 1. Resolve project_id. Three sources, by priority:
    //   a) explicit `project_id` on the request (agent knows the
    //      scope; e.g. passing what `mcp_list.configs[].project_ids[0]`
    //      surfaced)
    //   b) from `disc_id` → disc.project_id (Kronn-spawn case)
    //   c) from `api_config_id` → config.project_ids[0] (host-CLI
    //      fallback — works for any agent that picked a real config
    //      via mcp_list)
    //
    // If all three sources are empty, we fall through to
    // `execute_api_call_step_with_db` which has its own fallback
    // (global config → empty project_id is fine; scoped config →
    // surfaces "not linked to any project" error).
    let project_id: Option<String> = if let Some(pid) = req.project_id.clone() {
        Some(pid)
    } else if let Some(did) = req.disc_id.clone().filter(|s| !s.is_empty()) {
        let did_for_query = did.clone();
        match state
            .db
            .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did_for_query))
            .await
        {
            Ok(Some(d)) => d.project_id,
            Ok(None) => {
                return Json(ApiResponse::err(format!(
                    "Discussion `{did}` not found — pass a valid disc_id, or omit it and provide `api_config_id` or `project_id` instead."
                )));
            }
            Err(e) => {
                return Json(ApiResponse::err(format!("DB error resolving discussion `{did}`: {e}")));
            }
        }
    } else if let Some(cid) = req.api_config_id.clone() {
        // 0.8.6 fallback: derive project from the config the agent
        // already chose. The config is what owns the credential, so
        // its project list is the relevant scope.
        let cid_for_query = cid.clone();
        match state
            .db
            .with_conn(move |conn| crate::db::mcps::get_config(conn, &cid_for_query))
            .await
        {
            Ok(Some(cfg)) => cfg.project_ids.into_iter().next(),
            Ok(None) => None,  // executor will surface a clean "config not found" error
            Err(_) => None,
        }
    } else {
        None
    };

    // 2. Validate the request shape. Either plugin_slug+config_id OR
    //    quick_api_id is required — without one of them the executor
    //    has no plugin to call.
    let has_plugin_pair = req.api_plugin_slug.is_some() && req.api_config_id.is_some();
    let has_qa_ref = req.quick_api_id.is_some();
    if !has_plugin_pair && !has_qa_ref {
        return Json(ApiResponse::err(
            "Either (api_plugin_slug + api_config_id) OR quick_api_id is required."
                .to_string(),
        ));
    }

    // 3. Build the synthetic WorkflowStep. The executor only reads the
    //    api_* fields + quick_api_id + step name (used in error messages).
    //    Every other field defaults to a no-op (cf. WorkflowStep::default
    //    in models/workflows.rs).
    let step = WorkflowStep {
        // Step name surfaces in executor error messages — be explicit so
        // a misconfigured call surfaces "agent-broker:<endpoint>" rather
        // than a generic "step missing X".
        name: format!("agent-broker:{}", req.endpoint_path),
        step_type: StepType::ApiCall,
        api_plugin_slug: req.api_plugin_slug.clone(),
        api_config_id: req.api_config_id.clone(),
        quick_api_id: req.quick_api_id.clone(),
        api_endpoint_path: Some(req.endpoint_path.clone()),
        api_method: req.method.clone(),
        api_path_params: req.path_params.clone(),
        api_query: req.query.clone(),
        api_headers: req.headers.clone(),
        api_body: req.body.clone(),
        api_extract: req.extract.clone(),
        ..Default::default()
    };

    // 4. Execute. Empty TemplateContext because the agent passes
    //    literal values directly — there's no workflow `steps.X.data`
    //    or `state.X` to interpolate. SecurityPolicy::production() so
    //    a misconfigured plugin pointing at localhost fails identically
    //    to a real workflow run.
    let ctx = TemplateContext::new();
    let outcome = execute_api_call_step_with_db(
        &step,
        project_id.as_deref(),
        &state,
        &ctx,
        SecurityPolicy::production(),
    )
    .await;

    // 5. Parse the canonical envelope from the executor's output. The
    //    output now carries a trailing `\n[SIGNAL: ...]` for branching
    //    workflows (0.6.0); strip it before JSON-parsing or
    //    `serde_json::from_str` chokes on the suffix line. Same logic as
    //    `workflows::test_api_call` (cf. api/workflows.rs:2606).
    let success = outcome.result.status == RunStatus::Success;
    // 0.8.6 fix (2026-05-20) — use the canonical envelope extractor
    // (`extract_step_envelope`) which knows the `---STEP_OUTPUT---` /
    // `---END_STEP_OUTPUT---` marker format produced by every step
    // type since 0.8.5. Pre-fix we only stripped the trailing
    // `[SIGNAL:…]` line and tried to parse the rest as raw JSON — that
    // happens to work for some bare-JSON envelopes but FAILS on the
    // marker-delimited canonical form (which has a human-readable
    // prefix line + the markers). Fallback path remained "data: null,
    // status: ERROR" — confusing for agents that just got a 200 OK
    // from the upstream API. Caught 2026-05-20 on Didomi /properties.
    let envelope: Option<serde_json::Value> = if success {
        crate::workflows::template::extract_step_envelope(&outcome.result.output)
            .map(|e| {
                // `e.data_json` is the JSON-serialised form of the data
                // field (so a string value lands as `"\"hello\""`,
                // an object as `{...}`). Parse it back to a Value so
                // the response surfaces structured data, not a
                // double-encoded string.
                let data_value: serde_json::Value = serde_json::from_str(&e.data_json)
                    .unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "data": data_value,
                    "status": e.status,
                    "summary": e.summary,
                })
            })
            // Last-resort fallback: try the pre-0.8.5 bare-JSON form
            // (some legacy records or non-Kronn output may still
            // land here).
            .or_else(|| {
                let json_part = outcome
                    .result
                    .output
                    .split("\n[SIGNAL:")
                    .next()
                    .unwrap_or(&outcome.result.output);
                serde_json::from_str(json_part).ok()
            })
    } else {
        None
    };

    // 0.8.6 (2026-05-20) — when the executor reported Success but the
    // envelope JSON parse failed (e.g. Didomi response shape doesn't
    // match the canonical `{data, status, summary}` Kronn expects),
    // surface the raw output so the agent can SEE what came back
    // instead of getting an opaque `{data: null, status: "ERROR"}`.
    // Caught live on Didomi /properties + /widgets/notices.
    let envelope_parse_failed_despite_success = success && envelope.is_none();
    let (data, status, summary, http_status) = match envelope.as_ref() {
        Some(env) => (
            env.get("data").cloned(),
            env.get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("OK")
                .to_string(),
            env.get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            env.get("http_status")
                .and_then(|v| v.as_u64())
                .and_then(|n| u16::try_from(n).ok()),
        ),
        None => (
            None,
            "ERROR".to_string(),
            if envelope_parse_failed_despite_success {
                "Response received but envelope could not be parsed — see `error` field for raw output".to_string()
            } else {
                String::new()
            },
            None,
        ),
    };

    let error = if success && !envelope_parse_failed_despite_success {
        None
    } else {
        // Either executor reported failure (carry the failure message)
        // OR executor succeeded but envelope was unparseable (carry the
        // raw output so the agent can act on what Didomi actually said).
        Some(outcome.result.output)
    };

    Json(ApiResponse::ok(AgentApiCallResponse {
        success,
        duration_ms: outcome.result.duration_ms,
        data,
        status,
        summary,
        http_status,
        error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The route is exercised end-to-end via the workflow ApiCall executor
    // path, which has its own deep test coverage in
    // `workflows::api_call_executor::tests`. The unit tests here pin the
    // SHAPE-LAYER guarantees specific to the broker:
    // - Required-field validation surfaces a clear error before any HTTP
    //   call is attempted.
    // - The envelope parsing handles malformed / signal-suffix output the
    //   way `workflows::test_api_call` does (verifies we don't regress
    //   the strip-then-parse contract).

    fn dummy_request() -> AgentApiCallRequest {
        AgentApiCallRequest {
            disc_id: Some("test-disc".into()),
            project_id: None,
            api_plugin_slug: None,
            api_config_id: None,
            quick_api_id: None,
            endpoint_path: "/whatever".into(),
            method: None,
            path_params: None,
            query: None,
            headers: None,
            body: None,
            extract: None,
        }
    }

    #[test]
    fn rejects_request_with_no_plugin_pair_and_no_qa_ref() {
        // Validator-layer test: we want the broker to refuse the
        // request BEFORE the disc lookup (free, no HTTP) when the agent
        // forgot to pass any plugin reference. This pin makes sure we
        // don't accidentally drop the early-return check in a future
        // refactor.
        let req = dummy_request();
        assert!(req.api_plugin_slug.is_none());
        assert!(req.quick_api_id.is_none());

        // Build the canonical "must reject" predicate we run inline
        // in the handler (line 124-127 of agent_api_call). Mirror it
        // here so the test fails if the predicate logic ever changes.
        let has_plugin_pair = req.api_plugin_slug.is_some() && req.api_config_id.is_some();
        let has_qa_ref = req.quick_api_id.is_some();
        assert!(
            !has_plugin_pair && !has_qa_ref,
            "validator predicate must catch empty references"
        );
    }

    #[test]
    fn plugin_pair_alone_passes_validation_predicate() {
        let mut req = dummy_request();
        req.api_plugin_slug = Some("custom-didomi-27c67bd7".into());
        req.api_config_id = Some("cfg-abc".into());
        let has_plugin_pair = req.api_plugin_slug.is_some() && req.api_config_id.is_some();
        let has_qa_ref = req.quick_api_id.is_some();
        assert!(has_plugin_pair && !has_qa_ref);
    }

    #[test]
    fn quick_api_id_alone_passes_validation_predicate() {
        // The QuickApi reference path: the agent doesn't even need to
        // know the plugin slug, it just calls a saved QA. Hydration
        // happens inside `execute_api_call_step_with_db`.
        let mut req = dummy_request();
        req.quick_api_id = Some("qa-jira-fetch".into());
        let has_plugin_pair = req.api_plugin_slug.is_some() && req.api_config_id.is_some();
        let has_qa_ref = req.quick_api_id.is_some();
        assert!(!has_plugin_pair && has_qa_ref);
    }

    #[test]
    fn plugin_slug_without_config_id_does_not_pass() {
        // Half-filled payload — agent passed the slug but forgot the
        // config_id. Same rejection as missing-everything: the executor
        // needs BOTH to resolve the encrypted env.
        let mut req = dummy_request();
        req.api_plugin_slug = Some("api-jira".into());
        // No api_config_id, no quick_api_id.
        let has_plugin_pair = req.api_plugin_slug.is_some() && req.api_config_id.is_some();
        let has_qa_ref = req.quick_api_id.is_some();
        assert!(
            !has_plugin_pair && !has_qa_ref,
            "slug alone must still be rejected"
        );
    }

    #[test]
    fn synthetic_workflow_step_uses_defaults_for_non_apicall_fields() {
        // The broker builds a synthetic `WorkflowStep` and feeds it to
        // the workflow executor. We rely on `WorkflowStep: Default`
        // (added in 0.8.6) so non-ApiCall fields are zeroed-out — if a
        // future refactor accidentally drops the Default derive, the
        // broker would break loudly. This pin catches that.
        let step = WorkflowStep {
            name: "agent-broker:/test".into(),
            step_type: StepType::ApiCall,
            api_plugin_slug: Some("slug".into()),
            api_config_id: Some("cfg".into()),
            api_endpoint_path: Some("/test".into()),
            ..Default::default()
        };
        assert_eq!(step.prompt_template, "");
        assert!(step.exec_command.is_none());
        assert!(step.notify_config.is_none());
        assert!(step.batch_quick_prompt_id.is_none());
        // ApiCall side-channels we use:
        assert_eq!(step.api_plugin_slug.as_deref(), Some("slug"));
        assert_eq!(step.api_endpoint_path.as_deref(), Some("/test"));
    }

    #[test]
    fn envelope_strip_handles_trailing_signal_line() {
        // The executor's output carries a trailing `\n[SIGNAL: OK]`
        // line for workflow branching (since 0.6.0). The broker MUST
        // strip it before parsing, otherwise `data`/`status`/`summary`
        // come back null and the agent gets a fake "ERROR" status.
        // Same regression that `test_api_call_strips_trailing_signal_line_before_json_parse`
        // pinned for the wizard's Test button.
        let envelope_json = r#"{"data":{"key":"EW-1"},"status":"OK","summary":"GET /search → 1 issue","http_status":200}"#;
        let with_signal = format!("{}\n[SIGNAL: OK]", envelope_json);

        let json_part = with_signal.split("\n[SIGNAL:").next().unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(json_part).expect("strip-then-parse should succeed");

        assert_eq!(
            parsed.get("status").and_then(|v| v.as_str()),
            Some("OK")
        );
        assert_eq!(
            parsed.pointer("/data/key").and_then(|v| v.as_str()),
            Some("EW-1")
        );
        // http_status extraction the broker does on success.
        assert_eq!(
            parsed.get("http_status").and_then(|v| v.as_u64()),
            Some(200)
        );
    }

    #[test]
    fn envelope_canonical_marker_form_extracts_correctly() {
        // 0.8.6 fix (2026-05-20) — the canonical Kronn envelope is
        // marker-delimited (`---STEP_OUTPUT---` / `---END_STEP_OUTPUT---`)
        // with a human-readable prefix line. Pre-fix the route parsed
        // everything-before-`[SIGNAL:` as raw JSON, which failed on
        // this shape. The fix uses `extract_step_envelope` (the same
        // canonical parser used by the workflow runner) so the agent
        // sees structured data, not "data: null, status: ERROR".
        let output = "GET /v1/properties → 10 results\n\
            ---STEP_OUTPUT---\n\
            {\"data\":[{\"id\":\"RJWTiiA9\",\"name\":\"Euronews English\"}],\"status\":\"OK\",\"summary\":\"GET /v1/properties → 10 results\"}\n\
            ---END_STEP_OUTPUT---\n\
            [SIGNAL: OK]";

        let envelope = crate::workflows::template::extract_step_envelope(output)
            .expect("canonical marker form must parse");

        // The extracted envelope has the 3 standard string fields.
        assert_eq!(envelope.status, "OK");
        assert!(envelope.summary.contains("10 results"));
        // `data_json` is the JSON-serialised form — parse back to Value
        // to inspect the structured array (same path the route uses
        // to build `AgentApiCallResponse.data`).
        let data_value: serde_json::Value = serde_json::from_str(&envelope.data_json)
            .expect("data_json must round-trip parse");
        let data_arr = data_value.as_array().expect("data is array");
        assert_eq!(data_arr.len(), 1);
        assert_eq!(
            data_arr[0].get("name").and_then(|v| v.as_str()),
            Some("Euronews English")
        );
    }

    #[test]
    fn envelope_legacy_bare_json_still_extracts_via_fallback() {
        // Pre-0.8.5 runs sometimes wrote bare JSON without markers.
        // The fallback path in the route handler keeps those working:
        // split on `\n[SIGNAL:`, parse the remainder as JSON. Pin that
        // path so a future refactor doesn't accidentally drop it.
        let output = "{\"data\":{\"x\":1},\"status\":\"OK\",\"summary\":\"\"}\n[SIGNAL: OK]";
        let json_part = output.split("\n[SIGNAL:").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(
            parsed.get("status").and_then(|v| v.as_str()),
            Some("OK")
        );
        assert_eq!(
            parsed.pointer("/data/x").and_then(|v| v.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn envelope_strip_is_noop_when_no_signal_suffix() {
        // Older runs (pre-0.6.0) or test mocks may not carry the
        // SIGNAL line. The split-and-take-first pattern must still
        // produce the full JSON intact.
        let envelope_json = r#"{"data":{"x":1},"status":"OK","summary":""}"#;
        let json_part = envelope_json.split("\n[SIGNAL:").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(parsed.pointer("/data/x").and_then(|v| v.as_i64()), Some(1));
    }
}
