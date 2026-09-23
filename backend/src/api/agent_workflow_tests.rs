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
    let global = call(&local, "workflow_create_draft", global).await;
    assert!(global.ok, "{}", global.content);
    assert_eq!(global.content["concurrency_limit"], 1);
    let list = call(&general, "workflow_list", json!({})).await;
    assert!(list.ok);
    assert_eq!(list.content["workflows"].as_array().unwrap().len(), 1);
    assert_eq!(list.content["workflows"][0]["id"], global.content["id"]);
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
    let schema = crate::api::workflows::canonical_step_schema();
    assert_eq!(schema_section(&json!({})).unwrap(), schema);
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
    for section in ["template_vars", "data_pipeline_contract"] {
        assert_eq!(
            schema_section(&json!({"section":section})).unwrap()["contract"],
            schema[section]
        );
    }
    assert!(schema_section(&json!({"step_type":"invented"})).is_err());
    assert!(schema_section(&json!({"section":"invented"})).is_err());
}
