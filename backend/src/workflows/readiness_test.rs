use super::*;
use crate::models::{ExecScriptFile, WorkflowSafety, WorkflowTrigger};

fn workflow(id: &str, steps: Vec<WorkflowStep>, on_failure: Vec<WorkflowStep>) -> Workflow {
    Workflow {
        project_scope: None,
        pinned: false,
        id: id.into(),
        name: format!("name-{id}"),
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
        on_failure,
        exec_allowlist: vec!["python3".into(), "bash".into()],
        variables: vec![],
        enabled: true,
        retention: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// The shape of the PR Review SHADOW steps: inline Python reading its inputs
/// with `json.load(sys.stdin)`, the stdin line written by an agent.
fn agent_stdin_step(name: &str) -> WorkflowStep {
    WorkflowStep {
        name: name.into(),
        step_type: StepType::Exec,
        exec_command: Some("python3".into()),
        exec_args: vec![
            "-c".into(),
            "import json, sys; print(json.load(sys.stdin))".into(),
            "{{run.id}}".into(),
        ],
        exec_stdin: Some("{\"config\":{{review_config}}}".into()),
        exec_agent_written: Some(true),
        exec_agent_lines: vec!["stdin".into()],
        ..Default::default()
    }
}

fn sub(name: &str, target: &str) -> WorkflowStep {
    WorkflowStep {
        name: name.into(),
        step_type: StepType::SubWorkflow,
        sub_workflow_id: Some(target.into()),
        ..Default::default()
    }
}

fn chain() -> HashMap<String, Workflow> {
    let wf3 = workflow(
        "wf3",
        vec![agent_stdin_step("review"), agent_stdin_step("publish")],
        vec![],
    );
    let wf2 = workflow(
        "wf2",
        vec![agent_stdin_step("reserve"), sub("review_each", "wf3")],
        vec![agent_stdin_step("release_after_failure")],
    );
    let wf1 = workflow(
        "wf1",
        vec![agent_stdin_step("inventory"), sub("check_each", "wf2")],
        vec![],
    );
    [wf1, wf2, wf3]
        .into_iter()
        .map(|wf| (wf.id.clone(), wf))
        .collect()
}

fn approve_all(workflows: &mut HashMap<String, Workflow>) {
    for wf in workflows.values_mut() {
        for step in wf.steps.iter_mut().chain(wf.on_failure.iter_mut()) {
            if step.step_type == StepType::Exec {
                step.exec_unmodelled_args_approved = Some(true);
            }
        }
    }
}

#[test]
fn a_saved_enabled_chain_with_pending_approvals_is_not_ready() {
    let workflows = chain();
    let readiness = assess(&workflows["wf1"], &workflows);
    assert!(readiness.enabled, "saved and enabled");
    assert!(!readiness.ready, "{}", readiness.summary);
    assert_eq!(readiness.checked_workflow_ids, vec!["wf1", "wf2", "wf3"]);
    let named: Vec<(&str, &str, bool)> = readiness
        .blockers
        .iter()
        .map(|b| {
            (
                b.workflow_id.as_str(),
                b.step.as_deref().unwrap(),
                b.on_failure,
            )
        })
        .collect();
    assert_eq!(
        named,
        vec![
            ("wf1", "inventory", false),
            ("wf2", "reserve", false),
            ("wf2", "release_after_failure", true),
            ("wf3", "review", false),
            ("wf3", "publish", false),
        ]
    );
    for blocker in &readiness.blockers {
        assert_eq!(blocker.kind, WorkflowBlockerKind::HumanApproval);
        assert!(blocker.human_only);
        assert_eq!(blocker.phase.as_deref(), Some("stdin"));
        assert_eq!(blocker.reason.as_deref(), Some("unmodelled_program"));
        assert!(
            blocker.action.contains("cannot approve"),
            "{}",
            blocker.action
        );
    }
    assert_eq!(readiness.human_approval_count, 5);
    assert!(readiness.summary.starts_with("NOT READY"));
    assert!(readiness.summary.contains("neither means"));
}

#[test]
fn every_step_the_run_refuses_is_a_blocker_with_its_runtime_refusal() {
    let workflows = chain();
    let readiness = assess(&workflows["wf1"], &workflows);
    let steps: Vec<&WorkflowStep> = workflows
        .values()
        .flat_map(|wf| wf.steps.iter().chain(&wf.on_failure))
        .collect();
    let refused = steps
        .iter()
        .filter(|step| crate::core::inline_code::runtime_refusal(step).is_some())
        .count();
    assert_eq!(refused, readiness.blockers.len());
}

#[test]
fn a_human_approval_of_every_line_makes_the_chain_ready() {
    let mut workflows = chain();
    approve_all(&mut workflows);
    let readiness = assess(&workflows["wf1"], &workflows);
    assert!(readiness.ready, "{:?}", readiness.blockers);
    assert_eq!(readiness.human_approval_count, 0);
    assert!(readiness.summary.starts_with("READY"));
    assert!(readiness.summary.contains("not a guarantee"));
}

#[test]
fn a_clean_workflow_is_ready() {
    let step = WorkflowStep {
        name: "tests".into(),
        step_type: StepType::Exec,
        exec_command: Some("bash".into()),
        exec_args: vec!["-c".into(), "echo ok".into()],
        ..Default::default()
    };
    let wf = workflow("clean", vec![step], vec![]);
    let readiness = assess(&wf, &HashMap::new());
    assert!(readiness.ready);
    assert!(readiness.blockers.is_empty());
    assert_eq!(readiness.checked_workflow_ids, vec!["clean"]);
}

#[test]
fn fixable_blockers_are_not_human_only_and_carry_an_action() {
    let interpolated = WorkflowStep {
        name: "echo".into(),
        step_type: StepType::Exec,
        exec_command: Some("bash".into()),
        exec_args: vec!["-c".into(), "echo \"{{issue.title}}\"".into()],
        ..Default::default()
    };
    let outside = WorkflowStep {
        name: "outside".into(),
        step_type: StepType::Exec,
        exec_command: Some("terraform".into()),
        ..Default::default()
    };
    let bare_agent = WorkflowStep {
        name: "plan".into(),
        step_type: StepType::Agent,
        ..Default::default()
    };
    let scripted = WorkflowStep {
        name: "script".into(),
        step_type: StepType::Exec,
        exec_command: Some("python3".into()),
        exec_args: vec!["tool.py".into()],
        exec_script_files: vec![ExecScriptFile {
            path: "tool.py".into(),
            sha256: String::new(),
        }],
        ..Default::default()
    };
    let wf = workflow(
        "mixed",
        vec![interpolated, outside, scripted],
        vec![bare_agent],
    );
    let readiness = assess(&wf, &HashMap::new());
    let kinds: Vec<(WorkflowBlockerKind, &str, bool)> = readiness
        .blockers
        .iter()
        .map(|b| (b.kind, b.step.as_deref().unwrap(), b.human_only))
        .collect();
    assert!(kinds.contains(&(WorkflowBlockerKind::UnsafeInterpolation, "echo", false)));
    assert!(kinds.contains(&(WorkflowBlockerKind::ValidationError, "outside", false)));
    assert!(kinds.contains(&(WorkflowBlockerKind::HumanApproval, "script", true)));
    assert!(kinds.contains(&(WorkflowBlockerKind::MisconfiguredStep, "plan", false)));
    let rollback = readiness
        .blockers
        .iter()
        .find(|b| b.step.as_deref() == Some("plan"))
        .unwrap();
    assert!(rollback.on_failure);
    let rewrite = readiness
        .blockers
        .iter()
        .find(|b| b.kind == WorkflowBlockerKind::UnsafeInterpolation)
        .unwrap();
    assert!(rewrite.action.contains("Rewrite"), "{}", rewrite.action);
    assert_eq!(readiness.human_approval_count, 1);
}

#[test]
fn missing_children_and_cycles_stay_explicit() {
    let a = workflow(
        "a",
        vec![sub("to_b", "b"), sub("to_ghost", "ghost")],
        vec![],
    );
    let b = workflow("b", vec![sub("back_to_a", "a")], vec![]);
    let workflows: HashMap<String, Workflow> = [a.clone(), b]
        .into_iter()
        .map(|w| (w.id.clone(), w))
        .collect();
    let readiness = assess(&a, &workflows);
    assert!(!readiness.ready);
    let cycle = readiness
        .blockers
        .iter()
        .find(|b| b.kind == WorkflowBlockerKind::ChildCycle)
        .expect("cycle reported");
    assert_eq!(cycle.workflow_id, "b");
    assert!(cycle.message.contains("a → b → a"), "{}", cycle.message);
    let missing = readiness
        .blockers
        .iter()
        .find(|b| b.kind == WorkflowBlockerKind::MissingChild)
        .expect("missing child reported");
    assert_eq!(missing.step.as_deref(), Some("to_ghost"));
}

#[test]
fn a_rollback_sub_workflow_is_checked_too() {
    let child = workflow("child", vec![agent_stdin_step("undo")], vec![]);
    let parent = workflow("parent", vec![], vec![sub("rollback", "child")]);
    let workflows: HashMap<String, Workflow> =
        [child].into_iter().map(|w| (w.id.clone(), w)).collect();
    let readiness = assess(&parent, &workflows);
    assert_eq!(readiness.checked_workflow_ids, vec!["parent", "child"]);
    assert_eq!(readiness.blockers.len(), 1);
    assert_eq!(readiness.blockers[0].workflow_id, "child");
}

#[test]
fn a_failed_collection_is_never_ready() {
    let wf = workflow("x", vec![], vec![]);
    let readiness = collection_failure(&wf, "DB error: locked");
    assert!(!readiness.ready);
    assert_eq!(
        readiness.blockers[0].kind,
        WorkflowBlockerKind::CollectionError
    );
}

fn trigger(name: &str, target: &str) -> WorkflowStep {
    WorkflowStep {
        name: name.into(),
        step_type: StepType::TriggerWorkflow,
        sub_workflow_id: Some(target.into()),
        ..Default::default()
    }
}

#[test]
fn a_triggered_workflow_and_its_rollback_are_checked() {
    let child = workflow(
        "child",
        vec![agent_stdin_step("work")],
        vec![agent_stdin_step("undo")],
    );
    // A trigger loop back to the parent is allowed and must not recurse forever.
    let parent = workflow("parent", vec![trigger("launch", "child")], vec![]);
    let mut looped = child.clone();
    looped.steps.push(trigger("again", "parent"));
    let workflows: HashMap<String, Workflow> = [looped, parent.clone()]
        .into_iter()
        .map(|w| (w.id.clone(), w))
        .collect();
    let readiness = assess(&parent, &workflows);
    assert!(!readiness.ready);
    assert_eq!(readiness.checked_workflow_ids, vec!["parent", "child"]);
    let located: Vec<(&str, bool)> = readiness
        .blockers
        .iter()
        .map(|b| (b.step.as_deref().unwrap(), b.on_failure))
        .collect();
    assert_eq!(located, vec![("work", false), ("undo", true)]);
    assert!(!readiness
        .blockers
        .iter()
        .any(|b| b.kind == WorkflowBlockerKind::ChildCycle));
}

#[test]
fn the_default_todo_workflows_are_ready() {
    // TaskBoard steps are agentless and run no command: no Exec refusal.
    let defaults = crate::core::default_todo::workflows("fr");
    assert!(defaults
        .iter()
        .any(|w| w.steps.iter().any(|s| s.step_type == StepType::TaskBoard)));
    let workflows: HashMap<String, Workflow> =
        defaults.iter().map(|w| (w.id.clone(), w.clone())).collect();
    for wf in &defaults {
        let readiness = assess(wf, &workflows);
        assert!(readiness.ready, "{}: {:?}", wf.name, readiness.blockers);
        assert!(readiness.blockers.is_empty());
    }
}
