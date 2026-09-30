// KT-924 — an audit step on an HTTP agent: the real launcher, the real runner and
// the real bounded file tools. The two tests that need a provider use a mocked
// OpenAI-wire server (a loopback listener); everything else runs without a socket.
use super::*;
use crate::agents::tools::ToolCall;
use crate::api::audit::full::{rewrite_proof_verdict, RewriteProofVerdict};
use crate::api::audit::validation::{target_snapshot, validate_step_output, TargetSnapshot};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(frames: &[String]) -> String {
    frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n"
}

fn tool_calls(calls: &[(&str, Value)]) -> String {
    let calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, (name, arguments))| {
            json!({
                "index": index,
                "id": format!("call-{index}"),
                "function": {"name": name, "arguments": arguments.to_string()},
            })
        })
        .collect();
    json!({"choices": [{"index": 0, "delta": {"tool_calls": calls}}]}).to_string()
}

fn text(content: &str) -> String {
    json!({"choices": [{"index": 0, "delta": {"content": content}}]}).to_string()
}

/// A provider that answers the first request with `first` and every later one with
/// `then`, keeping each request body so the test can read what the model was told.
async fn provider(first: String, then: String) -> (MockServer, Arc<Mutex<Vec<Value>>>) {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let mut seen = seen.lock().unwrap();
            seen.push(body);
            let frame = if seen.len() == 1 { &first } else { &then };
            ResponseTemplate::new(200).set_body_string(sse(std::slice::from_ref(frame)))
        })
        .mount(&server)
        .await;
    (server, requests)
}

async fn litellm_state(endpoint: &str) -> AppState {
    let mut config = crate::core::config::default_config();
    config.agents.lite_llm.base_url = Some(endpoint.to_string());
    config.agents.model_tiers.lite_llm.reasoning = Some("audit-model".into());
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// One audit step, started the way `full_audit` and `partial_audit` start it.
/// Returns whether the run reported success, and everything the model said.
async fn run_step(state: &AppState, project: &Path, prompt: &str) -> (bool, String) {
    let launcher = AuditAgentLauncher::new(state, &AgentType::LiteLlm).await;
    let tokens = state.config.read().await.tokens.clone();
    let mut process = launcher
        .start(
            &AgentType::LiteLlm,
            ModelTier::Reasoning,
            project,
            &project.to_string_lossy(),
            prompt,
            &tokens,
            None,
        )
        .await
        .expect("an HTTP audit agent starts");
    let mut said = String::new();
    while let Some(line) = process.next_line().await {
        said.push_str(&line);
    }
    let status = process.child.wait().await.expect("lifeline");
    (status.success(), said)
}

/// The gates a step must clear once its agent has exited 0, in the order
/// `full_audit` and `partial_audit` apply them: the output validator, then the
/// rewrite proof against the snapshot taken before the agent ran.
fn step_passes_its_gates(
    cli_success: bool,
    project: &Path,
    target: &str,
    before: &TargetSnapshot,
) -> bool {
    let (success, _) = validate_step_output(cli_success, project, target);
    success
        && matches!(
            rewrite_proof_verdict(target, before, target_snapshot(project, target)),
            RewriteProofVerdict::Proven | RewriteProofVerdict::Skipped
        )
}

#[tokio::test]
async fn an_http_audit_step_writes_inside_the_project_and_is_refused_outside() {
    let outer = tempfile::tempdir().unwrap();
    let project = outer.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let outside = outer.path().join("outside.md");
    let deliverable = "# Project\n\nFilled in by the agent.\n";
    let (server, requests) = provider(
        tool_calls(&[
            (
                "write_file",
                json!({"path": "docs/AGENTS.md", "content": deliverable}),
            ),
            (
                "write_file",
                json!({"path": "../outside.md", "content": "escaped"}),
            ),
            (
                "write_file",
                json!({"path": outside.to_string_lossy(), "content": "escaped"}),
            ),
        ]),
        text("Done."),
    )
    .await;
    let state = litellm_state(&server.uri()).await;

    let (success, _) = run_step(&state, &project, "Fill docs/AGENTS.md").await;

    assert!(success, "the run itself succeeds");
    assert_eq!(
        std::fs::read_to_string(project.join("docs/AGENTS.md")).unwrap(),
        deliverable,
        "the deliverable is written by the native tool, inside the project"
    );
    assert!(!outside.exists(), "nothing lands outside the project");
    assert!(
        !outer.path().join("docs").exists(),
        "a relative escape must not create its own tree either"
    );

    let requests = requests.lock().unwrap();
    assert!(requests.len() >= 2, "one tool round, then the answer");
    assert_eq!(
        requests[0]["model"], "audit-model",
        "the model comes from the tier Settings configure, not a guess"
    );
    let mut declared: Vec<&str> = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool.pointer("/function/name")?.as_str())
        .collect();
    declared.sort_unstable();
    let mut expected: Vec<&str> = crate::api::agent_tools::AUDIT_TOOLS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        declared, expected,
        "the model is offered the bounded tools only"
    );
    let results: Vec<String> = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].to_string())
        .collect();
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(!results[0].contains("refused"), "{}", results[0]);
    assert!(results[1].contains("refused"), "{}", results[1]);
    assert!(results[2].contains("refused"), "{}", results[2]);
}

#[tokio::test]
async fn a_prose_only_http_run_exits_cleanly_and_writes_nothing() {
    // The premise of the silent-success risk: a model that answers in prose
    // instead of calling a tool finishes without error, so the exit code alone
    // would call the step a success.
    let prose = text("Here is the documentation you asked for: # Probe. It is thorough.");
    let (server, requests) = provider(prose.clone(), prose).await;
    let state = litellm_state(&server.uri()).await;
    let project = tempfile::tempdir().unwrap();

    let (success, said) = run_step(&state, project.path(), "Write docs/http-step-probe.md").await;

    assert!(success, "a prose-only run exits 0: {said}");
    assert_eq!(requests.lock().unwrap().len(), 1, "no tool round happened");
    assert!(
        std::fs::read_dir(project.path()).unwrap().next().is_none(),
        "and nothing was written"
    );
}

#[tokio::test]
async fn a_clean_exit_that_wrote_nothing_fails_the_step_gates() {
    // `cli_success` is true throughout: that is what an HTTP run that ended
    // without error reports, written its file or not.
    let target = "docs/http-step-probe.md";

    // (a) The deliverable never appeared.
    let project = tempfile::tempdir().unwrap();
    let before = target_snapshot(project.path(), target).unwrap();
    let (passes, warning) = validate_step_output(true, project.path(), target);
    assert!(!passes, "a missing deliverable fails the step");
    assert!(
        warning.is_some_and(|w| w.reason.contains("no output")),
        "and says why"
    );
    assert!(!step_passes_its_gates(
        true,
        project.path(),
        target,
        &before
    ));

    // (b) A file from an earlier audit is there and the agent left it alone: the
    // validator sees a plausible file, only the rewrite proof can tell.
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("docs")).unwrap();
    std::fs::write(
        project.path().join(target),
        "# Probe\n\nFrom the last audit.\n",
    )
    .unwrap();
    let before = target_snapshot(project.path(), target).unwrap();
    assert!(
        validate_step_output(true, project.path(), target).0,
        "the file looks fine to the validator: only the rewrite proof catches this"
    );
    assert!(
        !step_passes_its_gates(true, project.path(), target, &before),
        "an untouched target must not be recorded as a refreshed one"
    );

    // (c) Control: once the agent really writes through its tool, both gates pass.
    let project = tempfile::tempdir().unwrap();
    let before = target_snapshot(project.path(), target).unwrap();
    let state = litellm_state("http://127.0.0.1:1").await;
    let tools =
        crate::api::agent_tools::KronnToolExecutor::audit_arc(state, project.path().to_path_buf());
    let written = tools
        .execute(&ToolCall {
            id: "write".into(),
            name: "write_file".into(),
            arguments: json!({"path": target, "content": "# Probe\n\nWritten through the tool.\n"}),
        })
        .await;
    assert!(written.ok, "{}", written.content);
    assert!(step_passes_its_gates(true, project.path(), target, &before));
}

#[tokio::test]
async fn an_unreachable_provider_is_a_start_failure_never_a_clean_run() {
    let state = litellm_state("http://127.0.0.1:1").await;
    let project = tempfile::tempdir().unwrap();
    let launcher = AuditAgentLauncher::new(&state, &AgentType::LiteLlm).await;
    let tokens = state.config.read().await.tokens.clone();
    let started = launcher
        .start(
            &AgentType::LiteLlm,
            ModelTier::Reasoning,
            project.path(),
            &project.path().to_string_lossy(),
            "Write the probe doc",
            &tokens,
            None,
        )
        .await;
    assert!(
        started.is_err(),
        "the pipeline records this step as a start error"
    );
}

#[tokio::test]
async fn a_missing_model_is_named_instead_of_guessed() {
    // No tier model configured for LiteLLM: the step fails with the remedy, it
    // does not invent a model id the proxy would answer with an opaque 404.
    let mut config = crate::core::config::default_config();
    config.agents.lite_llm.base_url = Some("http://127.0.0.1:1".into());
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let project = tempfile::tempdir().unwrap();
    let launcher = AuditAgentLauncher::new(&state, &AgentType::LiteLlm).await;
    let tokens = state.config.read().await.tokens.clone();
    let error = launcher
        .start(
            &AgentType::LiteLlm,
            ModelTier::Reasoning,
            project.path(),
            &project.path().to_string_lossy(),
            "Write the probe doc",
            &tokens,
            None,
        )
        .await
        .err()
        .expect("no model, no run");
    assert!(error.contains("No LiteLLM model configured"), "{error}");
}

#[tokio::test]
async fn a_cli_agent_keeps_the_spawn_it_always_had() {
    let state = litellm_state("http://127.0.0.1:1").await;
    for agent in [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::OpenCode,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
    ] {
        let launcher = AuditAgentLauncher::new(&state, &agent).await;
        assert!(
            launcher.http.is_none(),
            "{agent:?} has a filesystem of its own"
        );
        // ...but it runs in Kronn's ACP host, so what `start` returns is a
        // lifeline and a Stop goes through the session (KT-927).
        assert!(
            launcher.stops_with_token(),
            "{agent:?} is stopped through its ACP session, not through a PID"
        );
    }
    for agent in [AgentType::Ollama, AgentType::LiteLlm] {
        let launcher = AuditAgentLauncher::new(&state, &agent).await;
        assert!(
            launcher.http.is_some(),
            "{agent:?} runs in Kronn's tool loop"
        );
        assert!(launcher.stops_with_token());
    }
}

#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn an_agent_forced_onto_its_direct_cli_is_still_stopped_by_its_pid() {
    // The explicit compatibility override: the process IS the agent again.
    std::env::set_var("KRONN_ACP_ADAPTER_CLAUDE", "0");
    let state = litellm_state("http://127.0.0.1:1").await;
    let launcher = AuditAgentLauncher::new(&state, &AgentType::ClaudeCode).await;
    std::env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
    assert!(launcher.http.is_none());
    assert!(
        !launcher.stops_with_token(),
        "a direct CLI is killed by its PID, which is the agent's own"
    );
}

#[tokio::test]
async fn the_stop_token_reaches_an_http_agent_no_process_kill_can_reach() {
    // An HTTP agent has no PID that matters: its request and tool loop are a task.
    // A token already tripped must stop it before the provider is ever asked.
    let state = litellm_state("http://127.0.0.1:1").await;
    let project = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let launcher = AuditAgentLauncher::new(&state, &AgentType::LiteLlm).await;
    let tokens = state.config.read().await.tokens.clone();
    let error = launcher
        .start(
            &AgentType::LiteLlm,
            ModelTier::Reasoning,
            project.path(),
            &project.path().to_string_lossy(),
            "Write the probe doc",
            &tokens,
            Some(cancel),
        )
        .await
        .err()
        .expect("a stopped run does not start");
    assert!(error.contains("cancelled"), "{error}");
}

#[tokio::test]
async fn cancel_audit_trips_the_token_of_a_running_http_step() {
    let state = litellm_state("http://127.0.0.1:1").await;
    let project = tempfile::tempdir().unwrap();
    let path_text = project.path().to_string_lossy().to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p-stop', 'P', ?1, datetime('now'), datetime('now'))",
                rusqlite::params![path_text],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let token = CancellationToken::new();
    {
        let mut tracker = state.audit_tracker.lock().unwrap();
        tracker.agent_cancels.insert("p-stop".into(), token.clone());
    }
    // The audit worker acknowledges a cancel once its step has stopped; here the
    // step stops when — and only when — the token is tripped.
    let tracker = state.audit_tracker.clone();
    let worker_token = token.clone();
    tokio::spawn(async move {
        worker_token.cancelled().await;
        tracker.lock().unwrap().cancelled.remove("p-stop");
    });

    let response = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("p-stop".to_string()),
    )
    .await;

    assert!(response.0.success, "{:?}", response.0.error);
    assert!(
        token.is_cancelled(),
        "Stop must reach the HTTP agent's task"
    );
    assert!(
        !state
            .audit_tracker
            .lock()
            .unwrap()
            .agent_cancels
            .contains_key("p-stop"),
        "the token is taken, not left for the next run"
    );
}

/// Drive the partial-audit handler on a fresh project and return its SSE body.
async fn partial_audit_stream(state: &AppState, project: &Path, agent: AgentType) -> String {
    use axum::response::IntoResponse;
    let path = project.to_string_lossy().into_owned();
    let row: crate::models::Project = serde_json::from_value(json!({
        "id": "proj-partial", "name": "partial", "path": path,
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": chrono::Utc::now().to_rfc3339(), "updated_at": chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &row))
        .await
        .unwrap();
    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let step = chain
        .iter()
        .position(crate::api::audit::partial_selectable)
        .expect("a refreshable section")
        + 1;
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-partial".to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent,
            tier: None,
            steps: vec![step],
        }),
    )
    .await
    .into_response();
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        axum::body::to_bytes(response.into_body(), 1 << 20),
    )
    .await
    .expect("the partial stream ends")
    .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

#[tokio::test]
async fn the_partial_audit_applies_the_same_gate_and_launcher() {
    // NVIDIA stays refused, with the one message the Full launch uses.
    let state = litellm_state("http://127.0.0.1:1").await;
    let project = tempfile::tempdir().unwrap();
    let refused = partial_audit_stream(&state, project.path(), AgentType::Nvidia).await;
    assert!(refused.contains("cannot run audits"), "{refused}");
    assert!(
        refused.contains("Ollama") && refused.contains("LiteLLM"),
        "{refused}"
    );

    // LiteLLM passes the gate and reaches the HTTP launcher: with no tier model
    // configured it fails on the launcher's own remedy, never on the gate.
    let mut config = crate::core::config::default_config();
    config.agents.lite_llm.base_url = Some("http://127.0.0.1:1".into());
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let project = tempfile::tempdir().unwrap();
    let admitted = partial_audit_stream(&state, project.path(), AgentType::LiteLlm).await;
    assert!(!admitted.contains("cannot run audits"), "{admitted}");
    assert!(
        admitted.contains("No LiteLLM model configured"),
        "{admitted}"
    );
}

// ── KT-926 — what an audit prompt says about OTHER repos ─────────────────────
//
// The prompt goes to the model provider, so the only repo it may name besides the
// audited one is a repo the user explicitly linked to the project. These tests run
// the two real pipelines (`full_audit`, `partial_audit`) on a scripted `claude`
// routed to the audited project, and read what that agent was handed on stdin —
// the prompt as the model receives it, not a string the test assembled. No socket.

#[cfg(unix)]
use crate::api::other_projects_fixture::{
    assert_no_candidate_pool, assert_no_unlinked_project, fresh_state, project_among_others,
    recorded_turns, recording_claude, route_claude, BRIEFING_MARKER, LINKED_LOCATION, LINKED_NAME,
    PROJECT_ID,
};

#[cfg(unix)]
async fn sse_body(response: axum::response::Response) -> String {
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        axum::body::to_bytes(response.into_body(), 1 << 22),
    )
    .await
    .expect("the audit stream ends")
    .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

#[cfg(unix)]
/// Every section a partial audit may refresh, as 1-based step numbers.
fn refreshable_steps() -> Vec<usize> {
    crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
        .iter()
        .enumerate()
        .filter(|(_, step)| crate::api::audit::partial_selectable(step))
        .map(|(index, _)| index + 1)
        .collect()
}

#[cfg(unix)]
/// How many steps the pipeline runs: the foundation steps plus the chained
/// sub-audits for a Full audit, every refreshable section for a partial one.
fn expected_step_count(pipeline: &str) -> usize {
    if pipeline == "full" {
        crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len()
    } else {
        refreshable_steps().len()
    }
}

/// Run one audit pipeline to its end and return the prompt of every step, as the
/// agent was handed it. The scripted `claude` records its stdin and its arguments,
/// then ends a turn that wrote nothing: the steps fail their gates, which is of no
/// interest here — only what each one was told is.
#[cfg(unix)]
async fn step_prompts_of(pipeline: &str, linked: bool) -> Vec<String> {
    use axum::response::IntoResponse;
    let tools = tempfile::tempdir().unwrap();
    let (fixture, log) = recording_claude(tools.path());

    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), linked).await;
    let _route = route_claude(project.path(), &fixture);

    let response = if pipeline == "full" {
        crate::api::audit::full::full_audit(
            axum::extract::State(state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
            axum::Json(crate::models::LaunchAuditRequest {
                agent: AgentType::ClaudeCode,
                tier: None,
                kind: None,
                custom_prompt: None,
                resume_run_id: None,
            }),
        )
        .await
        .into_response()
    } else {
        crate::api::audit::drift::partial_audit(
            axum::extract::State(state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
            axum::Json(crate::models::PartialAuditRequest {
                agent: AgentType::ClaudeCode,
                tier: None,
                steps: refreshable_steps(),
            }),
        )
        .await
        .into_response()
    };
    let stream = sse_body(response).await;
    assert!(
        !stream.contains("event: error"),
        "the {pipeline} audit must start: {stream}"
    );
    let prompts: Vec<String> = recorded_turns(&log)
        .into_iter()
        .filter(|turn| turn.contains(BRIEFING_MARKER))
        .collect();
    assert!(
        !prompts.is_empty(),
        "the {pipeline} audit handed its agent no step prompt: {stream}"
    );
    prompts
}

#[cfg(unix)]
#[tokio::test]
async fn no_step_of_the_full_or_partial_audit_names_an_unlinked_kronn_project() {
    for pipeline in ["full", "partial"] {
        let prompts = step_prompts_of(pipeline, false).await;
        assert!(
            prompts.len() >= expected_step_count(pipeline),
            "{pipeline}: {} step prompts for {} steps",
            prompts.len(),
            expected_step_count(pipeline)
        );
        for prompt in &prompts {
            assert_no_unlinked_project(prompt, pipeline);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_explicitly_linked_repo_stays_in_every_step_prompt_and_the_others_stay_out() {
    for pipeline in ["full", "partial"] {
        let prompts = step_prompts_of(pipeline, true).await;
        assert!(
            prompts.len() >= expected_step_count(pipeline),
            "{pipeline}: {} step prompts for {} steps",
            prompts.len(),
            expected_step_count(pipeline)
        );
        for prompt in &prompts {
            assert!(
                prompt.contains(LINKED_NAME) && prompt.contains(LINKED_LOCATION),
                "{pipeline}: the linked repo is a voluntary declaration and stays"
            );
            assert_no_unlinked_project(prompt, pipeline);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_audit_prompt_no_longer_reads_another_repos_docs_or_lists_the_machines_projects() {
    for pipeline in ["full", "partial"] {
        for prompt in step_prompts_of(pipeline, false).await {
            assert_no_candidate_pool(&prompt, pipeline);
            // Only a linked repo is read through its `docs/AGENTS.md`; with none
            // linked, no step tells the agent to open another repo's docs.
            assert!(
                !prompt.contains("<repo-path>/docs/AGENTS.md"),
                "{pipeline}: a prompt with no linked repo sends the agent to another repo's docs"
            );
        }
    }
}

#[test]
fn no_static_audit_prompt_carries_the_retired_companion_pool() {
    let mut prompts: Vec<String> = Vec::new();
    for kind in [
        crate::models::AuditKind::Full,
        crate::models::AuditKind::Security,
        crate::models::AuditKind::Docker,
    ] {
        prompts.extend(
            crate::api::audit::assemble_chained_steps(kind)
                .iter()
                .map(|step| step.prompt.to_string()),
        );
    }
    prompts.push(crate::api::audit::PROMPT_PREAMBLE.to_string());
    for language in ["en", "fr", "es"] {
        prompts.push(crate::api::audit::helpers::build_briefing_prompt(
            language, None,
        ));
        prompts.push(crate::api::audit::helpers::build_briefing_prompt(
            language,
            Some("notes"),
        ));
    }
    for prompt in prompts {
        assert!(!prompt.contains("Suggested companion repos"));
        assert!(!prompt.contains("Other Kronn projects"));
    }
}
