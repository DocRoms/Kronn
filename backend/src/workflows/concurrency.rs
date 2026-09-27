//! Workflow run admission: `concurrency_limit`, counted per workflow or, when
//! the workflow declares a `concurrency_key`, per key rendered at launch.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::models::{PromptVariable, PromptVariableSource, Workflow, WorkflowRun};

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
    for variable in variables.iter().filter(|variable| is_user_input(variable)) {
        if let Some(value) = values.get(&variable.name) {
            ctx.set(variable.name.clone(), value.clone());
        }
    }
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

/// Inserts `run` unless the workflow's limit is already reached for it. Call it
/// inside a single `with_conn` closure: on the shared connection the count and
/// the insert are then atomic, so two launches cannot both take the last slot.
pub fn insert_run_within_limit(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> anyhow::Result<Result<(), String>> {
    if let Some(max) = workflow.concurrency_limit {
        if workflow.concurrency_key.is_some() {
            let key = run.concurrency_key.as_deref();
            let active = crate::db::workflows::count_active_runs_for_key(conn, &workflow.id, key)?;
            if active >= max {
                let label = key.map_or_else(|| "an empty key".to_string(), |k| format!("`{k}`"));
                return Ok(Err(format!(
                    "Concurrency limit reached for key {label} ({active}/{max}): a run with this key is already active."
                )));
            }
        } else {
            let active = crate::db::workflows::count_active_runs(conn, &workflow.id)?;
            if active >= max {
                return Ok(Err(format!("Concurrency limit reached ({active}/{max})")));
            }
        }
    }
    crate::db::workflows::insert_run(conn, run)?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
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
}
