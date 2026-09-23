use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "real local workflow authoring; KRONN_WORKFLOW_BENCH=1 and KRONN_BENCH_MODEL required"]
async fn bench_native_three_step_workflow() {
    use crate::agents::runner::{
        parse_http_turn_telemetry, start_agent_with_config, AgentStartConfig,
    };
    assert_eq!(std::env::var("KRONN_WORKFLOW_BENCH").as_deref(), Ok("1"));
    let model = std::env::var("KRONN_BENCH_MODEL").expect("explicit model");
    let state = super::quick_prompt_tests::state_with_prompts().await;
    let directory = tempfile::tempdir().unwrap();
    let executor = KronnToolExecutor::arc(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
    );
    let catalogue = executor.catalogue();
    let catalogue_bytes = serde_json::to_vec(&catalogue).unwrap().len();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tools = RecordingTools {
        executor,
        catalogue,
        calls: calls.clone(),
    };
    let prompt = "Crée un workflow désactivé nommé « Compter trois valeurs », à déclenchement manuel, avec exactement trois étapes déterministes. Une étape JsonData émet l'objet {\"items\":[1,2,3]}. Une étape TransformData compte ces éléments dans le champ count. Une dernière étape TransformData copie ce nombre vers le champ total. Consulte d'abord les contrats canoniques des types d'étapes nécessaires. Ne lance aucun modèle enfant, aucune API, notification ou commande, et n'active pas le workflow.";
    let config = crate::core::config::default_config();
    let started = std::time::Instant::now();
    let mut process = start_agent_with_config(AgentStartConfig {
        model_override: Some(&model),
        tools: Some(Arc::new(tools)),
        ..AgentStartConfig::new(
            &AgentType::Ollama,
            directory.path().to_str().unwrap(),
            prompt,
            &config.tokens,
        )
    })
    .await
    .unwrap();
    let mut output = String::new();
    while let Some(line) = process.next_line().await {
        output.push_str(&line);
    }
    let exit_success = process.child.wait().await.unwrap().success();
    let turns = parse_http_turn_telemetry(&process.captured_stderr_flushed().await);
    let calls = calls.lock().unwrap().clone();
    let workflow = state
        .db
        .with_conn(|conn| {
            let list = crate::db::workflows::list_workflows(conn)?;
            let Some(saved) = list.into_iter().find(|w| w.name == "Compter trois valeurs") else {
                return Ok(None);
            };
            crate::db::workflows::get_workflow(conn, &saved.id)
        })
        .await
        .unwrap();
    let mut context = crate::workflows::template::TemplateContext::new();
    let mut executed = Vec::new();
    let mut final_value = Value::Null;
    if let Some(workflow) = workflow
        .as_ref()
        .filter(|w| !w.enabled && w.steps.len() == 3)
    {
        // Execute only the deterministic types requested by the fixture.
        // Never let model-authored steps initiate an external side effect.
        for step in &workflow.steps {
            let outcome = match step.step_type {
                crate::models::StepType::JsonData => {
                    crate::workflows::json_data_step::execute_json_data_step(step).await
                }
                crate::models::StepType::TransformData => {
                    crate::workflows::transform_data_step::execute_transform_data_step(
                        step, &context,
                    )
                    .await
                }
                _ => break,
            };
            let success = outcome.result.status == crate::models::RunStatus::Success;
            executed.push(json!({"name":step.name,"ok":success,"output":outcome.result.output}));
            if !success {
                break;
            }
            context.set_step_output(&step.name, &outcome.result.output);
            final_value = context
                .resolve_value("previous_step.data")
                .unwrap_or(Value::Null);
        }
    }
    let ok = exit_success
        && executed.len() == 3
        && final_value == json!({"total":3})
        && calls
            .iter()
            .any(|c| c["name"] == "workflow_step_schema" && c["ok"] == true);
    let report = json!({"model":model,"prompt":prompt,"ok":ok,"exit_success":exit_success,"catalogue_bytes":catalogue_bytes,
        "total_prompt_tokens":turns.iter().map(|t|t.prompt_tokens).sum::<u64>(),"turns":turns.len(),"seconds":started.elapsed().as_secs_f64(),
        "calls":calls,"emitted_text":output,"workflow":workflow,"executed_steps":executed,"final_value":final_value});
    println!("{report}");
    if let Ok(path) = std::env::var("KRONN_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(
        ok,
        "native workflow authoring or deterministic execution failed; inspect report"
    );
}

struct RecordingTools {
    executor: Arc<dyn ToolExecutor>,
    catalogue: Vec<Value>,
    calls: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for RecordingTools {
    fn catalogue(&self) -> Vec<Value> {
        self.catalogue.clone()
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let outcome = self.executor.execute(call).await;
        self.calls.lock().unwrap().push(json!({
            "name":call.name, "arguments":call.arguments, "ok":outcome.ok,
            "error":outcome.content.get("error"),
        }));
        outcome
    }
}

async fn campaign_state(scenario: &str, directory: &std::path::Path) -> AppState {
    let state = super::quick_prompt_tests::state_with_prompts().await;
    if scenario == "media" {
        state.db.with_conn(|conn| {
            conn.execute("UPDATE external_api_connections SET image_model='openai/gpt-image-2' WHERE id='saved-provider'", [])?;
            Ok(())
        }).await.unwrap();
    }
    if scenario == "edit" {
        std::fs::write(directory.join("status.txt"), "Status: pending\n").unwrap();
        let path = directory.display().to_string();
        state
            .db
            .with_conn(move |conn| {
                conn.execute("UPDATE projects SET path=?1 WHERE id='a'", [path])?;
                conn.execute(
                    "UPDATE discussions SET project_id='a' WHERE id='general'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }
    state
}

#[tokio::test]
async fn native_campaign_fixtures_reach_media_edit_and_resume_handlers() {
    let directory = tempfile::tempdir().unwrap();
    for (scenario, name, arguments) in [
        (
            "media",
            "media_generate",
            json!({"connection_id":"saved-provider","modality":"image","prompt":"A lighthouse"}),
        ),
        ("edit", "read_file", json!({"path":"status.txt"})),
        ("resume", "agent_resume_status", json!({})),
    ] {
        let state = campaign_state(scenario, directory.path()).await;
        let executor =
            KronnToolExecutor::arc(state, Some("general".into()), AgentType::Ollama, None, None);
        let result = executor
            .execute(&ToolCall {
                id: scenario.into(),
                name: name.into(),
                arguments,
            })
            .await;
        assert!(result.ok, "{scenario}: {}", result.content);
        if scenario == "media" {
            let implicit = executor
                .execute(&ToolCall {
                    id: "implicit-media".into(),
                    name: "media_generate".into(),
                    arguments: json!({"modality":"image","prompt":"A lighthouse"}),
                })
                .await;
            assert!(implicit.ok, "{}", implicit.content);
            assert_eq!(implicit.content["success"], true);
            assert_eq!(implicit.content["data"]["connection_id"], "saved-provider");
            assert!(implicit.content["data"]["job_id"].as_str().is_some());
            assert_eq!(
                implicit.content["data"]["job_id"],
                result.content["data"]["job_id"]
            );
        }
        if scenario == "edit" {
            let result = executor.execute(&ToolCall {id:"write".into(),name:"write_file".into(),arguments:json!({"path":"status.txt","content":"Status: ready\n","expected_sha256":result.content["content_sha256"]})}).await;
            assert!(result.ok, "{}", result.content);
            assert_eq!(
                std::fs::read_to_string(directory.path().join("status.txt")).unwrap(),
                "Status: ready\n"
            );
        }
    }
}

#[test]
#[ignore = "prints the static workflow declaration measurement"]
fn measure_workflow_catalogue_surface() {
    let full = full_discussion_catalogue();
    let initial = tiered(full.clone());
    let family = declarations_for_family("automations");
    let without_workflows = |items: &[Value]| {
        items
            .iter()
            .filter(|tool| {
                !tool["function"]["name"]
                    .as_str()
                    .unwrap()
                    .starts_with("workflow_")
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    // 39e8a4fb's family description; the six-model campaign preserves the Quick Prompt
    // earlier measurement separately. Reconstruct only this workflow increment.
    let old_description = "save and run reusable APIs, commands and prompts: `qa_create_draft`, `qa_update`, `qe_create_draft`, `qe_update`, `qe_run`, `qe_list`, `qp_list`, `qp_run`, `qp_create_draft`, `qp_update`";
    let new_description = TOOL_FAMILIES
        .iter()
        .find(|(name, _, _)| *name == "automations")
        .unwrap()
        .1;
    let restore_index = |mut items: Vec<Value>| {
        if let Some(index) = items
            .iter_mut()
            .find(|tool| tool["function"]["name"] == "tools_load")
        {
            index["function"]["description"] = json!(index["function"]["description"]
                .as_str()
                .unwrap()
                .replace(new_description, old_description));
        }
        items
    };
    for (surface, before, after) in [
        ("initial", restore_index(initial.clone()), initial),
        ("automations", without_workflows(&family), family),
        ("full", restore_index(without_workflows(&full)), full),
    ] {
        let bytes_before = serde_json::to_vec(&before).unwrap().len();
        let bytes_after = serde_json::to_vec(&after).unwrap().len();
        println!(
            "{}",
            json!({"surface":surface,"baseline":"39e8a4fb","before_tools":before.len(),"after_tools":after.len(),"before_bytes":bytes_before,"after_bytes":bytes_after,"delta_bytes":bytes_after as i64-bytes_before as i64})
        );
    }
}

#[tokio::test]
#[ignore = "uses a real model; KRONN_QP_BENCH=1, KRONN_BENCH_MODEL, optional KRONN_BENCH_CONFIG for LiteLLM"]
async fn bench_quick_prompt_native_catalogue() {
    use crate::agents::runner::{
        parse_http_turn_telemetry, start_agent_with_config, AgentStartConfig,
    };
    assert_eq!(std::env::var("KRONN_QP_BENCH").as_deref(), Ok("1"));
    let model = std::env::var("KRONN_BENCH_MODEL").expect("explicit model required");
    // Read-only: config::load can migrate and save a config as a side effect.
    let config: crate::models::AppConfig = match std::env::var("KRONN_BENCH_CONFIG") {
        Ok(path) => toml::from_str(&std::fs::read_to_string(path).expect("read config"))
            .expect("parse config"),
        Err(_) => crate::core::config::default_config(),
    };
    let agent = if std::env::var("KRONN_BENCH_CONFIG").is_ok() {
        AgentType::LiteLlm
    } else {
        AgentType::Ollama
    };
    let endpoints = crate::models::setup::HttpEndpoints::from_agents(&config.agents);
    let directory = tempfile::tempdir().unwrap();
    let previous_tiering = std::env::var("KRONN_TIERED_TOOLS").ok();
    let mut reports = Vec::new();
    let scenario_filter = std::env::var("KRONN_BENCH_SCENARIO").ok();
    assert!(
        scenario_filter
            .as_deref()
            .is_none_or(|value| value == "media"),
        "the only supported focused scenario is media"
    );
    let media_without_id = std::env::var("KRONN_BENCH_MEDIA_WITHOUT_ID").as_deref() == Ok("1");
    let media_prompt = if media_without_id {
        "Lance la génération d'une image de phare au lever du jour. Confirme seulement son lancement, sans attendre le résultat."
    } else {
        "Lance la génération d'une image de phare au lever du jour avec la connexion enregistrée saved-provider, configurée pour les images. Confirme seulement son lancement, sans attendre le résultat."
    };
    // A follow-up catalogue growth check can replay only the shipping default.
    let modes: &[bool] = if std::env::var("KRONN_BENCH_FULL_ONLY").as_deref() == Ok("1") {
        &[false]
    } else {
        &[false, true]
    };
    for (scenario, prompt) in [
        ("list", "Quels Quick Prompts enregistrés sont accessibles dans cette discussion et quelles variables obligatoires attendent-ils ?"),
        ("run", "Lance une fois le Quick Prompt enregistré « Résumé » sur le sujet « été ». Confirme son lancement sans attendre la réponse."),
        ("create", "Enregistre un nouveau Quick Prompt nommé « Synthèse test » qui résume le sujet donné dans {{topic}}. Déclare cette variable obligatoire avec un libellé et un texte indicatif compréhensibles. Ne le lance pas."),
        ("update", "Pour le Quick Prompt enregistré « Résumé », change seulement sa description en « Nouvelle description de test ». Conserve tous les autres réglages."),
        ("media", media_prompt),
        ("edit", "Dans le fichier status.txt du projet, remplace exactement Status: pending par Status: ready. Lis le fichier avant de le modifier, conserve sa fin de ligne et ne fais pas de commit."),
        ("resume", "Consulte les tâches en arrière-plan et les reprises en attente pour toi dans cette discussion, puis indique s'il en existe. Ne lance rien."),
    ] {
        if scenario_filter.as_deref().is_some_and(|filter| scenario != filter) {
            continue;
        }
        for &tiered_mode in modes {
            let state = campaign_state(scenario, directory.path()).await;
            std::env::set_var("KRONN_TIERED_TOOLS", if tiered_mode { "1" } else { "0" });
            let executor = KronnToolExecutor::arc(state.clone(),Some("general".into()),agent.clone(),None,None);
            let catalogue = executor.catalogue();
            let catalogue_bytes = serde_json::to_vec(&catalogue).unwrap().len();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let tools = RecordingTools {executor,catalogue,calls:calls.clone()};
            let started = std::time::Instant::now();
            let mut process = start_agent_with_config(AgentStartConfig {
                model_override:Some(&model), http_endpoints:Some(&endpoints), tools:Some(Arc::new(tools)),
                ..AgentStartConfig::new(&agent,directory.path().to_str().unwrap(),prompt,&config.tokens)
            }).await.expect("provider start");
            let mut output = String::new();
            while let Some(line) = process.next_line().await { output.push_str(&line); }
            let exit_success = process.child.wait().await.unwrap().success();
            let turns = parse_http_turn_telemetry(&process.captured_stderr_flushed().await);
            let calls = calls.lock().unwrap().clone();
            let expected = match scenario { "list"=>"qp_list", "run"=>"qp_run", "create"=>"qp_create_draft", "update"=>"qp_update", "media"=>"media_generate", "resume"=>"agent_resume_status", _=>"" };
            let reached = calls.iter().any(|call| call["ok"] == true && (call["name"] == expected || (scenario == "edit" && ["write_file","edit_file","edit_lines"].iter().any(|name|call["name"] == *name))));
            let edited = scenario != "edit" || std::fs::read_to_string(directory.path().join("status.txt")).unwrap() == "Status: ready\n";
            let persisted = state.db.with_conn(move |conn| {
                Ok(match scenario {
                    "run" => conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs",[],|row|row.get::<_,i64>(0))? == 1,
                    "create" => crate::db::quick_prompts::list_quick_prompts(conn)?.iter().any(|qp| qp.name == "Synthèse test" && qp.prompt_template.contains("{{topic}}") && qp.variables.iter().any(|v|v.name == "topic" && v.required && !v.label.is_empty())),
                    "update" => crate::db::quick_prompts::get_quick_prompt(conn,"global")?.is_some_and(|qp|qp.description == "Nouvelle description de test" && qp.connection_id.as_deref() == Some("saved-provider") && qp.prompt_template == "Summarize {{topic}}"),
                    "media" => conn.query_row("SELECT COUNT(*) FROM media_jobs WHERE connection_id='saved-provider' AND modality='image'",[],|row|row.get::<_,i64>(0))? == 1,
                    "edit" => edited,
                    _ => true,
                })
            }).await.unwrap();
            let report = json!({"model":model,"scenario":scenario,"prompt":prompt,"tiered":tiered_mode,"ok":exit_success && reached && persisted,"exit_success":exit_success,"reached":reached,"persisted":persisted,"catalogue_bytes":catalogue_bytes,"first_prompt_tokens":turns.first().map(|t|t.prompt_tokens),"total_prompt_tokens":turns.iter().map(|t|t.prompt_tokens).sum::<u64>(),"turns":turns.len(),"seconds":started.elapsed().as_secs_f64(),"calls":calls,"emitted_text":output});
            println!("{}",report);
            reports.push(report);
            if let Ok(path) = std::env::var("KRONN_BENCH_REPORT") {
                std::fs::write(path,serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
            }
        }
    }
    if let Ok(path) = std::env::var("KRONN_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
    }
    match previous_tiering {
        Some(value) => std::env::set_var("KRONN_TIERED_TOOLS", value),
        None => std::env::remove_var("KRONN_TIERED_TOOLS"),
    }
    assert!(
        reports.iter().all(|report| report["ok"] == true),
        "one or more native Quick Prompt scenarios failed; inspect the recorded calls"
    );
}
