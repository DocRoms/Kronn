use super::*;
use crate::models::{WorkflowSafety, WorkflowTrigger};
use std::collections::HashMap;

fn workflow(steps: Vec<WorkflowStep>) -> Workflow {
    Workflow {
        refs: Default::default(),
        project_scope: None,
        pinned: false,
        id: "wf".into(),
        name: "PR review".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps,
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        concurrency_key: None,
        guards: None,
        artifacts: HashMap::new(),
        on_failure: vec![],
        exec_allowlist: vec!["python3".into(), "gh".into(), "curl".into(), "bash".into()],
        variables: vec![],
        enabled: false,
        retention: None,
        shadow: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn exec(name: &str, command: &str, args: &[&str]) -> WorkflowStep {
    WorkflowStep {
        name: name.into(),
        step_type: StepType::Exec,
        exec_command: Some(command.into()),
        exec_args: args.iter().map(|a| a.to_string()).collect(),
        ..Default::default()
    }
}

fn github() -> ConfiguredApi {
    ConfiguredApi::from_base_url("GitHub", "https://api.github.com").unwrap()
}

/// The shape observed on the PR review chain: one long script, a mode argument.
fn big_script() -> String {
    format!("import sys\n{}\nprint(sys.argv[1])", "x = 1\n".repeat(80))
}

#[test]
fn a_script_copied_into_several_exec_steps_is_one_warning() {
    let script = big_script();
    let steps = ["collect-files", "collect-reviews", "collect-comments"]
        .iter()
        .map(|name| exec(name, "python3", &["-c", &script, "_", name]))
        .collect();
    let found = warnings(&workflow(steps), &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, WorkflowWarningKind::DuplicatedExecScript);
    assert_eq!(
        found[0].steps,
        vec!["collect-files", "collect-reviews", "collect-comments"]
    );
    assert!(found[0].message.contains(&script.len().to_string()));
    assert!(found[0].action.contains("exec_script_files"));
    assert!(found[0].action.contains("authoring_doctrine"));
}

#[test]
fn the_same_long_exec_args_copied_whole_are_flagged_too() {
    let long = "a".repeat(120);
    let args = ["run", long.as_str(), long.as_str()];
    let found = warnings(
        &workflow(vec![exec("one", "make", &args), exec("two", "make", &args)]),
        &[],
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].steps, vec!["one", "two"]);
}

#[test]
fn short_repeated_commands_and_distinct_scripts_are_not_drift() {
    let one = big_script();
    let two = format!("{one}\nprint('other')");
    let found = warnings(
        &workflow(vec![
            exec("test-a", "cargo", &["test"]),
            exec("test-b", "cargo", &["test"]),
            exec("a", "python3", &["-c", &one]),
            exec("b", "python3", &["-c", &two]),
        ]),
        &[],
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn gh_and_curl_toward_a_service_with_a_configured_plugin_are_flagged() {
    let steps = vec![
        exec("files", "gh", &["api", "repos/o/r/pulls/1/files"]),
        exec(
            "inline",
            "python3",
            &[
                "-c",
                "import subprocess; subprocess.run(['gh', 'api', 'graphql'])",
            ],
        ),
        exec("curl", "curl", &["-s", "https://api.github.com/repos/o/r"]),
        exec("unrelated", "curl", &["-s", "https://example.org/x"]),
        exec("tests", "cargo", &["test"]),
    ];
    let found = warnings(&workflow(steps), &[github()]);
    let flagged: Vec<&str> = found
        .iter()
        .map(|w| {
            assert_eq!(w.kind, WorkflowWarningKind::ExecBypassesPlugin);
            w.steps[0].as_str()
        })
        .collect();
    assert_eq!(flagged, vec!["files", "inline", "curl"]);
    assert!(found[0].message.contains("GitHub"));
    assert!(found[0].action.contains("ApiCall"));
}

#[test]
fn without_a_configured_plugin_there_is_nothing_to_bypass() {
    let steps = vec![exec("files", "gh", &["api", "repos/o/r/pulls/1/files"])];
    assert!(warnings(&workflow(steps.clone()), &[]).is_empty());
    let jira = ConfiguredApi::from_base_url("Jira", "https://acme.atlassian.net").unwrap();
    assert!(warnings(&workflow(steps), &[jira]).is_empty());
}

#[test]
fn a_plugin_host_with_a_placeholder_still_matches() {
    let api = ConfiguredApi::from_base_url("Jira", "https://${ENV.JIRA_DOMAIN}.atlassian.net/rest")
        .unwrap();
    assert_eq!(api.host, "*.atlassian.net");
    assert!(api.serves("acme.atlassian.net"));
    assert!(!api.serves("atlassian.net.evil.example"));
    assert!(!github().serves("github.com"));
    assert!(github().serves("API.github.com"));
}

#[test]
fn warnings_never_block_and_the_summary_names_them() {
    let script = big_script();
    let steps = vec![
        exec("a", "python3", &["-c", &script, "_", "a"]),
        exec("b", "python3", &["-c", &script, "_", "b"]),
    ];
    let wf = workflow(steps);
    let verdict = crate::workflows::readiness::assess_walk(
        &wf,
        &HashMap::new(),
        &HashMap::new(),
        &[github()],
        &mut |_| Ok(None),
    );
    assert!(verdict.ready, "{verdict:?}");
    assert!(verdict.blockers.is_empty());
    assert_eq!(verdict.warnings.len(), 1);
    assert!(verdict.summary.starts_with("READY"));
    assert!(verdict
        .summary
        .contains("1 design warning(s), not blockers"));
    let json = serde_json::to_value(&verdict).unwrap();
    assert_eq!(json["warnings"][0]["kind"], "duplicated_exec_script");

    // A clean workflow serialises no `warnings` key at all.
    let clean = crate::workflows::readiness::assess(
        &workflow(vec![exec("t", "cargo", &["test"])]),
        &HashMap::new(),
    );
    assert!(clean.warnings.is_empty());
    assert!(serde_json::to_value(&clean)
        .unwrap()
        .get("warnings")
        .is_none());
    assert!(!clean.summary.contains("design warning"));
}
