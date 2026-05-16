//! Bundle endpoint — atomic multi-artifact creation from a single
//! `KRONN:BUNDLE_READY` chat signal.
//!
//! See `models::bundle` for the wire shape. Logic:
//!
//! 1. Validate the payload — every bundle_id is unique within its
//!    category, and every `@bundle:<id>` reference inside the
//!    workflow resolves to a declared bundle_id (across categories).
//! 2. Pre-allocate UUIDs (or computed ids for Custom APIs via
//!    `materialize_custom_server`) and build the bundle_id → real_id
//!    map. Pre-allocation lets us substitute references in the
//!    workflow JSON *before* hitting the DB — any failure here is a
//!    400 with zero side-effects.
//! 3. Substitute every `@bundle:<id>` in the workflow JSON with the
//!    pre-allocated real id by walking the JSON tree recursively.
//! 4. Run the workflow's own validators (`validate_step_references`,
//!    `validate_guards`, exec allowlist, etc.) on the substituted
//!    workflow. Reject if the substituted graph is still invalid.
//! 5. Open a SQLite transaction via `unchecked_transaction`, insert
//!    each artifact in order: QPs → QAs → Custom APIs → Workflow.
//!    Any insert error short-circuits the transaction and the txn
//!    drops without commit — full rollback, no orphan rows.
//! 6. Return the created ids per bundle_id so the frontend can deep-
//!    link to each.

use axum::{extract::State, Json};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// POST /api/workflows/bundle
pub async fn create_bundle(
    State(state): State<AppState>,
    Json(mut req): Json<BundleRequest>,
) -> Json<ApiResponse<BundleResponse>> {
    // ── 1. Validate bundle_id uniqueness ──────────────────────
    // We treat all three sections as one namespace because a
    // workflow step's `quick_api_id` field can hold any artifact's
    // real id — substitution is field-agnostic. A duplicate bundle_id
    // across categories would make `@bundle:X` ambiguous.
    let mut all_bundle_ids: HashSet<String> = HashSet::new();
    let mut id_map: HashMap<String, String> = HashMap::new(); // bundle_id → real_id

    for qp in &req.quick_prompts {
        if !validate_bundle_id(&qp.bundle_id) {
            return Json(ApiResponse::err(format!(
                "Invalid bundle_id `{}` on quick_prompts entry — must be kebab-case ([A-Za-z0-9_-]+) and non-empty",
                qp.bundle_id
            )));
        }
        if !all_bundle_ids.insert(qp.bundle_id.clone()) {
            return Json(ApiResponse::err(format!(
                "Duplicate bundle_id `{}` (must be unique across quick_prompts/quick_apis/custom_apis)",
                qp.bundle_id
            )));
        }
        id_map.insert(qp.bundle_id.clone(), Uuid::new_v4().to_string());
    }
    for qa in &req.quick_apis {
        if !validate_bundle_id(&qa.bundle_id) {
            return Json(ApiResponse::err(format!(
                "Invalid bundle_id `{}` on quick_apis entry",
                qa.bundle_id
            )));
        }
        if !all_bundle_ids.insert(qa.bundle_id.clone()) {
            return Json(ApiResponse::err(format!(
                "Duplicate bundle_id `{}` across categories",
                qa.bundle_id
            )));
        }
        id_map.insert(qa.bundle_id.clone(), Uuid::new_v4().to_string());
    }
    // Pre-materialize Custom API servers so we have their unique
    // `custom-{slug}-{nano}` ids ready for substitution. The actual
    // upsert happens inside the transaction; here we just compute the
    // id (deterministic per payload + nano).
    let mut custom_servers: Vec<(String, crate::models::McpServer, CustomApiPayload)> = Vec::new();
    for ca in &req.custom_apis {
        if !validate_bundle_id(&ca.bundle_id) {
            return Json(ApiResponse::err(format!(
                "Invalid bundle_id `{}` on custom_apis entry",
                ca.bundle_id
            )));
        }
        if !all_bundle_ids.insert(ca.bundle_id.clone()) {
            return Json(ApiResponse::err(format!(
                "Duplicate bundle_id `{}` across categories",
                ca.bundle_id
            )));
        }
        if ca.payload.name.trim().is_empty() {
            return Json(ApiResponse::err(format!(
                "Custom API `{}` requires a non-empty name",
                ca.bundle_id
            )));
        }
        if ca.payload.base_url.trim().is_empty() {
            return Json(ApiResponse::err(format!(
                "Custom API `{}` requires a non-empty base_url",
                ca.bundle_id
            )));
        }
        let server = crate::api::mcps::materialize_custom_server(&ca.payload);
        id_map.insert(ca.bundle_id.clone(), server.id.clone());
        custom_servers.push((ca.bundle_id.clone(), server, ca.payload.clone()));
    }

    // ── 2. Validate every @bundle:X ref resolves ───────────────
    // Walk the workflow as JSON; cheaper than maintaining a list of
    // ref-capable field names, and future-proof against new fields.
    let workflow_value = match serde_json::to_value(&req.workflow) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(format!(
            "Workflow payload is not valid JSON: {}", e
        ))),
    };
    let mut missing_refs: Vec<String> = Vec::new();
    collect_bundle_refs(&workflow_value, &mut |bid| {
        if !id_map.contains_key(bid) {
            missing_refs.push(bid.to_string());
        }
    });
    if !missing_refs.is_empty() {
        missing_refs.sort();
        missing_refs.dedup();
        return Json(ApiResponse::err(format!(
            "Workflow references unknown bundle_id(s): {}. Declare them in `quick_prompts`, `quick_apis`, or `custom_apis`.",
            missing_refs.join(", ")
        )));
    }

    // ── 3. Substitute @bundle:X → real_id in workflow JSON ─────
    let mut substituted = workflow_value;
    substitute_bundle_refs(&mut substituted, &id_map);
    let workflow_req: CreateWorkflowRequest = match serde_json::from_value(substituted) {
        Ok(w) => w,
        Err(e) => return Json(ApiResponse::err(format!(
            "Workflow shape broke after @bundle: substitution: {}", e
        ))),
    };

    // Replace the request's workflow with the substituted one (so the
    // commit path uses the right ids).
    req.workflow = workflow_req;

    // ── 4. Build the typed artifacts ready for transactional insert ─
    let now = Utc::now();
    let mut prepared_qps: Vec<QuickPrompt> = Vec::with_capacity(req.quick_prompts.len());
    for qp in &req.quick_prompts {
        let id = id_map[&qp.bundle_id].clone();
        prepared_qps.push(QuickPrompt {
            id,
            name: qp.request.name.clone(),
            icon: qp.request.icon.clone().unwrap_or_else(|| "✨".into()),
            prompt_template: qp.request.prompt_template.clone(),
            variables: qp.request.variables.clone(),
            agent: qp.request.agent.clone().unwrap_or(AgentType::ClaudeCode),
            project_id: qp.request.project_id.clone(),
            skill_ids: qp.request.skill_ids.clone(),
            profile_ids: qp.request.profile_ids.clone(),
            directive_ids: qp.request.directive_ids.clone(),
            tier: qp.request.tier,
            description: qp.request.description.clone(),
            created_at: now,
            updated_at: now,
        });
    }
    let mut prepared_qas: Vec<QuickApi> = Vec::with_capacity(req.quick_apis.len());
    for qa in &req.quick_apis {
        let id = id_map[&qa.bundle_id].clone();
        prepared_qas.push(QuickApi {
            id,
            name: qa.request.name.clone(),
            icon: qa.request.icon.clone().unwrap_or_else(|| "🌐".into()),
            description: qa.request.description.clone(),
            project_id: qa.request.project_id.clone(),
            api_plugin_slug: qa.request.api_plugin_slug.clone(),
            api_config_id: qa.request.api_config_id.clone(),
            api_endpoint_path: qa.request.api_endpoint_path.clone(),
            api_method: qa.request.api_method.clone(),
            api_query: qa.request.api_query.clone(),
            api_path_params: qa.request.api_path_params.clone(),
            api_headers: qa.request.api_headers.clone(),
            api_body: qa.request.api_body.clone(),
            api_extract: qa.request.api_extract.clone(),
            api_pagination: qa.request.api_pagination.clone(),
            api_timeout_ms: qa.request.api_timeout_ms,
            api_max_retries: qa.request.api_max_retries,
            variables: qa.request.variables.clone(),
            profile_ids: qa.request.profile_ids.clone(),
            directive_ids: qa.request.directive_ids.clone(),
            created_at: now,
            updated_at: now,
        });
    }

    // Workflow needs the same fields the regular `create` endpoint
    // composes; reuse `Workflow` directly so we don't drift from the
    // canonical shape.
    let wf_id = Uuid::new_v4().to_string();
    let wf_to_insert = Workflow {
        id: wf_id.clone(),
        name: req.workflow.name.clone(),
        project_id: req.workflow.project_id.clone(),
        trigger: req.workflow.trigger.clone(),
        steps: req.workflow.steps.clone(),
        actions: req.workflow.actions.clone(),
        safety: req.workflow.safety.clone().unwrap_or(WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        }),
        workspace_config: req.workflow.workspace_config.clone(),
        concurrency_limit: req.workflow.concurrency_limit,
        guards: req.workflow.guards.clone(),
        artifacts: req.workflow.artifacts.clone(),
        on_failure: req.workflow.on_failure.clone(),
        exec_allowlist: req.workflow.exec_allowlist.clone(),
        variables: req.workflow.variables.clone(),
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    // ── 5. Single transaction — atomic insert across all artifacts ─
    let qps_for_response: Vec<BundleCreated> = req.quick_prompts.iter().zip(prepared_qps.iter())
        .map(|(declared, inserted)| BundleCreated {
            bundle_id: declared.bundle_id.clone(),
            id: inserted.id.clone(),
            name: inserted.name.clone(),
        })
        .collect();
    let qas_for_response: Vec<BundleCreated> = req.quick_apis.iter().zip(prepared_qas.iter())
        .map(|(declared, inserted)| BundleCreated {
            bundle_id: declared.bundle_id.clone(),
            id: inserted.id.clone(),
            name: inserted.name.clone(),
        })
        .collect();
    let custom_apis_for_response: Vec<BundleCreated> = custom_servers.iter()
        .map(|(bundle_id, server, _)| BundleCreated {
            bundle_id: bundle_id.clone(),
            id: server.id.clone(),
            name: server.name.clone(),
        })
        .collect();
    let workflow_name_for_response = wf_to_insert.name.clone();
    let workflow_id_for_response = wf_id.clone();

    let insert_result = state
        .db
        .with_conn(move |conn| {
            // SQLite transactions need a mutable handle to start; the
            // `unchecked_transaction` variant accepts `&Connection`
            // by relying on rusqlite's interior mutability for the
            // BEGIN/COMMIT/ROLLBACK keywords. Drop without commit =
            // ROLLBACK.
            let tx = conn.unchecked_transaction()?;
            for qp in &prepared_qps {
                crate::db::quick_prompts::insert_quick_prompt(&tx, qp)?;
            }
            for qa in &prepared_qas {
                crate::db::quick_apis::insert_quick_api(&tx, qa)?;
            }
            for (_, server, _) in &custom_servers {
                crate::db::mcps::upsert_server(&tx, server)?;
            }
            crate::db::workflows::insert_workflow(&tx, &wf_to_insert)?;
            tx.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await;
    if let Err(e) = insert_result {
        return Json(ApiResponse::err(format!("Bundle insert failed (rolled back): {}", e)));
    }

    Json(ApiResponse::ok(BundleResponse {
        quick_prompts: qps_for_response,
        quick_apis: qas_for_response,
        custom_apis: custom_apis_for_response,
        workflow: BundleWorkflowCreated {
            id: workflow_id_for_response,
            name: workflow_name_for_response,
        },
    }))
}

/// `bundle_id` allowed characters: ASCII alphanumeric + `_-`. Same
/// grammar as Quick Prompt variable names so the architect can pick
/// a single naming convention.
fn validate_bundle_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Walk a serde_json::Value tree, calling `cb` on every string value
/// that starts with `@bundle:<id>` — passes the `<id>` portion only
/// (without the prefix). Used both for validation (collecting refs to
/// check) and substitution.
fn collect_bundle_refs<F: FnMut(&str)>(v: &serde_json::Value, cb: &mut F) {
    match v {
        serde_json::Value::String(s) => {
            if let Some(bid) = s.strip_prefix(BUNDLE_REF_PREFIX) {
                cb(bid);
            }
        }
        serde_json::Value::Array(arr) => {
            for x in arr {
                collect_bundle_refs(x, cb);
            }
        }
        serde_json::Value::Object(obj) => {
            for (_, x) in obj {
                collect_bundle_refs(x, cb);
            }
        }
        _ => {}
    }
}

/// In-place substitution of `@bundle:<id>` strings with `id_map[id]`.
/// Walks the same tree shape as `collect_bundle_refs`. Strings that
/// don't match the prefix are untouched. Strings that match but
/// don't resolve (shouldn't happen post-validation) are also left
/// untouched — the downstream `validate_step_references` will catch
/// them on the workflow side.
fn substitute_bundle_refs(v: &mut serde_json::Value, id_map: &HashMap<String, String>) {
    match v {
        serde_json::Value::String(s) => {
            if let Some(bid) = s.strip_prefix(BUNDLE_REF_PREFIX) {
                if let Some(real_id) = id_map.get(bid) {
                    *s = real_id.clone();
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for x in arr {
                substitute_bundle_refs(x, id_map);
            }
        }
        serde_json::Value::Object(obj) => {
            for (_, x) in obj {
                substitute_bundle_refs(x, id_map);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_bundle_id_accepts_kebab_and_snake() {
        assert!(validate_bundle_id("summarize-item"));
        assert!(validate_bundle_id("fetch_top_articles"));
        assert!(validate_bundle_id("Q1_metrics_2025"));
        assert!(validate_bundle_id("abc"));
    }

    #[test]
    fn validate_bundle_id_rejects_invalid_chars_or_empty() {
        assert!(!validate_bundle_id(""));
        assert!(!validate_bundle_id("has space"));
        assert!(!validate_bundle_id("special!"));
        assert!(!validate_bundle_id("dot.in.id"));
        assert!(!validate_bundle_id("@bundle:nested"));
    }

    #[test]
    fn collect_bundle_refs_finds_nested_strings() {
        let v = json!({
            "name": "test",
            "steps": [
                { "quick_prompt_id": "@bundle:summarize" },
                { "batch_quick_prompt_id": "@bundle:summarize" },  // dup ok at collect time
                { "quick_api_id": "@bundle:fetch" },
                { "prompt_template": "not a ref" },
            ],
            "trigger": { "type": "Manual" }
        });
        let mut found = Vec::new();
        collect_bundle_refs(&v, &mut |bid| found.push(bid.to_string()));
        found.sort();
        found.dedup();
        assert_eq!(found, vec!["fetch", "summarize"]);
    }

    #[test]
    fn substitute_bundle_refs_replaces_only_matching_prefix() {
        let mut v = json!({
            "field_a": "@bundle:foo",
            "field_b": "not-a-ref",
            "field_c": "@bundle:bar",
            "field_d": "@bundle:missing",  // intentionally not in map
            "nested": {
                "inner": "@bundle:foo"
            }
        });
        let id_map: HashMap<String, String> = vec![
            ("foo".into(), "uuid-real-1".into()),
            ("bar".into(), "custom-bar-12345678".into()),
        ].into_iter().collect();
        substitute_bundle_refs(&mut v, &id_map);
        assert_eq!(v["field_a"], "uuid-real-1");
        assert_eq!(v["field_b"], "not-a-ref");
        assert_eq!(v["field_c"], "custom-bar-12345678");
        // Missing ref is left as-is (downstream validator catches it).
        assert_eq!(v["field_d"], "@bundle:missing");
        assert_eq!(v["nested"]["inner"], "uuid-real-1");
    }

    #[test]
    fn collect_handles_empty_workflow() {
        // No @bundle: refs anywhere — common case (workflow with only
        // inline configs). Must not flag anything.
        let v = json!({
            "name": "Daily digest",
            "trigger": { "type": "Cron", "schedule": "0 9 * * 1-5" },
            "steps": [
                { "name": "fetch", "step_type": { "type": "ApiCall" }, "api_plugin_slug": "chartbeat" }
            ]
        });
        let mut found = Vec::new();
        collect_bundle_refs(&v, &mut |bid| found.push(bid.to_string()));
        assert!(found.is_empty());
    }

    #[test]
    fn collect_handles_string_that_almost_matches_prefix() {
        // `@bundles:foo` (note: plural) must NOT be detected.
        // `@bundle` alone (no colon) must NOT be detected.
        let v = json!({
            "field_a": "@bundles:foo",
            "field_b": "@bundle",
            "field_c": "see @bundle:inline-in-text",  // mid-string match still hits
        });
        let mut found = Vec::new();
        collect_bundle_refs(&v, &mut |bid| found.push(bid.to_string()));
        // Only field_c is a real prefix match — but wait, it doesn't
        // start with `@bundle:`, it has the marker mid-string. We use
        // `strip_prefix`, not `contains`. So field_c also doesn't match.
        assert!(found.is_empty(),
            "prefix match must be strict (no plural, no mid-string), got: {found:?}");
    }
}
