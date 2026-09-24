use super::*;
use crate::models::{QuickApi, QuickExec, WorkflowStep};
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
