use super::*;
use crate::models::{NotifyConfig, QuickApi, QuickExec, WorkflowStep};
use chrono::Utc;
use std::collections::HashMap;

fn api(
    headers: &[(&str, &str)],
    query: &[(&str, &str)],
    body: Option<serde_json::Value>,
) -> QuickApi {
    let map = |pairs: &[(&str, &str)]| {
        (!pairs.is_empty()).then(|| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>()
        })
    };
    QuickApi {
        id: "qa-1".into(),
        pinned: false,
        name: "Chartbeat".into(),
        icon: String::new(),
        description: String::new(),
        project_id: None,
        api_plugin_slug: "api-chartbeat".into(),
        api_config_id: "cfg-1".into(),
        api_endpoint_path: "/live".into(),
        api_method: Some("GET".into()),
        api_query: map(query),
        api_path_params: None,
        api_headers: map(headers),
        api_body: body,
        api_extract: None,
        api_pagination: None,
        api_timeout_ms: None,
        api_max_retries: None,
        variables: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn exec(args: &[&str]) -> QuickExec {
    QuickExec {
        approved_by_trust: None,
        id: "qe-1".into(),
        name: "Deploy".into(),
        icon: String::new(),
        description: String::new(),
        project_id: None,
        command: "deploy".into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        timeout_secs: 30,
        output_format: Default::default(),
        variables: vec![],
        pinned: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        unmodelled_args_approved: None,
        agent_written: None,
    }
}

#[test]
fn masks_credential_headers_and_keeps_ordinary_ones_and_references() {
    let mut qa = api(
        &[
            ("Authorization", "Bearer sk-live-1234567890abcdefghij"),
            ("Accept", "application/json"),
            ("X-Api-Key", "{{connection.key}}"),
        ],
        &[],
        None,
    );
    let mut found = vec![];
    redact_quick_api(&mut qa, &mut found);
    let headers = qa.api_headers.unwrap();
    assert_eq!(headers["Authorization"], REDACTED);
    assert_eq!(headers["Accept"], "application/json");
    assert_eq!(
        headers["X-Api-Key"], "{{connection.key}}",
        "a reference is not a secret"
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "api_headers.Authorization");
    assert!(
        !format!("{found:?}").contains("sk-live"),
        "the value never leaves"
    );
}

#[test]
fn a_reference_with_a_scheme_is_kept_but_a_literal_beside_a_reference_is_not() {
    let mut qa = api(
        &[
            ("Authorization", "Bearer {{connection.token}}"),
            (
                "Cookie",
                "session={{vars.session}}; sid=abcdef1234567890abcd",
            ),
        ],
        &[("api_key", "{{vars.key}}")],
        None,
    );
    let mut found = vec![];
    redact_quick_api(&mut qa, &mut found);
    let headers = qa.api_headers.unwrap();
    assert_eq!(headers["Authorization"], "Bearer {{connection.token}}");
    assert_eq!(
        headers["Cookie"], REDACTED,
        "a literal next to a placeholder still leaks"
    );
    assert_eq!(qa.api_query.unwrap()["api_key"], "{{vars.key}}");
    assert_eq!(found.len(), 1);
}

#[test]
fn a_literal_secret_in_the_command_itself_is_masked() {
    let mut qe = exec(&[]);
    qe.command = "TOKEN=sk-live-1234567890abcdefghij deploy".into();
    let mut found = vec![];
    redact_quick_exec(&mut qe, &mut found);
    assert_eq!(qe.command, REDACTED);
    assert_eq!(found[0].field, "command");
}

#[test]
fn masks_secret_query_values_and_body_leaves_by_their_key() {
    let mut qa = api(
        &[],
        &[("api_key", "0123456789abcdef0123"), ("limit", "10")],
        Some(serde_json::json!({"client_secret": "s3cr3t-value", "page": {"size": "20"}})),
    );
    let mut found = vec![];
    redact_quick_api(&mut qa, &mut found);
    assert_eq!(qa.api_query.as_ref().unwrap()["api_key"], REDACTED);
    assert_eq!(qa.api_query.as_ref().unwrap()["limit"], "10");
    let body = qa.api_body.unwrap();
    assert_eq!(body["client_secret"], REDACTED);
    assert_eq!(body["page"]["size"], "20");
    let mut fields: Vec<_> = found.into_iter().map(|f| f.field).collect();
    fields.sort();
    assert_eq!(fields, ["api_body.client_secret", "api_query.api_key"]);
}

#[test]
fn masks_the_value_after_a_secret_flag_and_inline_assignments() {
    let mut qe = exec(&[
        "--region",
        "eu-west-1",
        "--token",
        "abc123",
        "--password=hunter2",
        "--verbose",
    ]);
    let mut found = vec![];
    redact_quick_exec(&mut qe, &mut found);
    assert_eq!(
        qe.args,
        [
            "--region",
            "eu-west-1",
            "--token",
            REDACTED,
            REDACTED,
            "--verbose"
        ]
    );
    let fields: Vec<_> = found.into_iter().map(|f| f.field).collect();
    assert_eq!(fields, ["args.3", "args.4"]);
}

#[test]
fn masks_api_call_step_headers_in_workflows() {
    let mut step = WorkflowStep {
        name: "fetch".into(),
        api_headers: Some(HashMap::from([(
            "Authorization".to_string(),
            "Basic dXNlcjpwYXNz".to_string(),
        )])),
        ..WorkflowStep::default()
    };
    let mut found = vec![];
    redact_steps("wf-1", "Nightly", std::iter::once(&mut step), &mut found);
    assert_eq!(step.api_headers.unwrap()["Authorization"], REDACTED);
    assert_eq!(found[0].kind, "workflow_step");
    assert_eq!(found[0].name, "Nightly › fetch");
}

#[test]
fn masks_notify_and_gate_urls_in_workflows() {
    let mut step = WorkflowStep {
        name: "notify".into(),
        notify_config: Some(NotifyConfig {
            url: "https://hooks.example.test/services/private-token".into(),
            method: "POST".into(),
            headers: HashMap::from([("Authorization".into(), "Bearer literal-token".into())]),
            body_template: "done".into(),
        }),
        gate_notify_url: Some("https://hooks.example.test/gate/private-token".into()),
        ..WorkflowStep::default()
    };
    let mut found = vec![];
    redact_steps("wf-1", "Nightly", std::iter::once(&mut step), &mut found);
    let notify = step.notify_config.unwrap();
    assert_eq!(notify.url, REDACTED);
    assert_eq!(notify.headers["Authorization"], REDACTED);
    assert_eq!(step.gate_notify_url.as_deref(), Some(REDACTED));
    let fields = found
        .into_iter()
        .map(|field| field.field)
        .collect::<Vec<_>>();
    assert!(fields.contains(&"notify_config.url".to_string()));
    assert!(fields.contains(&"notify_config.headers.Authorization".to_string()));
    assert!(fields.contains(&"gate_notify_url".to_string()));
}

/// KT-1967 — an Exec step is a Quick Exec line by another name: its command,
/// arguments and stdin, the JSON a step carries and the variables it hands on
/// are masked like the fields of the other step types.
#[test]
fn masks_literal_secrets_in_every_hand_typed_field_of_a_step() {
    let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
    let mut step = WorkflowStep {
        name: "deploy".into(),
        exec_command: Some("deploy".into()),
        exec_args: vec!["--token".into(), "abc123".into(), "--env={{env}}".into()],
        exec_setup_command: Some("login".into()),
        exec_setup_args: vec!["--password=hunter2".into(), "--quiet".into()],
        exec_stdin: Some(format!("export KEY={key}")),
        json_data_payload: Some(serde_json::json!({
            "api_key": "literal-key", "region": "eu-west-1"
        })),
        quick_prompt_variables: HashMap::from([
            ("password".to_string(), "hunter2".to_string()),
            ("topic".to_string(), "{{steps.plan.output}}".to_string()),
        ]),
        sub_workflow_variables: HashMap::from([("token".to_string(), "abc123".to_string())]),
        notify_config: Some(NotifyConfig {
            url: "https://example.test/hook".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: format!("{{\"key\": \"{key}\"}}"),
        }),
        ..WorkflowStep::default()
    };
    let mut found = vec![];
    redact_steps("wf-1", "Nightly", std::iter::once(&mut step), &mut found);
    assert_eq!(step.exec_args, ["--token", REDACTED, "--env={{env}}"]);
    assert_eq!(step.exec_setup_args, [REDACTED, "--quiet"]);
    assert_eq!(step.exec_stdin.as_deref(), Some(REDACTED));
    let payload = step.json_data_payload.unwrap();
    assert_eq!(payload["api_key"], REDACTED);
    assert_eq!(payload["region"], "eu-west-1");
    assert_eq!(step.quick_prompt_variables["password"], REDACTED);
    assert_eq!(
        step.quick_prompt_variables["topic"],
        "{{steps.plan.output}}"
    );
    assert_eq!(step.sub_workflow_variables["token"], REDACTED);
    assert_eq!(step.notify_config.unwrap().body_template, REDACTED);
    // The commands themselves hold nothing secret here.
    assert_eq!(step.exec_command.as_deref(), Some("deploy"));
    assert_eq!(step.exec_setup_command.as_deref(), Some("login"));
    let mut fields: Vec<_> = found.into_iter().map(|field| field.field).collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "exec_args.1",
            "exec_setup_args.0",
            "exec_stdin",
            "json_data_payload.api_key",
            "notify_config.body_template",
            "notify_config.url",
            "quick_prompt_variables.password",
            "sub_workflow_variables.token",
        ]
    );

    let mut command = WorkflowStep {
        name: "inline".into(),
        exec_command: Some(key.into()),
        exec_setup_command: Some(format!("login {key}")),
        ..WorkflowStep::default()
    };
    let mut found = vec![];
    redact_steps("wf-1", "Nightly", std::iter::once(&mut command), &mut found);
    assert_eq!(command.exec_command.as_deref(), Some(REDACTED));
    assert_eq!(command.exec_setup_command.as_deref(), Some(REDACTED));
}

/// A collect source is an Exec line by another name, a transform fallback a
/// JSON literal, and a secret under a key or read on stdin has no shape.
#[test]
fn masks_secrets_no_shape_gives_away_and_the_sibling_fields_of_a_step() {
    use crate::models::{CollectApiDataConfig, TransformDataConfig};
    let source = serde_json::json!({
        "alias": "stats",
        "quick_exec": { "command": "curl", "args": ["--token", "abc123", "{{url}}"] },
        "variables": { "password": "hunter2", "page": "{{page}}" },
    });
    let mut step = WorkflowStep {
        name: "collect".into(),
        exec_command: Some("docker".into()),
        exec_args: vec!["login".into(), "--password-stdin".into()],
        exec_stdin: Some("hunter2".into()),
        notify_config: Some(NotifyConfig {
            url: "{{hook}}".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: r#"{"api_key": "abc123", "text": "done"}"#.into(),
        }),
        collect_api_data: Some(
            serde_json::from_value::<CollectApiDataConfig>(
                serde_json::json!({ "sources": [source] }),
            )
            .unwrap(),
        ),
        transform_data: Some(
            serde_json::from_value::<TransformDataConfig>(serde_json::json!({
                "input_from": "steps.collect.data",
                "fields": [{
                    "target": "t", "source": "$.a",
                    "fallback": {"token": "abc123", "count": 0},
                }],
            }))
            .unwrap(),
        ),
        ..WorkflowStep::default()
    };
    let mut found = vec![];
    redact_steps("wf-1", "Nightly", std::iter::once(&mut step), &mut found);
    assert_eq!(step.exec_stdin.as_deref(), Some(REDACTED));
    assert_eq!(step.notify_config.unwrap().body_template, REDACTED);
    let source = &step.collect_api_data.unwrap().sources[0];
    assert_eq!(
        source.quick_exec.as_ref().unwrap().args,
        ["--token", REDACTED, "{{url}}"]
    );
    assert_eq!(source.variables["password"], REDACTED);
    assert_eq!(source.variables["page"], "{{page}}");
    let fallback = step.transform_data.unwrap().fields[0]
        .fallback
        .clone()
        .unwrap();
    assert_eq!(fallback["token"], REDACTED);
    assert_eq!(fallback["count"], 0);

    // A body or a stdin that holds no secret stays as typed.
    let mut plain = WorkflowStep {
        name: "plain".into(),
        exec_args: vec!["--stdin".into()],
        exec_stdin: Some("hello".into()),
        notify_config: Some(NotifyConfig {
            url: "{{hook}}".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            body_template: r#"{"text": "done", "count": 2}"#.into(),
        }),
        ..WorkflowStep::default()
    };
    let mut found = vec![];
    redact_steps("wf-1", "Nightly", std::iter::once(&mut plain), &mut found);
    assert_eq!(plain.exec_stdin.as_deref(), Some("hello"));
    assert_eq!(
        plain.notify_config.unwrap().body_template,
        r#"{"text": "done", "count": 2}"#
    );
    // A notify URL is always masked: nothing else of this step is.
    let fields: Vec<_> = found.iter().map(|field| field.field.as_str()).collect();
    assert_eq!(fields, ["notify_config.url"]);
}

#[test]
fn an_export_without_literal_secrets_is_unchanged() {
    let mut qa = api(
        &[("Accept", "application/json")],
        &[("limit", "10")],
        Some(serde_json::json!({"q": "news"})),
    );
    let before = serde_json::to_value(&qa).unwrap();
    let mut qe = exec(&["--region", "eu-west-1"]);
    let mut found = vec![];
    redact_quick_api(&mut qa, &mut found);
    redact_quick_exec(&mut qe, &mut found);
    assert!(found.is_empty());
    assert_eq!(serde_json::to_value(&qa).unwrap(), before);
}

#[test]
fn masks_a_plain_token_query_parameter() {
    let mut qa = api(&[], &[("token", "synthetic-short-value")], None);
    let mut found = vec![];
    redact_quick_api(&mut qa, &mut found);
    assert_eq!(qa.api_query.as_ref().unwrap()["token"], REDACTED);
}

#[test]
fn masks_an_inline_token_flag_and_a_password_value_starting_with_a_hyphen() {
    let mut qe = exec(&[
        "--token=synthetic-short-value",
        "--password",
        "-synthetic-short-value",
        "API_KEY=abc",
    ]);
    let mut found = vec![];
    redact_quick_exec(&mut qe, &mut found);
    assert_eq!(qe.args, [REDACTED, "--password", REDACTED, REDACTED]);
}

#[test]
fn names_that_merely_contain_a_secret_word_stay_visible() {
    let mut qe = exec(&[
        "--max-tokens",
        "512",
        "--tokenizer",
        "cl100k",
        "--token={{vars.token}}",
        "--sort-key=date",
    ]);
    let mut found = vec![];
    redact_quick_exec(&mut qe, &mut found);
    assert!(found.is_empty(), "{found:?}");
    let body = serde_json::json!({"author": "Ana", "token_type": "Bearer"});
    let mut qa = api(
        &[],
        &[("max_tokens", "512"), ("sort_key", "date")],
        Some(body),
    );
    redact_quick_api(&mut qa, &mut found);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn secret_names_are_recognised_word_by_word_and_in_camel_case() {
    for name in [
        "token",
        "--auth-token",
        "apiKey",
        "X-Api-Key",
        "client_secret",
        "privateKey",
        "db_password",
    ] {
        assert!(secret_name(name), "{name}");
    }
    for name in [
        "max_tokens",
        "tokenizer",
        "sort_key",
        "author",
        "cacheKey",
        "--verbose",
    ] {
        assert!(!secret_name(name), "{name}");
    }
}
