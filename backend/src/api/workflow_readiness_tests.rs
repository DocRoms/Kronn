//! KT-1138 — a saved chain says whether it can start, on every write and read.
use super::*;
use crate::models::{BundleRequest, WorkflowBlockerKind};

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

/// A PR Review SHADOW step: inline Python reading its inputs from stdin.
fn stdin_step(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name, "step_type": {"type": "Exec"}, "exec_command": "python3",
        "exec_args": ["-c", "import json, sys; print(json.load(sys.stdin))", "{{run.id}}"],
        "exec_stdin": "{\"config\":{{review_config}}}",
        // What an agent may claim; none of it is honoured from an agent.
        "exec_unmodelled_args_approved": true,
        "exec_agent_written": false,
        "exec_agent_lines": []
    })
}

fn sub_step(name: &str, target: &str) -> serde_json::Value {
    serde_json::json!({"name": name, "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": target})
}

/// WF1 → WF2 (with a rollback) → WF3, as the reproduction's agent bundle.
fn shadow_bundle() -> BundleRequest {
    let variables = serde_json::json!([{"name": "review_config", "label": "c", "placeholder": "", "required": false}]);
    serde_json::from_value(serde_json::json!({
        "child_workflows": [
            {"bundle_id": "wf3", "name": "WF3", "project_id": null,
             "trigger": {"type": "Manual"}, "exec_allowlist": ["python3"], "variables": variables,
             "steps": [stdin_step("review"), stdin_step("publish")]},
            {"bundle_id": "wf2", "name": "WF2", "project_id": null,
             "trigger": {"type": "Manual"}, "exec_allowlist": ["python3"], "variables": variables,
             "steps": [stdin_step("reserve"), sub_step("review_each", "@bundle:wf3")],
             "on_failure": [stdin_step("release_after_failure")]}
        ],
        "workflow": {"name": "WF1", "project_id": null, "trigger": {"type": "Manual"},
                     "exec_allowlist": ["python3"], "variables": variables,
                     "steps": [stdin_step("inventory"), sub_step("check_each", "@bundle:wf2")]}
    }))
    .unwrap()
}

async fn load(state: &AppState, id: &str) -> Workflow {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
        .await
        .unwrap()
        .unwrap()
}

async fn verdict(state: &AppState, id: &str) -> WorkflowReadiness {
    let Json(response) = readiness(State(state.clone()), Path(id.to_string())).await;
    response.data.expect("readiness")
}

fn located(readiness: &WorkflowReadiness) -> Vec<(String, String, bool, bool)> {
    readiness
        .blockers
        .iter()
        .map(|b| {
            (
                b.workflow_name.clone(),
                b.step.clone().unwrap_or_default(),
                b.on_failure,
                b.human_only,
            )
        })
        .collect()
}

#[tokio::test]
async fn an_agent_bundle_reports_every_pending_approval_of_its_chain() {
    let state = state();
    let Json(response) =
        crate::api::bundle::create_bundle(State(state.clone()), Json(shadow_bundle())).await;
    assert!(response.success, "{:?}", response.error);
    let created = response.data.unwrap();
    let readiness = &created.readiness;
    assert!(
        !readiness.ready,
        "saved is not ready: {}",
        readiness.summary
    );
    assert_eq!(readiness.workflow_id, created.workflow.id);
    assert_eq!(readiness.checked_workflow_ids.len(), 3);
    let row = |wf: &str, step: &str, rollback: bool| (wf.into(), step.into(), rollback, true);
    assert_eq!(
        located(readiness),
        vec![
            row("WF1", "inventory", false),
            row("WF2", "reserve", false),
            row("WF2", "release_after_failure", true),
            row("WF3", "review", false),
            row("WF3", "publish", false),
        ]
    );
    assert!(readiness
        .blockers
        .iter()
        .all(|b| b.kind == WorkflowBlockerKind::HumanApproval
            && b.reason.as_deref() == Some("unmodelled_program")
            && b.phase.as_deref() == Some("stdin")));
    // The envelope says it too, for whoever reads only the notice.
    assert_eq!(response.readiness.as_ref(), Some(readiness));
    assert!(response
        .notice
        .as_deref()
        .is_some_and(|n| n.starts_with("NOT READY")));
}

#[tokio::test]
async fn no_agent_path_approves_clears_provenance_or_lifts_the_refusal() {
    let state = state();
    let Json(response) =
        crate::api::bundle::create_bundle(State(state.clone()), Json(shadow_bundle())).await;
    let created = response.data.unwrap();
    for id in std::iter::once(created.workflow.id.clone())
        .chain(created.child_workflows.iter().map(|c| c.id.clone()))
    {
        let saved = load(&state, &id).await;
        for step in saved.steps.iter().chain(&saved.on_failure) {
            if step.step_type != StepType::Exec {
                continue;
            }
            assert_eq!(step.exec_unmodelled_args_approved, None, "{}", step.name);
            assert_eq!(step.exec_agent_lines, vec!["stdin".to_string()]);
            assert!(crate::core::inline_code::runtime_refusal(step).is_some());
        }
        // An agent re-saving the same steps, claiming approval and a human
        // writer, changes nothing.
        let mut claimed = serde_json::to_value(&saved.steps).unwrap();
        for step in claimed.as_array_mut().unwrap() {
            step["exec_unmodelled_args_approved"] = true.into();
            step["exec_agent_written"] = serde_json::Value::Null;
            step["exec_agent_lines"] = serde_json::json!([]);
        }
        let Json(by_agent) = update_as(
            state.clone(),
            id.clone(),
            serde_json::from_value(serde_json::json!({"steps": claimed})).unwrap(),
            WorkflowWriter::Agent,
        )
        .await;
        assert!(by_agent.success, "{:?}", by_agent.error);
        let readiness = by_agent.readiness.expect("a write carries readiness");
        assert!(!readiness.ready);
        assert!(readiness.blockers.iter().all(|b| b.human_only));
        let resaved = load(&state, &id).await;
        for step in resaved
            .steps
            .iter()
            .filter(|s| s.step_type == StepType::Exec)
        {
            assert_eq!(step.exec_unmodelled_args_approved, None);
            assert!(!step.exec_agent_lines.is_empty(), "provenance kept");
            assert!(crate::core::inline_code::runtime_refusal(step).is_some());
        }
    }
    // Reading is read-only: the verdict endpoint approves nothing.
    let before = verdict(&state, &created.workflow.id).await;
    let after = verdict(&state, &created.workflow.id).await;
    assert_eq!(before.blockers, after.blockers);
}

#[tokio::test]
async fn fixing_the_fixable_leaves_only_human_approvals_then_a_human_lifts_them() {
    let state = state();
    let Json(response) =
        crate::api::bundle::create_bundle(State(state.clone()), Json(shadow_bundle())).await;
    let created = response.data.unwrap();
    let root = created.workflow.id.clone();
    let wf3 = created
        .child_workflows
        .iter()
        .find(|c| c.name == "WF3")
        .unwrap()
        .id
        .clone();

    // An older saved step that interpolates into code, as a legacy row.
    let mut legacy = load(&state, &wf3).await;
    let mut unsafe_step = legacy.steps[0].clone();
    unsafe_step.name = "legacy".into();
    unsafe_step.exec_command = Some("python3".into());
    unsafe_step.exec_args = vec!["-c".into(), "print('{{review_config}}')".into()];
    unsafe_step.exec_stdin = None;
    unsafe_step.exec_agent_written = None;
    unsafe_step.exec_agent_lines = vec![];
    legacy.steps.push(unsafe_step);
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_workflow(conn, &legacy))
        .await
        .unwrap();
    let readiness = verdict(&state, &root).await;
    let fixable: Vec<_> = readiness
        .blockers
        .iter()
        .filter(|b| !b.human_only)
        .collect();
    assert_eq!(fixable.len(), 1, "{:?}", readiness.blockers);
    assert_eq!(fixable[0].kind, WorkflowBlockerKind::UnsafeInterpolation);
    assert_eq!(fixable[0].workflow_name, "WF3");
    assert!(
        fixable[0].action.contains("Rewrite"),
        "{}",
        fixable[0].action
    );

    // The agent applies the action; what stays needs a human.
    let mut steps = load(&state, &wf3).await.steps;
    steps.last_mut().unwrap().exec_args = vec![
        "-c".into(),
        "import sys; print(sys.argv[1])".into(),
        "{{review_config}}".into(),
    ];
    let Json(fixed) = update_as(
        state.clone(),
        wf3.clone(),
        serde_json::from_value(serde_json::json!({"steps": steps})).unwrap(),
        WorkflowWriter::Agent,
    )
    .await;
    assert!(fixed.success, "{:?}", fixed.error);
    let readiness = verdict(&state, &root).await;
    assert!(!readiness.ready);
    assert!(readiness.blockers.iter().all(|b| b.human_only));
    assert_eq!(readiness.human_approval_count, 6);

    // A human approves every line in the editor: the chain can start.
    for id in
        std::iter::once(root.clone()).chain(created.child_workflows.iter().map(|c| c.id.clone()))
    {
        let saved = load(&state, &id).await;
        let approve = |chain: &[WorkflowStep]| {
            let mut chain = chain.to_vec();
            for step in &mut chain {
                if step.step_type == StepType::Exec {
                    step.exec_unmodelled_args_approved = Some(true);
                }
            }
            chain
        };
        let Json(by_human) = update_as(
            state.clone(),
            id,
            serde_json::from_value(serde_json::json!({
                "steps": approve(&saved.steps),
                "on_failure": approve(&saved.on_failure),
            }))
            .unwrap(),
            WorkflowWriter::Human,
        )
        .await;
        assert!(by_human.success, "{:?}", by_human.error);
    }
    let readiness = verdict(&state, &root).await;
    assert!(readiness.ready, "{:?}", readiness.blockers);
    assert!(readiness.summary.contains("not a guarantee"));
    let Json(got) = get(State(state.clone()), Path(root)).await;
    assert_eq!(got.readiness.map(|r| r.ready), Some(true));
}

#[tokio::test]
async fn create_get_and_list_agree_and_a_clean_workflow_is_ready() {
    let state = state();
    let clean: CreateWorkflowRequest = serde_json::from_value(serde_json::json!({
        "name": "clean", "project_id": null, "trigger": {"type": "Manual"},
        "exec_allowlist": ["bash"],
        "steps": [{"name": "t", "step_type": {"type": "Exec"},
                   "exec_command": "bash", "exec_args": ["-c", "echo ok"]}]
    }))
    .unwrap();
    let Json(created) = create_as(state.clone(), clean, WorkflowWriter::Agent).await;
    assert!(created.success, "{:?}", created.error);
    let readiness = created.readiness.clone().expect("create carries readiness");
    assert!(readiness.ready, "{:?}", readiness.blockers);
    assert!(!readiness.enabled, "an agent's draft is saved disabled");
    let id = created.data.unwrap().id;
    let Json(got) = get(State(state.clone()), Path(id.clone())).await;
    assert_eq!(got.readiness, Some(readiness));

    let Json(pending) =
        crate::api::bundle::create_bundle(State(state.clone()), Json(shadow_bundle())).await;
    let root = pending.data.unwrap().workflow.id;
    let Json(list) = list_with_visibility(&state, None).await;
    let list = list.data.unwrap();
    let card = |id: &str| list.iter().find(|w| w.id == id).unwrap();
    assert_eq!(card(&id).blocker_count, Some(0), "zero is reported");
    assert_eq!(card(&id).human_approval_count, Some(0));
    assert_eq!(card(&root).blocker_count, Some(5), "children included");
    assert_eq!(card(&root).human_approval_count, Some(5));
}

/// A TriggerWorkflow target is its own run, yet it can block what the parent
/// started: the bundle, the read and the on-demand check all report it.
#[tokio::test]
async fn a_triggered_child_s_blockers_reach_bundle_get_and_validate() {
    let state = state();
    let variables = serde_json::json!([{"name": "review_config", "label": "c", "placeholder": "", "required": false}]);
    let bundle: BundleRequest = serde_json::from_value(serde_json::json!({
        "child_workflows": [
            {"bundle_id": "child", "name": "Child", "project_id": null,
             "trigger": {"type": "Manual"}, "exec_allowlist": ["python3"], "variables": variables,
             "steps": [stdin_step("work")], "on_failure": [stdin_step("undo")]}
        ],
        "workflow": {"name": "Parent", "project_id": null, "trigger": {"type": "Manual"},
                     "steps": [{"name": "launch", "step_type": {"type": "TriggerWorkflow"},
                                "sub_workflow_id": "@bundle:child"}]}
    }))
    .unwrap();
    let Json(response) =
        crate::api::bundle::create_bundle(State(state.clone()), Json(bundle)).await;
    assert!(response.success, "{:?}", response.error);
    let created = response.data.unwrap();
    let from_bundle = created.readiness.clone();
    assert!(!from_bundle.ready);
    assert_eq!(from_bundle.checked_workflow_ids.len(), 2);
    assert_eq!(
        located(&from_bundle),
        vec![
            ("Child".into(), "work".into(), false, true),
            ("Child".into(), "undo".into(), true, true),
        ]
    );
    let root = created.workflow.id;
    assert_eq!(verdict(&state, &root).await.blockers, from_bundle.blockers);
    let Json(got) = get(State(state.clone()), Path(root)).await;
    assert_eq!(
        got.readiness.map(|r| r.blockers),
        Some(from_bundle.blockers)
    );
}
