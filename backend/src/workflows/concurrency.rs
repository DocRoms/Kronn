//! Workflow run admission: `concurrency_limit`, counted per workflow or, when
//! the workflow declares a `concurrency_key`, per key rendered at launch.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::models::{PromptVariable, PromptVariableSource, RunStatus, Workflow, WorkflowRun};

const MAX_KEY_CHARS: usize = 256;

/// A blank key means "no key": the limit stays per workflow.
pub fn normalize_key(key: Option<String>) -> Option<String> {
    key.map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn is_user_input(variable: &PromptVariable) -> bool {
    variable.source.clone().unwrap_or_default() == PromptVariableSource::UserInput
}

/// Save-time contract. The rendered key is stored in clear on every run and
/// shown in run lists, so it may only read launch inputs, never a variable
/// resolved from the project environment or the Kronn context.
pub fn validate_key(
    key: Option<&str>,
    limit: Option<u32>,
    variables: &[PromptVariable],
) -> Result<(), String> {
    let Some(template) = key else {
        return Ok(());
    };
    if limit.is_none() {
        return Err(
            "`concurrency_key` needs `concurrency_limit`: the limit is what is counted per key."
                .into(),
        );
    }
    if template.chars().count() > MAX_KEY_CHARS {
        return Err(format!(
            "`concurrency_key` is limited to {MAX_KEY_CHARS} characters."
        ));
    }
    let paths = crate::workflows::template::placeholder_paths(template)
        .map_err(|error| format!("`concurrency_key` is not a valid template: {error}"))?;
    if paths.is_empty() {
        return Err(
            "`concurrency_key` must read at least one launch variable, e.g. `{{ticketKey}}`."
                .into(),
        );
    }
    for path in paths {
        let Some(variable) = variables.iter().find(|variable| variable.name == path) else {
            return Err(format!(
                "`concurrency_key` reads `{path}`, which is not a launch variable of this workflow."
            ));
        };
        if !is_user_input(variable) {
            return Err(format!(
                "`concurrency_key` cannot read `{path}`: it is resolved from the project environment or the Kronn context and may be secret, while the rendered key is stored in clear on every run. Use a user_input variable."
            ));
        }
    }
    Ok(())
}

/// Renders the key from the run's resolved launch values. Only user inputs are
/// visible to it, whatever the saved template says. An empty result shares the
/// bucket of every other keyless run.
pub fn render_key(
    template: &str,
    variables: &[PromptVariable],
    values: &HashMap<String, String>,
) -> Result<Option<String>, String> {
    let mut ctx = crate::workflows::template::TemplateContext::new();
    ctx.set_launch_variables(
        variables,
        variables
            .iter()
            .filter(|variable| is_user_input(variable))
            .filter_map(|variable| {
                values
                    .get(&variable.name)
                    .map(|value| (variable.name.clone(), value.clone()))
            }),
    );
    let rendered = ctx
        .render_strict(template)
        .map_err(|error| format!("`concurrency_key` could not be rendered: {error}"))?;
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return Ok(None);
    }
    if rendered.chars().count() > MAX_KEY_CHARS || rendered.chars().any(char::is_control) {
        return Err(format!(
            "The rendered concurrency key must be one line of at most {MAX_KEY_CHARS} characters."
        ));
    }
    Ok(Some(rendered.to_string()))
}

/// A multi-project workflow's limit counts the runs of each project apart
/// (KT-851); a single-project one keeps one bucket.
fn project_bucket<'a>(workflow: &Workflow, run: &'a WorkflowRun) -> Option<Option<&'a str>> {
    workflow
        .project_scope
        .is_some()
        .then_some(run.project_id.as_deref())
}

/// Most blocking runs a refusal names; the rest is counted.
const MAX_NAMED_BLOCKERS: usize = 3;

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// One blocking run, with the actions that lift it (KT-1657).
fn describe_blocker(workflow_name: &str, run: &WorkflowRun) -> String {
    let id = short_id(&run.id);
    let child_of = run
        .parent_run_id
        .as_deref()
        .map(|parent| format!(" (child of run {})", short_id(parent)))
        .unwrap_or_default();
    let head = format!("run {id}{child_of} of \"{workflow_name}\"");
    match run.status {
        RunStatus::WaitingQuota => {
            let when = match super::quota_wait::pending_wait(run).and_then(|wait| wait.wake_at) {
                Some(at) => format!(
                    "paused on a provider quota, resumes automatically at {}",
                    at.format("%Y-%m-%d %H:%M UTC")
                ),
                None => "paused on a provider quota, waiting for a human to resume it".to_string(),
            };
            // A child is resumed through its parent.
            let target = run.parent_run_id.as_deref().unwrap_or(&run.id);
            format!(
                "{head} is WaitingQuota: {when}. Actions: resume it now (POST /api/workflow-runs/{target}/resume), open the run, cancel it, or switch the agent of its waiting step (POST /api/workflow-runs/{}/quota-agent)",
                run.id
            )
        }
        RunStatus::WaitingApproval => format!(
            "{head} is WaitingApproval: it waits at a gate. Actions: open the run to approve or reject it, or cancel it"
        ),
        _ => format!(
            "{head} is {:?}. Actions: open the run, or cancel it",
            run.status
        ),
    }
}

/// What holds the slots a refused launch needed, one clause per run; empty
/// when they cannot be listed (the refusal then stands on its count alone).
fn describe_blockers(
    conn: &Connection,
    workflow: &Workflow,
    key: Option<Option<&str>>,
    exclude_run_id: Option<&str>,
    project: Option<Option<&str>>,
) -> String {
    let Ok(runs) =
        crate::db::workflows::list_admitted_runs(conn, &workflow.id, key, exclude_run_id, project)
    else {
        return String::new();
    };
    if runs.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = runs
        .iter()
        .take(MAX_NAMED_BLOCKERS)
        .map(|run| describe_blocker(&workflow.name, run))
        .collect();
    if runs.len() > MAX_NAMED_BLOCKERS {
        parts.push(format!(
            "{} more run(s) hold a slot",
            runs.len() - MAX_NAMED_BLOCKERS
        ));
    }
    format!(" Blocked by: {}.", parts.join("; "))
}

/// Inserts `run` unless the workflow's limit is already reached for it. Call it
/// inside a single `with_conn` closure: on the shared connection the count and
/// the insert are then atomic, so two launches cannot both take the last slot.
pub fn insert_run_within_limit(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> anyhow::Result<Result<(), String>> {
    if let Some(max) = workflow.concurrency_limit {
        let project = project_bucket(workflow, run);
        if workflow.concurrency_key.is_some() {
            let key = run.concurrency_key.as_deref();
            let active = crate::db::workflows::count_admitted_runs(
                conn,
                &workflow.id,
                Some(key),
                None,
                project,
            )?;
            if active >= max {
                let label = key.map_or_else(|| "an empty key".to_string(), |k| format!("`{k}`"));
                let blockers = describe_blockers(conn, workflow, Some(key), None, project);
                return Ok(Err(format!(
                    "Concurrency limit reached for key {label} ({active}/{max}): a run with this key is already active.{blockers}"
                )));
            }
        } else {
            let active =
                crate::db::workflows::count_admitted_runs(conn, &workflow.id, None, None, project)?;
            if active >= max {
                let blockers = describe_blockers(conn, workflow, None, None, project);
                return Ok(Err(format!(
                    "Concurrency limit reached ({active}/{max}).{blockers}"
                )));
            }
        }
    }
    crate::db::workflows::insert_run(conn, run)?;
    Ok(Ok(()))
}

/// Why `parent_run_id` can no longer take a child (cancelled, stopped or
/// finished), or `None` when it still runs. Read in the closure that inserts
/// or claims the child: a cancellation settles the parent and its children
/// in one transaction on the same connection, so it is either seen here or
/// settles the child afterwards. No Pending child appears once it settled.
pub fn parent_refuses_children(
    conn: &Connection,
    parent_run_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    let Some(parent) = parent_run_id else {
        return Ok(None);
    };
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM workflow_runs WHERE id = ?1",
            [parent],
            |row| row.get(0),
        )
        .optional()?;
    Ok(match status.as_deref() {
        Some("Running" | "Pending" | "WaitingApproval" | "WaitingQuota") => None,
        Some(other) => Some(format!("parent run {parent} is {other}")),
        None => Some(format!("parent run {parent} no longer exists")),
    })
}

/// Refuses a child admission or resume that runs past `deadline`, the
/// earliest timeout the runners above it enforce: their executors are gone
/// even while the parent row still says Running.
pub fn past_deadline(deadline: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    deadline
        .filter(|deadline| chrono::Utc::now() >= *deadline)
        .map(|deadline| format!("the run's timeout passed at {}", deadline.to_rfc3339()))
}

/// [`insert_run_within_limit`] for a sub-workflow child, refused outright
/// (an error, not a limit refusal to wait on) once its parent stopped or
/// its runners' deadline passed.
pub fn insert_child_within_limit(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
) -> anyhow::Result<Result<(), String>> {
    let tx = conn.unchecked_transaction()?;
    if let Some(reason) = past_deadline(deadline) {
        anyhow::bail!("child not started: {reason}");
    }
    if let Some(reason) = parent_refuses_children(&tx, run.parent_run_id.as_deref())? {
        anyhow::bail!("child not started: {reason}");
    }
    let admitted = insert_run_within_limit(&tx, workflow, run)?;
    tx.commit()?;
    Ok(admitted)
}

/// Re-admission of a paused or interrupted run about to execute again: the
/// limit is counted without the run itself. Call it in the same `with_conn`
/// closure as the claim, like [`insert_run_within_limit`].
pub fn resume_within_limit(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> anyhow::Result<Result<(), String>> {
    let Some(max) = workflow.concurrency_limit else {
        return Ok(Ok(()));
    };
    let key = workflow
        .concurrency_key
        .is_some()
        .then_some(run.concurrency_key.as_deref());
    let project = project_bucket(workflow, run);
    let active =
        crate::db::workflows::count_admitted_runs(conn, &workflow.id, key, Some(&run.id), project)?;
    if active >= max {
        let blockers = describe_blockers(conn, workflow, key, Some(&run.id), project);
        return Ok(Err(format!(
            "Concurrency limit reached ({active}/{max}): another run of this workflow is active. Resume this run once it has finished.{blockers}"
        )));
    }
    Ok(Ok(()))
}

#[cfg(all(test, feature = "full-tests"))]
pub mod tests {
    use super::*;

    fn variable(name: &str, source: Option<PromptVariableSource>) -> PromptVariable {
        PromptVariable {
            name: name.into(),
            label: name.into(),
            placeholder: String::new(),
            description: None,
            required: true,
            pattern: None,
            source_ref: match source {
                Some(PromptVariableSource::ProjectEnv) => Some("<env.TOKEN>".into()),
                Some(PromptVariableSource::KronnContext) => Some("<context.project_id>".into()),
                _ => None,
            },
            source,
            allow_manual_override: false,
            control: None,
        }
    }

    #[test]
    fn a_key_reads_only_declared_user_inputs() {
        let vars = vec![
            variable("ticketKey", None),
            variable("token", Some(PromptVariableSource::ProjectEnv)),
            variable("project", Some(PromptVariableSource::KronnContext)),
        ];
        assert!(validate_key(Some("{{ticketKey}}"), Some(1), &vars).is_ok());
        assert!(validate_key(Some("pr-{{ticketKey ?? \"none\"}}"), Some(1), &vars).is_ok());
        assert!(validate_key(None, None, &vars).is_ok());

        let secret = validate_key(Some("{{token}}"), Some(1), &vars).unwrap_err();
        assert!(secret.contains("may be secret"), "{secret}");
        let context = validate_key(Some("{{project}}-{{ticketKey}}"), Some(1), &vars).unwrap_err();
        assert!(context.contains("`project`"), "{context}");
        let unknown = validate_key(Some("{{ticket}}"), Some(1), &vars).unwrap_err();
        assert!(unknown.contains("not a launch variable"), "{unknown}");
        let run_id = validate_key(Some("{{run.id}}"), Some(1), &vars).unwrap_err();
        assert!(run_id.contains("not a launch variable"), "{run_id}");
        let constant = validate_key(Some("same"), Some(1), &vars).unwrap_err();
        assert!(
            constant.contains("at least one launch variable"),
            "{constant}"
        );
        let no_limit = validate_key(Some("{{ticketKey}}"), None, &vars).unwrap_err();
        assert!(no_limit.contains("needs `concurrency_limit`"), "{no_limit}");
        assert!(validate_key(Some("{{ticketKey"), Some(1), &vars).is_err());
    }

    #[test]
    fn a_blank_optional_input_takes_the_key_fallback() {
        let mut optional = variable("depot", None);
        optional.required = false;
        let vars = vec![optional];
        let template = "fix-{{depot ?? \"front\"}}";
        for blank in ["", "  "] {
            let values = HashMap::from([("depot".to_string(), blank.to_string())]);
            assert_eq!(
                render_key(template, &vars, &values).unwrap().as_deref(),
                Some("fix-front")
            );
        }
        let values = HashMap::from([("depot".to_string(), "api".to_string())]);
        assert_eq!(
            render_key(template, &vars, &values).unwrap().as_deref(),
            Some("fix-api")
        );
    }

    #[test]
    fn a_key_renders_from_user_inputs_and_never_from_a_resolved_secret() {
        let vars = vec![
            variable("ticketKey", None),
            variable("token", Some(PromptVariableSource::ProjectEnv)),
        ];
        let values = HashMap::from([
            ("ticketKey".to_string(), "EW-1".to_string()),
            ("token".to_string(), "s3cr3t".to_string()),
        ]);
        assert_eq!(
            render_key("pr-{{ticketKey}}", &vars, &values).unwrap(),
            Some("pr-EW-1".into())
        );
        let leak = render_key("{{token}}", &vars, &values).unwrap_err();
        assert!(!leak.contains("s3cr3t"), "{leak}");

        let blank = HashMap::from([("ticketKey".to_string(), String::new())]);
        assert_eq!(render_key("{{ticketKey}}", &vars, &blank).unwrap(), None);
        let two_lines = HashMap::from([("ticketKey".to_string(), "a\nb".to_string())]);
        assert!(render_key("{{ticketKey}}", &vars, &two_lines).is_err());
        assert_eq!(normalize_key(Some("  ".into())), None);
    }

    fn run_for(id: &str, project: &str, status: crate::models::RunStatus) -> WorkflowRun {
        serde_json::from_value(serde_json::json!({
            "id": id, "workflow_id": "wf-scope", "status": status,
            "trigger_context": null, "step_results": [], "tokens_used": 0,
            "workspace_path": null, "started_at": "2026-01-01T00:00:00Z",
            "finished_at": null, "run_type": "linear", "batch_total": 0,
            "batch_completed": 0, "batch_failed": 0, "batch_no_response": 0,
            "batch_name": null, "parent_run_id": null, "state": {},
            "produced_branches": [], "concurrency_key": null,
            "project_id": project
        }))
        .unwrap()
    }

    /// A run parked on a provider quota, as KT-811 leaves it.
    fn quota_run(id: &str, parent: Option<&str>, wake_at: Option<&str>) -> WorkflowRun {
        let mut run = run_for(id, "p", crate::models::RunStatus::WaitingQuota);
        run.parent_run_id = parent.map(String::from);
        run.step_results = vec![serde_json::from_value(serde_json::json!({
            "step_name": "Agent", "status": "WaitingQuota", "output": "", "duration_ms": 0,
            "quota_wait": {
                "id": "w1", "reset_at": "2026-11-11T02:00:00Z",
                "wake_at": wake_at, "attempt": 1
            }
        }))
        .unwrap()];
        run
    }
}
