use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use rusqlite::OptionalExtension;
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
    pub discussion_id: Option<&'a str>,
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
        let supplied_value = supplied
            .get(&variable.name)
            .filter(|v| !v.trim().is_empty());
        let resolved = if source != PromptVariableSource::UserInput
            && variable.allow_manual_override
            && supplied_value.is_some()
        {
            supplied_value
                .cloned()
                .map(|v| (v, "manual_override".to_string(), true))
        } else {
            match source {
                PromptVariableSource::UserInput => supplied_value
                    .cloned()
                    .map(|v| (v, "user_input".to_string(), false)),
                PromptVariableSource::KronnContext => variable
                    .source_ref
                    .as_deref()
                    .and_then(reference_name)
                    .and_then(|key| context.get(key))
                    .cloned()
                    .map(|v| (v, variable.source_ref.clone().unwrap_or_default(), false)),
                PromptVariableSource::ProjectEnv => variable
                    .source_ref
                    .as_deref()
                    .and_then(reference_name)
                    .and_then(|key| environment.get(key))
                    .and_then(|matches| (matches.len() == 1).then(|| matches[0].clone()))
                    .map(|(source_ref, value)| (value, source_ref, false)),
            }
        };
        match resolved {
            Some((value, effective_source_ref, overridden))
                if !variable.required || !value.trim().is_empty() =>
            {
                values.insert(variable.name.clone(), value);
                provenance.push(VariableProvenance {
                    name: variable.name.clone(),
                    source,
                    source_ref: variable.source_ref.clone(),
                    effective_source_ref,
                    overridden,
                });
            }
            _ if !variable.required => {}
            _ => {
                let cause = match source {
                    PromptVariableSource::ProjectEnv => variable
                        .source_ref
                        .as_deref()
                        .and_then(reference_name)
                        .and_then(|key| environment.get(key))
                        .map_or("missing_source", |m| {
                            if m.len() > 1 {
                                "ambiguous_source"
                            } else {
                                "empty_value"
                            }
                        }),
                    PromptVariableSource::KronnContext => "missing_context",
                    PromptVariableSource::UserInput => "missing_user_input",
                };
                failures.push(VariablePreflightFailure {
                    name: variable.name.clone(),
                    source_ref: variable.source_ref.clone(),
                    project_id: project_id.map(str::to_owned),
                    environment_ref: environment_ref.to_string(),
                    cause: cause.to_string(),
                });
            }
        }
    }
    // Unknown request keys are never propagated to execution.
    values.retain(|key, _| declared.contains(key.as_str()));
    if failures.is_empty() {
        Ok(ResolvedVariables {
            values,
            provenance,
            resolved_at: Utc::now(),
        })
    } else {
        Err(failures)
    }
}

pub fn expiry(resolved_at: DateTime<Utc>, retention_days: u32) -> Option<DateTime<Utc>> {
    (retention_days > 0).then(|| resolved_at + Duration::days(i64::from(retention_days)))
}

/// Resolve the product retention precedence without reading secret material.
/// A discussion override, including zero, wins over the global setting.
pub fn effective_retention_days(
    conn: &rusqlite::Connection,
    discussion_id: Option<&str>,
    global_days: u32,
) -> anyhow::Result<u32> {
    let Some(discussion_id) = discussion_id else {
        return Ok(global_days);
    };
    let value = conn
        .query_row(
            "SELECT execution_variable_retention_days FROM discussions WHERE id=?1",
            [discussion_id],
            |row| row.get::<_, Option<u32>>(0),
        )
        .optional()?;
    Ok(value.flatten().unwrap_or(global_days))
}

/// Build the allowlisted scalar context available to `<context.NAME>`
/// declarations. Nested objects and arrays are deliberately excluded.
pub fn scalar_context(value: &serde_json::Value) -> HashMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter_map(|(name, value)| match value {
            serde_json::Value::String(value) => Some((name.clone(), value.clone())),
            serde_json::Value::Number(value) => Some((name.clone(), value.to_string())),
            serde_json::Value::Bool(value) => Some((name.clone(), value.to_string())),
            _ => None,
        })
        .collect()
}

/// Load only encrypted configuration variables authorized for the selected
/// project, resolve every declaration as one preflight, and persist one
/// run-scoped encrypted snapshot before the caller performs side effects.
pub fn prepare(
    conn: &rusqlite::Connection,
    request: PrepareRequest<'_>,
) -> anyhow::Result<Result<PreparedExecutionVariables, Vec<VariablePreflightFailure>>> {
    let key =
        crate::core::crypto::parse_secret(request.encryption_secret).map_err(anyhow::Error::msg)?;
    if let Some(snapshot_id) = crate::db::execution_variable_snapshots::snapshot_id_for_run(
        conn,
        request.run_kind,
        request.run_id,
    )? {
        let metadata = crate::db::execution_variable_snapshots::metadata(
            conn,
            request.run_kind,
            request.run_id,
        )?
        .ok_or_else(|| anyhow::anyhow!("execution variable snapshot metadata missing"))?;
        let values = crate::db::execution_variable_snapshots::load_values(
            conn,
            request.run_kind,
            request.run_id,
            &key,
            Utc::now(),
        )?
        .ok_or_else(|| anyhow::anyhow!("execution variable snapshot unavailable or expired"))?;
        return Ok(Ok(PreparedExecutionVariables {
            resolved: ResolvedVariables {
                values,
                provenance: metadata.provenance,
                resolved_at: metadata.resolved_at,
            },
            snapshot_id,
        }));
    }
    let mut environment: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if let Some(project_id) = request.project_id {
        for config in crate::db::mcps::configs_for_project(conn, project_id)? {
            let values = match crate::db::mcps::decrypt_env(
                &config.env_encrypted,
                request.encryption_secret,
            ) {
                Ok(values) => values,
                Err(_) => continue,
            };
            for (name, value) in values {
                environment
                    .entry(name.clone())
                    .or_default()
                    .push((format!("mcp_config:{}:<env.{name}>", config.id), value));
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
    let retention_days =
        effective_retention_days(conn, request.discussion_id, request.retention_days)?;
    let snapshot_id = crate::db::execution_variable_snapshots::insert(
        conn,
        crate::db::execution_variable_snapshots::NewSnapshot {
            run_kind: request.run_kind,
            run_id: request.run_id,
            project_id: request.project_id,
            environment_ref: request.environment_ref,
            resolved_at: resolved.resolved_at,
            retention_days,
            expires_at: expiry(resolved.resolved_at, retention_days),
            values: &resolved.values,
            provenance: &resolved.provenance,
        },
        &key,
    )?;
    Ok(Ok(PreparedExecutionVariables {
        resolved,
        snapshot_id,
    }))
}

fn reference_name(reference: &str) -> Option<&str> {
    reference
        .strip_prefix('<')?
        .strip_suffix('>')?
        .split_once('.')
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_var() -> PromptVariable {
        PromptVariable {
            name: "token".into(),
            label: "Token".into(),
            placeholder: "".into(),
            description: None,
            required: true,
            pattern: None,
            source: Some(PromptVariableSource::ProjectEnv),
            source_ref: Some("<env.TOKEN>".into()),
            allow_manual_override: false,
        }
    }

    #[test]
    fn absent_project_and_missing_source_group_every_preflight_failure() {
        // DoD 9: an absent project / missing environment source must fail the
        // preflight for every required reference at once, so the user fixes all
        // of them before a single relaunch — never dispatch on a partial set.
        let mut second = env_var();
        second.name = "secret".into();
        second.label = "Secret".into();
        second.source_ref = Some("<env.SECRET>".into());
        let declarations = vec![env_var(), second];
        let supplied = HashMap::new();
        let context = HashMap::new();
        // No project / empty environment: nothing resolves.
        let failures = resolve(
            &declarations,
            &supplied,
            &context,
            &HashMap::new(),
            None,
            "project",
        )
        .unwrap_err();
        assert_eq!(failures.len(), 2);
        assert!(failures.iter().all(|f| f.cause == "missing_source"));
        assert!(failures.iter().all(|f| f.project_id.is_none()));
    }

    #[test]
    fn resolves_current_value_and_rejects_ambiguous_sources() {
        let supplied = HashMap::new();
        let context = HashMap::new();
        let first = HashMap::from([("TOKEN".into(), vec![("mcp:a".into(), "one".into())])]);
        assert_eq!(
            resolve(
                &[env_var()],
                &supplied,
                &context,
                &first,
                Some("p"),
                "project"
            )
            .unwrap()
            .values["token"],
            "one"
        );
        let second = HashMap::from([("TOKEN".into(), vec![("mcp:a".into(), "two".into())])]);
        assert_eq!(
            resolve(
                &[env_var()],
                &supplied,
                &context,
                &second,
                Some("p"),
                "project"
            )
            .unwrap()
            .values["token"],
            "two"
        );
        let ambiguous = HashMap::from([(
            "TOKEN".into(),
            vec![
                ("mcp:a".into(), "one".into()),
                ("mcp:b".into(), "two".into()),
            ],
        )]);
        assert_eq!(
            resolve(
                &[env_var()],
                &supplied,
                &context,
                &ambiguous,
                Some("p"),
                "project"
            )
            .unwrap_err()[0]
                .cause,
            "ambiguous_source"
        );
    }

    #[test]
    fn every_launcher_resolves_fresh_project_values_after_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("variables.db");
        let secret = crate::core::crypto::generate_secret();
        let declarations = vec![env_var()];
        let supplied = HashMap::new();
        let context = HashMap::new();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('p','Project','/tmp/project',?1,?1)",
            [&now],
        )
        .unwrap();
        crate::db::mcps::upsert_server(
            &conn,
            &crate::models::McpServer {
                id: "server".into(),
                name: "Server".into(),
                description: String::new(),
                transport: crate::models::McpTransport::Stdio {
                    command: "server".into(),
                    args: vec![],
                },
                source: crate::models::McpSource::Registry,
                api_spec: None,
            },
        )
        .unwrap();
        let initial_env = HashMap::from([("TOKEN".to_string(), "fresh-one".to_string())]);
        crate::db::mcps::insert_config(
            &conn,
            &crate::models::McpConfig {
                id: "config".into(),
                server_id: "server".into(),
                label: "Project variables".into(),
                env_keys: vec!["TOKEN".into()],
                env_encrypted: crate::db::mcps::encrypt_env(&initial_env, &secret).unwrap(),
                args_override: None,
                is_global: false,
                include_general: false,
                config_hash: "initial".into(),
                project_ids: vec!["p".into()],
                host_sync: crate::models::HostSyncMode::None,
            },
        )
        .unwrap();

        for (index, run_kind) in ["quick_prompt", "quick_api", "quick_exec", "workflow"]
            .into_iter()
            .enumerate()
        {
            let run_id = format!("first-{index}");
            let prepared = prepare(
                &conn,
                PrepareRequest {
                    declarations: &declarations,
                    supplied: &supplied,
                    context: &context,
                    project_id: Some("p"),
                    discussion_id: None,
                    environment_ref: "project_mcp_configs",
                    run_kind,
                    run_id: &run_id,
                    encryption_secret: &secret,
                    retention_days: 30,
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(prepared.resolved.values["token"], "fresh-one");
        }

        let changed_env = HashMap::from([("TOKEN".to_string(), "fresh-two".to_string())]);
        let changed_encrypted = crate::db::mcps::encrypt_env(&changed_env, &secret).unwrap();
        assert!(crate::db::mcps::update_config(
            &conn,
            "config",
            None,
            Some(&changed_encrypted),
            None,
            None,
            None,
            Some("changed"),
            None,
            None,
            None,
        )
        .unwrap());
        drop(conn);

        // Reopening the database models a Kronn restart. New executions must
        // consult the current encrypted project source instead of reusing any
        // value from a prior run or from template state.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        for (index, run_kind) in ["quick_prompt", "quick_api", "quick_exec", "workflow"]
            .into_iter()
            .enumerate()
        {
            let run_id = format!("second-{index}");
            let prepared = prepare(
                &conn,
                PrepareRequest {
                    declarations: &declarations,
                    supplied: &supplied,
                    context: &context,
                    project_id: Some("p"),
                    discussion_id: None,
                    environment_ref: "project_mcp_configs",
                    run_kind,
                    run_id: &run_id,
                    encryption_secret: &secret,
                    retention_days: 30,
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(prepared.resolved.values["token"], "fresh-two");
        }

        let encrypted_payloads: Vec<String> = conn
            .prepare("SELECT values_encrypted FROM execution_variable_snapshots")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(encrypted_payloads.len(), 8);
        assert!(encrypted_payloads
            .iter()
            .all(|payload| !payload.contains("fresh-one") && !payload.contains("fresh-two")));
    }

    #[test]
    fn override_is_explicit_and_zero_retention_has_no_expiry() {
        let mut declaration = env_var();
        declaration.allow_manual_override = true;
        let resolved = resolve(
            &[declaration],
            &HashMap::from([("token".into(), "override".into())]),
            &HashMap::new(),
            &HashMap::new(),
            Some("p"),
            "project",
        )
        .unwrap();
        assert!(resolved.provenance[0].overridden);
        assert!(expiry(resolved.resolved_at, 0).is_none());
        assert_eq!(
            expiry(resolved.resolved_at, 30).unwrap(),
            resolved.resolved_at + Duration::days(30)
        );

        let mut context_declaration = env_var();
        context_declaration.source = Some(PromptVariableSource::KronnContext);
        context_declaration.source_ref = Some("<context.TOKEN>".into());
        context_declaration.allow_manual_override = true;
        let context_override = resolve(
            &[context_declaration],
            &HashMap::from([("token".into(), "context-override".into())]),
            &HashMap::from([("TOKEN".into(), "context-default".into())]),
            &HashMap::new(),
            Some("p"),
            "project",
        )
        .unwrap();
        assert_eq!(context_override.values["token"], "context-override");
        assert!(context_override.provenance[0].overridden);
    }

    #[test]
    fn prepare_reuses_the_immutable_snapshot_for_the_same_run() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let secret = crate::core::crypto::generate_secret();
        let mut declaration = env_var();
        declaration.source = Some(PromptVariableSource::UserInput);
        declaration.source_ref = None;
        let declarations = vec![declaration];
        let first = HashMap::from([("token".into(), "first".into())]);
        let second = HashMap::from([("token".into(), "second".into())]);
        let context = HashMap::new();
        let initial = prepare(
            &conn,
            PrepareRequest {
                declarations: &declarations,
                supplied: &first,
                context: &context,
                project_id: None,
                discussion_id: None,
                environment_ref: "project_mcp_configs",
                run_kind: "workflow",
                run_id: "stable-run",
                encryption_secret: &secret,
                retention_days: 30,
            },
        )
        .unwrap()
        .unwrap();
        let resumed = prepare(
            &conn,
            PrepareRequest {
                declarations: &declarations,
                supplied: &second,
                context: &context,
                project_id: None,
                discussion_id: None,
                environment_ref: "project_mcp_configs",
                run_kind: "workflow",
                run_id: "stable-run",
                encryption_secret: &secret,
                retention_days: 30,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(initial.snapshot_id, resumed.snapshot_id);
        assert_eq!(resumed.resolved.values["token"], "first");
    }

    #[test]
    fn discussion_retention_override_wins_including_zero() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id,title,agent,language,created_at,updated_at,execution_variable_retention_days) VALUES ('d','d','Codex','en',?1,?1,0)",
            [&now],
        )
        .unwrap();
        assert_eq!(effective_retention_days(&conn, Some("d"), 30).unwrap(), 0);
        assert_eq!(effective_retention_days(&conn, None, 30).unwrap(), 30);
    }

    #[test]
    fn new_run_picks_up_a_changed_project_env_value_while_same_run_stays_deterministic() {
        // DoD 2/9: a new execution resolves the *current* project environment
        // value through the real authorized loader; a technical resume of the
        // same run reuses its immutable snapshot even after the environment
        // changes underneath, and only a genuinely new run refreshes it — all
        // without editing the declaration template.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let secret = crate::core::crypto::generate_secret();

        let server = crate::models::McpServer {
            id: "srv".into(),
            name: "S".into(),
            description: String::new(),
            transport: crate::models::McpTransport::Stdio {
                command: "test".into(),
                args: vec![],
            },
            source: crate::models::McpSource::Registry,
            api_spec: None,
        };
        crate::db::mcps::upsert_server(&conn, &server).unwrap();

        let env_v1 = HashMap::from([("TOKEN".to_string(), "v1".to_string())]);
        let config = crate::models::McpConfig {
            id: "cfg".into(),
            server_id: "srv".into(),
            label: "Global".into(),
            env_keys: vec!["TOKEN".into()],
            env_encrypted: crate::db::mcps::encrypt_env(&env_v1, &secret).unwrap(),
            args_override: None,
            is_global: true,
            include_general: true,
            config_hash: "h".into(),
            project_ids: vec![],
            host_sync: crate::models::HostSyncMode::None,
        };
        crate::db::mcps::insert_config(&conn, &config).unwrap();

        let declarations = vec![env_var()]; // ProjectEnv <env.TOKEN>, required.
        let supplied = HashMap::new();
        let context = HashMap::new();

        let first = prepare(
            &conn,
            PrepareRequest {
                declarations: &declarations,
                supplied: &supplied,
                context: &context,
                project_id: Some("p"),
                discussion_id: None,
                environment_ref: "project_mcp_configs",
                run_kind: "workflow",
                run_id: "run-A",
                encryption_secret: &secret,
                retention_days: 30,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(first.resolved.values["token"], "v1");

        // Environment changes underneath after the first run resolved.
        let env_v2 = HashMap::from([("TOKEN".to_string(), "v2".to_string())]);
        conn.execute(
            "UPDATE mcp_configs SET env_encrypted=?1 WHERE id='cfg'",
            [crate::db::mcps::encrypt_env(&env_v2, &secret).unwrap()],
        )
        .unwrap();

        // Same run resumes deterministically on its immutable snapshot.
        let resumed = prepare(
            &conn,
            PrepareRequest {
                declarations: &declarations,
                supplied: &supplied,
                context: &context,
                project_id: Some("p"),
                discussion_id: None,
                environment_ref: "project_mcp_configs",
                run_kind: "workflow",
                run_id: "run-A",
                encryption_secret: &secret,
                retention_days: 30,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(resumed.snapshot_id, first.snapshot_id);
        assert_eq!(resumed.resolved.values["token"], "v1");

        // A genuinely new run resolves the current value without template edits.
        let second = prepare(
            &conn,
            PrepareRequest {
                declarations: &declarations,
                supplied: &supplied,
                context: &context,
                project_id: Some("p"),
                discussion_id: None,
                environment_ref: "project_mcp_configs",
                run_kind: "workflow",
                run_id: "run-B",
                encryption_secret: &secret,
                retention_days: 30,
            },
        )
        .unwrap()
        .unwrap();
        assert_ne!(second.snapshot_id, first.snapshot_id);
        assert_eq!(second.resolved.values["token"], "v2");
    }
}
