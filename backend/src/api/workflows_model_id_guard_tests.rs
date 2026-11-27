//! KT-1998 — every model id a workflow saves, or a launch admits, goes
//! through the shared rule. A plain `#[cfg(test)]` module, so the
//! `model_id_guard` filter runs it with or without the `full-tests` feature.

use super::*;

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

/// A dash-led id, line breaks or a tab around an id, and a long run of
/// spaces before one: each is refused on a model that is stored as given.
fn bad_models() -> Vec<String> {
    vec![
        "--help".into(),
        " -c".into(),
        "\ngpt-5\n".into(),
        "\tgpt-5".into(),
        format!("{}gpt-5", " ".repeat(100_000)),
    ]
}

fn assert_model_refusal(error: &str, bad: &str) {
    assert!(
        error.contains("must not start with '-'") || error.contains("one line"),
        "{bad:?}: {error}"
    );
}

async fn stored(state: &AppState) -> Vec<Workflow> {
    state
        .db
        .with_conn(crate::db::workflows::list_workflows)
        .await
        .unwrap()
}

fn base() -> WorkflowStep {
    WorkflowStep {
        name: "ask".into(),
        step_type: StepType::Agent,
        agent: AgentType::OpenCode,
        prompt_template: "Say hello".into(),
        ..Default::default()
    }
}

fn with_model(model: &str) -> WorkflowStep {
    let mut step = base();
    step.agent_settings = Some(AgentSettings {
        model: Some(model.into()),
        ..AgentSettings::default()
    });
    step
}

fn shadow_agent(agent: AgentType, model: &str) -> crate::models::ShadowAgent {
    crate::models::ShadowAgent {
        agent,
        model: Some(model.into()),
        connection_id: None,
        reasoning_effort: None,
    }
}

fn shadowed(model: &str) -> WorkflowStep {
    let mut step = base();
    step.shadows = vec![crate::models::StepShadow {
        id: "alt".into(),
        agent: Some(shadow_agent(AgentType::GeminiCli, model)),
        ..Default::default()
    }];
    step
}

fn merged(model: &str) -> WorkflowStep {
    let mut step = shadowed("gemini-2.5-pro");
    step.shadow_merge = Some(crate::models::ShadowMerge {
        agent: Some(shadow_agent(AgentType::ClaudeCode, model)),
        ..Default::default()
    });
    step
}

fn request(steps: Vec<WorkflowStep>) -> CreateWorkflowRequest {
    serde_json::from_value(serde_json::json!({
        "name": "guarded", "project_id": null, "trigger": {"type": "Manual"},
        "steps": steps
    }))
    .unwrap()
}

async fn create_plain(state: &AppState) -> Workflow {
    let Json(created) = create_as(
        state.clone(),
        request(vec![with_model("gpt-5")]),
        WorkflowWriter::Human,
    )
    .await;
    created.data.expect("a plain id is saved")
}

fn kept_model(workflow: &Workflow) -> Option<&str> {
    workflow.steps[0]
        .agent_settings
        .as_ref()
        .and_then(|settings| settings.model.as_deref())
}

/// A workflow save refuses a bad model on a step, a shadow or the merge pass,
/// from an agent or a human, and writes nothing.
#[tokio::test]
async fn model_id_guard_workflow_create_refuses_step_shadow_and_merge_models() {
    let state = state();
    for bad in bad_models() {
        for (writer, step) in [
            (WorkflowWriter::Agent, with_model(&bad)),
            (WorkflowWriter::Human, with_model(&bad)),
            (WorkflowWriter::Human, shadowed(&bad)),
            (WorkflowWriter::Human, merged(&bad)),
        ] {
            let Json(refused) = create_as(state.clone(), request(vec![step]), writer).await;
            assert_model_refusal(&refused.error.expect("refused"), &bad);
        }
    }
    assert!(stored(&state).await.is_empty(), "create wrote nothing");
}

/// The full update refuses a bad step model and keeps the stored one.
#[tokio::test]
async fn model_id_guard_workflow_update_refuses_a_bad_step_model() {
    let state = state();
    let saved = create_plain(&state).await;
    for bad in bad_models() {
        let Json(refused) = update_as(
            state.clone(),
            saved.id.clone(),
            serde_json::from_value(serde_json::json!({ "steps": [with_model(&bad)] })).unwrap(),
            WorkflowWriter::Human,
        )
        .await;
        assert_model_refusal(&refused.error.expect("update refuses"), &bad);
    }
    assert_eq!(kept_model(&stored(&state).await[0]), Some("gpt-5"));
}

/// The single-step PATCH refuses a bad model and keeps the stored one.
#[tokio::test]
async fn model_id_guard_workflow_step_patch_refuses_a_bad_step_model() {
    let state = state();
    let saved = create_plain(&state).await;
    for bad in bad_models() {
        let patch: UpdateWorkflowStepRequest = serde_json::from_value(serde_json::json!({
            "step_name": "ask",
            "fields": { "agent_settings": { "model": bad } },
        }))
        .unwrap();
        let Json(refused) = update_step(
            State(state.clone()),
            Path(saved.id.clone()),
            None,
            None,
            Json(patch),
        )
        .await;
        assert_model_refusal(&refused.error.expect("the patch refuses"), &bad);
    }
    assert_eq!(kept_model(&stored(&state).await[0]), Some("gpt-5"));
}

fn export_content(workflow: Workflow, prompts: Vec<QuickPrompt>) -> String {
    serde_json::to_string(&WorkflowExportEnvelope {
        referenced_skills: vec![],
        referenced_directives: vec![],
        referenced_profiles: vec![],
        kind: WORKFLOW_EXPORT_KIND.into(),
        version: EXPORT_VERSION,
        exported_at: chrono::Utc::now(),
        workflow,
        referenced_quick_prompts: prompts,
        referenced_quick_apis: vec![],
        referenced_quick_execs: vec![],
        referenced_pages: vec![],
        referenced_workflows: vec![],
        redacted_fields: vec![],
        not_carried: Vec::new(),
    })
    .unwrap()
}

/// A workflow import refuses a bad model on a step and on a bundled Quick
/// Prompt; nothing is written.
#[tokio::test]
async fn model_id_guard_workflow_import_refuses_step_and_bundled_prompt_models() {
    let state = state();
    let template = create_plain(&state).await;
    let prompt: QuickPrompt = serde_json::from_value(serde_json::json!({
        "id": "qp-src", "name": "Bundled", "icon": "✨",
        "prompt_template": "Say hello", "variables": [], "agent": "OpenCode",
        "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now(),
    }))
    .unwrap();
    for bad in bad_models() {
        let mut workflow = template.clone();
        workflow.steps = vec![with_model(&bad)];
        let mut bad_prompt = prompt.clone();
        bad_prompt.agent_settings = Some(AgentSettings {
            model: Some(bad.clone()),
            ..AgentSettings::default()
        });
        for content in [
            export_content(workflow, vec![prompt.clone()]),
            export_content(template.clone(), vec![bad_prompt]),
        ] {
            let Json(refused) = import_workflow(
                State(state.clone()),
                None,
                None,
                Json(ImportWorkflowRequest {
                    skill_choices: vec![],
                    content,
                    project_id: None,
                }),
            )
            .await;
            assert_model_refusal(&refused.error.expect("the import refuses"), &bad);
        }
    }
    assert_eq!(
        stored(&state).await.len(),
        1,
        "the import wrote no workflow"
    );
    let prompts = state
        .db
        .with_conn(crate::db::quick_prompts::list_quick_prompts)
        .await
        .unwrap();
    assert!(prompts.is_empty(), "the import wrote no Quick Prompt");
}

/// A delegated worker's model (mapped or default) and a Decide step's model
/// go through the shared rule, on create and through the step check every
/// save (create, update, patch, import) runs.
#[tokio::test]
async fn model_id_guard_workflow_delegate_worker_and_decide_models_are_refused() {
    let state = state();
    for bad in bad_models() {
        let worker = crate::models::DelegateWorker {
            agent: AgentType::Codex,
            tier: None,
            model: Some(bad.clone()),
        };
        let mut mapped = base();
        mapped.step_type = StepType::DelegateSubtasks;
        mapped.delegate_subtasks = Some(crate::models::DelegateSubtasksConfig {
            parent_task: "KT-1".into(),
            worker_map: [("back".to_string(), worker.clone())].into(),
            ..Default::default()
        });
        let mut defaulted = mapped.clone();
        defaulted.delegate_subtasks = Some(crate::models::DelegateSubtasksConfig {
            parent_task: "KT-1".into(),
            default_worker: Some(worker),
            ..Default::default()
        });
        for step in [mapped, defaulted] {
            let error = validate_step_models(&step).expect_err("a worker model is refused");
            assert!(error.contains("worker `"), "{bad:?}: {error}");
            let Json(refused) =
                create_as(state.clone(), request(vec![step]), WorkflowWriter::Human).await;
            assert_model_refusal(&refused.error.expect("create refuses"), &bad);
        }
        let mut decide = base();
        decide.decide = Some(crate::models::DecideConfig {
            model: Some(bad.clone()),
            ..Default::default()
        });
        let error = validate_step_models(&decide).expect_err("a Decide model is refused");
        assert!(error.contains("decide"), "{bad:?}: {error}");
        assert_model_refusal(&error, &bad);
    }
    assert!(stored(&state).await.is_empty(), "create wrote nothing");
}

/// A launch's per-step agent choice (`step_agents`) is refused before the
/// run row exists.
#[tokio::test]
async fn model_id_guard_workflow_trigger_refuses_a_step_agent_model() {
    let state = state();
    let saved = create_plain(&state).await;
    let count_runs = |state: AppState| async move {
        state
            .db
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM workflow_runs", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap()
    };
    for bad in ["--help", " -c", "gpt\n5", "sonnet\u{0}x"] {
        let mut launch = crate::core::launch_context::LaunchContext::default();
        launch.step_agents.insert(
            "ask".into(),
            StepAgentOverride {
                agent: AgentType::Codex,
                model: Some(bad.into()),
                reasoning_effort: None,
            },
        );
        let error = create_manual_run(
            &state,
            &saved.id,
            Default::default(),
            Default::default(),
            launch,
        )
        .await
        .expect_err("the launch is refused");
        assert!(error.contains("step_agents.ask.model"), "{bad:?}: {error}");
        assert_model_refusal(&error, bad);
    }
    assert_eq!(count_runs(state.clone()).await, 0, "no run row was created");
}
