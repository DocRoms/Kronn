//! Real local-model qualification, isolated from the user's saved Pages.
use super::tests::call;
use super::*;
use std::sync::{Arc, Mutex};

async fn fixture() -> (AppState, Arc<dyn ToolExecutor>, Value) {
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::arc(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
    );
    let seed = call(
        executor.as_ref(),
        "page_create",
        json!({
            "title":"Mon suivi", "slug":"mon-suivi", "html":"<!doctype html><h1>Mon suivi</h1>",
            "datasets":[{"name":"summary","kind":"snapshot","initial":{"count":0}}]
        }),
    )
    .await;
    assert!(seed.ok, "{}", seed.content);
    (state, executor, seed.content)
}

// Run the real workflow engine only after checking the model-authored draft is
// a deterministic data pipeline. Fixture execution is not a new launch tool.
async fn publish_draft(
    state: &AppState,
    workflow: &crate::models::Workflow,
) -> Result<crate::models::WorkflowRun, String> {
    use crate::models::{RunStatus, StepType, WorkflowRun};
    if workflow.enabled
        || workflow.project_id.is_some()
        || workflow.steps.len() != 2
        || workflow.steps[0].step_type != StepType::JsonData
        || workflow.steps[1].step_type != StepType::PublishPageData
        || workflow.workspace_config.is_some()
        || !workflow.artifacts.is_empty()
        || !workflow.variables.is_empty()
        || !workflow.actions.is_empty()
        || !workflow.on_failure.is_empty()
        || workflow.steps.iter().any(|step| !step.on_result.is_empty())
    {
        return Err("Fixture only executes a disabled, general JsonData → PublishPageData draft without workspace, artifacts, variables, actions or failure steps".into());
    }
    let mut run: WorkflowRun = serde_json::from_value(json!({
        "id":uuid::Uuid::new_v4().to_string(), "workflow_id":workflow.id,
        "status":"Pending", "step_results":[], "tokens_used":0,
        "started_at":chrono::Utc::now()
    }))
    .map_err(|e| e.to_string())?;
    let saved = run.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &saved))
        .await
        .map_err(|e| e.to_string())?;
    let config = crate::core::config::default_config();
    crate::workflows::runner::execute_run(
        state.clone(),
        workflow,
        &mut run,
        &config.tokens,
        &config.agents,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| e.to_string())?;
    if run.status != RunStatus::Success {
        return Err(format!(
            "Workflow failed: {}",
            serde_json::to_string(&run).map_err(|e| e.to_string())?
        ));
    }
    Ok(run)
}

#[tokio::test]
async fn native_page_pipeline_publishes_through_the_real_workflow_runner() {
    let (state, executor, seed) = fixture().await;
    let created = call(executor.as_ref(), "workflow_create_draft", json!({
        "name":"Page fixture", "trigger":{"type":"Manual"},
        "steps":[
            {"name":"data","step_type":{"type":"JsonData"},"json_data_payload":{"count":3}},
            {"name":"publish","step_type":{"type":"PublishPageData"},"page_publish":{"page_id":seed["id"],"writes":[{"dataset":"summary","operation":"replace","value_from":"steps.data.data"}]}}
        ]
    })).await;
    assert!(created.ok, "{}", created.content);
    let workflow = serde_json::from_value(created.content.clone()).unwrap();
    let run = publish_draft(&state, &workflow).await.unwrap();
    assert_eq!(run.step_results.len(), 2);
    let detail = call(executor.as_ref(), "page_get", json!({"page_id":seed["id"]})).await;
    assert_eq!(detail.content["data_revision"], 1);
    assert_eq!(detail.content["datasets"][0]["current"], json!({"count":3}));
    assert_eq!(detail.content["workflows"][0]["id"], created.content["id"]);
    let id = seed["id"].as_str().unwrap().to_owned();
    let publications = state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_pages::list_live_page_publications(conn, &id, 3)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(publications.len(), 1);
    assert_eq!(
        publications[0].workflow_run_id.as_deref(),
        Some(run.id.as_str())
    );
    assert!(
        call(
            executor.as_ref(),
            "page_update_html",
            json!({"page_id":"mon-suivi","html":"<h1>Published</h1>"})
        )
        .await
        .ok
    );
    let after = call(
        executor.as_ref(),
        "page_get",
        json!({"page_id":"mon-suivi"}),
    )
    .await;
    assert_eq!(after.content["data_revision"], 1);
    assert_eq!(after.content["datasets"], detail.content["datasets"]);
}

struct RecordingTools {
    executor: Arc<dyn ToolExecutor>,
    calls: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for RecordingTools {
    fn catalogue(&self) -> Vec<Value> {
        self.executor.catalogue()
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let allowed = [
            "page_list",
            "page_get",
            "page_create",
            "page_update_html",
            "page_add_dataset",
            "workflow_list",
            "workflow_get",
            "workflow_step_schema",
            "workflow_create_draft",
            "workflow_update",
            "tool_manual",
            "tools_load",
        ]
        .contains(&call.name.as_str());
        let result = if allowed {
            self.executor.execute(call).await
        } else {
            fail(
                call,
                "This isolated Page trial only executes Page and disabled workflow authoring tools",
            )
        };
        self.calls.lock().unwrap().push(json!({"name":call.name,"arguments":call.arguments,"ok":result.ok,"content":result.content,"fixture_refusal":!allowed}));
        result
    }
}

#[tokio::test]
#[ignore = "real Ollama trial; requires KRONN_PAGE_BENCH=1, KRONN_BENCH_MODEL and KRONN_BENCH_REPORT"]
async fn bench_native_page_workflow() {
    use crate::agents::runner::{
        parse_http_turn_telemetry, start_agent_with_config, AgentIo, AgentStartConfig,
    };
    assert_eq!(std::env::var("KRONN_PAGE_BENCH").as_deref(), Ok("1"));
    let model = std::env::var("KRONN_BENCH_MODEL").expect("explicit installed local model");
    let report_path = std::env::var("KRONN_BENCH_REPORT").expect("retain every attempt");
    let (state, executor, seed) = fixture().await;
    let catalogue = executor.catalogue();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tools = RecordingTools {
        executor: executor.clone(),
        calls: calls.clone(),
    };
    let directory = tempfile::tempdir().unwrap();
    let prompt = "Find the existing live Page with slug mon-suivi and read it. Replace its complete HTML with a self-contained document titled Mon suivi that displays summary data from window.KronnPageData and updates on kronn:page-data. Add an empty collection dataset named tickets. Then save a disabled manual workflow named Page trial with exactly two deterministic steps: JsonData emits {\"count\":3}; PublishPageData replaces the existing Page's summary dataset with that typed output. Discover the canonical workflow step schemas before authoring. Do not enable or launch the workflow; the test operator will execute this isolated draft. Use actual native tools and saved ids, not an HTML preview or instructions for a human.";
    let config = crate::core::config::default_config();
    let started = std::time::Instant::now();
    let attempt = start_agent_with_config(AgentStartConfig {
        model_override: Some(&model),
        tools: Some(Arc::new(tools)),
        ..AgentStartConfig::new(
            &AgentType::Ollama,
            directory.path().to_str().unwrap(),
            prompt,
            &config.tokens,
        )
    })
    .await;
    let (exit_success, output, turns, error) = match attempt {
        Ok(mut process) => {
            let mut output = String::new();
            let timed_out = tokio::time::timeout(std::time::Duration::from_secs(180), async {
                while let Some(line) = process.next_line().await {
                    output.push_str(&line);
                }
            })
            .await
            .is_err();
            if timed_out {
                process.kill().await;
            }
            let exited = process.child.wait().await.unwrap().success();
            let mut stderr = process.captured_stderr_flushed().await;
            if timed_out {
                stderr.push("Page trial exceeded its 180-second wall-clock limit".into());
            }
            (
                exited && !timed_out,
                output,
                parse_http_turn_telemetry(&stderr),
                Some(stderr),
            )
        }
        Err(error) => (false, String::new(), Vec::new(), Some(vec![error])),
    };
    let workflow = state
        .db
        .with_read_conn(|conn| {
            let found = crate::db::workflows::list_workflows(conn)?
                .into_iter()
                .find(|w| w.name == "Page trial");
            match found {
                Some(w) => crate::db::workflows::get_workflow(conn, &w.id),
                None => Ok(None),
            }
        })
        .await
        .unwrap();
    let publication = match workflow.as_ref() {
        Some(workflow) => publish_draft(&state, workflow).await,
        None => Err("No Page trial workflow was saved".into()),
    };
    let detail = call(executor.as_ref(), "page_get", json!({"page_id":seed["id"]})).await;
    let calls = calls.lock().unwrap().clone();
    let ok = exit_success
        && publication.is_ok()
        && detail.ok
        && detail.content["data_revision"] == 1
        && detail.content["revision"]["revision"]
            .as_u64()
            .is_some_and(|r| r >= 2)
        && detail.content["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "summary" && d["current"] == json!({"count":3}))
        && detail.content["datasets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "tickets" && d["kind"] == "collection")
        && calls
            .iter()
            .any(|c| c["name"] == "page_get" && c["ok"] == true)
        && calls
            .iter()
            .any(|c| c["name"] == "workflow_step_schema" && c["ok"] == true);
    let report = json!({"model":model,"tiered":tiered_tools_enabled(),"prompt":prompt,"ok":ok,"exit_success":exit_success,
        "catalogue_bytes":serde_json::to_vec(&catalogue).unwrap().len(),"catalogue_tools":catalogue.len(),
        "seconds":started.elapsed().as_secs_f64(),"total_prompt_tokens":turns.iter().map(|t|t.prompt_tokens).sum::<u64>(),
        "turns":turns.len(),"calls":calls,"emitted_text":output,"diagnostic":error,
        "workflow":workflow,"publication":publication,"page":detail.content});
    std::fs::write(report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{report}");
    assert!(ok, "native Page trial failed; retain its report");
}
