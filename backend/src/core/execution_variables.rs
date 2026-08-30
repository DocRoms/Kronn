use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{PromptVariable, PromptVariableSource};

pub const DEFAULT_RETENTION_DAYS: u32 = 30;

pub struct PreparedExecutionVariables {
    pub resolved: ResolvedVariables,
    pub snapshot_id: String,
}

pub struct PrepareRequest<'a> {
    pub declarations: &'a [PromptVariable],
    pub supplied: &'a HashMap<String, String>,
    pub context: &'a HashMap<String, String>,
    pub project_id: Option<&'a str>,
    pub environment_ref: &'a str,
    pub run_kind: &'a str,
    pub run_id: &'a str,
    pub encryption_secret: &'a str,
    pub retention_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VariableProvenance {
    pub name: String,
    pub source: PromptVariableSource,
    pub source_ref: Option<String>,
    pub effective_source_ref: String,
    pub overridden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VariablePreflightFailure {
    pub name: String,
    pub source_ref: Option<String>,
    pub project_id: Option<String>,
    pub environment_ref: String,
    pub cause: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedVariables {
    pub values: HashMap<String, String>,
    pub provenance: Vec<VariableProvenance>,
    pub resolved_at: DateTime<Utc>,
}

pub fn resolve(
    declarations: &[PromptVariable],
    supplied: &HashMap<String, String>,
    context: &HashMap<String, String>,
    environment: &HashMap<String, Vec<(String, String)>>,
    project_id: Option<&str>,
    environment_ref: &str,
) -> Result<ResolvedVariables, Vec<VariablePreflightFailure>> {
    let mut values = HashMap::new();
    let mut provenance = Vec::new();
    let mut failures = Vec::new();
    let declared: HashSet<&str> = declarations.iter().map(|v| v.name.as_str()).collect();

    for variable in declarations {
        let source = variable.source.clone().unwrap_or_default();
        let supplied_value = supplied.get(&variable.name).filter(|v| !v.trim().is_empty());
        let resolved = match source {
            PromptVariableSource::UserInput => supplied_value
                .cloned()
                .map(|v| (v, "user_input".to_string(), false)),
            PromptVariableSource::KronnContext => variable.source_ref.as_deref()
                .and_then(reference_name)
                .and_then(|key| context.get(key))
                .cloned()
                .map(|v| (v, variable.source_ref.clone().unwrap_or_default(), false)),
            PromptVariableSource::ProjectEnv if variable.allow_manual_override && supplied_value.is_some() => {
                supplied_value.cloned().map(|v| (v, "manual_override".to_string(), true))
            }
            PromptVariableSource::ProjectEnv => variable.source_ref.as_deref()
                .and_then(reference_name)
                .and_then(|key| environment.get(key))
                .and_then(|matches| (matches.len() == 1).then(|| matches[0].clone()))
                .map(|(source_ref, value)| (value, source_ref, false)),
        };
        match resolved {
            Some((value, effective_source_ref, overridden)) if !variable.required || !value.trim().is_empty() => {
                values.insert(variable.name.clone(), value);
                provenance.push(VariableProvenance { name: variable.name.clone(), source, source_ref: variable.source_ref.clone(), effective_source_ref, overridden });
            }
            _ if !variable.required => {}
            _ => {
                let cause = match source {
                    PromptVariableSource::ProjectEnv => variable.source_ref.as_deref().and_then(reference_name)
                        .and_then(|key| environment.get(key)).map_or("missing_source", |m| if m.len() > 1 { "ambiguous_source" } else { "empty_value" }),
                    PromptVariableSource::KronnContext => "missing_context",
                    PromptVariableSource::UserInput => "missing_user_input",
                };
                failures.push(VariablePreflightFailure { name: variable.name.clone(), source_ref: variable.source_ref.clone(), project_id: project_id.map(str::to_owned), environment_ref: environment_ref.to_string(), cause: cause.to_string() });
            }
        }
    }
    // Unknown request keys are never propagated to execution.
    values.retain(|key, _| declared.contains(key.as_str()));
    if failures.is_empty() { Ok(ResolvedVariables { values, provenance, resolved_at: Utc::now() }) } else { Err(failures) }
}

pub fn expiry(resolved_at: DateTime<Utc>, retention_days: u32) -> Option<DateTime<Utc>> {
    (retention_days > 0).then(|| resolved_at + Duration::days(i64::from(retention_days)))
}

/// Load only encrypted configuration variables authorized for the selected
/// project, resolve every declaration as one preflight, and persist one
/// run-scoped encrypted snapshot before the caller performs side effects.
pub fn prepare(
    conn: &rusqlite::Connection,
    request: PrepareRequest<'_>,
) -> anyhow::Result<Result<PreparedExecutionVariables, Vec<VariablePreflightFailure>>> {
    let mut environment: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if let Some(project_id) = request.project_id {
        for config in crate::db::mcps::configs_for_project(conn, project_id)? {
            let values = match crate::db::mcps::decrypt_env(&config.env_encrypted, request.encryption_secret) {
                Ok(values) => values,
                Err(_) => continue,
            };
            for (name, value) in values {
                environment.entry(name.clone()).or_default().push((
                    format!("mcp_config:{}:<env.{name}>", config.id),
                    value,
                ));
            }
        }
    }
    let resolved = match resolve(
        request.declarations,
        request.supplied,
        request.context,
        &environment,
        request.project_id,
        request.environment_ref,
    ) {
        Ok(resolved) => resolved,
        Err(failures) => return Ok(Err(failures)),
    };
    let key = crate::core::crypto::parse_secret(request.encryption_secret)
        .map_err(anyhow::Error::msg)?;
    let snapshot_id = crate::db::execution_variable_snapshots::insert(
        conn,
        crate::db::execution_variable_snapshots::NewSnapshot {
            run_kind: request.run_kind,
            run_id: request.run_id,
            project_id: request.project_id,
            environment_ref: request.environment_ref,
            resolved_at: resolved.resolved_at,
            expires_at: expiry(resolved.resolved_at, request.retention_days),
            values: &resolved.values,
            provenance: &resolved.provenance,
        },
        &key,
    )?;
    Ok(Ok(PreparedExecutionVariables { resolved, snapshot_id }))
}

fn reference_name(reference: &str) -> Option<&str> {
    reference.strip_prefix('<')?.strip_suffix('>')?.split_once('.').map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_var() -> PromptVariable { PromptVariable { name: "token".into(), label: "Token".into(), placeholder: "".into(), description: None, required: true, pattern: None, source: Some(PromptVariableSource::ProjectEnv), source_ref: Some("<env.TOKEN>".into()), allow_manual_override: false } }

    #[test]
    fn resolves_current_value_and_rejects_ambiguous_sources() {
        let supplied = HashMap::new(); let context = HashMap::new();
        let first = HashMap::from([("TOKEN".into(), vec![("mcp:a".into(), "one".into())])]);
        assert_eq!(resolve(&[env_var()], &supplied, &context, &first, Some("p"), "project").unwrap().values["token"], "one");
        let second = HashMap::from([("TOKEN".into(), vec![("mcp:a".into(), "two".into())])]);
        assert_eq!(resolve(&[env_var()], &supplied, &context, &second, Some("p"), "project").unwrap().values["token"], "two");
        let ambiguous = HashMap::from([("TOKEN".into(), vec![("mcp:a".into(), "one".into()), ("mcp:b".into(), "two".into())])]);
        assert_eq!(resolve(&[env_var()], &supplied, &context, &ambiguous, Some("p"), "project").unwrap_err()[0].cause, "ambiguous_source");
    }

    #[test]
    fn override_is_explicit_and_zero_retention_has_no_expiry() {
        let mut declaration = env_var(); declaration.allow_manual_override = true;
        let resolved = resolve(&[declaration], &HashMap::from([("token".into(), "override".into())]), &HashMap::new(), &HashMap::new(), Some("p"), "project").unwrap();
        assert!(resolved.provenance[0].overridden);
        assert!(expiry(resolved.resolved_at, 0).is_none());
        assert_eq!(expiry(resolved.resolved_at, 30).unwrap(), resolved.resolved_at + Duration::days(30));
    }
}
