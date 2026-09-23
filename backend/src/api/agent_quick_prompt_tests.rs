use super::*;
use crate::models::QuickPrompt;
use std::sync::Arc;

fn saved_prompt(id: &str, project_id: Option<&str>) -> QuickPrompt {
    serde_json::from_value(json!({
        "id": id, "name": "Résumé", "icon": "Q", "description": "Summarize a topic",
        "prompt_template": "Summarize {{topic}}", "project_id": project_id,
        "variables": [{"name":"topic","label":"Topic","placeholder":"Kronn","required":true}],
        "agent": "Custom", "connection_id": "saved-provider", "tier": "reasoning",
        "agent_settings": {"model":"saved-model","reasoning_effort":"high","max_tokens":3210},
        "skill_ids": ["skill"], "profile_ids": ["persona"], "directive_ids": ["rule"],
        "created_at":"2026-09-22T00:00:00Z", "updated_at":"2026-09-22T00:00:00Z"
    }))
    .unwrap()
}

pub(super) async fn state_with_prompts() -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        for project in ["a", "b"] {
            conn.execute("INSERT INTO projects (id,name,path,created_at,updated_at) VALUES (?1,?1,?1,'2026-09-22T00:00:00Z','2026-09-22T00:00:00Z')", [project])?;
        }
        for (id, project) in [("general", None), ("room-a", Some("a"))] {
            conn.execute("INSERT INTO discussions (id,title,project_id,created_at,updated_at) VALUES (?1,?1,?2,'2026-09-22T00:00:00Z','2026-09-22T00:00:00Z')", rusqlite::params![id, project])?;
        }
        let connection = serde_json::from_value(json!({
            "id":"saved-provider", "display_name":"Saved provider", "mention_alias":"saved",
            "endpoint":"http://127.0.0.1:1", "credential_slug":"saved", "origin_preset":"other",
            "created_at":"2026-09-22T00:00:00Z", "updated_at":"2026-09-22T00:00:00Z"
        })).unwrap();
        crate::db::external_api_connections::insert(conn, &connection)?;
        for (id, project) in [("global", None), ("qp-a", Some("a")), ("qp-b", Some("b"))] {
            crate::db::quick_prompts::insert_quick_prompt(conn, &saved_prompt(id, project))?;
        }
        Ok(())
    }).await.unwrap();
    let mut config = crate::core::config::default_config();
    config.encryption_secret = Some(crate::core::crypto::generate_secret());
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn call(executor: &KronnToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: name.into(),
            name: name.into(),
            arguments,
        })
        .await
}

#[tokio::test]
async fn native_signal_manual_reads_the_shared_registry_without_creating_an_action() {
    let state = state_with_prompts().await;
    let executor = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let manuals = call(&executor, "tool_manual", json!({})).await;
    assert!(manuals.content["available"]
        .as_array()
        .unwrap()
        .contains(&json!("signals")));
    let manual = call(&executor, "tool_manual", json!({"tool":"signals"})).await;
    assert!(manual.ok, "{}", manual.content);
    assert_eq!(
        manual.content["catalogue"],
        crate::api::signal_catalog::catalogue()
    );
    let actions: i64 = state
        .db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM discussion_actions", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(actions, 0);
}

fn attempted_overrides() -> Value {
    json!({
        "agent":"Codex", "connection_id":"injected", "tier":"economy",
        "agent_settings":{"model":"injected"}, "skill_ids":[], "profile_ids":[], "directive_ids":[],
        "name":"Nouveau", "description":"Changed", "icon":"N", "prompt_template":"New {{topic}}",
        "variables":[{"name":"topic","label":"Sujet","placeholder":"Release","required":true}]
    })
}

#[tokio::test]
async fn quick_prompt_authoring_filters_overrides_and_merges_the_stored_definition() {
    let state = state_with_prompts().await;
    let executor = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let created = call(&executor, "qp_create_draft", attempted_overrides()).await;
    assert!(created.ok, "{}", created.content);
    let created: QuickPrompt = serde_json::from_value(created.content).unwrap();
    assert_eq!(created.agent, AgentType::ClaudeCode);
    assert_eq!(created.tier, crate::models::ModelTier::Default);
    assert!(created.connection_id.is_none() && created.agent_settings.is_none());
    assert!(
        created.skill_ids.is_empty()
            && created.profile_ids.is_empty()
            && created.directive_ids.is_empty()
    );
    assert_eq!(
        (
            &*created.name,
            &*created.description,
            &*created.icon,
            &*created.prompt_template
        ),
        ("Nouveau", "Changed", "N", "New {{topic}}")
    );
    assert_eq!(created.variables[0].label, "Sujet");

    let before = saved_prompt("global", None);
    let mut arguments = attempted_overrides();
    arguments["qp_id"] = json!("global");
    arguments.as_object_mut().unwrap().remove("variables");
    let updated = call(&executor, "qp_update", arguments).await;
    assert!(updated.ok, "{}", updated.content);
    let updated: QuickPrompt = serde_json::from_value(updated.content).unwrap();
    assert_eq!(updated.name, "Nouveau");
    assert_eq!(updated.prompt_template, "New {{topic}}");
    assert_eq!(updated.agent, before.agent);
    assert_eq!(updated.connection_id, before.connection_id);
    assert_eq!(updated.tier, before.tier);
    assert_eq!(
        serde_json::to_value(updated.agent_settings).unwrap(),
        serde_json::to_value(before.agent_settings).unwrap()
    );
    assert_eq!(updated.skill_ids, before.skill_ids);
    assert_eq!(updated.profile_ids, before.profile_ids);
    assert_eq!(updated.directive_ids, before.directive_ids);
    assert_eq!(
        updated.variables[0].label, "Topic",
        "omitted variables survive"
    );
    let replaced = call(
        &executor,
        "qp_update",
        json!({"qp_id":"global", "variables":[]}),
    )
    .await;
    assert!(replaced.ok, "{}", replaced.content);
    assert_eq!(replaced.content["variables"], json!([]));
}

#[tokio::test]
async fn quick_prompt_scope_applies_to_list_create_update_and_run() {
    let state = state_with_prompts().await;
    for (room, expected, forbidden) in [
        ("general", vec!["global"], "qp-a"),
        ("room-a", vec!["global", "qp-a"], "qp-b"),
    ] {
        let executor = KronnToolExecutor::new(state.clone(), Some(room.into()));
        let listed = call(&executor, "qp_list", json!({})).await;
        assert!(listed.ok, "{}", listed.content);
        let mut ids = listed.content["quick_prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|qp| qp["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, expected);
        assert!(!listed.content.to_string().contains("prompt_template"));
        for name in ["qp_update", "qp_run"] {
            let denied = call(
                &executor,
                name,
                json!({"qp_id":forbidden, "name":"Intrusion", "vars":{"topic":"test"}}),
            )
            .await;
            assert!(!denied.ok, "{name}: {}", denied.content);
            assert!(denied.content.to_string().contains("scope"));
        }
        let denied = call(
            &executor,
            "qp_create_draft",
            json!({"name":"Foreign","prompt_template":"Hello","project_id":"b"}),
        )
        .await;
        assert!(
            !denied.ok && denied.content.to_string().contains("scope"),
            "{}",
            denied.content
        );
    }
    let executor = KronnToolExecutor::new(state.clone(), Some("room-a".into()));
    let created = call(
        &executor,
        "qp_create_draft",
        json!({"name":"Local","prompt_template":"Hello"}),
    )
    .await;
    assert!(created.ok, "{}", created.content);
    assert_eq!(created.content["project_id"], "a");
    let general = call(
        &executor,
        "qp_create_draft",
        json!({"name":"General","prompt_template":"Hello","project_id":null}),
    )
    .await;
    assert!(
        general.ok && general.content["project_id"].is_null(),
        "{}",
        general.content
    );
    for project in [json!(null), json!("b")] {
        let unchanged = call(
            &executor,
            "qp_update",
            json!({"qp_id":"qp-a","project_id":project}),
        )
        .await;
        assert!(unchanged.ok, "{}", unchanged.content);
        assert_eq!(unchanged.content["project_id"], "a");
    }
    let jobs = state
        .db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(Into::into)
        })
        .await
        .unwrap();
    assert_eq!(jobs, 0, "rejected runs enqueue nothing");
}

#[tokio::test]
async fn quick_prompt_run_preserves_configuration_and_queues_one_child_in_the_resolved_scope() {
    let state = state_with_prompts().await;
    let key = crate::core::crypto::parse_secret(
        state
            .config
            .read()
            .await
            .encryption_secret
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    for (room, qp_id, project) in [
        ("general", "global", None),
        ("room-a", "global", Some("a")),
        ("room-a", "qp-a", Some("a")),
    ] {
        let executor = KronnToolExecutor::new(state.clone(), Some(room.into()));
        let mut arguments = attempted_overrides();
        arguments["qp_id"] = json!(qp_id);
        arguments["vars"] = json!({"topic":"été"});
        arguments["project_id"] = json!("b");
        arguments["launch"] = json!({"discussion_id":"forged","project_id":"b"});
        let outcome = call(&executor, "qp_run", arguments).await;
        assert!(outcome.ok, "{}", outcome.content);
        let child_id = outcome.content["disc_id"].as_str().unwrap().to_owned();
        let (child, message, jobs) = state.db.with_conn(move |conn| {
            let child = crate::db::discussions::get_discussion(conn, &child_id)?.unwrap();
            let message = crate::db::discussions::list_messages(conn, &child_id)?.remove(0);
            let jobs = conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id=?1 AND status='Pending'", [&child_id], |row| row.get::<_, i64>(0))?;
            let values = crate::db::execution_variable_snapshots::load_values(conn, "quick_prompt", &child_id, &key, chrono::Utc::now())?.unwrap();
            let metadata = crate::db::execution_variable_snapshots::metadata(conn, "quick_prompt", &child_id)?.unwrap();
            let rendered = crate::models::render_quick_prompt_template_from_snapshot(&message.content, &values, &metadata.provenance);
            Ok((child, rendered, jobs))
        }).await.unwrap();
        assert_eq!(child.project_id.as_deref(), project);
        assert_eq!(child.agent, AgentType::Custom);
        assert_eq!(child.connection_id.as_deref(), Some("saved-provider"));
        assert_eq!(child.model.as_deref(), Some("saved-model"));
        assert_eq!(child.tier, crate::models::ModelTier::Reasoning);
        assert_eq!(child.skill_ids, vec!["skill"]);
        assert_eq!(child.profile_ids, vec!["persona"]);
        assert_eq!(child.directive_ids, vec!["rule"]);
        assert_eq!(message, "Summarize été");
        assert_eq!(jobs, 1);
    }
}

#[tokio::test]
async fn a_real_principal_loads_tool_families_but_workers_cannot_expand_their_catalogue() {
    let state = state_with_prompts().await;
    let principal = KronnToolExecutor::arc(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
    );
    for (family, _, expected) in TOOL_FAMILIES {
        let outcome = principal
            .execute(&ToolCall {
                id: "load".into(),
                name: "tools_load".into(),
                arguments: json!({"family":family}),
            })
            .await;
        assert!(outcome.ok, "{}", outcome.content);
        let loaded = outcome.content["__kronn_tools_add"].as_array().unwrap();
        assert_eq!(loaded.len(), expected.len(), "{family}");
        for name in *expected {
            assert!(
                loaded.iter().any(|tool| tool["function"]["name"] == *name),
                "{family}/{name}"
            );
        }
    }
    let unknown = principal
        .execute(&ToolCall {
            id: "index".into(),
            name: "tools_load".into(),
            arguments: json!({"family":"unknown"}),
        })
        .await;
    assert!(unknown.ok && unknown.content["families"].as_array().is_some());
    let worker = KronnToolExecutor::arc_for_worker_room(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
        None,
    );
    let workflow =
        KronnToolExecutor::workflow_arc(state, None, "workflow-run".into(), "step".into());
    for executor in [worker, workflow] {
        let denied = executor
            .execute(&ToolCall {
                id: "expand".into(),
                name: "tools_load".into(),
                arguments: json!({"family":"automations"}),
            })
            .await;
        assert!(
            !denied.ok && denied.content.get("__kronn_tools_add").is_none(),
            "{}",
            denied.content
        );
    }
}

#[test]
fn quick_prompt_declarations_are_loaded_together_and_exclude_destructive_or_batch_actions() {
    let family = declarations_for_family("automations");
    let worker = worker_room_catalogue(full_discussion_catalogue());
    let core = tiered(full_discussion_catalogue());
    for name in ["qp_list", "qp_run", "qp_create_draft", "qp_update"] {
        assert!(family.iter().any(|tool| tool["function"]["name"] == name));
        assert!(!worker.iter().any(|tool| tool["function"]["name"] == name));
        assert!(!core.iter().any(|tool| tool["function"]["name"] == name));
    }
    for name in ["qp_delete", "qp_batch_run"] {
        assert!(!full_discussion_catalogue()
            .iter()
            .any(|tool| tool["function"]["name"] == name));
    }
    for tool in family.iter().filter(|tool| {
        tool["function"]["name"]
            .as_str()
            .unwrap()
            .starts_with("qp_")
    }) {
        assert!(tool["function"]["parameters"]["properties"]
            .get("agent")
            .is_none());
    }
    assert!(quick_prompt_write_request(None, &json!({"name":"Invalid","prompt_template":"{{topic}}","variables":[{"name":"topic","required":true}]}), None).is_err());
    assert!(quick_prompt_write_request(None, &json!([]), None).is_err());
}

#[test]
fn every_declared_principal_tool_has_an_arm_in_its_selected_dispatcher() {
    // Structural coverage only: macros or extracted dispatch arms need an
    // explicit parser update. The real family-loading tests prove execution.
    use syn::visit::Visit;
    #[derive(Default)]
    struct NamedArms(std::collections::HashSet<String>);
    impl<'ast> Visit<'ast> for NamedArms {
        fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
            if matches!(node.op, syn::BinOp::Eq(_)) {
                if let (syn::Expr::Field(field), syn::Expr::Lit(literal)) =
                    (node.left.as_ref(), node.right.as_ref())
                {
                    if matches!(&field.member, syn::Member::Named(name) if name == "name") {
                        if let syn::Lit::Str(value) = &literal.lit {
                            self.0.insert(value.value());
                        }
                    }
                }
            }
        }
        fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
            if let syn::Expr::MethodCall(call) = node.expr.as_ref() {
                if call.method == "as_str" {
                    for arm in &node.arms {
                        self.visit_pat(&arm.pat);
                    }
                }
            }
        }
        fn visit_pat(&mut self, node: &'ast syn::Pat) {
            if let syn::Pat::Lit(literal) = node {
                if let syn::Lit::Str(value) = &literal.lit {
                    self.0.insert(value.value());
                }
            } else {
                syn::visit::visit_pat(self, node);
            }
        }
    }
    let source = syn::parse_file(include_str!("agent_tools.rs")).unwrap();
    let arms = source
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(item) => Some(&item.items),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            syn::ImplItem::Fn(method) => {
                let mut names = NamedArms::default();
                names.visit_block(&method.block);
                Some((method.sig.ident.to_string(), names.0))
            }
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    for (method, minimum) in [
        ("execute", 20),
        ("execute_workspace_tool", 10),
        ("execute_orchestration_tool", 5),
        ("execute_agent_resume_tool", 4),
    ] {
        assert!(
            arms.get(method).is_some_and(|names| names.len() >= minimum),
            "missing or unparsed dispatcher {method}"
        );
    }
    let mut declared = full_discussion_catalogue();
    assert!(
        declared.len() >= 50,
        "an empty or truncated catalogue cannot validate routing"
    );
    declared.push(tools_load_declaration());
    for tool in declared {
        let name = tool["function"]["name"].as_str().unwrap();
        let method = match tool_dispatch(name) {
            ToolDispatch::Core => "execute",
            ToolDispatch::Workspace => "execute_workspace_tool",
            ToolDispatch::Orchestration => "execute_orchestration_tool",
            ToolDispatch::Resume => "execute_agent_resume_tool",
            ToolDispatch::DiscussionRead | ToolDispatch::DiscussionList => continue,
        };
        assert!(
            arms[method].contains(name),
            "{name} is declared but has no named arm in {method}"
        );
    }
}

#[tokio::test]
async fn quick_prompt_saved_http_settings_reach_each_provider_turn_after_template_edit() {
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let provider = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let has_result = body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
            let frame = if has_result {
                json!({"choices":[{"index":0,"delta":{"content":"Saved settings applied."},"finish_reason":"stop"}]})
            } else {
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"read-schema","type":"function","function":{"name":"workflow_step_schema","arguments":"{\"step_type\":\"JsonData\"}"}}]},"finish_reason":"tool_calls"}]})
            };
            ResponseTemplate::new(200).set_body_string(format!("data: {frame}\n\ndata: [DONE]\n\n"))
        }).expect(2).mount(&provider).await;
    let state = state_with_prompts().await;
    let endpoint = provider.uri();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE external_api_connections SET endpoint=?1 WHERE id='saved-provider'",
                [endpoint],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let executor = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let run = call(
        &executor,
        "qp_run",
        json!({"qp_id":"global","vars":{"topic":"fixture"}}),
    )
    .await;
    assert!(run.ok, "{}", run.content);
    let child = run.content["disc_id"].as_str().unwrap().to_string();
    state
        .db
        .with_conn(|conn| {
            let mut qp = crate::db::quick_prompts::get_quick_prompt(conn, "global")?.unwrap();
            qp.agent_settings.as_mut().unwrap().reasoning_effort = Some("low".into());
            qp.agent_settings.as_mut().unwrap().max_tokens = Some(7);
            // Exercise a real stored-template edit after the run was queued.
            crate::db::quick_prompts::update_quick_prompt(conn, &qp)?;
            Ok(())
        })
        .await
        .unwrap();
    let stream = crate::api::discussions::streaming::make_agent_stream(state, child, None).await;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream.into_response().into_body().collect(),
    )
    .await
    .expect("bounded fixture completion")
    .unwrap()
    .to_bytes();
    let events = String::from_utf8_lossy(&body);
    assert!(events.contains("Saved settings applied."), "{events}");
    let requests = provider.received_requests().await.unwrap();
    let chats: Vec<Value> = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/chat/completions")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(chats.len(), 2);
    for body in chats {
        assert_eq!(body["model"], "saved-model");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_tokens"], 3210);
    }
}

#[tokio::test]
async fn quick_prompt_unsupported_saved_limit_refuses_without_a_child_or_dispatch_job() {
    let state = state_with_prompts().await;
    state
        .db
        .with_conn(|conn| {
            let mut qp = crate::db::quick_prompts::get_quick_prompt(conn, "global")?.unwrap();
            qp.agent = AgentType::ClaudeCode;
            qp.connection_id = None;
            crate::db::quick_prompts::update_quick_prompt(conn, &qp)
        })
        .await
        .unwrap();
    let executor = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let outcome = call(
        &executor,
        "qp_run",
        json!({"qp_id":"global","vars":{"topic":"fixture"}}),
    )
    .await;
    assert!(!outcome.ok);
    assert!(
        outcome
            .content
            .to_string()
            .contains("cannot apply max_tokens"),
        "{}",
        outcome.content
    );
    state
        .db
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row("SELECT count(*) FROM discussions", [], |row| row
                    .get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                conn.query_row("SELECT count(*) FROM agent_dispatch_jobs", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                conn.query_row("SELECT count(*) FROM workflow_runs", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn quick_prompt_native_run_captures_the_saved_cli_effort_before_dispatch() {
    let state = state_with_prompts().await;
    state
        .db
        .with_conn(|conn| {
            let mut qp = crate::db::quick_prompts::get_quick_prompt(conn, "global")?.unwrap();
            qp.agent = AgentType::ClaudeCode;
            qp.connection_id = None;
            qp.agent_settings.as_mut().unwrap().max_tokens = None;
            crate::db::quick_prompts::update_quick_prompt(conn, &qp)
        })
        .await
        .unwrap();
    let executor = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let outcome = call(
        &executor,
        "qp_run",
        json!({"qp_id":"global","vars":{"topic":"fixture"}}),
    )
    .await;
    assert!(outcome.ok, "{}", outcome.content);
    let child = outcome.content["disc_id"].as_str().unwrap().to_string();
    let effort = state
        .db
        .with_conn(move |conn| {
            crate::db::discussion_effort::for_run(
                conn,
                &child,
                &AgentType::ClaudeCode,
                crate::models::ModelTier::Reasoning,
                Some("saved-model"),
            )
        })
        .await
        .unwrap();
    assert_eq!(effort.as_deref(), Some("high"));
}
