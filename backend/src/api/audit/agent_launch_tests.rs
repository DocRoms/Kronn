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
async fn audit_launcher_writes_sixteen_real_findings_then_their_index() {
    let mut calls: Vec<_> = (1..=16)
        .map(|n| {
            (
                "write_file",
                json!({
                    "path":format!("docs/tech-debt/TD-{n}.md"), "content":format!("# Finding {n}\n")
                }),
            )
        })
        .collect();
    let index = (1..=16)
        .map(|n| format!("- [TD-{n}](tech-debt/TD-{n}.md)\n"))
        .collect::<String>();
    calls.push((
        "write_file",
        json!({"path":"docs/index.md","content":index}),
    ));
    let (server, _) = provider(
        tool_calls(&calls),
        text("All findings and their index were written."),
    )
    .await;
    let state = litellm_state(&server.uri()).await;
    let project = tempfile::tempdir().unwrap();
    assert!(
        run_step(
            &state,
            project.path(),
            "Write sixteen findings and their index"
        )
        .await
        .0
    );
    assert_eq!(
        std::fs::read_dir(project.path().join("docs/tech-debt"))
            .unwrap()
            .count(),
        16
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("docs/index.md")).unwrap(),
        index
    );
}

#[tokio::test]
async fn http_resume_repairs_an_auxiliary_document_from_a_previously_successful_step() {
    use axum::response::IntoResponse;
    for reference in ["code.rs:1", "invented.rs:1"] {
        let correct = reference == "code.rs:1";
        use sha2::{Digest, Sha256};
        let human = "<!-- kronn:section name=\"decision\" owner=\"human\" -->\nHuman decision remains.\n<!-- kronn:section:end -->\n";
        let original = format!("# Finding\n[src: file: invented.rs:1]\n{human}");
        let receipt: String = Sha256::digest(original.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let (server, requests) = provider(
            tool_calls(&[
                ("read_file", json!({"path":"docs/tech-debt/TD-repair.md"})),
                (
                    "write_file",
                    json!({
                        "path":"docs/tech-debt/TD-repair.md",
                        "expected_sha256":receipt,
                        "content":format!("# Finding\n\nVerified source [src: file: {reference}]\n")
                    }),
                ),
            ]),
            text("The documented reference is corrected."),
        )
        .await;
        let state = litellm_state(&server.uri()).await;
        let project = tempfile::tempdir().unwrap();
        project_among_others(&state, project.path(), false).await;
        std::fs::create_dir_all(project.path().join("docs/tech-debt")).unwrap();
        std::fs::create_dir_all(project.path().join("docs/conventions")).unwrap();
        std::fs::write(
            project
                .path()
                .join("docs/conventions/agents-md-format-v1.md"),
            "# Format\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("docs/AGENTS.md"),
            format!("# Project\n{}", "Established documentation.\n".repeat(500)),
        )
        .unwrap();
        std::fs::write(project.path().join("code.rs"), "actual source\n").unwrap();
        std::fs::write(
            project.path().join("docs/tech-debt/TD-repair.md"),
            format!("# Finding\n[src: file: invented.rs:1]\n{human}"),
        )
        .unwrap();
        let total =
            crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len() as u32;
        state
            .db
            .with_conn(move |conn| {
                use crate::db::audit_runs as runs;
                let now = chrono::Utc::now() - chrono::Duration::hours(1);
                runs::insert_running(conn, "before-repair", PROJECT_ID, "Full", "LiteLlm", now)?;
                for step in 1..=total {
                    runs::insert_audit_step_start(conn, "before-repair", step, "doc", now)?;
                    runs::finalize_audit_step(
                        conn,
                        "before-repair",
                        step,
                        now,
                        10,
                        &runs::StepTokens::UNKNOWN,
                        None,
                        true,
                        None,
                        false,
                    )?;
                }
                runs::update_last_completed_step(conn, "before-repair", total)?;
                runs::mark_interrupted(conn, "before-repair", "final documentary gate failed")
            })
            .await
            .unwrap();
        let response = crate::api::audit::full::full_audit(
            axum::extract::State(state.clone()),
            axum::extract::Path(PROJECT_ID.into()),
            axum::Json(crate::models::LaunchAuditRequest {
                agent: AgentType::LiteLlm,
                tier: Some(ModelTier::Reasoning),
                kind: None,
                custom_prompt: None,
                resume_run_id: Some("before-repair".into()),
            }),
        )
        .await
        .into_response();
        let stream = sse_body(response).await;
        let done = sse_events(&stream, "done").pop().expect("terminal outcome");
        assert_eq!(
            done["status"],
            if correct { "complete" } else { "interrupted" },
            "{stream}"
        );
        assert_eq!(
            done["steps_to_redo"],
            if correct { json!([]) } else { json!([1]) },
            "{stream}"
        );
        assert_eq!(
            done["discussion_id"].is_string(),
            correct,
            "validation requires a clean gate: {stream}"
        );
        if !correct {
            assert_eq!(
                sse_events(&stream, "step_retry").len(),
                2,
                "exactly two corrective retries: {stream}"
            );
            assert_eq!(
                requests.lock().unwrap().len(),
                4,
                "one tool response and three bounded attempts"
            );
        }
        assert_eq!(
            sse_events(&stream, "step_start").len(),
            1,
            "only the owning recovery step reruns"
        );
        let repaired =
            std::fs::read_to_string(project.path().join("docs/tech-debt/TD-repair.md")).unwrap();
        assert!(
            repaired.contains(human.trim_end()),
            "human section restored before validation: {repaired:?}"
        );
        assert_eq!(repaired.contains("invented.rs"), !correct);
        assert!(
            std::fs::read_dir(project.path().join("docs/.kronn-citation-originals"))
                .unwrap()
                .any(
                    |entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()
                        == format!("# Finding\n[src: file: invented.rs:1]\n{human}")
                ),
            "the original survives correction"
        );
        assert!(requests.lock().unwrap()[0]
            .to_string()
            .contains("targeted correction required"));
    }
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
        let launcher =
            AuditAgentLauncher::with_route(&state, &agent, crate::acp::production_route(&agent))
                .await;
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
async fn an_agent_forced_onto_its_direct_cli_is_still_stopped_by_its_pid() {
    // The explicit compatibility override: the process IS the agent again.
    let state = litellm_state("http://127.0.0.1:1").await;
    let launcher = AuditAgentLauncher::with_route(
        &state,
        &AgentType::ClaudeCode,
        crate::acp::AcpProductionRoute::DirectCliMigration,
    )
    .await;
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

// ── KT-931 — a failed step no longer voids the run ───────────────────────────
//
// The real `full_audit` pipeline, resuming a run that lost ONE step (the bench
// run A1: 15 of 16 steps done, the fifth failed for an external cause) on the
// scripted `claude`. The agent writes nothing, so the resumed step fails its
// gates again — exactly the case where the run used to end with no validation
// at all while the 15 documents of the other steps existed.

/// The `data:` payloads of every `event: <name>` frame in an SSE body.
#[cfg(unix)]
fn sse_events(body: &str, name: &str) -> Vec<Value> {
    let header = format!("event: {name}\n");
    body.split("\n\n")
        .filter_map(|frame| frame.trim_start().strip_prefix(header.as_str()))
        .filter_map(|rest| rest.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

#[cfg(unix)]
#[tokio::test]
async fn a_resume_reruns_only_the_failed_step_and_the_partial_run_is_still_validated() {
    use axum::response::IntoResponse;
    let tools = tempfile::tempdir().unwrap();
    let (fixture, log) = recording_claude(tools.path());
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;
    let _route = route_claude(project.path(), &fixture);
    // The 15 carried steps wrote their documents in the predecessor run; the
    // project already has its docs, so no raw template is installed over them
    // (whose unrewritten links the invented-path guard would, rightly, refuse).
    std::fs::create_dir_all(project.path().join("docs/conventions")).unwrap();
    std::fs::write(
        project.path().join("docs/AGENTS.md"),
        "# Audited project\n\nWritten by the predecessor run.\n",
    )
    .unwrap();
    // The anti-hallucination section every run (re)writes into docs/AGENTS.md
    // links to this convention.
    std::fs::write(
        project
            .path()
            .join("docs/conventions/agents-md-format-v1.md"),
        "# AGENTS.md format\n\nThe format of the entry file.\n",
    )
    .unwrap();

    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let total = chain.len() as u32;
    let failed_step = 5u32;

    // The interrupted predecessor: every step finished, the fifth unsuccessfully.
    state
        .db
        .with_conn(move |conn| {
            use crate::db::audit_runs as runs;
            let started = chrono::Utc::now() - chrono::Duration::hours(1);
            runs::insert_running(conn, "run-prev", PROJECT_ID, "Full", "ClaudeCode", started)?;
            let mut succeeded = 0;
            for step in 1..=total {
                let label = format!("docs/step-{step}.md");
                runs::insert_audit_step_start(conn, "run-prev", step, &label, started)?;
                let ok = step != failed_step;
                runs::finalize_audit_step(
                    conn,
                    "run-prev",
                    step,
                    started,
                    10,
                    &runs::StepTokens::UNKNOWN,
                    None,
                    ok,
                    (!ok).then_some("Mac went to sleep"),
                    false,
                )?;
                if ok {
                    succeeded += 1;
                    runs::update_last_completed_step(conn, "run-prev", succeeded)?;
                }
            }
            runs::mark_interrupted(conn, "run-prev", "warned steps: [5]")
        })
        .await
        .unwrap();

    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some("run-prev".into()),
        }),
    )
    .await
    .into_response();
    let stream = sse_body(response).await;
    assert!(
        !stream.contains("event: error"),
        "the resume must run: {stream}"
    );

    // A resume re-runs ONLY the failed step: one agent turn, 15 steps skipped.
    let step_prompts = recorded_turns(&log)
        .into_iter()
        .filter(|turn| turn.contains(BRIEFING_MARKER))
        .count();
    assert_eq!(
        step_prompts, 1,
        "only step {failed_step} may reach the agent: {stream}"
    );
    assert_eq!(sse_events(&stream, "step_skipped").len() as u32, total - 1);
    let started = sse_events(&stream, "step_start");
    assert_eq!(started.len(), 1, "{stream}");
    assert_eq!(started[0]["step"], failed_step);

    // The step failed again, yet the run is not voided: it is validated for
    // the 15 steps that succeeded and names the one to redo.
    let done = sse_events(&stream, "done")
        .pop()
        .expect("a terminal done event");
    assert_eq!(done["status"], "interrupted", "{done}");
    assert_eq!(done["last_completed_step"], total - 1, "{done}");
    assert_eq!(done["steps_to_redo"], json!([failed_step]), "{done}");
    let discussion_id = done["discussion_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a partial run must create its validation discussion: {stream}"))
        .to_string();
    assert!(stream.contains("event: validation_created"), "{stream}");

    let (run, discussion, resumed_steps) = state
        .db
        .with_conn(move |conn| {
            let run = crate::db::audit_runs::list_recent(conn, PROJECT_ID, 1)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no audit run recorded"))?;
            let discussion = crate::db::discussions::get_discussion(conn, &discussion_id)?;
            let steps = crate::db::audit_runs::list_audit_steps(conn, &run.id)?;
            Ok((run, discussion, steps))
        })
        .await
        .unwrap();
    assert_ne!(run.id, "run-prev");
    assert_eq!(run.status, "Interrupted");
    assert_eq!(
        run.last_completed_step,
        total - 1,
        "the progress counts every success"
    );
    assert_eq!(
        run.validation_discussion_id,
        done["discussion_id"].as_str().map(str::to_string)
    );
    let prompt = &discussion.expect("the discussion is persisted").messages[0].content;
    assert!(
        prompt.contains(&format!("{failed_step}/{total}")),
        "the validation lists the step to redo: {prompt}"
    );

    // The resumed run recorded what it carried over: a second resume still
    // only has the failed step to run.
    let still_done = crate::api::audit::full::already_succeeded_step_indices(&resumed_steps);
    let expected: std::collections::HashSet<u32> =
        (1..=total).filter(|step| *step != failed_step).collect();
    assert_eq!(still_done, expected);
}

// KT-931 review (Romuald) — the founding step (step 1, the one producing
// `docs/AGENTS.md`) gates the partial validation: if IT is the one that
// failed, the other 15 successes get no validation discussion, only a
// resumable run naming step 1 to redo.
#[cfg(unix)]
#[tokio::test]
async fn a_resume_that_loses_only_the_founding_step_gets_no_validation() {
    use axum::response::IntoResponse;
    let tools = tempfile::tempdir().unwrap();
    let (fixture, log) = recording_claude(tools.path());
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;
    let _route = route_claude(project.path(), &fixture);
    // Step 1 (the founding step) never wrote `docs/AGENTS.md`: no entry
    // point exists on disk, same as in the predecessor run. The `docs/`
    // directory itself exists (steps 2..=16 "succeeded" and would have
    // written into it), so the documentary-optimization gate has
    // something to resolve against and doesn't block on its own.
    std::fs::create_dir_all(project.path().join("docs")).unwrap();

    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let total = chain.len() as u32;
    let founding_step = 1u32;

    // The interrupted predecessor: every step finished, the founding one
    // unsuccessfully.
    state
        .db
        .with_conn(move |conn| {
            use crate::db::audit_runs as runs;
            let started = chrono::Utc::now() - chrono::Duration::hours(1);
            runs::insert_running(conn, "run-prev", PROJECT_ID, "Full", "ClaudeCode", started)?;
            let mut succeeded = 0;
            for step in 1..=total {
                let label = format!("docs/step-{step}.md");
                runs::insert_audit_step_start(conn, "run-prev", step, &label, started)?;
                let ok = step != founding_step;
                runs::finalize_audit_step(
                    conn,
                    "run-prev",
                    step,
                    started,
                    10,
                    &runs::StepTokens::UNKNOWN,
                    None,
                    ok,
                    (!ok).then_some("Mac went to sleep"),
                    false,
                )?;
                if ok {
                    succeeded += 1;
                    runs::update_last_completed_step(conn, "run-prev", succeeded)?;
                }
            }
            runs::mark_interrupted(conn, "run-prev", "warned steps: [1]")
        })
        .await
        .unwrap();

    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some("run-prev".into()),
        }),
    )
    .await
    .into_response();
    let stream = sse_body(response).await;
    assert!(
        !stream.contains("event: error"),
        "the resume must run: {stream}"
    );

    // A resume re-runs ONLY the founding step: one agent turn, the rest skipped.
    let step_prompts = recorded_turns(&log)
        .into_iter()
        .filter(|turn| turn.contains(BRIEFING_MARKER))
        .count();
    assert_eq!(
        step_prompts, 1,
        "only step {founding_step} may reach the agent: {stream}"
    );
    assert_eq!(sse_events(&stream, "step_skipped").len() as u32, total - 1);

    // The founding step fails again: no validation discussion is created,
    // even though 15 other steps succeeded.
    let done = sse_events(&stream, "done")
        .pop()
        .expect("a terminal done event");
    assert_eq!(done["status"], "interrupted", "{done}");
    assert_eq!(done["last_completed_step"], total - 1, "{done}");
    assert_eq!(done["steps_to_redo"], json!([founding_step]), "{done}");
    assert!(
        done["discussion_id"].is_null(),
        "no entry point, no validation: {done}"
    );
    assert!(!stream.contains("event: validation_created"), "{stream}");

    let run = state
        .db
        .with_conn(move |conn| {
            crate::db::audit_runs::list_recent(conn, PROJECT_ID, 1)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no audit run recorded"))
        })
        .await
        .unwrap();
    assert_ne!(run.id, "run-prev");
    assert_eq!(run.status, "Interrupted");
    assert_eq!(
        run.last_completed_step,
        total - 1,
        "the progress still counts every success"
    );
    assert!(
        run.validation_discussion_id.is_none(),
        "the run must link no validation discussion"
    );
}
