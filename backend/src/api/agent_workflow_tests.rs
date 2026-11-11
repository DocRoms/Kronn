use super::*;

async fn call(executor: &KronnToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: name.into(),
            name: name.into(),
            arguments,
        })
        .await
}

fn draft() -> Value {
    json!({"name":"Three steps", "trigger":{"type":"Manual"}, "enabled":true,
    "variables":[{"name":"topic","label":"Topic","placeholder":"test","required":false}],
    "steps":[
            {"name":"seed","step_type":{"type":"JsonData"},"json_data_payload":{"items":[1,2,3]}},
        {"name":"shape","step_type":"TransformData","transform_data":{"input_from":"steps.seed.data","fields":[{"target":"count","source":"$.items[*]","operation":"count"}]}},
            {"name":"done","step_type":{"type":"TransformData"},"transform_data":{"input_from":"steps.shape.data","fields":[{"target":"total","source":"$.count","operation":"copy"}]}}
    ]})
}

#[tokio::test]
async fn workflow_authoring_creates_a_disabled_draft_and_preserves_omitted_fields() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::new(state.clone(), Some("room-a".into()));
    let created = call(&executor, "workflow_create_draft", draft()).await;
    assert!(created.ok, "{}", created.content);
    assert_eq!(created.content["enabled"], false);
    assert_eq!(created.content["project_id"], "a");
    assert_eq!(created.content["steps"].as_array().unwrap().len(), 3);
    assert!(created.content["steps"][0]["id"].is_string());
    assert_eq!(created.content["kronn_readiness"]["ready"], true);
    let id = created.content["id"].clone();
    let updated = call(
        &executor,
        "workflow_update",
        json!({"workflow_id":id,"name":"Renamed","enabled":true,"project_id":"b"}),
    )
    .await;
    assert!(updated.ok, "{}", updated.content);
    assert_eq!(updated.content["name"], "Renamed");
    for field in [
        "steps",
        "variables",
        "trigger",
        "project_id",
        "enabled",
        "actions",
        "safety",
    ] {
        assert_eq!(
            updated.content[field], created.content[field],
            "{field} must survive"
        );
    }
    let got = call(&executor, "workflow_get", json!({"workflow_id":id})).await;
    assert!(got.ok);
    assert_eq!(got.content, updated.content);
    let cleared = call(
        &executor,
        "workflow_update",
        json!({"workflow_id":id,"variables":[]}),
    )
    .await;
    assert!(cleared.ok);
    assert_eq!(cleared.content["variables"], Value::Null); // empty collections are omitted by Workflow serialization
    let empty = call(
        &executor,
        "workflow_update",
        json!({"workflow_id":id,"steps":[]}),
    )
    .await;
    assert!(!empty.ok, "empty step lists must not persist");
    let active_id = id.as_str().unwrap().to_owned();
    state
        .db
        .with_conn(move |conn| {
            conn.execute("UPDATE workflows SET enabled=1 WHERE id=?1", [active_id])?;
            Ok(())
        })
        .await
        .unwrap();
    let denied = call(
        &executor,
        "workflow_update",
        json!({"workflow_id":id,"name":"Unexpected"}),
    )
    .await;
    assert!(!denied.ok);
    assert!(denied.content.to_string().contains("Workflows UI"));
}

#[tokio::test]
async fn workflow_scope_limits_reads_and_writes_and_cron_defaults_to_one() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let local = KronnToolExecutor::new(state.clone(), Some("room-a".into()));
    let general = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let a = call(&local, "workflow_create_draft", draft()).await;
    assert!(a.ok, "{}", a.content);
    let mut global = draft();
    global["project_id"] = Value::Null;
    global["trigger"] = json!({"type":"Cron","schedule":"0 8 * * *"});
    // A null project is unset: the draft lands in the room's project, as
    // through a CLI agent's bridge (KT-1346).
    let global = call(&local, "workflow_create_draft", global).await;
    assert!(global.ok, "{}", global.content);
    assert_eq!(global.content["project_id"], "a");
    assert_eq!(global.content["concurrency_limit"], 1);
    let list = call(&general, "workflow_list", json!({})).await;
    assert!(list.ok);
    assert_eq!(list.content["workflows"].as_array().unwrap().len(), 0);
    for name in ["workflow_get", "workflow_update"] {
        let denied = call(
            &general,
            name,
            json!({"workflow_id":a.content["id"],"name":"No"}),
        )
        .await;
        assert!(!denied.ok, "{name} must enforce project scope");
    }
    let mut foreign = draft();
    foreign["project_id"] = json!("b");
    assert!(!call(&local, "workflow_create_draft", foreign).await.ok);
    let mut invalid = draft();
    invalid["steps"][1] = json!({"name":"unapproved", "step_type":{"type":"Exec"}, "exec_command":"unapproved-command"});
    assert!(
        !call(&local, "workflow_create_draft", invalid).await.ok,
        "shared API validation must reject commands outside the allowlist"
    );
}

#[tokio::test]
async fn workflow_tools_are_unavailable_to_workers_even_if_called_directly() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let mut worker = KronnToolExecutor::new(state, Some("general".into()));
    worker.worker_room = true;
    for definition in declarations() {
        let name = definition["function"]["name"].as_str().unwrap();
        assert!(!worker
            .catalogue()
            .iter()
            .any(|t| t["function"]["name"] == name));
        assert!(!call(&worker, name, draft()).await.ok, "{name}");
    }
}

#[test]
fn workflow_schema_sections_preserve_the_canonical_contract() {
    let lean = crate::api::workflows::canonical_step_schema();
    let schema = crate::api::workflows::full_step_schema();
    assert_eq!(schema_section(&json!({})).unwrap(), lean);
    assert_eq!(
        schema_section(&json!({})).unwrap()["step_types_closed_set"],
        schema["step_types_closed_set"]
    );
    for kind in schema["step_types_closed_set"].as_array().unwrap() {
        let name = kind.as_str().unwrap();
        let selected = schema_section(&json!({"step_type":name})).unwrap();
        assert_eq!(selected["contract"], schema["fields_by_type"][name]);
        let _: crate::models::StepType = serde_json::from_value(json!({"type":name})).unwrap();
        let _: crate::models::WorkflowStep =
            serde_json::from_value(selected["contract"]["example"].clone()).unwrap();
    }
    for section in [
        "template_vars",
        "data_pipeline_contract",
        "authoring_doctrine",
    ] {
        assert_eq!(
            schema_section(&json!({"section":section})).unwrap()["contract"],
            schema[section]
        );
    }
    assert!(schema_section(&json!({"step_type":"invented"})).is_err());
    assert!(schema_section(&json!({"section":"invented"})).is_err());
}

/// KT-1681 — what an agent always pays for carries the required fields and one
/// pointer per step type; every note, runtime contract and example stays one
/// `step_type` read away, and the doctrine still comes with the default.
#[test]
fn the_default_contract_is_lean_and_every_step_type_stays_authorable() {
    let lean = crate::api::workflows::canonical_step_schema();
    let full = crate::api::workflows::full_step_schema();
    assert!(
        lean.to_string().len() * 2 < full.to_string().len(),
        "lean {} B, full {} B",
        lean.to_string().len(),
        full.to_string().len()
    );
    // KT-1216: the doctrine reaches an agent with the default read.
    assert_eq!(lean["authoring_doctrine"], full["authoring_doctrine"]);
    assert_eq!(lean["step_types_closed_set"], full["step_types_closed_set"]);
    assert_eq!(lean["shape"], full["shape"]);
    assert!(lean["how_to_read"]
        .as_str()
        .unwrap()
        .contains("workflow_step_schema({step_type})"));
    for kind in full["step_types_closed_set"].as_array().unwrap() {
        let name = kind.as_str().unwrap();
        let summary = &lean["fields_by_type"][name];
        let whole = &full["fields_by_type"][name];
        // Required fields travel whole; optional ones by name.
        assert_eq!(summary["required"], whole["required"], "{name}");
        let optional: Vec<&str> = summary["optional"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        for (listed, entry) in optional
            .iter()
            .zip(whole["optional"].as_array().into_iter().flatten())
        {
            assert!(entry.as_str().unwrap().starts_with(listed), "{name}");
        }
        // Whatever the summary leaves out, it says where to read it, and the
        // read serves it: the working example and every named guidance key.
        let more: Vec<&str> = summary["more"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        assert!(more.contains(&"example"), "{name} has no example pointer");
        let read = schema_section(&json!({"step_type":name})).unwrap();
        for key in more {
            assert!(!read["contract"][key].is_null(), "{name}.{key}");
        }
        assert!(summary.get("example").is_none() && summary.get("note").is_none());
    }
    // The shared contracts left out of the default are whole behind `section`.
    assert!(lean["template_vars"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name == "run.variant"));
    assert!(lean["template_vars"].get("time_expressions").is_none());
    let templates = schema_section(&json!({"section":"template_vars"})).unwrap();
    assert!(templates["contract"]["time_expressions"]["canonical"].is_string());
    assert!(templates["contract"]["namespaces"]["refs.<name>"].is_string());
    // Guidance that moved is reachable from the tool the agent reads it with.
    let declared = declarations();
    let tool = declared
        .iter()
        .find(|tool| tool["function"]["name"] == "workflow_step_schema")
        .unwrap();
    let properties = &tool["function"]["parameters"]["properties"];
    assert!(properties["step_type"].is_object() && properties["section"].is_object());
}

/// B7-07 — an in-process principal's `last_run` is one of its project's runs.
#[tokio::test]
async fn workflow_list_shows_only_the_principal_s_project_runs() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflows(id, name, project_id, trigger_json, steps_json, created_at, updated_at) \
                 VALUES ('wf-shared', 'shared', NULL, '{\"type\":\"Manual\"}', '[]', \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO workflow_runs(id, workflow_id, project_id, status, started_at) VALUES \
                 ('run-in-a', 'wf-shared', 'a', 'Success', '2026-01-01T00:00:00Z'), \
                 ('run-in-b', 'wf-shared', 'b', 'Success', '2026-01-02T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let executor = KronnToolExecutor::new(state, Some("room-a".into()));
    let list = call(&executor, "workflow_list", json!({})).await;
    assert!(list.ok, "{}", list.content);
    let shared = list.content["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workflow| workflow["id"] == "wf-shared")
        .expect("the shared workflow is listed");
    assert_eq!(shared["last_run"]["id"], "run-in-a", "{shared}");
}

/// KT-1138: a native principal sees the verdict and cannot approve its own line.
#[tokio::test]
async fn a_native_draft_with_an_agent_line_is_not_ready_and_stays_unapproved() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::new(state.clone(), Some("room-a".into()));
    let step = json!({"name":"read","step_type":{"type":"Exec"},"exec_command":"python3",
        "exec_args":["-c","import json, sys; print(json.load(sys.stdin))"],
        "exec_stdin":"{{topic}}","exec_unmodelled_args_approved":true,"exec_agent_lines":[]});
    let created = call(
        &executor,
        "workflow_create_draft",
        json!({
        "name":"Reader","trigger":{"type":"Manual"},"exec_allowlist":["python3"],
        "variables":[{"name":"topic","label":"Topic","placeholder":"","required":false}],
        "steps":[step]}),
    )
    .await;
    assert!(created.ok, "{}", created.content);
    let readiness = &created.content["kronn_readiness"];
    assert_eq!(readiness["ready"], false);
    assert_eq!(readiness["blockers"][0]["kind"], "human_approval");
    assert_eq!(readiness["blockers"][0]["human_only"], true);
    assert!(created.content["kronn_notice"].is_string());
    let id = created.content["id"].clone();
    let mut steps = created.content["steps"].clone();
    steps[0]["exec_unmodelled_args_approved"] = json!(true);
    steps[0]["exec_agent_lines"] = json!([]);
    let again = call(
        &executor,
        "workflow_update",
        json!({"workflow_id":id,"steps":steps}),
    )
    .await;
    assert!(again.ok, "{}", again.content);
    assert_eq!(again.content["kronn_readiness"]["ready"], false);
    assert_eq!(
        again.content["steps"][0]["exec_unmodelled_args_approved"],
        Value::Null
    );
    let got = call(&executor, "workflow_get", json!({"workflow_id":id})).await;
    assert_eq!(got.content["kronn_readiness"]["human_approval_count"], 1);
}

/// KT-1216 — the authoring tools send the agent to the doctrine, and the
/// manuals do too.
#[test]
fn the_authoring_tools_point_at_the_doctrine() {
    let described: std::collections::HashMap<String, String> = declarations()
        .iter()
        .map(|d| {
            (
                d["function"]["name"].as_str().unwrap().to_string(),
                d["function"]["description"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for tool in [
        "workflow_step_schema",
        "workflow_create_draft",
        "workflow_update",
    ] {
        assert!(described[tool].contains("authoring_doctrine"), "{tool}");
    }
    for tool in ["workflow_create_draft", "workflow_update"] {
        let manual = super::super::tool_manual(Some(tool), None)["manual"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(manual.contains("authoring_doctrine"), "{tool}: {manual}");
    }
}
