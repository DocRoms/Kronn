//! Bounded, real-model comparison of inline and discoverable proposal contracts.
use super::*;
use std::sync::{Arc, Mutex};

const INLINE_NOTICE: &str = "Actions Automatisation — si une action QP, QA, QE ou Workflow existante serait une suite utile, résous d'abord son vrai id et son contrat de variables avec les outils de catalogue/get. Propose-la sans l'exécuter avec exactement un bloc `kronn-action` : `{\"kind\":\"quick_prompt|quick_api|quick_exec|workflow\",\"target_id\":\"<id réel>\",\"project_id\":\"<id optionnel>\",\"values\":[{\"name\":\"<variable déclarée>\",\"value\":\"<suggestion éditable>\",\"provenance\":\"agent_suggestion\",\"suggested_by\":\"<ton alias>\"}]}`. Kronn valide et persiste la proposition ; seul le clic humain la lance. N'invente jamais d'id ou de variable et n'inclus jamais une valeur secrète/résolue.\n\n";

// Rejected prompt-only candidate, retained to reproduce the comparison.
const DISCOVERABLE_NOTICE: &str = "Actions Automatisation — pour proposer un QP, QA, QE ou Workflow existant, lis `tool_manual({tool: \"signals\"})`, puis émets un bloc `kronn-action` conforme. Seul le clic humain le lance. N'invente aucun id/variable et n'inclus aucun secret.\n\n";

struct ProposalTools {
    executor: Arc<dyn ToolExecutor>,
    calls: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for ProposalTools {
    fn catalogue(&self) -> Vec<Value> {
        self.executor.catalogue()
    }

    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        // Preserve the real full catalogue, but prevent benchmark mistakes from
        // executing an automation. Rejected attempts fail the observation.
        let allowed = [
            "tool_manual",
            "qp_list",
            "qa_list",
            "qe_list",
            "workflow_list",
            "workflow_get",
            "workflow_step_schema",
            "mcp_list",
            "git_status",
        ]
        .contains(&call.name.as_str());
        let outcome = if allowed {
            self.executor.execute(call).await
        } else {
            fail(
                call,
                "This proposal-only benchmark permits catalogue reads only",
            )
        };
        self.calls.lock().unwrap().push(json!({"name":call.name,"arguments":call.arguments,"ok":outcome.ok,"rejected":!allowed,"error":outcome.content.get("error")}));
        outcome
    }
}

async fn proposal_fixture(kind: &str) -> (AppState, String) {
    let state = super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::new(state.clone(), Some("general".into()));
    let (name, arguments) = match kind {
        "quick_prompt" => return (state, "global".into()),
        "quick_api" => (
            "qa_create_draft",
            json!({"name":"Lire ticket", "api_plugin_slug":"tracker", "api_config_id":"fixture", "api_endpoint_path":"/ticket", "variables":[]}),
        ),
        "workflow" => (
            "workflow_create_draft",
            json!({"name":"Rapport", "trigger":{"type":"Manual"}, "steps":[{"name":"data", "step_type":{"type":"JsonData"}, "json_data_payload":{"items":[1,2,3]}}]}),
        ),
        _ => panic!("unknown fixture"),
    };
    let outcome = executor
        .execute(&ToolCall {
            id: "seed".into(),
            name: name.into(),
            arguments,
        })
        .await;
    assert!(outcome.ok, "{}", outcome.content);
    (
        state,
        outcome.content["id"]
            .as_str()
            .expect("saved fixture id")
            .into(),
    )
}

async fn ingest_proposal(state: &AppState, content: &str) -> (Value, i64) {
    let message: crate::models::DiscussionMessage = serde_json::from_value(json!({
        "id":"proposal", "role":"Agent", "content":content, "agent_type":"Ollama",
        "timestamp":"2026-09-22T00:00:00Z"
    }))
    .unwrap();
    state.db.with_conn(move |conn| {
        crate::db::discussions::insert_message(conn,"general", &message)?;
        let actions = crate::db::discussion_actions::list_for_discussion(crate::db::kronn_action_engine::Reconcile::Projected, conn,"general")?;
        let launches = conn.query_row("SELECT (SELECT COUNT(*) FROM shared_runs) + (SELECT COUNT(*) FROM agent_dispatch_jobs)",[],|row|row.get::<_,i64>(0))?;
        Ok((serde_json::to_value(actions)?, launches))
    }).await.unwrap()
}

#[tokio::test]
async fn signal_benchmark_fixtures_accept_real_catalogue_proposals() {
    for kind in ["quick_prompt", "quick_api", "workflow"] {
        let (state, target_id) = proposal_fixture(kind).await;
        let text = format!(
            "```kronn-action\n{}\n```",
            json!({"kind":kind,"target_id":target_id})
        );
        let (actions, launches) = ingest_proposal(&state, &text).await;
        assert_eq!(actions[0]["state"], "proposed", "{actions}");
        assert_eq!(launches, 0);
    }
}

#[tokio::test]
async fn signal_benchmark_guard_preserves_exploratory_reads_and_refuses_launches() {
    let state = super::quick_prompt_tests::state_with_prompts().await;
    let executor = KronnToolExecutor::arc(
        state.clone(),
        Some("general".into()),
        AgentType::Ollama,
        None,
        None,
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tools = ProposalTools {
        executor: executor.clone(),
        calls: calls.clone(),
    };
    for name in ["mcp_list", "git_status", "workflow_step_schema"] {
        let call = ToolCall {
            id: name.into(),
            name: name.into(),
            arguments: json!({}),
        };
        let expected = executor.execute(&call).await;
        let actual = tools.execute(&call).await;
        assert_eq!(actual.ok, expected.ok, "{name}");
        assert_eq!(actual.content, expected.content, "{name}");
        assert_eq!(calls.lock().unwrap().last().unwrap()["rejected"], false);
    }
    let launch = tools
        .execute(&ToolCall {
            id: "launch".into(),
            name: "qp_run".into(),
            arguments: json!({"qp_id":"global","variables":{"topic":"été"}}),
        })
        .await;
    assert!(!launch.ok);
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
    assert_eq!(jobs, 0);
}

#[tokio::test]
#[ignore = "real local signal discovery; KRONN_SIGNAL_BENCH=1 and KRONN_BENCH_MODEL required"]
async fn bench_native_signal_discovery() {
    use crate::agents::runner::{
        parse_http_turn_telemetry, start_agent_with_config, AgentStartConfig,
    };
    assert_eq!(std::env::var("KRONN_SIGNAL_BENCH").as_deref(), Ok("1"));
    let model = std::env::var("KRONN_BENCH_MODEL").expect("explicit local model");
    let directory = tempfile::tempdir().unwrap();
    let config = crate::core::config::default_config();
    let mut reports = Vec::new();
    for (kind, request) in [
        (
            "quick_prompt",
            "Propose une carte pour le Quick Prompt existant « Résumé », avec le sujet été.",
        ),
        (
            "quick_api",
            "Propose une carte pour la Quick API existante « Lire ticket ».",
        ),
        (
            "workflow",
            "Propose une carte pour le workflow existant « Rapport ».",
        ),
    ] {
        // Counterbalance the within-pair order across the three scenarios.
        let modes = if kind == "quick_api" {
            [true, false]
        } else {
            [false, true]
        };
        for discoverable in modes {
            let (state, target_id) = proposal_fixture(kind).await;
            let executor = KronnToolExecutor::arc(
                state.clone(),
                Some("general".into()),
                AgentType::Ollama,
                None,
                None,
            );
            let catalogue_bytes = serde_json::to_vec(&executor.catalogue()).unwrap().len();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let tools = ProposalTools {
                executor,
                calls: calls.clone(),
            };
            let notice = if discoverable {
                DISCOVERABLE_NOTICE
            } else {
                INLINE_NOTICE
            };
            let prompt = format!("{notice}Tu réponds sous l'alias @ollama. {request} Ne la lance pas et ne modifie aucune définition. Utilise une seule proposition et laisse les entrées inconnues à remplir par l'humain.");
            let started = std::time::Instant::now();
            let mut process = start_agent_with_config(AgentStartConfig {
                model_override: Some(&model),
                tools: Some(Arc::new(tools)),
                ..AgentStartConfig::new(
                    &AgentType::Ollama,
                    directory.path().to_str().unwrap(),
                    &prompt,
                    &config.tokens,
                )
            })
            .await
            .expect("provider start");
            let mut output = String::new();
            while let Some(line) = process.next_line().await {
                output.push_str(&line);
            }
            let exit_success = process.child.wait().await.unwrap().success();
            let turns = parse_http_turn_telemetry(&process.captured_stderr_flushed().await);
            let calls = calls.lock().unwrap().clone();
            let (actions, launches) = ingest_proposal(&state, &output).await;
            let discovery = match kind {
                "quick_prompt" => "qp_list",
                "quick_api" => "qa_list",
                _ => "workflow_list",
            };
            let target_read = calls
                .iter()
                .any(|c| c["name"] == discovery && c["ok"] == true);
            let contract_read = calls.iter().any(|c| {
                c["name"] == "tool_manual" && c["arguments"]["tool"] == "signals" && c["ok"] == true
            });
            let valid_card = actions.as_array().is_some_and(|a| a.len() == 1)
                && actions[0]["state"] == "proposed"
                && actions[0]["kind"] == kind
                && actions[0]["target_id"] == target_id;
            let suggested_topic = kind != "quick_prompt"
                || actions[0]["values"].as_array().is_some_and(|values| {
                    values.iter().any(|v| {
                        v["name"] == "topic"
                            && v["value"] == "été"
                            && v["provenance"] == "agent_suggestion"
                    })
                });
            let no_rejected_call = calls.iter().all(|c| c["rejected"] == false);
            let ok = exit_success
                && target_read
                && valid_card
                && suggested_topic
                && no_rejected_call
                && launches == 0
                && (!discoverable || contract_read);
            let report = json!({"model":model,"kind":kind,"discoverable":discoverable,"ok":ok,"exit_success":exit_success,"target_read":target_read,"contract_read":contract_read,"valid_card":valid_card,"suggested_topic":suggested_topic,"launches":launches,"catalogue_bytes":catalogue_bytes,"notice_bytes":notice.len(),"first_prompt_tokens":turns.first().map(|t|t.prompt_tokens),"total_prompt_tokens":turns.iter().map(|t|t.prompt_tokens).sum::<u64>(),"turns":turns.len(),"seconds":started.elapsed().as_secs_f64(),"prompt":prompt,"calls":calls,"emitted_text":output,"actions":actions});
            println!("{report}");
            reports.push(report);
            if let Ok(path) = std::env::var("KRONN_BENCH_REPORT") {
                std::fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
            }
        }
    }
    assert!(
        reports.iter().all(|r| r["ok"] == true),
        "some signal observations failed; inspect the preserved report"
    );
}
