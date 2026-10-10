use super::*;

pub(super) async fn call(executor: &dyn ToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: name.into(),
            name: name.into(),
            arguments,
        })
        .await
}

fn page() -> Value {
    json!({"title":"Mon suivi été", "slug":"mon-suivi", "html":"<!doctype html><h1>Été 🦀</h1>",
        "datasets":[{"name":"summary","kind":"snapshot","initial":{"count":0}}]})
}

#[tokio::test]
async fn native_pages_follow_mcp_project_scope_inheritance_and_selector_contracts() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let local = KronnToolExecutor::arc(
        state.clone(),
        Some("room-a".into()),
        AgentType::Ollama,
        None,
        None,
    );
    let general = KronnToolExecutor::new(state.clone(), None);
    let mut request = page();
    request["created_by_agent"] = json!("Forged");
    request["source_message_id"] = json!("not-an-authoring-field");
    let created = call(local.as_ref(), "page_create", request).await;
    assert!(created.ok, "{}", created.content);
    assert_eq!(created.content["project_id"], "a");
    assert_eq!(created.content["revision"]["created_by_agent"], "Ollama");
    let id = created.content["id"].clone();
    let same_project = KronnToolExecutor::new(state.clone(), Some("room-a".into()));
    let by_id = call(&same_project, "page_get", json!({"page_id":id})).await;
    let by_slug = call(&same_project, "page_get", json!({"page_id":" mon-suivi "})).await;
    let by_alias = call(&same_project, "page_get", json!({"id":"mon-suivi"})).await;
    assert!(by_id.ok, "{}", by_id.content);
    assert_eq!(by_id.content, by_slug.content);
    assert_eq!(by_id.content, by_alias.content);
    assert_eq!(by_id.content["discussions"][0]["discussion_id"], "room-a");
    assert_eq!(by_id.content["discussions"][0]["relation"], "created_from");
    assert_eq!(by_id.content["workflows"], json!([]));
    assert_eq!(by_id.content["trusted_actions"], json!([]));
    let list = call(&same_project, "page_list", json!({})).await;
    assert!(list.ok);
    assert_eq!(list.content.as_array().unwrap().len(), 1);
    assert_eq!(list.content[0]["id"], id);
    assert!(list.content[0].get("revision").is_none());

    // Another scope neither sees nor edits the project's Page.
    let hidden = call(&general, "page_list", json!({})).await;
    assert_eq!(hidden.content, json!([]));
    for (name, arguments) in [
        ("page_get", json!({"page_id":"mon-suivi"})),
        (
            "page_update_html",
            json!({"page_id":"mon-suivi","html":"<h1>x</h1>"}),
        ),
        (
            "page_add_dataset",
            json!({"page_id":"mon-suivi","name":"x","kind":"snapshot"}),
        ),
    ] {
        let refused = call(&general, name, arguments).await;
        assert!(!refused.ok, "{name}: {}", refused.content);
    }

    // Bindings cannot leave the executor's scope.
    for (field, value) in [("project_id", "b"), ("discussion_id", "general")] {
        let mut explicit = page();
        explicit["slug"] = json!("explicit");
        explicit[field] = json!(value);
        let refused = call(local.as_ref(), "page_create", explicit).await;
        assert!(!refused.ok, "{field}: {}", refused.content);
    }
    assert!(
        !call(&same_project, "page_get", json!({"page_id":"explicit"}))
            .await
            .ok
    );
    let mut standalone = page();
    standalone["slug"] = json!("standalone");
    standalone["datasets"] = json!([]);
    assert!(call(&general, "page_create", standalone).await.ok);
    let standalone = call(&general, "page_get", json!({"page_id":"standalone"})).await;
    assert_eq!(standalone.content["project_id"], Value::Null);
    // A project-less Page stays reachable from a project scope.
    assert!(
        call(&same_project, "page_get", json!({"page_id":"standalone"}))
            .await
            .ok
    );
    assert_eq!(standalone.content["discussions"], json!([]));
    assert_eq!(standalone.content["datasets"], json!([]));
}

#[tokio::test]
async fn native_page_validation_failures_leave_no_partial_page() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::new(state, Some("room-a".into()));
    for (field, value) in [
        ("title", json!(" ")),
        ("title", json!("é".repeat(201))),
        ("html", json!("\n")),
        ("html", json!("x".repeat(1_000_001))),
        ("html", json!(42)),
        ("slug", json!("Bad--slug")),
        ("datasets", Value::Null),
        ("datasets", json!({})),
        ("datasets", json!([{"name":"bad name","kind":"snapshot"}])),
        ("datasets", json!([{"name":"a","kind":"unknown"}])),
        (
            "datasets",
            json!([{"name":"a","kind":"snapshot","max_points":0}]),
        ),
        (
            "datasets",
            json!([{"name":"a","kind":"snapshot","max_age_days":0}]),
        ),
        (
            "datasets",
            json!([{"name":"a","kind":"snapshot"},{"name":"a","kind":"snapshot"}]),
        ),
        ("project_id", json!("missing-project")),
        ("discussion_id", json!("missing-room")),
    ] {
        let mut request = page();
        request[field] = value;
        let result = call(&executor, "page_create", request).await;
        assert!(!result.ok, "{field}: {}", result.content);
        assert!(result.content["error"].is_string());
        assert_eq!(
            call(&executor, "page_list", json!({})).await.content,
            json!([])
        );
    }
    let mut absent = page();
    absent.as_object_mut().unwrap().remove("datasets");
    assert!(!call(&executor, "page_create", absent).await.ok);
    for name in ["page_get", "page_update_html", "page_add_dataset"] {
        for selector in [Value::Null, json!(" "), json!("missing"), json!(3)] {
            let result = call(
                &executor,
                name,
                json!({"page_id":selector,"html":"<h1>Hi</h1>","name":"a","kind":"snapshot"}),
            )
            .await;
            assert!(!result.ok, "{name}: {}", result.content);
        }
    }
    let mut boundary = page();
    boundary["title"] = json!("é".repeat(200));
    boundary["html"] = json!("é".repeat(500_000));
    let created = call(&executor, "page_create", boundary).await;
    assert!(created.ok, "{}", created.content);
    assert!(
        !call(&executor, "page_create", page()).await.ok,
        "duplicate slug"
    );
}

#[tokio::test]
async fn native_page_revisions_and_dataset_conflicts_preserve_history() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::arc(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
    );
    let created = call(executor.as_ref(), "page_create", page()).await;
    assert!(created.ok);
    let id = created.content["id"].as_str().unwrap().to_owned();
    let dataset = json!({"page_id":id,"name":"trend","kind":"time_series","initial":[{"v":1},{"v":2}],"schema":{"type":"object"},"max_points":3,"max_age_days":7});
    let added = call(executor.as_ref(), "page_add_dataset", dataset.clone()).await;
    assert!(added.ok, "{}", added.content);
    assert_eq!(added.content["max_points"], 3);
    assert_eq!(added.content["max_age_days"], 7);
    let mut repeat = dataset.clone();
    repeat["initial"] = json!([{"v":999}]);
    assert_eq!(
        call(executor.as_ref(), "page_add_dataset", repeat)
            .await
            .content,
        added.content
    );
    for (field, value) in [
        ("kind", json!("snapshot")),
        ("kind", json!("bad")),
        ("name", json!(" ")),
        ("max_points", json!(0)),
        ("max_points", json!(-1)),
        ("max_age_days", json!(0)),
    ] {
        let mut invalid = dataset.clone();
        invalid[field] = value;
        assert!(
            !call(executor.as_ref(), "page_add_dataset", invalid)
                .await
                .ok
        );
    }
    let before = call(executor.as_ref(), "page_get", json!({"page_id":id})).await;
    let html = "<!doctype html><h1>New 🦀</h1><script>window.KronnPageData</script>";
    let update = call(
        executor.as_ref(),
        "page_update_html",
        json!({"page_id":"mon-suivi","html":html,"created_by_agent":"Forged"}),
    )
    .await;
    assert!(update.ok, "{}", update.content);
    assert_eq!(update.content["revision"], 2);
    assert_eq!(update.content["created_by_agent"], "Ollama");
    for html in [json!(" "), json!("é".repeat(500_001)), json!(false)] {
        assert!(
            !call(
                executor.as_ref(),
                "page_update_html",
                json!({"page_id":id,"html":html})
            )
            .await
            .ok
        );
    }
    let after = call(executor.as_ref(), "page_get", json!({"page_id":id})).await;
    assert_eq!(after.content["datasets"], before.content["datasets"]);
    assert_eq!(
        after.content["data_revision"],
        before.content["data_revision"]
    );
    assert_eq!(after.content["revision"]["html"], html);
    let revisions = state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_revisions(conn, &id))
        .await
        .unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[1].html, page()["html"].as_str().unwrap());
}

#[tokio::test]
async fn native_page_html_uses_shared_inert_action_ingestion() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::new(state.clone(), None);
    let mut request = page();
    let html = r#"<button data-kronn-action="run">Run</button><script type="application/kronn-action" data-action-id="run">{"kind":"quick_prompt","target_id":"global"}</script><script type="application/kronn-action" data-action-id="bad">{"kind":"workflow","target_id":"missing"}</script><script>fetch('https://example.com')</script>"#;
    request["html"] = json!(html);
    let created = call(&executor, "page_create", request).await;
    assert!(created.ok, "{}", created.content);
    let id = created.content["id"].as_str().unwrap().to_owned();
    assert!(
        call(
            &executor,
            "page_update_html",
            json!({"page_id":id,"html":html})
        )
        .await
        .ok
    );
    let (actions, launches) = state.db.with_read_conn(move |conn| {
        let actions = crate::db::live_page_actions::list_for_live_page(conn, &id)?;
        let launches = conn.query_row("SELECT (SELECT COUNT(*) FROM shared_runs) + (SELECT COUNT(*) FROM agent_dispatch_jobs)",[],|row|row.get::<_,i64>(0))?;
        Ok((serde_json::to_value(actions)?, launches))
    }).await.unwrap();
    assert_eq!(actions.as_array().unwrap().len(), 2);
    let valid = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["action_ref"] == "run")
        .unwrap();
    let invalid = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["action_ref"] == "bad")
        .unwrap();
    assert_eq!(valid["state"], "proposed");
    assert_eq!(invalid["state"], "preflight_failed");
    assert_eq!(
        launches, 0,
        "HTML ingestion must never launch an automation"
    );
}

#[tokio::test]
async fn native_page_family_loads_and_dispatches_all_five_operations() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor =
        KronnToolExecutor::arc(state, Some("general".into()), AgentType::Ollama, None, None);
    let full = full_discussion_catalogue();
    let core = tiered(full.clone());
    let family = declarations_for_family("pages");
    assert_eq!(family, declarations());
    for tool in &family {
        let name = &tool["function"]["name"];
        assert!(full.iter().any(|t| &t["function"]["name"] == name));
        assert!(!core.iter().any(|t| &t["function"]["name"] == name));
    }
    let loaded = call(executor.as_ref(), "tools_load", json!({"family":"pages"})).await;
    assert!(loaded.ok, "{}", loaded.content);
    assert_eq!(loaded.content["__kronn_tools_add"], json!(family));
    for (name, arguments) in [
        ("page_list", json!({})),
        ("page_create", page()),
        ("page_get", json!({"page_id":"mon-suivi"})),
        (
            "page_update_html",
            json!({"page_id":"mon-suivi","html":"<h1>Updated</h1>"}),
        ),
        (
            "page_add_dataset",
            json!({"page_id":"mon-suivi","name":"items","kind":"collection","initial":[]}),
        ),
    ] {
        let result = call(executor.as_ref(), name, arguments).await;
        assert!(result.ok, "{name}: {}", result.content);
    }
}

#[test]
fn native_page_surface_has_a_bounded_measured_cost() {
    let full = full_discussion_catalogue();
    let without_pages = full
        .iter()
        .filter(|t| !t["function"]["name"].as_str().unwrap().starts_with("page_"))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&full).unwrap().len();
    let before = serde_json::to_vec(&without_pages).unwrap().len();
    let core = serde_json::to_vec(&tiered(full.clone())).unwrap().len();
    let page_bytes = serde_json::to_vec(&declarations()).unwrap().len();
    if let Ok(path) = std::env::var("KRONN_PAGE_CONTRACT_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&declarations()).unwrap()).unwrap();
    }
    println!("native Page surface: full={} tools/{} B; without Pages={} B; delta={} B; opt-in core={} B; Page family={} B",full.len(),bytes,before,bytes-before,core,page_bytes);
    assert!(
        page_bytes <= 4_000,
        "Page family grew to {page_bytes} B; keep detail in tool_manual"
    );
    for name in ["page_create", "page_update_html"] {
        assert!(tool_manual(Some(name))["manual"]
            .as_str()
            .unwrap()
            .contains("HTML"));
    }
}

#[tokio::test]
async fn native_workflow_page_tools_inherit_run_scope_and_execute_all_operations() {
    for project in [Some("a".to_owned()), None] {
        let state = super::super::quick_prompt_tests::state_with_prompts().await;
        let executor = KronnToolExecutor::workflow_arc(
            state.clone(),
            project.clone(),
            "fixture-run".into(),
            "write-page".into(),
        );
        let catalogue = executor.catalogue();
        for name in [
            "page_list",
            "page_get",
            "page_create",
            "page_update_html",
            "page_add_dataset",
            "tool_manual",
        ] {
            assert!(catalogue
                .iter()
                .any(|tool| tool["function"]["name"] == name));
        }

        let created = call(executor.as_ref(), "page_create", page()).await;
        assert!(created.ok, "{}", created.content);
        assert_eq!(created.content["project_id"], json!(project));
        let id = created.content["id"].clone();
        let reader_room = if project.is_some() {
            "room-a"
        } else {
            "general"
        };
        let reader = KronnToolExecutor::new(state, Some(reader_room.into()));
        let before = call(&reader, "page_get", json!({"page_id":id})).await;
        assert!(before.ok);
        assert_eq!(before.content["discussions"], json!([]));
        let before_list = call(&reader, "page_list", json!({})).await;
        assert!(before_list.ok);
        let list = call(executor.as_ref(), "page_list", json!({})).await;
        assert!(list.ok);
        assert_eq!(list.content, before_list.content);
        assert_eq!(list.content.as_array().unwrap().len(), 1);
        assert_eq!(list.content[0]["id"], id);
        for selector in [id.clone(), json!("mon-suivi")] {
            let detail = call(executor.as_ref(), "page_get", json!({"page_id":selector})).await;
            assert!(detail.ok);
            assert_eq!(detail.content, before.content);
        }
        for name in ["page_create", "page_update_html"] {
            let manual = call(executor.as_ref(), "tool_manual", json!({"tool":name})).await;
            assert!(manual.ok, "{}", manual.content);
            assert!(manual.content["manual"].as_str().unwrap().contains("HTML"));
        }

        let updated = call(
            executor.as_ref(),
            "page_update_html",
            json!({"page_id":id,"html":"<h1>Workflow Page</h1>"}),
        )
        .await;
        assert!(updated.ok, "{}", updated.content);
        let added = call(
            executor.as_ref(),
            "page_add_dataset",
            json!({"page_id":id,"name":"items","kind":"collection"}),
        )
        .await;
        assert!(added.ok, "{}", added.content);
        let after = call(executor.as_ref(), "page_get", json!({"page_id":id})).await;
        assert!(after.ok);
        assert_eq!(after.content["revision"]["html"], "<h1>Workflow Page</h1>");
        assert_ne!(
            after.content["revision"]["id"],
            before.content["revision"]["id"]
        );
        assert_eq!(after.content["datasets"].as_array().unwrap().len(), 2);
        assert!(after.content["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dataset| dataset["name"] == "items" && dataset["kind"] == "collection"));
        assert_eq!(after.content["discussions"], json!([]));
        assert!(
            !call(
                executor.as_ref(),
                "page_update_html",
                json!({"page_id":id,"html":""})
            )
            .await
            .ok
        );
        assert_eq!(
            call(&reader, "page_get", json!({"page_id":id}))
                .await
                .content,
            after.content
        );
        assert!(
            !call(
                executor.as_ref(),
                "task_create",
                json!({"title":"Outside workflow scope"})
            )
            .await
            .ok
        );
    }
}

/// KT-1098 — after a real rename, native calls by the old slug read, revise and
/// rename the page again; another project's scope is refused by either slug.
#[tokio::test]
async fn native_pages_follow_a_renamed_slug_within_their_scope() {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id,title,project_id,created_at,updated_at) \
                 VALUES ('room-b','room-b','b','2026-09-22T00:00:00Z','2026-09-22T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let own = KronnToolExecutor::new(state.clone(), Some("room-a".into()));
    let other = KronnToolExecutor::new(state.clone(), Some("room-b".into()));
    let created = call(&own, "page_create", page()).await;
    assert!(created.ok, "{}", created.content);
    let id = created.content["id"].as_str().unwrap().to_owned();

    let renamed = call(
        &own,
        "page_update_html",
        json!({"page_id":"mon-suivi","slug":"suivi-a"}),
    )
    .await;
    assert!(renamed.ok, "{}", renamed.content);
    assert_eq!(renamed.content["slug"], "suivi-a");
    assert_eq!(renamed.content["slug_aliases"], json!(["mon-suivi"]));
    assert!(renamed.content.get("revision").is_none());

    let read = call(&own, "page_get", json!({"page_id":"mon-suivi"})).await;
    assert_eq!(read.content["id"], id.as_str(), "{}", read.content);
    let revised = call(
        &own,
        "page_update_html",
        json!({"page_id":"mon-suivi","html":"<p>v2</p>"}),
    )
    .await;
    assert_eq!(revised.content["revision"], 2, "{}", revised.content);
    let again = call(
        &own,
        "page_update_html",
        json!({"page_id":"mon-suivi","slug":"suivi-b","html":"<p>v3</p>"}),
    )
    .await;
    assert_eq!(again.content["revision"], 3, "{}", again.content);
    let read = call(&own, "page_get", json!({"page_id":"suivi-a"})).await;
    assert_eq!(read.content["slug"], "suivi-b");

    // Invalid arguments are refused before any write.
    for arguments in [
        json!({"page_id":"suivi-b","slug":"suivi-c","html":"  "}),
        json!({"page_id":"suivi-b","slug":"suivi-c","html":"é".repeat(500_001)}),
        json!({"page_id":"suivi-b","slug":7}),
        json!({"page_id":"suivi-b"}),
    ] {
        let refused = call(&own, "page_update_html", arguments).await;
        assert!(!refused.ok, "{}", refused.content);
    }
    let unchanged = call(&own, "page_get", json!({"page_id":id})).await;
    assert_eq!(unchanged.content["slug"], "suivi-b");
    assert_eq!(unchanged.content["revision"]["revision"], 3);

    for selector in ["mon-suivi", "suivi-a", "suivi-b", id.as_str()] {
        for (name, arguments) in [
            ("page_get", json!({"page_id":selector})),
            (
                "page_update_html",
                json!({"page_id":selector,"slug":"taken-over"}),
            ),
            (
                "page_update_html",
                json!({"page_id":selector,"html":"<p>x</p>"}),
            ),
        ] {
            let refused = call(&other, name, arguments).await;
            assert!(!refused.ok, "{name} {selector}: {}", refused.content);
        }
    }
    let unchanged = call(&own, "page_get", json!({"page_id":id})).await;
    assert_eq!(unchanged.content["slug"], "suivi-b");
}
