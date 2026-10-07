//! Symbolic resource references in workflows (ADR-005, KT-917).
//!
//! Templated step fields name a resource as `{{ref:<kind>:<slug>}}`; the run
//! resolves it in its own project before a step renders. Structured fields
//! that hold an id (`sub_workflow_id`, `quick_prompt_id`, `quick_api_id`…)
//! are written to `kronn/` as `ref:<kind>:<slug>` and turned back into local
//! ids when a workflow is saved or imported: graph validation needs literal
//! ids. Resolution itself is `db::resource_identities::resolve_symbolic_reference`.

use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;

use crate::db::resource_identities::{resolve_symbolic_reference, REFERENCE_KINDS};
use crate::models::{StepType, Workflow, WorkflowStep};

/// The prefix of a symbolic reference in a template or a structured field.
pub const REF_PREFIX: &str = "ref:";

/// `(kind, slug)` of `ref:<kind>:<slug>`, also accepted wrapped as a template
/// (`{{ref:<kind>:<slug>}}`). `None` for anything else, a literal id included.
pub fn parse_reference(value: &str) -> Option<(&str, &str)> {
    let mut text = value.trim();
    if let Some(inner) = text
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
    {
        text = inner.trim();
    }
    let (kind, slug) = text.strip_prefix(REF_PREFIX)?.split_once(':')?;
    let slug = slug.trim();
    (REFERENCE_KINDS.contains(&kind) && !slug.is_empty() && !slug.contains(char::is_whitespace))
        .then_some((kind, slug))
}

/// Every `ref:<kind>:<slug>` key a template reads, in order, deduplicated.
pub fn template_references(template: &str) -> Vec<String> {
    let Ok(paths) = crate::workflows::template::placeholder_paths(template) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| parse_reference(path).is_some())
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn collect_value_references(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(text) if text.contains("{{") => {
            out.extend(template_references(text));
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_value_references(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_value_references(item, out);
            }
        }
        _ => {}
    }
}

/// The template references of every string a step carries: whatever field an
/// executor renders, its references are known before the run starts.
pub fn step_template_references(steps: &[WorkflowStep]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for step in steps {
        if let Ok(value) = serde_json::to_value(step) {
            collect_value_references(&value, &mut out);
        }
    }
    out
}

/// The run's template values for every reference its steps (and the Quick
/// Prompts and Quick APIs they load) read, resolved in `project_id` first.
/// A reference left out stays unknown: strict rendering then fails the step
/// before it launches, naming the reference.
pub fn resolve_template_references(
    conn: &Connection,
    workflow: &Workflow,
    project_id: Option<&str>,
) -> anyhow::Result<HashMap<String, String>> {
    let steps: Vec<WorkflowStep> = workflow
        .steps
        .iter()
        .chain(workflow.on_failure.iter())
        .cloned()
        .collect();
    let mut references = step_template_references(&steps);
    for step in &steps {
        let prompt_ids = step
            .quick_prompt_id
            .iter()
            .chain(step.batch_quick_prompt_id.iter())
            .chain(step.batch_chain_prompt_ids.iter());
        for id in prompt_ids {
            let Some(id) = structured_id(conn, "prompt", id, project_id)? else {
                continue;
            };
            if let Some(prompt) = crate::db::quick_prompts::get_quick_prompt(conn, &id)? {
                references.extend(template_references(&prompt.prompt_template));
            }
        }
        if let Some(id) = step.quick_api_id.as_deref() {
            if let Some(id) = structured_id(conn, "qa", id, project_id)? {
                if let Some(api) = crate::db::quick_apis::get_quick_api(conn, &id)? {
                    if let Ok(value) = serde_json::to_value(&api) {
                        collect_value_references(&value, &mut references);
                    }
                }
            }
        }
    }
    let mut values = HashMap::new();
    for reference in references {
        let Some((kind, slug)) = parse_reference(&reference) else {
            continue;
        };
        match resolve_symbolic_reference(conn, &format!("{kind}:{slug}"), project_id) {
            Ok(Some(id)) => {
                values.insert(reference, id);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%reference, "symbolic reference not resolved: {error}"),
        }
    }
    Ok(values)
}

/// A structured field's local id: the id itself, or what its `ref:` resolves to.
fn structured_id(
    conn: &Connection,
    expected_kind: &str,
    value: &str,
    project_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match parse_reference(value) {
        Some((kind, slug)) if kind == expected_kind => {
            resolve_symbolic_reference(conn, &format!("{kind}:{slug}"), project_id)
        }
        Some(_) => Ok(None),
        None => Ok(Some(value.to_string())),
    }
}

/// One structured id field of a step: its reference kind and a handle on it.
fn structured_fields(step: &mut WorkflowStep) -> Vec<(&'static str, &mut String)> {
    let mut fields: Vec<(&'static str, &mut String)> = Vec::new();
    if matches!(
        step.step_type,
        StepType::SubWorkflow | StepType::TriggerWorkflow
    ) {
        if let Some(target) = step.sub_workflow_id.as_mut() {
            fields.push(("workflow", target));
        }
    }
    if let Some(id) = step.quick_prompt_id.as_mut() {
        fields.push(("prompt", id));
    }
    if let Some(id) = step.batch_quick_prompt_id.as_mut() {
        fields.push(("prompt", id));
    }
    for id in step.batch_chain_prompt_ids.iter_mut() {
        fields.push(("prompt", id));
    }
    if let Some(id) = step.quick_api_id.as_mut() {
        fields.push(("qa", id));
    }
    if let Some(config) = step.collect_api_data.as_mut() {
        for source in config.sources.iter_mut() {
            if !source.quick_api_id.is_empty() {
                fields.push(("qa", &mut source.quick_api_id));
            }
            if !source.quick_exec_id.is_empty() {
                fields.push(("qe", &mut source.quick_exec_id));
            }
        }
    }
    fields
}

/// Rewrites every `ref:` held by a structured step field into the local id it
/// names, first in `project_id`. Every unknown or ambiguous reference is
/// reported, by step and reference, and nothing is half-applied. With
/// `keep_resource_refs`, Quick Prompt and Quick API references stay symbolic
/// so a multi-project workflow resolves them in each run's project. A
/// workflow target is always made literal (graph validation), and so is a
/// Quick Exec (its command is checked against the allowlist at save).
pub fn resolve_structured_references(
    conn: &Connection,
    steps: &mut [WorkflowStep],
    project_id: Option<&str>,
    keep_resource_refs: bool,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let mut resolved: Vec<WorkflowStep> = steps.to_vec();
    for step in resolved.iter_mut() {
        let name = step.name.clone();
        for (field_kind, value) in structured_fields(step) {
            let Some((kind, slug)) = parse_reference(value) else {
                continue;
            };
            if kind != field_kind {
                errors.push(format!(
                    "Step « {name} »: `{}` names a {kind}, this field expects a {field_kind}",
                    value.trim()
                ));
                continue;
            }
            if keep_resource_refs && matches!(field_kind, "prompt" | "qa") {
                let canonical = format!("{REF_PREFIX}{kind}:{slug}");
                *value = canonical;
                continue;
            }
            match resolve_symbolic_reference(conn, &format!("{kind}:{slug}"), project_id) {
                Ok(Some(id)) => *value = id,
                Ok(None) => errors.push(format!(
                    "Step « {name} »: unknown resource reference `{REF_PREFIX}{kind}:{slug}` (import or create it first)"
                )),
                Err(error) => errors.push(format!("Step « {name} »: {error}")),
            }
        }
    }
    if errors.is_empty() {
        steps.clone_from_slice(&resolved);
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// Run-time half for a multi-project workflow: every structured `ref:` that
/// resolves in the run's project becomes its local id. One that does not
/// stays symbolic, so the step that loads it fails before launch, naming it.
pub fn resolve_run_structured_references(
    conn: &Connection,
    workflow: &mut Workflow,
    project_id: Option<&str>,
) -> anyhow::Result<()> {
    for step in workflow
        .steps
        .iter_mut()
        .chain(workflow.on_failure.iter_mut())
    {
        for (field_kind, value) in structured_fields(step) {
            let Some((kind, slug)) = parse_reference(value) else {
                continue;
            };
            if kind != field_kind {
                continue;
            }
            match resolve_symbolic_reference(conn, &format!("{kind}:{slug}"), project_id) {
                Ok(Some(id)) => *value = id,
                Ok(None) => {}
                Err(error) => tracing::warn!(reference = %value, "not resolved: {error}"),
            }
        }
    }
    Ok(())
}

/// Whether `workflow` runs the Quick Prompt (`kind` "prompt") or Quick API
/// ("qa") `id` anywhere: steps and rollback, direct, batch, chained or
/// collection fields, by literal id or by a `ref:` that resolves to it in
/// any project the workflow can run in.
pub fn workflow_uses(
    conn: &Connection,
    workflow: &Workflow,
    kind: &str,
    id: &str,
) -> anyhow::Result<bool> {
    let mut projects = crate::workflows::project_scope::scheduled_projects(conn, workflow)?;
    projects.push(workflow.project_id.clone());
    projects.push(None);
    projects.dedup();
    for step in workflow.steps.iter().chain(workflow.on_failure.iter()) {
        let mut step = step.clone();
        for (field_kind, value) in structured_fields(&mut step) {
            if field_kind != kind {
                continue;
            }
            let Some((ref_kind, slug)) = parse_reference(value) else {
                if value.trim() == id {
                    return Ok(true);
                }
                continue;
            };
            if ref_kind != kind {
                continue;
            }
            for project in &projects {
                if resolve_symbolic_reference(
                    conn,
                    &format!("{ref_kind}:{slug}"),
                    project.as_deref(),
                )?
                .as_deref()
                    == Some(id)
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// The enabled workflows that run `kind`/`id` (see [`workflow_uses`]). An
/// ambiguous reference is an error, so the caller can refuse its change.
pub fn enabled_workflows_using(
    conn: &Connection,
    kind: &str,
    id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut ids = Vec::new();
    for workflow in crate::db::workflows::list_workflows(conn)? {
        if workflow.enabled && workflow_uses(conn, &workflow, kind, id)? {
            ids.push(workflow.id);
        }
    }
    Ok(ids)
}

/// Disables the given workflows; returns how many were still enabled.
pub fn disable_workflows(conn: &Connection, ids: &[String]) -> anyhow::Result<usize> {
    let mut disabled = 0;
    for id in ids {
        disabled += conn.execute(
            "UPDATE workflows SET enabled = 0 WHERE id = ?1 AND enabled = 1",
            rusqlite::params![id],
        )?;
    }
    Ok(disabled)
}

/// The slug a resource of `kind` is published under: its identity in the
/// workflow's project, else the one its name gives, as publication does.
fn publication_slug(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    id: &str,
) -> anyhow::Result<Option<String>> {
    let identity_kind = match kind {
        "workflow" => "workflow",
        "prompt" => "quick_prompt",
        "qa" => "quick_api",
        "qe" => "quick_exec",
        "artifact" => "artifact",
        _ => return Ok(None),
    };
    for scope in [project_key, ""] {
        if let Some(identity) =
            crate::db::resource_identities::find_by_target(conn, scope, identity_kind, id)?
        {
            return Ok(Some(identity.slug));
        }
        if scope.is_empty() {
            break;
        }
    }
    let name = match kind {
        "workflow" => crate::db::workflows::get_workflow(conn, id)?.map(|item| item.name),
        "prompt" => crate::db::quick_prompts::get_quick_prompt(conn, id)?.map(|item| item.name),
        "qa" => crate::db::quick_apis::get_quick_api(conn, id)?.map(|item| item.name),
        "qe" => crate::db::quick_execs::get_quick_exec(conn, id)?.map(|item| item.name),
        "artifact" => {
            return Ok(crate::db::live_pages::get_live_page_summary(conn, id)?
                .map(|page| page.slug)
                .filter(|slug| !slug.is_empty()))
        }
        _ => None,
    };
    Ok(name
        .map(|name| crate::core::repository_resources::ascii_slug(&name))
        .filter(|slug| !slug.is_empty()))
}

/// The workflow as `kronn/` carries it: structured ids replaced by
/// `ref:<kind>:<slug>`, and a literal Page id by `{{ref:artifact:<slug>}}`
/// (that field is a runtime template). An id this instance does not know is
/// left as it is.
pub fn symbolize_workflow(conn: &Connection, workflow: &mut Workflow) -> anyhow::Result<()> {
    let project_key =
        crate::db::resource_identities::project_key(conn, workflow.project_id.as_deref())?;
    for step in workflow
        .steps
        .iter_mut()
        .chain(workflow.on_failure.iter_mut())
    {
        for (kind, value) in structured_fields(step) {
            if parse_reference(value).is_some() || value.trim().is_empty() {
                continue;
            }
            if let Some(slug) = publication_slug(conn, &project_key, kind, value.trim())? {
                *value = format!("{REF_PREFIX}{kind}:{slug}");
            }
        }
        if let Some(config) = step.page_publish.as_mut() {
            let page_id = config.page_id.trim().to_string();
            if !page_id.is_empty() && !page_id.contains("{{") {
                if let Some(slug) = publication_slug(conn, &project_key, "artifact", &page_id)? {
                    config.page_id = format!("{{{{{REF_PREFIX}artifact:{slug}}}}}");
                }
            }
        }
    }
    Ok(())
}

/// Whether a workflow keeps any structured `ref:` to resolve per run.
pub fn has_structured_references(workflow: &Workflow) -> bool {
    let mut copy = workflow.clone();
    copy.steps
        .iter_mut()
        .chain(copy.on_failure.iter_mut())
        .any(|step| {
            structured_fields(step)
                .into_iter()
                .any(|(_, value)| parse_reference(value).is_some())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reference_accepts_the_bare_and_the_template_forms_only() {
        assert_eq!(
            parse_reference("ref:workflow:triage"),
            Some(("workflow", "triage"))
        );
        assert_eq!(
            parse_reference(" {{ ref:artifact:suivi-équipe }} "),
            Some(("artifact", "suivi-équipe"))
        );
        assert_eq!(parse_reference("3f2a-uuid"), None);
        assert_eq!(parse_reference("ref:unknown:slug"), None);
        assert_eq!(parse_reference("ref:workflow:"), None);
        assert_eq!(parse_reference("ref:workflow:two words"), None);
        assert_eq!(parse_reference(""), None);
    }

    #[test]
    fn template_references_lists_each_reference_once() {
        assert_eq!(
            template_references(
                "run {{ref:workflow:a}} then {{ ref:qe:lint }} and {{ref:workflow:a}} {{steps.x.output}}"
            ),
            vec!["ref:workflow:a".to_string(), "ref:qe:lint".to_string()]
        );
        assert!(template_references("{{unclosed").is_empty());
    }

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn step(json: serde_json::Value) -> WorkflowStep {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn structured_references_resolve_atomically_and_name_each_failure() {
        let conn = conn();
        crate::db::resource_identities::tests::seed_workflow(&conn, "wf-1", "Child", None);
        crate::db::resource_identities::tests::seed_prompt(&conn, "qp-1", "Review", None);
        let mut steps = vec![
            step(
                serde_json::json!({"name": "a", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "ref:workflow:child"}),
            ),
            step(
                serde_json::json!({"name": "b", "step_type": {"type": "Agent"}, "quick_prompt_id": "{{ref:prompt:review}}"}),
            ),
        ];
        resolve_structured_references(&conn, &mut steps, None, false).unwrap();
        assert_eq!(steps[0].sub_workflow_id.as_deref(), Some("wf-1"));
        assert_eq!(steps[1].quick_prompt_id.as_deref(), Some("qp-1"));

        let mut broken = vec![
            step(
                serde_json::json!({"name": "a", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "ref:workflow:child"}),
            ),
            step(
                serde_json::json!({"name": "b", "step_type": {"type": "Agent"}, "quick_prompt_id": "ref:workflow:child"}),
            ),
            step(
                serde_json::json!({"name": "c", "step_type": {"type": "Agent"}, "quick_prompt_id": "ref:prompt:missing"}),
            ),
        ];
        let error = resolve_structured_references(&conn, &mut broken, None, false).unwrap_err();
        assert!(
            error.contains("« b »") && error.contains("expects a prompt"),
            "{error}"
        );
        assert!(error.contains("ref:prompt:missing"), "{error}");
        assert_eq!(
            broken[0].sub_workflow_id.as_deref(),
            Some("ref:workflow:child"),
            "nothing is half-applied"
        );

        let mut kept = vec![
            step(
                serde_json::json!({"name": "a", "step_type": {"type": "TriggerWorkflow"}, "sub_workflow_id": "{{ref:workflow:child}}"}),
            ),
            step(
                serde_json::json!({"name": "b", "step_type": {"type": "Agent"}, "quick_prompt_id": "{{ ref:prompt:review }}"}),
            ),
        ];
        resolve_structured_references(&conn, &mut kept, None, true).unwrap();
        assert_eq!(kept[0].sub_workflow_id.as_deref(), Some("wf-1"));
        assert_eq!(
            kept[1].quick_prompt_id.as_deref(),
            Some("ref:prompt:review")
        );
    }

    #[test]
    fn symbolize_names_known_resources_and_keeps_unknown_ids() {
        let conn = conn();
        crate::db::resource_identities::tests::seed_workflow(&conn, "wf-1", "Child Target", None);
        crate::db::resource_identities::tests::seed_page(&conn, "page-1", "Suivi", "suivi", None);
        let mut workflow: Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf-p", "name": "Parent", "project_id": null,
            "trigger": {"type": "Manual"},
            "steps": [
                {"name": "a", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "wf-1"},
                {"name": "b", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-elsewhere"},
                {"name": "c", "step_type": {"type": "PublishPageData"}, "page_publish": {"page_id": "page-1", "writes": []}}
            ],
            "actions": [], "safety": {}, "workspace_config": null, "concurrency_limit": null,
            "enabled": true, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        symbolize_workflow(&conn, &mut workflow).unwrap();
        assert_eq!(
            workflow.steps[0].sub_workflow_id.as_deref(),
            Some("ref:workflow:child-target")
        );
        assert_eq!(
            workflow.steps[1].quick_prompt_id.as_deref(),
            Some("qp-elsewhere")
        );
        assert_eq!(
            workflow.steps[2].page_publish.as_ref().unwrap().page_id,
            "{{ref:artifact:suivi}}"
        );
        assert!(has_structured_references(&workflow));
    }

    #[test]
    fn a_kept_prompt_reference_resolves_in_each_runs_project() {
        let conn = conn();
        for id in ["proj-a", "proj-b"] {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?1, ?1, ?1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [id],
            )
            .unwrap();
        }
        crate::db::resource_identities::tests::seed_prompt(&conn, "qp-a", "Review", Some("proj-a"));
        crate::db::resource_identities::tests::seed_prompt(&conn, "qp-b", "Review", Some("proj-b"));
        let mut workflow: Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf", "name": "Multi", "project_id": null,
            "trigger": {"type": "Manual"},
            "steps": [{"name": "a", "step_type": {"type": "Agent"}, "quick_prompt_id": "ref:prompt:review"}],
            "actions": [], "safety": {}, "workspace_config": null, "concurrency_limit": null,
            "enabled": true, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        for (project, expected) in [("proj-a", "qp-a"), ("proj-b", "qp-b")] {
            let mut run = workflow.clone();
            resolve_run_structured_references(&conn, &mut run, Some(project)).unwrap();
            assert_eq!(run.steps[0].quick_prompt_id.as_deref(), Some(expected));
        }
        workflow.steps[0].quick_prompt_id = Some("ref:prompt:absent".into());
        resolve_run_structured_references(&conn, &mut workflow, Some("proj-a")).unwrap();
        assert_eq!(
            workflow.steps[0].quick_prompt_id.as_deref(),
            Some("ref:prompt:absent"),
            "left for the step to fail on, naming it"
        );
    }
}
