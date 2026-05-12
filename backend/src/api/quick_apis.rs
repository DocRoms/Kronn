//! HTTP API for `QuickApi` — reusable API call templates (0.6.0).
//!
//! Mirror of `quick_prompts.rs` but the engine is HTTP, not LLM. Routes:
//!
//! - `GET    /api/quick-apis`            list
//! - `POST   /api/quick-apis`            create
//! - `PUT    /api/quick-apis/:id`        update
//! - `DELETE /api/quick-apis/:id`        delete
//! - `GET    /api/quick-apis/:id/export` self-contained JSON download
//! - `POST   /api/quick-apis/import`     import an exported envelope
//! - `POST   /api/quick-apis/:id/run`    standalone run with variables
use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

const QA_EXPORT_KIND: &str = "kronn.quick_api";
const QA_EXPORT_VERSION: u32 = 1;

/// GET /api/quick-apis
pub async fn list(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<QuickApi>>> {
    match state.db.with_conn(crate::db::quick_apis::list_quick_apis).await {
        Ok(items) => Json(ApiResponse::ok(items)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/quick-apis
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateQuickApiRequest>,
) -> Json<ApiResponse<QuickApi>> {
    if req.name.is_empty() || req.name.len() > 200 {
        return Json(ApiResponse::err("Name must be 1-200 characters"));
    }
    if req.api_plugin_slug.is_empty() {
        return Json(ApiResponse::err("api_plugin_slug is required"));
    }
    if req.api_config_id.is_empty() {
        return Json(ApiResponse::err("api_config_id is required"));
    }
    if req.api_endpoint_path.is_empty() {
        return Json(ApiResponse::err("api_endpoint_path is required"));
    }

    let now = Utc::now();
    let qa = QuickApi {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        icon: req.icon.unwrap_or_else(|| "🔌".into()),
        description: req.description,
        project_id: req.project_id,
        api_plugin_slug: req.api_plugin_slug,
        api_config_id: req.api_config_id,
        api_endpoint_path: req.api_endpoint_path,
        api_method: req.api_method,
        api_query: req.api_query,
        api_path_params: req.api_path_params,
        api_headers: req.api_headers,
        api_body: req.api_body,
        api_extract: req.api_extract,
        api_pagination: req.api_pagination,
        api_timeout_ms: req.api_timeout_ms,
        api_max_retries: req.api_max_retries,
        variables: req.variables,
        created_at: now,
        updated_at: now,
    };

    let q = qa.clone();
    match state.db.with_conn(move |conn| crate::db::quick_apis::insert_quick_api(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(qa)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/quick-apis/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateQuickApiRequest>,
) -> Json<ApiResponse<QuickApi>> {
    let qa_id = id.clone();
    let existing = match state.db.with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &qa_id)).await {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick API not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let updated = QuickApi {
        id: existing.id,
        name: if req.name.is_empty() { existing.name } else { req.name },
        icon: req.icon.unwrap_or(existing.icon),
        // Description is taken from the request even if empty — that's how the user clears it.
        description: req.description,
        project_id: req.project_id,
        // API fields require the user to re-supply them; partial PUT is intentional.
        api_plugin_slug: if req.api_plugin_slug.is_empty() { existing.api_plugin_slug } else { req.api_plugin_slug },
        api_config_id: if req.api_config_id.is_empty() { existing.api_config_id } else { req.api_config_id },
        api_endpoint_path: if req.api_endpoint_path.is_empty() { existing.api_endpoint_path } else { req.api_endpoint_path },
        api_method: req.api_method.or(existing.api_method),
        api_query: req.api_query.or(existing.api_query),
        api_path_params: req.api_path_params.or(existing.api_path_params),
        api_headers: req.api_headers.or(existing.api_headers),
        api_body: req.api_body.or(existing.api_body),
        api_extract: req.api_extract.or(existing.api_extract),
        api_pagination: req.api_pagination.or(existing.api_pagination),
        api_timeout_ms: req.api_timeout_ms.or(existing.api_timeout_ms),
        api_max_retries: req.api_max_retries.or(existing.api_max_retries),
        variables: req.variables,
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let q = updated.clone();
    match state.db.with_conn(move |conn| crate::db::quick_apis::update_quick_api(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(updated)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/quick-apis/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::quick_apis::delete_quick_api(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/quick-apis/:id/export
pub async fn export_qa(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let qa_id = id.clone();
    let qa = match state.db.with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &qa_id)).await {
        Ok(Some(q)) => q,
        Ok(None) => return (StatusCode::NOT_FOUND, "Quick API not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)).into_response(),
    };

    let envelope = QuickApiExportEnvelope {
        kind: QA_EXPORT_KIND.to_string(),
        version: QA_EXPORT_VERSION,
        exported_at: Utc::now(),
        quick_api: qa.clone(),
    };

    let safe_name: String = qa.name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let filename = format!("{}.kronn-qa.json", safe_name);

    let body = match serde_json::to_string_pretty(&envelope) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Serialization error: {}", e)).into_response(),
    };

    (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename)),
        ],
        body,
    ).into_response()
}

/// POST /api/quick-apis/import
pub async fn import_qa(
    State(state): State<AppState>,
    Json(req): Json<ImportQuickApiRequest>,
) -> Json<ApiResponse<QuickApi>> {
    let envelope: QuickApiExportEnvelope = match serde_json::from_str(&req.content) {
        Ok(env) => env,
        Err(e) => return Json(ApiResponse::err(format!("JSON invalide : {}", e))),
    };

    if envelope.kind != QA_EXPORT_KIND {
        return Json(ApiResponse::err(format!(
            "Type incorrect : attendu `{}`, reçu `{}`. Vérifie que tu importes bien un Quick API exporté depuis Kronn.",
            QA_EXPORT_KIND, envelope.kind
        )));
    }
    if envelope.version > QA_EXPORT_VERSION {
        return Json(ApiResponse::err(format!(
            "Version d'export non supportée ({} > {} max). Mets à jour Kronn pour importer ce fichier.",
            envelope.version, QA_EXPORT_VERSION
        )));
    }

    let mut qa = envelope.quick_api;
    if qa.name.trim().is_empty() {
        return Json(ApiResponse::err("Le Quick API importé n'a pas de nom — fichier corrompu ?"));
    }
    if qa.api_endpoint_path.trim().is_empty() {
        return Json(ApiResponse::err("Le Quick API importé n'a pas d'endpoint — fichier corrompu ?"));
    }

    let now = Utc::now();
    qa.id = Uuid::new_v4().to_string();
    qa.project_id = req.project_id;
    qa.created_at = now;
    qa.updated_at = now;

    let q = qa.clone();
    match state.db.with_conn(move |conn| crate::db::quick_apis::insert_quick_api(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(qa)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/quick-apis/:id/run
///
/// Standalone execution of a saved QuickApi. The user passes values for
/// the declared `variables`; the backend builds an ephemeral `WorkflowStep`
/// that mirrors the QuickApi's API config, runs it via the same executor
/// the workflow runner uses, and returns the parsed envelope (or the
/// error message on failure).
pub async fn run_qa(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RunQuickApiRequest>,
) -> Json<ApiResponse<RunQuickApiResponse>> {
    let qa_id = id.clone();
    let qa = match state.db.with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &qa_id)).await {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick API not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Validate required variables.
    for v in &qa.variables {
        if v.required {
            let val = req.variables.get(&v.name).map(|s| s.trim()).unwrap_or("");
            if val.is_empty() {
                return Json(ApiResponse::err(format!(
                    "Variable obligatoire manquante : `{}`",
                    v.name
                )));
            }
        }
    }

    // Build template context from variables.
    let mut ctx = crate::workflows::template::TemplateContext::new();
    for (k, v) in &req.variables {
        ctx.set(k.clone(), v.clone());
    }

    // Build an ephemeral WorkflowStep that mirrors the QuickApi config.
    // Re-uses the runtime executor so the execution path is identical to a
    // workflow run — same security policy, same retry logic, same envelope.
    let step = WorkflowStep {
        name: format!("__qa_{}__", qa.id),
        step_type: StepType::ApiCall,
        description: None,
        agent: AgentType::ClaudeCode, // unused for ApiCall
        prompt_template: String::new(),
        mode: StepMode::Normal,
        output_format: StepOutputFormat::Structured,
        on_result: vec![],
        agent_settings: None,
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        mcp_config_ids: vec![],
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        batch_quick_prompt_id: None,
        batch_items_from: None,
        batch_wait_for_completion: None,
        batch_max_items: None,
        batch_workspace_mode: None,
        batch_chain_prompt_ids: vec![],
        batch_concurrent_limit: None,
        quick_api_id: None,
        notify_config: None,
        api_plugin_slug: Some(qa.api_plugin_slug),
        api_config_id: Some(qa.api_config_id),
        api_endpoint_path: Some(qa.api_endpoint_path),
        api_method: qa.api_method,
        api_query: qa.api_query,
        api_path_params: qa.api_path_params,
        api_headers: qa.api_headers,
        api_body: qa.api_body,
        api_extract: qa.api_extract,
        api_pagination: qa.api_pagination,
        api_timeout_ms: qa.api_timeout_ms,
        api_max_retries: qa.api_max_retries,
        api_output_var: None,
        gate_message: None,
        gate_request_changes_target: None,
        gate_notify_url: None,
        exec_command: None,
        exec_args: vec![],
        exec_timeout_secs: None,
        exec_setup_command: None,
        exec_setup_args: vec![],
        quick_prompt_id: None,
        json_data_payload: None,
    };

    let outcome = crate::workflows::api_call_executor::execute_api_call_step_with_db(
        &step,
        qa.project_id.as_deref(),
        &state,
        &ctx,
        crate::workflows::api_call_executor::SecurityPolicy::production(),
    )
    .await;

    let success = outcome.result.status == RunStatus::Success;
    // Same strip-then-parse trick as `/test-api-call`: the executor now
    // appends `\n[SIGNAL: OK]` after the JSON envelope.
    let envelope: Option<serde_json::Value> = if success {
        let json_part = outcome.result.output
            .split("\n[SIGNAL:")
            .next()
            .unwrap_or(&outcome.result.output);
        serde_json::from_str(json_part).ok()
    } else {
        None
    };
    let error = if success { None } else { Some(outcome.result.output) };

    Json(ApiResponse::ok(RunQuickApiResponse {
        success,
        duration_ms: outcome.result.duration_ms,
        envelope,
        error,
    }))
}

/// POST /api/quick-apis/:id/batch
///
/// Fan-out the same QA over a list of items. Internally this is the same
/// executor `BatchApiCall` workflow steps use — we just build an ephemeral
/// step that references the saved QA and lets the executor do its job.
/// Result envelope shape: `{ data: { items[], total, succeeded, failed }, ... }`.
pub async fn batch_run_qa(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<BatchRunQuickApiRequest>,
) -> Json<ApiResponse<BatchRunQuickApiResponse>> {
    let qa_id = id.clone();
    let qa = match state.db.with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &qa_id)).await {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick API not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Validate items shape early so the user gets a clear "no items" error
    // before we waste time spawning a no-op batch.
    let items_arr = match &req.items {
        serde_json::Value::Array(arr) => arr,
        _ => return Json(ApiResponse::err("`items` must be a JSON array (of strings or objects).")),
    };
    if items_arr.is_empty() {
        return Json(ApiResponse::err("`items` is empty — nothing to run."));
    }

    // Normalize string items to objects keyed by the QA's first variable
    // name. The QA's request templates use `{{<var_name>}}` (the user's
    // convention), and the executor exposes object keys as bare template
    // variables — so without this normalization a 1-var QA would never
    // see its `{{host}}` resolved (string items only set `batch.item`).
    // Object items are left as-is; they're already keyed by var name.
    let first_var_name = qa.variables.first().map(|v| v.name.clone());
    let normalized_items: Vec<serde_json::Value> =
        normalize_batch_items(items_arr, first_var_name.as_deref());

    // Serialize the items array as a JSON literal — the executor's
    // template engine renders the literal as-is when items_from doesn't
    // contain `{{` placeholders. No template variables in standalone runs.
    let items_literal = match serde_json::to_string(&normalized_items) {
        Ok(s) => s,
        Err(e) => return Json(ApiResponse::err(format!("Could not serialize items: {}", e))),
    };

    // Build an ephemeral BatchApiCall step that references the saved QA.
    // Per-field overrides on the step are all None → the executor will
    // pull every API field from the QA.
    let step = WorkflowStep {
        name: format!("__qa_batch_{}__", qa.id),
        step_type: StepType::BatchApiCall,
        description: None,
        agent: AgentType::ClaudeCode, // unused for BatchApiCall
        prompt_template: String::new(),
        mode: StepMode::Normal,
        output_format: StepOutputFormat::Structured,
        on_result: vec![],
        agent_settings: None,
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        mcp_config_ids: vec![],
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        batch_quick_prompt_id: None,
        batch_items_from: Some(items_literal),
        batch_wait_for_completion: None,
        batch_max_items: None,
        batch_workspace_mode: None,
        batch_chain_prompt_ids: vec![],
        batch_concurrent_limit: req.concurrent_limit,
        quick_api_id: Some(qa.id.clone()),
        notify_config: None,
        api_plugin_slug: None, api_config_id: None, api_endpoint_path: None,
        api_method: None, api_query: None, api_path_params: None, api_headers: None,
        api_body: None, api_extract: None, api_pagination: None,
        api_timeout_ms: None, api_max_retries: None, api_output_var: None,
        gate_message: None, gate_request_changes_target: None, gate_notify_url: None,
        exec_command: None, exec_args: vec![], exec_timeout_secs: None,
        exec_setup_command: None, exec_setup_args: vec![],
        quick_prompt_id: None,
        json_data_payload: None,
    };

    let ctx = crate::workflows::template::TemplateContext::new();
    let outcome = crate::workflows::batch_apicall_step::execute_batch_apicall_step(
        &step,
        qa.project_id.as_deref(),
        &state,
        &ctx,
    ).await;

    // Strip the trailing `\n[SIGNAL: ...]` line before parsing — same trick
    // as /test-api-call and /run.
    let json_part = outcome.result.output
        .split("\n[SIGNAL:")
        .next()
        .unwrap_or(&outcome.result.output);
    let envelope: Option<serde_json::Value> = serde_json::from_str(json_part).ok();
    // Pull the "status" field from the envelope (OK/PARTIAL/ERROR). Falls
    // back to the run status when the envelope didn't parse.
    let status = envelope.as_ref()
        .and_then(|v| v.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or(if outcome.result.status == RunStatus::Success { "OK" } else { "ERROR" })
        .to_string();
    let error = if envelope.is_none() && outcome.result.status != RunStatus::Success {
        Some(outcome.result.output)
    } else {
        None
    };

    Json(ApiResponse::ok(BatchRunQuickApiResponse {
        status,
        duration_ms: outcome.result.duration_ms,
        envelope,
        error,
    }))
}

/// Normalize a batch items array so each item is a JSON object keyed by
/// the QA's variable names — the format the executor's per-item template
/// engine expects. String items are wrapped in `{<first_var_name>: <string>}`
/// so a 1-variable QA can accept newline-separated values; object items
/// are passed through unchanged. Pure function (no AppState, no IO) so
/// the normalization rules are unit-testable.
///
/// `first_var_name = None` means the QA declares no variables — strings
/// are then left as-is (the executor falls back to `{{batch.item}}`).
fn normalize_batch_items(
    items: &[serde_json::Value],
    first_var_name: Option<&str>,
) -> Vec<serde_json::Value> {
    items.iter().map(|item| {
        match item {
            serde_json::Value::String(s) => match first_var_name {
                Some(name) => {
                    let mut obj = serde_json::Map::new();
                    obj.insert(name.to_string(), serde_json::Value::String(s.clone()));
                    serde_json::Value::Object(obj)
                }
                None => item.clone(),
            },
            _ => item.clone(),
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_string_items_with_first_var_wraps_each_into_object() {
        // The headline use case — user pastes `["fr.euronews.com", ...]`
        // with a 1-variable QA `host`. Each string must become
        // `{ "host": "..." }` so the executor exposes `host` as a bare
        // template variable downstream.
        let items = vec![
            serde_json::json!("fr.euronews.com"),
            serde_json::json!("de.euronews.com"),
            serde_json::json!("euronews.com"),
        ];
        let out = normalize_batch_items(&items, Some("host"));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], serde_json::json!({ "host": "fr.euronews.com" }));
        assert_eq!(out[1], serde_json::json!({ "host": "de.euronews.com" }));
        assert_eq!(out[2], serde_json::json!({ "host": "euronews.com" }));
    }

    #[test]
    fn normalize_object_items_pass_through_unchanged() {
        // For multi-var QAs the user posts JSON objects directly — those
        // must NOT be re-wrapped (the keys are already the var names).
        let items = vec![
            serde_json::json!({ "host": "de.example.com", "limit": "5" }),
            serde_json::json!({ "host": "fr.example.com", "limit": "10" }),
        ];
        let out = normalize_batch_items(&items, Some("host"));
        assert_eq!(out, items);
    }

    #[test]
    fn normalize_string_items_without_var_name_pass_through() {
        // A QA with zero variables (rare but legal — a static call). The
        // executor falls back to `{{batch.item}}` for strings, so we
        // intentionally don't wrap them.
        let items = vec![
            serde_json::json!("ping1"),
            serde_json::json!("ping2"),
        ];
        let out = normalize_batch_items(&items, None);
        assert_eq!(out, items);
    }

    #[test]
    fn normalize_mixed_items_handles_each_per_shape() {
        // Defensive: a malformed batch with both shapes mixed. Strings
        // get wrapped, objects pass through. The executor doesn't care
        // either way — but if we silently dropped one shape we'd get
        // partial fan-outs in production with no error.
        let items = vec![
            serde_json::json!("fr.euronews.com"),
            serde_json::json!({ "host": "de.example.com" }),
            serde_json::json!("it.euronews.com"),
        ];
        let out = normalize_batch_items(&items, Some("host"));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], serde_json::json!({ "host": "fr.euronews.com" }));
        assert_eq!(out[1], serde_json::json!({ "host": "de.example.com" }));
        assert_eq!(out[2], serde_json::json!({ "host": "it.euronews.com" }));
    }

    #[test]
    fn batch_run_strip_signal_before_envelope_parse() {
        // Regression guard for the same bug we hit on `/test-api-call`:
        // the batch handler also has to strip the trailing `\n[SIGNAL: ...]`
        // line before serde_json::from_str. Mirror of the QA single-run
        // tests in api/workflows.rs::tests.
        let envelope_json = r#"{"data":{"items":[],"total":0},"status":"OK","summary":""}"#;
        let with_signal = format!("{}\n[SIGNAL: OK]", envelope_json);
        let json_part = with_signal.split("\n[SIGNAL:").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_part)
            .expect("strip-then-parse should succeed on batch envelope");
        assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("OK"));
    }
}
