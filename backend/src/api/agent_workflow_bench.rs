//! KT-1216 — does an agent with no memory, given the MCP surface alone, build
//! the PR review chain with native reads? The scorer is deterministic and unit
//! tested; the real-model trial is `#[ignore]`d and measures two arms, with and
//! without the doctrine, so the before/after is read from a report, not assumed.
use super::*;
use std::sync::{Arc, Mutex};

use crate::models::{StepType, Workflow};
use crate::workflows::doctrine_warnings::{self, ConfiguredApi};

async fn call(executor: &dyn ToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: name.into(),
            name: name.into(),
            arguments,
        })
        .await
}

/// What a built workflow does with the two services the chain reads.
#[derive(Debug, Default, PartialEq)]
struct ChainScore {
    github_apicalls: usize,
    jira_apicalls: usize,
    /// Exec steps reaching a configured plugin's service with `gh`/`curl`.
    exec_bypasses: usize,
    /// Sets of Exec steps carrying the same script.
    copied_scripts: usize,
    /// Exec steps passing state through a file instead of step outputs.
    state_files: usize,
}

impl ChainScore {
    fn good(&self) -> bool {
        self.github_apicalls > 0
            && self.jira_apicalls > 0
            && self.exec_bypasses == 0
            && self.copied_scripts == 0
            && self.state_files == 0
    }
}

fn score(workflow: &Workflow, apis: &[ConfiguredApi]) -> ChainScore {
    let mut score = ChainScore::default();
    for step in workflow.steps.iter().chain(workflow.on_failure.iter()) {
        match step.step_type {
            StepType::ApiCall | StepType::BatchApiCall => {
                let slug = step.api_plugin_slug.as_deref().unwrap_or_default();
                if slug.contains("github") {
                    score.github_apicalls += 1;
                }
                if slug.contains("jira") || slug.contains("atlassian") {
                    score.jira_apicalls += 1;
                }
            }
            StepType::Exec => {
                let text = format!(
                    "{} {}",
                    step.exec_args.join(" "),
                    step.exec_stdin.as_deref().unwrap_or_default()
                );
                if text.contains("context.json") {
                    score.state_files += 1;
                }
            }
            _ => {}
        }
    }
    for warning in doctrine_warnings::warnings(workflow, apis) {
        match warning.kind {
            crate::models::WorkflowWarningKind::ExecBypassesPlugin => score.exec_bypasses += 1,
            crate::models::WorkflowWarningKind::DuplicatedExecScript => score.copied_scripts += 1,
        }
    }
    score
}

fn apis() -> Vec<ConfiguredApi> {
    vec![
        ConfiguredApi::from_base_url("GitHub", "https://api.github.com").unwrap(),
        ConfiguredApi::from_base_url("Atlassian", "https://acme.atlassian.net").unwrap(),
    ]
}

/// A GitHub API plugin and a Jira one, each with a saved configuration.
async fn fixture() -> (AppState, Arc<dyn ToolExecutor>) {
    use crate::models::{ApiAuthKind, ApiSpec, McpServer, McpSource, McpTransport};
    let state = super::super::quick_prompt_tests::state_with_prompts().await;
    state
        .db
        .with_conn(|conn| {
            for (id, name, base) in [
                ("mcp-github", "GitHub", "https://api.github.com"),
                ("mcp-atlassian", "Atlassian", "https://acme.atlassian.net"),
            ] {
                crate::db::mcps::upsert_server(
                    conn,
                    &McpServer {
                        id: id.into(),
                        name: name.into(),
                        description: String::new(),
                        transport: McpTransport::ApiOnly,
                        source: McpSource::Manual,
                        api_spec: Some(ApiSpec {
                            base_url: base.into(),
                            auth: ApiAuthKind::Bearer {
                                env_key: "TOKEN".into(),
                            },
                            endpoints: vec![],
                            docs_url: None,
                            config_keys: vec![],
                            default_headers: vec![],
                            test_endpoint: None,
                        }),
                    },
                )?;
                conn.execute(
                    "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json, is_global) VALUES (?1, ?2, ?2, '', '[]', 1)",
                    rusqlite::params![format!("cfg-{id}"), id],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let executor = KronnToolExecutor::arc(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
    );
    (state, executor)
}

fn copied_script_step(name: &str, mode: &str) -> Value {
    let script = format!(
        "import subprocess, sys\n{}\nsubprocess.run(['gh', 'api', sys.argv[1]])",
        "x = 1\n".repeat(60)
    );
    json!({"name":name,"step_type":{"type":"Exec"},"exec_command":"python3",
           "exec_args":["-c", script, "_", mode], "exec_timeout_secs":60})
}

/// The chain Codex built, in miniature: every read an Exec, one script copied.
fn before_shape() -> Value {
    json!({"name":"PR review before","trigger":{"type":"Manual"},"exec_allowlist":["python3"],
    "steps":[
        copied_script_step("collect-files", "files"),
        copied_script_step("collect-reviews", "reviews"),
        copied_script_step("collect-ticket", "ticket"),
    ]})
}

/// The split production uses: ApiCall for the reads, Exec for the logic.
fn after_shape() -> Value {
    json!({"name":"PR review after","trigger":{"type":"Manual"},"exec_allowlist":["python3"],
    "steps":[
        {"name":"pr-files","step_type":{"type":"ApiCall"},"api_plugin_slug":"mcp-github",
         "api_endpoint_path":"/repos/o/r/pulls/1/files","api_method":"GET",
         "api_pagination":{"type":"LinkHeader","page_size_param":"per_page","page_size":100}},
        {"name":"ticket","step_type":{"type":"ApiCall"},"api_plugin_slug":"mcp-atlassian",
         "api_endpoint_path":"/rest/api/3/issue/KT-1","api_method":"GET"},
        {"name":"digest","step_type":{"type":"Exec"},"exec_command":"python3",
         "exec_args":["-c","import json,sys; print(len(json.load(sys.stdin)))"],
         "exec_stdin":"{{steps.pr-files.data_json}}"},
    ]})
}

#[tokio::test]
async fn the_scorer_tells_the_codex_shape_from_the_production_split() {
    let (state, executor) = fixture().await;
    let mut scored = Vec::new();
    for shape in [before_shape(), after_shape()] {
        let created = call(executor.as_ref(), "workflow_create_draft", shape).await;
        assert!(created.ok, "{}", created.content);
        let workflow: Workflow = serde_json::from_value(created.content.clone()).unwrap();
        scored.push((
            score(&workflow, &apis()),
            created.content["kronn_readiness"].clone(),
        ));
    }
    let (before, before_readiness) = &scored[0];
    let (after, after_readiness) = &scored[1];
    assert!(!before.good(), "{before:?}");
    assert_eq!(before.github_apicalls + before.jira_apicalls, 0);
    assert_eq!(before.exec_bypasses, 3, "{before:?}");
    assert_eq!(before.copied_scripts, 1, "{before:?}");
    assert!(after.good(), "{after:?}");
    // What the agent reads back at save time: the same verdict, as warnings.
    let kinds: Vec<&str> = before_readiness["warnings"]
        .as_array()
        .expect("drift is reported")
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"duplicated_exec_script"), "{kinds:?}");
    assert!(kinds.contains(&"exec_bypasses_plugin"), "{kinds:?}");
    assert_eq!(before_readiness["ready"], true, "warnings never block");
    assert!(
        after_readiness.get("warnings").is_none(),
        "{after_readiness}"
    );
    drop(state);
}

struct RecordingTools {
    executor: Arc<dyn ToolExecutor>,
    calls: Arc<Mutex<Vec<Value>>>,
    /// The "before" arm: what the surface said without the doctrine.
    without_doctrine: bool,
}

/// The surface as it was before KT-1216: no doctrine, no ApiCall limits, no
/// readiness warnings. An ablation of this build, not a second build.
fn strip_doctrine(outcome: &mut ToolOutcome) {
    let Some(content) = outcome.content.as_object_mut() else {
        return;
    };
    content.remove("authoring_doctrine");
    content.remove("read_first");
    if let Some(api) = content
        .get_mut("fields_by_type")
        .and_then(|fields| fields.get_mut("ApiCall"))
        .and_then(Value::as_object_mut)
    {
        api.remove("pagination_limits");
        api.remove("graphql");
    }
    if let Some(readiness) = content
        .get_mut("kronn_readiness")
        .and_then(Value::as_object_mut)
    {
        readiness.remove("warnings");
    }
}

#[async_trait::async_trait]
impl ToolExecutor for RecordingTools {
    fn catalogue(&self) -> Vec<Value> {
        self.executor.catalogue()
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let allowed = [
            "mcp_list",
            "qa_list",
            "qe_list",
            "workflow_list",
            "workflow_get",
            "workflow_step_schema",
            "workflow_create_draft",
            "workflow_update",
            "tool_manual",
            "tools_load",
        ]
        .contains(&call.name.as_str());
        let mut result = if allowed {
            self.executor.execute(call).await
        } else {
            fail(
                call,
                "This isolated trial only reads catalogues and authors disabled drafts",
            )
        };
        if self.without_doctrine {
            strip_doctrine(&mut result);
        }
        self.calls.lock().unwrap().push(json!({"name":call.name,"arguments":call.arguments,"ok":result.ok,"fixture_refusal":!allowed}));
        result
    }
}

#[tokio::test]
#[ignore = "real Ollama trial; requires KRONN_WORKFLOW_BENCH=1, KRONN_BENCH_MODEL, KRONN_BENCH_REPORT and KRONN_BENCH_ARM=with|without"]
async fn bench_native_pr_review_chain() {
    use crate::agents::runner::{
        parse_http_turn_telemetry, start_agent_with_config, AgentIo, AgentStartConfig,
    };
    assert_eq!(std::env::var("KRONN_WORKFLOW_BENCH").as_deref(), Ok("1"));
    let model = std::env::var("KRONN_BENCH_MODEL").expect("explicit installed local model");
    let report_path = std::env::var("KRONN_BENCH_REPORT").expect("retain every attempt");
    let without_doctrine = match std::env::var("KRONN_BENCH_ARM").as_deref() {
        Ok("with") => false,
        Ok("without") => true,
        other => panic!("KRONN_BENCH_ARM must be `with` or `without`, got {other:?}"),
    };
    let (state, executor) = fixture().await;
    let catalogue = executor.catalogue();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tools = RecordingTools {
        executor: executor.clone(),
        calls: calls.clone(),
        without_doctrine,
    };
    let directory = tempfile::tempdir().unwrap();
    let prompt = "Build and save, as a disabled manual workflow named PR review bench, a pull request review chain. It reads the changed files, the reviews and the review comments of a GitHub pull request (all pages), and the Jira ticket named in the pull request title; it then has an agent review the change against the ticket and posts nothing. Use the actual native tools and saved ids. Do not enable or launch it.";
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
    let (exit_success, turns, error) = match attempt {
        Ok(mut process) => {
            let timed_out = tokio::time::timeout(std::time::Duration::from_secs(300), async {
                while process.next_line().await.is_some() {}
            })
            .await
            .is_err();
            if timed_out {
                process.kill().await;
            }
            let exited = process.child.wait().await.unwrap().success();
            let stderr = process.captured_stderr_flushed().await;
            (
                exited && !timed_out,
                parse_http_turn_telemetry(&stderr),
                Some(stderr),
            )
        }
        Err(error) => (false, Vec::new(), Some(vec![error])),
    };
    let workflow = state
        .db
        .with_read_conn(|conn| {
            let found = crate::db::workflows::list_workflows(conn)?
                .into_iter()
                .find(|w| w.name == "PR review bench");
            match found {
                Some(w) => crate::db::workflows::get_workflow(conn, &w.id),
                None => Ok(None),
            }
        })
        .await
        .unwrap();
    let chain = workflow.as_ref().map(|w| score(w, &apis()));
    let calls = calls.lock().unwrap().clone();
    let report = json!({"arm":if without_doctrine {"without"} else {"with"},"model":model,
        "prompt":prompt,"saved":workflow.is_some(),"good":chain.as_ref().map(ChainScore::good),
        "score":chain.as_ref().map(|s| format!("{s:?}")),
        "read_doctrine":calls.iter().any(|c| c["name"]=="workflow_step_schema" && c["ok"]==true),
        "exit_success":exit_success,"catalogue_tools":catalogue.len(),
        "seconds":started.elapsed().as_secs_f64(),"turns":turns.len(),
        "total_prompt_tokens":turns.iter().map(|t|t.prompt_tokens).sum::<u64>(),
        "calls":calls,"diagnostic":error,"workflow":workflow});
    std::fs::write(report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{report}");
    // A measurement, not a gate: the `without` arm is expected to fail the score.
    assert!(
        workflow.is_some(),
        "the agent saved no draft; retain the report"
    );
}
