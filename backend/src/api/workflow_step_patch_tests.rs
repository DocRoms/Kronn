//! KT-1139 — `PATCH /api/workflows/:id/step` edits one step through the same
//! save path as `PUT`, so no agent protection can be bypassed by it.

use super::*;
use serde_json::json;

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn bridge() -> Option<axum::Extension<crate::core::bridge_token::BridgeCaller>> {
    Some(axum::Extension(crate::core::bridge_token::BridgeCaller {
        token_id: "t".into(),
        project: None,
        own_discussions: vec![],
        own_run: None,
        agent: None,
    }))
}

fn step_request(body: serde_json::Value) -> UpdateWorkflowStepRequest {
    serde_json::from_value(body).unwrap()
}

/// A human-saved Cron workflow: an approved `terraform plan {{x}}` and a
/// JsonData step, plus a rollback step.
async fn human_workflow(state: &AppState, enabled: bool) -> Workflow {
    let request: CreateWorkflowRequest = serde_json::from_value(json!({
        "name": "patched", "project_id": null,
        "trigger": {"type": "Cron", "schedule": "0 * * * *"},
        "exec_allowlist": ["terraform"],
        "steps": [
            {"name": "plan", "step_type": {"type": "Exec"}, "exec_command": "terraform",
             "exec_args": ["plan", "{{x}}"], "exec_unmodelled_args_approved": true},
            {"name": "emit", "step_type": {"type": "JsonData"}, "json_data_payload": {"a": 1}}
        ],
        "on_failure": [
            {"name": "undo", "step_type": {"type": "JsonData"}, "json_data_payload": {"u": 1}}
        ]
    }))
    .unwrap();
    let Json(created) = create_as(state.clone(), request, WorkflowWriter::Human).await;
    let created = created.data.expect("created");
    assert_eq!(
        created.steps[0].exec_unmodelled_args_approved,
        Some(true),
        "the human approval is stored"
    );
    if !enabled {
        return created;
    }
    let Json(on) = update_as(
        state.clone(),
        created.id.clone(),
        serde_json::from_value(json!({"enabled": true})).unwrap(),
        WorkflowWriter::Human,
    )
    .await;
    on.data.expect("enabled")
}

async fn patch(
    state: &AppState,
    id: &str,
    caller: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    body: serde_json::Value,
) -> ApiResponse<Workflow> {
    let Json(response) = update_step(
        State(state.clone()),
        Path(id.to_string()),
        caller,
        Json(step_request(body)),
    )
    .await;
    response
}

#[test]
fn a_step_is_found_by_name_or_position_and_only_its_sent_fields_change() {
    let mut first = WorkflowStep {
        name: "a".into(),
        prompt_template: "keep".into(),
        ..Default::default()
    };
    first.description = Some("old".into());
    let second = WorkflowStep {
        name: "b".into(),
        ..Default::default()
    };
    let steps = vec![first, second];

    let by_name = patch_step(
        steps.clone(),
        &step_request(
            json!({"step_name": "a", "fields": {"description": null, "stall_timeout_secs": 7}}),
        ),
    )
    .unwrap();
    assert_eq!(by_name[0].prompt_template, "keep");
    assert_eq!(by_name[0].description, None, "null clears a field");
    assert_eq!(by_name[0].stall_timeout_secs, Some(7));
    assert_eq!(by_name[1].name, "b");

    let by_index = patch_step(
        steps.clone(),
        &step_request(json!({"step_index": 2, "fields": {"prompt_template": "new"}})),
    )
    .unwrap();
    assert_eq!(by_index[1].prompt_template, "new");
    assert_eq!(by_index[0].prompt_template, "keep");

    for (body, needle) in [
        (
            json!({"step_name": "zz", "fields": {"stall_timeout_secs": 1}}),
            "Steps: a, b",
        ),
        (
            json!({"step_index": 0, "fields": {"stall_timeout_secs": 1}}),
            "out of range",
        ),
        (
            json!({"step_index": 3, "fields": {"stall_timeout_secs": 1}}),
            "out of range",
        ),
        (json!({"fields": {"stall_timeout_secs": 1}}), "exactly one"),
        (
            json!({"step_name": "a", "step_index": 1, "fields": {"stall_timeout_secs": 1}}),
            "exactly one",
        ),
        (json!({"step_name": "a", "fields": {}}), "No step field"),
        (
            json!({"step_name": "a", "fields": {"enabled": true}}),
            "Unknown step field `enabled`",
        ),
        (
            json!({"step_name": "a", "fields": {"stall_timeout_secs": "x"}}),
            "Invalid step field",
        ),
    ] {
        let error = patch_step(steps.clone(), &step_request(body.clone())).unwrap_err();
        assert!(error.contains(needle), "{body}: {error}");
    }
}

#[test]
fn the_request_refuses_unknown_top_level_keys() {
    let parsed = serde_json::from_value::<UpdateWorkflowStepRequest>(
        json!({"step_name": "a", "fields": {}, "enabled": true}),
    );
    assert!(parsed.is_err(), "a workflow field cannot ride along");
}

/// An agent's targeted edit of an approved Exec line drops the approval and
/// leaves the line waiting for a human, exactly like a full `PUT`; its edit of
/// another step keeps the approval.
#[tokio::test]
async fn an_agent_step_patch_keeps_every_approval_rule() {
    let state = state();
    let wf = human_workflow(&state, false).await;

    let elsewhere = patch(
        &state,
        &wf.id,
        bridge(),
        json!({"step_name": "emit", "fields": {"json_data_payload": {"a": 2}}}),
    )
    .await;
    assert!(elsewhere.success, "{:?}", elsewhere.error);
    let saved = elsewhere.data.unwrap();
    assert_eq!(saved.steps[1].json_data_payload, Some(json!({"a": 2})));
    assert_eq!(
        saved.steps[0].exec_unmodelled_args_approved,
        Some(true),
        "an untouched approved line keeps its approval"
    );

    let smuggled = patch(
        &state,
        &wf.id,
        bridge(),
        json!({"step_index": 1, "fields": {
            "exec_args": ["plan", "-destroy", "{{x}}"],
            "exec_unmodelled_args_approved": true,
            "exec_agent_written": null,
            "exec_agent_lines": []
        }}),
    )
    .await;
    assert!(smuggled.success, "{:?}", smuggled.error);
    assert!(
        smuggled
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("plan")),
        "the agent is told a human must approve: {:?}",
        smuggled.notice
    );
    let step = &smuggled.data.unwrap().steps[0];
    assert_eq!(
        step.exec_unmodelled_args_approved, None,
        "an agent never approves"
    );
    assert_eq!(
        step.exec_agent_written,
        Some(true),
        "the line is the agent's"
    );
    assert!(crate::core::inline_code::runtime_refusal(step).is_some());

    // The same change by a human is trusted.
    let human = patch(
        &state,
        &wf.id,
        None,
        json!({"step_name": "plan", "fields": {"exec_unmodelled_args_approved": true}}),
    )
    .await;
    assert!(human.success, "{:?}", human.error);
    assert_eq!(
        crate::core::inline_code::runtime_refusal(&human.data.unwrap().steps[0]),
        None
    );
}

/// KT-1037: an agent's targeted change to what an enabled workflow executes
/// disables it and records why; it cannot enable one.
#[tokio::test]
async fn an_agent_step_patch_disables_an_enabled_workflow() {
    let state = state();
    let wf = human_workflow(&state, true).await;
    assert!(wf.enabled);
    let edited = patch(
        &state,
        &wf.id,
        bridge(),
        json!({"step_name": "emit", "fields": {"json_data_payload": {"b": 2}}}),
    )
    .await;
    assert!(edited.success, "{:?}", edited.error);
    assert!(!edited.data.unwrap().enabled, "an agent edit turns it off");
    let records = state
        .db
        .with_read_conn(crate::db::workflows::list_auto_disabled)
        .await
        .unwrap();
    assert!(
        records
            .iter()
            .any(|r| r.id == wf.id && r.reason == AutoDisableReason::AgentEdit),
        "{records:?}"
    );
}

/// The rollback chain is targeted explicitly; the main steps are untouched.
#[tokio::test]
async fn on_failure_targets_the_rollback_chain() {
    let state = state();
    let wf = human_workflow(&state, false).await;
    let edited = patch(
        &state,
        &wf.id,
        None,
        json!({"step_name": "undo", "on_failure": true, "fields": {"json_data_payload": {"u": 2}}}),
    )
    .await;
    assert!(edited.success, "{:?}", edited.error);
    let saved = edited.data.unwrap();
    assert_eq!(saved.on_failure[0].json_data_payload, Some(json!({"u": 2})));
    assert_eq!(saved.steps.len(), wf.steps.len());
    let missing = patch(
        &state,
        &wf.id,
        None,
        json!({"step_name": "undo", "fields": {"json_data_payload": {}}}),
    )
    .await;
    assert!(!missing.success, "`undo` is not a main step");
    let gone = patch(
        &state,
        "no-such-workflow",
        None,
        json!({"step_name": "undo", "fields": {"stall_timeout_secs": 1}}),
    )
    .await;
    assert!(!gone.success);
}

/// A step patch stores exactly what the equivalent full `PUT` stores, for an
/// agent: the targeted route is a shortcut, never a different rule set.
#[tokio::test]
async fn a_step_patch_saves_what_the_full_update_saves() {
    let state = state();
    let via_patch = human_workflow(&state, false).await;
    let via_put = human_workflow(&state, false).await;
    let fields =
        json!({"exec_args": ["plan", "-out", "{{x}}"], "exec_unmodelled_args_approved": true});

    let patched = patch(
        &state,
        &via_patch.id,
        bridge(),
        json!({"step_name": "plan", "fields": fields}),
    )
    .await;
    assert!(patched.success, "{:?}", patched.error);

    let mut steps = serde_json::to_value(&via_put.steps).unwrap();
    for (key, value) in fields.as_object().unwrap() {
        steps[0][key] = value.clone();
    }
    let Json(put) = update(
        State(state.clone()),
        Path(via_put.id.clone()),
        bridge(),
        Json(serde_json::from_value(json!({"steps": steps})).unwrap()),
    )
    .await;
    assert!(put.success, "{:?}", put.error);

    let strip_ids = |wf: Workflow| {
        let mut value = serde_json::to_value(wf.steps).unwrap();
        for step in value.as_array_mut().unwrap() {
            step.as_object_mut().unwrap().remove("id");
        }
        value
    };
    assert_eq!(
        strip_ids(patched.data.unwrap()),
        strip_ids(put.data.unwrap())
    );
    assert_eq!(patched.notice.is_some(), put.notice.is_some());
}

/// An unknown field name is refused whatever its value, `null` and empty
/// collections included; clearing a known field stays valid.
#[test]
fn an_unknown_field_name_is_refused_whatever_its_value() {
    let steps = vec![WorkflowStep {
        name: "a".into(),
        description: Some("old".into()),
        ..Default::default()
    }];
    for value in [json!(null), json!([]), json!({}), json!("x")] {
        let error = patch_step(
            steps.clone(),
            &step_request(json!({"step_name": "a", "fields": {"promt_template": value}})),
        )
        .unwrap_err();
        assert!(
            error.contains("Unknown step field `promt_template`"),
            "{value}: {error}"
        );
    }
    let cleared = patch_step(
        steps,
        &step_request(json!({"step_name": "a", "fields": {"description": null}})),
    )
    .unwrap();
    assert_eq!(cleared[0].description, None);
    let names = step_field_names();
    for known in [
        "name",
        "step_type",
        "prompt_template",
        "exec_args",
        "json_data_payload",
    ] {
        assert!(names.contains(&known), "{known} in {names:?}");
    }
}

/// A human edit landing while an agent's step PATCH is in flight is never
/// overwritten: the PATCH gets a conflict and saves nothing. Covers both
/// windows (after the PATCH's read, and right before its write) and both
/// chains.
#[tokio::test]
async fn a_concurrent_edit_of_another_step_is_never_overwritten() {
    for window in ["read", "write"] {
        for chain in ["steps", "on_failure"] {
            let state = state();
            let wf = human_workflow(&state, false).await;
            let key = match window {
                "read" => format!("{}#step-read", wf.id),
                _ => wf.id.clone(),
            };
            let (reached, resume) = before_write_hook::arm(&key);
            let agent = {
                let (state, id) = (state.clone(), wf.id.clone());
                tokio::spawn(async move {
                    patch(
                        &state,
                        &id,
                        bridge(),
                        json!({"step_name": "emit", "fields": {"json_data_payload": {"a": 2}}}),
                    )
                    .await
                })
            };
            reached.notified().await;
            let human_edit = match chain {
                "steps" => json!({"step_name": "plan", "fields": {"description": "human"}}),
                _ => json!({"step_name": "undo", "on_failure": true,
                            "fields": {"json_data_payload": {"u": 9}}}),
            };
            let human = patch(&state, &wf.id, None, human_edit).await;
            assert!(human.success, "{window}/{chain}: {:?}", human.error);
            resume.notify_one();
            let agent = agent.await.unwrap();
            assert!(
                !agent.success,
                "{window}/{chain}: the stale PATCH must not save"
            );
            assert!(
                agent
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("changed since")),
                "{window}/{chain}: {:?}",
                agent.error
            );
            let stored = state
                .db
                .with_read_conn({
                    let id = wf.id.clone();
                    move |conn| crate::db::workflows::get_workflow(conn, &id)
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.steps[1].json_data_payload,
                Some(json!({"a": 1})),
                "{window}/{chain}: nothing of the PATCH was written"
            );
            match chain {
                "steps" => assert_eq!(stored.steps[0].description.as_deref(), Some("human")),
                _ => assert_eq!(
                    stored.on_failure[0].json_data_payload,
                    Some(json!({"u": 9}))
                ),
            }
        }
    }
}

/// A human save of a workflow-level field during a step PATCH is never
/// overwritten by the PATCH's earlier read: conflict, nothing written.
#[tokio::test]
async fn a_concurrent_rename_is_never_overwritten() {
    concurrent_workflow_field_edit(json!({"name": "renamed by a human"})).await;
}

#[tokio::test]
async fn a_concurrent_concurrency_limit_edit_is_never_overwritten() {
    concurrent_workflow_field_edit(json!({"concurrency_limit": 3})).await;
}

async fn concurrent_workflow_field_edit(human_patch: serde_json::Value) {
    let state = state();
    let wf = human_workflow(&state, false).await;
    let (reached, resume) = before_write_hook::arm(&wf.id);
    let agent = {
        let (state, id) = (state.clone(), wf.id.clone());
        tokio::spawn(async move {
            patch(
                &state,
                &id,
                bridge(),
                json!({"step_name": "emit", "fields": {"json_data_payload": {"a": 2}}}),
            )
            .await
        })
    };
    reached.notified().await;
    let Json(human) = update(
        State(state.clone()),
        Path(wf.id.clone()),
        None,
        Json(serde_json::from_value(human_patch.clone()).unwrap()),
    )
    .await;
    assert!(human.success, "{human_patch}: {:?}", human.error);
    resume.notify_one();
    let agent = agent.await.unwrap();
    assert!(
        !agent.success,
        "{human_patch}: the stale PATCH must not save"
    );
    assert!(
        agent
            .error
            .as_deref()
            .is_some_and(|e| e.contains("changed since")),
        "{human_patch}: {:?}",
        agent.error
    );
    let stored = state
        .db
        .with_read_conn({
            let id = wf.id.clone();
            move |conn| crate::db::workflows::get_workflow(conn, &id)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.steps[1].json_data_payload,
        Some(json!({"a": 1})),
        "{human_patch}"
    );
    match human_patch.get("name") {
        Some(_) => assert_eq!(stored.name, "renamed by a human"),
        None => assert_eq!(stored.concurrency_limit, Some(3)),
    }
}

/// The revision a step PATCH compares covers exactly the columns the UPDATE
/// writes: a column added to one but not the other fails here.
#[test]
fn the_compared_revision_covers_every_written_column() {
    let sql = crate::db::workflows::UPDATE_WORKFLOW_SQL;
    let set = &sql[sql.find(" SET ").unwrap() + 5..sql.find(" WHERE ").unwrap()];
    let mut written: Vec<&str> = set
        .split(',')
        .filter_map(|part| part.split_once(" = ").map(|(col, _)| col.trim()))
        .filter(|col| !col.is_empty() && !col.contains(' '))
        .collect();
    let mut compared = crate::db::workflows::WRITTEN_COLUMNS.to_vec();
    written.sort_unstable();
    compared.sort_unstable();
    assert_eq!(written, compared);
}
