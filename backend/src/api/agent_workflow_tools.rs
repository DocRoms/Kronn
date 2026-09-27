//! Workflow authoring for native principals, sharing the HTTP validators.
use super::*;
use axum::{extract::Path, extract::State, Json};

#[cfg(test)]
#[path = "agent_workflow_tests.rs"]
mod tests;

pub(super) fn declarations() -> Vec<Value> {
    let mut editable = json!({
        "name":{"type":"string"}, "trigger":{"type":"object","description":"Tagged object, e.g. {type: Manual}."},
        "steps":{"type":"array","items":{"type":"object"},"description":"1–20 steps. Read workflow_step_schema first; step_type is a tagged object."},
        "actions":{"type":"array","items":{"type":"object"}},
        "safety":{"type":"object"}, "workspace_config":{"type":"object"},
        "guards":{"type":"object"}, "artifacts":{"type":"object"},
        "on_failure":{"type":"array","items":{"type":"object"}},
        "exec_allowlist":{"type":"array","items":{"type":"string"}},
        "concurrency_limit":{"type":"integer"},
        "variables":{"type":"array","items":{"type":"object"},"description":"Each: {name, label, placeholder, required}."}
    });
    let mut create = editable.clone();
    create["project_id"] = json!({"type":["string","null"],"description":"Omitted inherits this room's project; null creates a general draft."});
    editable["workflow_id"] = json!({"type":"string"});
    [
        ("workflow_list", "List accessible workflows with ids, trigger, enabled state and step counts.", json!({}), json!([])),
        ("workflow_get", "Read the complete accessible workflow before editing it; omitted fields are preserved by update.", json!({"workflow_id":{"type":"string"}}), json!(["workflow_id"])),
        ("workflow_step_schema", "Read the complete canonical step contracts before authoring. Optional step_type narrows to one type; section selects a shared contract.", json!({"step_type":{"type":"string"},"section":{"type":"string","enum":["template_vars","data_pipeline_contract"]}}), json!([])),
        ("workflow_create_draft", "Save a disabled workflow for human review. Never runs or enables it. Read workflow_step_schema and tool_manual for authoring rules.", create, json!(["name","trigger","steps"])),
        ("workflow_update", "Patch a disabled workflow, preserving omitted fields and project. Enabled workflows must first be disabled in the Workflows UI. Read workflow_get before replacing steps.", editable, json!(["workflow_id"])),
    ].into_iter().map(|(name,description,properties,required)| json!({"type":"function","function":{"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required}}})).collect()
}

fn schema_section(arguments: &Value) -> Result<Value, String> {
    let schema = crate::api::workflows::canonical_step_schema();
    if let Some(kind) = arguments["step_type"].as_str() {
        let fields = schema["fields_by_type"].get(kind).ok_or_else(|| format!("Unknown step_type {kind}; call workflow_step_schema with no arguments for the closed set"))?;
        return Ok(json!({"shape":schema["shape"],"step_type":kind,"contract":fields}));
    }
    if let Some(section) = arguments["section"].as_str() {
        if !["template_vars", "data_pipeline_contract"].contains(&section) {
            return Err("section must be template_vars or data_pipeline_contract".into());
        }
        return Ok(json!({"section":section,"contract":schema[section]}));
    }
    Ok(schema)
}

fn draft_patch(
    arguments: &Value,
    project_id: Option<&str>,
    creating: bool,
) -> Result<Value, String> {
    let fields = [
        "name",
        "trigger",
        "steps",
        "actions",
        "safety",
        "workspace_config",
        "concurrency_limit",
        "concurrency_key",
        "guards",
        "artifacts",
        "on_failure",
        "exec_allowlist",
        "variables",
    ];
    let mut patch = Value::Object(
        arguments
            .as_object()
            .ok_or("arguments must be an object")?
            .iter()
            .filter(|(key, _)| fields.contains(&key.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    // Never activate a schedule through the authoring surface.
    patch["enabled"] = json!(false);
    if creating {
        let project = arguments
            .get("project_id")
            .cloned()
            .unwrap_or_else(|| json!(project_id));
        if !project.is_null() && !project.is_string() {
            return Err("project_id must be a string or null".into());
        }
        if !project_is_in_scope(project.as_str(), project_id) {
            return Err("Workflow project is outside this discussion's scope".into());
        }
        patch["project_id"] = project;
    }
    if let Some(name) = patch.get("name").and_then(Value::as_str) {
        if name.trim().is_empty() {
            return Err("Workflow name must not be empty".into());
        }
    }
    for field in ["steps", "on_failure"] {
        if let Some(steps) = patch.get_mut(field).and_then(Value::as_array_mut) {
            if (field == "steps" && steps.is_empty()) || steps.len() > 20 {
                return Err(format!(
                    "{field} must contain {}–20 steps",
                    if field == "steps" { 1 } else { 0 }
                ));
            }
            for step in steps {
                for key in ["step_type", "mode", "output_format"] {
                    if let Some(value) = step[key].as_str().map(str::to_owned) {
                        step[key] = json!({"type":value});
                    }
                }
            }
        }
    }
    if creating
        && ["Cron", "Tracker"].contains(&patch["trigger"]["type"].as_str().unwrap_or_default())
        && patch["concurrency_limit"].is_null()
    {
        patch["concurrency_limit"] = json!(1);
    }
    Ok(patch)
}

impl KronnToolExecutor {
    pub(super) async fn execute_workflow_tool(&self, call: &ToolCall) -> ToolOutcome {
        if self.worker_room || self.workflow_run_id.is_some() {
            return fail(
                call,
                "Workflow authoring is available only to discussion principals",
            );
        }
        let project_id = self.effective_project_id().await;
        match call.name.as_str() {
            "workflow_step_schema" => match schema_section(&call.arguments) {
                Ok(schema) => ok(call, schema),
                Err(error) => fail(call, error),
            },
            "workflow_list" => {
                let Json(response) = crate::api::workflows::list(State(self.state.clone())).await;
                match (response.success, response.data) {
                    (true, Some(items)) => ok(
                        call,
                        json!({"workflows":items.into_iter().filter(|w|project_is_in_scope(w.project_id.as_deref(),project_id.as_deref())).collect::<Vec<_>>()}),
                    ),
                    _ => fail(
                        call,
                        response
                            .error
                            .unwrap_or_else(|| "Workflow list failed".into()),
                    ),
                }
            }
            "workflow_create_draft" => {
                let patch = match draft_patch(&call.arguments, project_id.as_deref(), true) {
                    Ok(p) => p,
                    Err(e) => return fail(call, e),
                };
                let request: crate::models::CreateWorkflowRequest =
                    match serde_json::from_value(patch) {
                        Ok(r) => r,
                        Err(e) => {
                            return fail(
                                call,
                                format!("Invalid workflow: {e}. Read workflow_step_schema."),
                            )
                        }
                    };
                let Json(response) =
                    crate::api::workflows::create(State(self.state.clone()), Json(request)).await;
                unwrap_api(call, response.success, response.data, response.error)
            }
            "workflow_get" | "workflow_update" => {
                let Some(id) = call.arguments["workflow_id"].as_str() else {
                    return fail(call, "workflow_id is required; use workflow_list");
                };
                let Json(response) =
                    crate::api::workflows::get(State(self.state.clone()), Path(id.to_owned()))
                        .await;
                let saved = match (response.success, response.data) {
                    (true, Some(w))
                        if project_is_in_scope(w.project_id.as_deref(), project_id.as_deref()) =>
                    {
                        w
                    }
                    _ => return fail(
                        call,
                        "Workflow is not available in this discussion's scope; use workflow_list",
                    ),
                };
                if call.name == "workflow_get" {
                    return ok(call, json!(saved));
                }
                if saved.enabled {
                    return fail(call,"This workflow is enabled. Disable it in the Workflows UI before editing, or create a new disabled draft with workflow_create_draft.");
                }
                let patch = match draft_patch(&call.arguments, project_id.as_deref(), false) {
                    Ok(p) => p,
                    Err(e) => return fail(call, e),
                };
                // The shared API applies a partial UpdateWorkflowRequest; do not
                // fill absent collections with empty defaults or move its project.
                let request: crate::models::UpdateWorkflowRequest =
                    match serde_json::from_value(patch) {
                        Ok(r) => r,
                        Err(e) => return fail(call, format!("Invalid workflow patch: {e}")),
                    };
                let Json(response) = crate::api::workflows::update(
                    State(self.state.clone()),
                    Path(saved.id),
                    Json(request),
                )
                .await;
                unwrap_api(call, response.success, response.data, response.error)
            }
            _ => fail(call, "Unknown workflow tool"),
        }
    }
}
