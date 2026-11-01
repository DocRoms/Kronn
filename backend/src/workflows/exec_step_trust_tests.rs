//! KT-1492 — a line a trust level approved runs under the level's sandbox (the
//! Kronn data directory closed to it); with no sandbox on the host, or the vault
//! protection turned off, it runs as it would for a human.

use super::*;
use crate::core::task_worker_sandbox::Mechanism;
use crate::core::trust_sandbox::set_test_unavailable;
use serial_test::serial;

fn step(command: &str, args: &[&str], by_trust: bool) -> WorkflowStep {
    WorkflowStep {
        name: "list".into(),
        step_type: StepType::Exec,
        exec_command: Some(command.into()),
        exec_args: args.iter().map(|arg| arg.to_string()).collect(),
        exec_unmodelled_args_approved: by_trust.then_some(true),
        exec_approved_by_trust: by_trust.then_some(true),
        ..Default::default()
    }
}

#[test]
#[serial]
fn only_a_step_a_level_approved_is_wrapped_in_the_sandbox() {
    let args = vec!["s3".to_string(), "ls".to_string()];
    let (program, launch) = confined_for_level(&step("aws", &["s3", "ls"], false), "aws", &args);
    assert_eq!((program.as_str(), &launch), ("aws", &args));

    let (program, launch) = confined_for_level(&step("aws", &["s3", "ls"], true), "aws", &args);
    assert_eq!(program, Mechanism::of_host().unwrap().launcher());
    assert_eq!(&launch[launch.len() - 3..], ["aws", "s3", "ls"]);
    assert!(launch.len() > 3, "the sandbox's own arguments come first");
}

fn run_touch(marker: &std::path::Path, dir: &std::path::Path) -> StepOutcome {
    let step = step("touch", &[marker.to_str().unwrap()], true);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_exec_step(
            &step,
            &["touch".to_string()],
            dir.to_str().unwrap(),
            &TemplateContext::new(),
        ))
}

/// A host with no sandbox does not refuse a level's line: it runs, without the
/// vault protection (the level was raised knowing it).
#[test]
#[serial]
fn a_level_approved_step_runs_unconfined_when_the_host_has_no_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    set_test_unavailable(Some("no sandbox here"));
    let (program, _) = confined_for_level(&step("touch", &[], true), "touch", &[]);
    let result = run_touch(&marker, dir.path());
    set_test_unavailable(None);
    assert_eq!(program, "touch", "no launcher: nothing to wrap it in");
    assert_eq!(
        result.result.status,
        RunStatus::Success,
        "{}",
        result.result.output
    );
    assert!(marker.exists(), "the step did not run");
}

/// "Protect the Kronn vault from agents" off: the owner's choice, the line runs
/// as it would for a human.
#[test]
#[serial]
fn a_level_approved_step_runs_unconfined_when_the_vault_protection_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    crate::core::trust_sandbox::set_protect_vault(false);
    let (program, launch) = confined_for_level(&step("touch", &[], true), "touch", &[]);
    let result = run_touch(&marker, dir.path());
    crate::core::trust_sandbox::set_protect_vault(true);
    assert_eq!((program.as_str(), launch.len()), ("touch", 0));
    assert_eq!(
        result.result.status,
        RunStatus::Success,
        "{}",
        result.result.output
    );
    assert!(marker.exists());
}

/// The line reads Kronn's key from inside the sandbox: it cannot. Applied for
/// real where the host allows it; skipped where a sandbox cannot be nested.
#[tokio::test]
#[serial]
async fn a_level_approved_step_cannot_read_the_encryption_key() {
    if let Err(reason) = Mechanism::usable_on_host() {
        eprintln!("skipped: no sandbox can be applied here ({reason})");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("encryption_key"), "TOPSECRET").unwrap();
    std::env::set_var("KRONN_DATA_DIR", data.path());
    let key = data.path().join("encryption_key");
    let key = key.to_str().unwrap();
    let work = tempfile::tempdir().unwrap();
    let allow = ["cat".to_string()];

    let control = execute_exec_step(
        &step("cat", &[key], false),
        &allow,
        work.path().to_str().unwrap(),
        &TemplateContext::new(),
    )
    .await;
    assert!(
        control.result.output.contains("TOPSECRET"),
        "control: a human-approved step reads it: {}",
        control.result.output
    );

    let confined = execute_exec_step(
        &step("cat", &[key], true),
        &allow,
        work.path().to_str().unwrap(),
        &TemplateContext::new(),
    )
    .await;
    assert!(
        !confined.result.output.contains("TOPSECRET"),
        "the sandbox leaked the key: {}",
        confined.result.output
    );
    assert_eq!(confined.result.status, RunStatus::Failed);
}
