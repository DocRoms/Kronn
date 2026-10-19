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

#[cfg(unix)]
#[path = "coverage_repair_tests.rs"]
mod coverage_repair_tests;

#[cfg(unix)]
#[path = "detector_retry_tests.rs"]
mod detector_retry_tests;

#[cfg(unix)]
#[path = "reaudit_unchanged_tests.rs"]
mod reaudit_unchanged_tests;

#[cfg(unix)]
#[path = "cross_audit_e2e_tests.rs"]
mod cross_audit_e2e_tests;

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
    // Not from wiremock's pool: a pooled server keeps its address across tests,
    // so another test's late request could land in this one's count.
    let server = MockServer::builder().start().await;
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
    config.agents.model_tiers.lite_llm.economy = Some("economy-model".into());
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
            "docs/AGENTS.md",
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

/// A committed fixture, so `git status` after the run lists only what it wrote.
fn commit_fixture(root: &Path) {
    for args in [
        &["init", "-q"][..],
        &["add", "-A"],
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    ] {
        let out = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
}

/// After an audit only documentation shows; Kronn's working state is ignored.
fn assert_git_shows_only_docs(root: &Path) {
    let out = crate::core::cmd::git_cmd()
        .args([
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=all",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&out.stdout);
    for line in status.lines() {
        let path = &line[3..];
        // The run state is versioned on purpose (KT-1199): a colleague who
        // fetches resumes or validates from it. Any other `.kronn-` file is
        // leaked working state.
        let versioned = path.ends_with(&format!(
            "docs/{}",
            crate::core::audit_run_state::RUN_STATE_FILENAME
        ));
        assert!(versioned || !path.contains(".kronn-"), "{status}");
        if line.starts_with("!! ") {
            // Template installation also excludes the documented scratch area.
            assert!(
                path.starts_with(".kronn/") || path.starts_with("docs/var/"),
                "{status}"
            );
        } else {
            // Documentation and its versioned metadata (`.kronn.json`, checksums).
            assert!(
                path.starts_with("docs/") || path.ends_with(".md"),
                "{status}"
            );
        }
    }
    assert!(!root.join(".gitignore").exists(), "{status}");
}

#[cfg(unix)]
#[tokio::test]
async fn http_resume_repairs_an_auxiliary_document_from_a_previously_successful_step() {
    use axum::response::IntoResponse;
    // Kept until the end: the first pass's validation discussion still runs, and
    // a freed port could be handed to the second pass's server.
    let mut servers = Vec::new();
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
        commit_fixture(project.path());
        // What an older version left in docs/: moved out on the first run.
        let legacy = project.path().join("docs/.kronn-citation-originals");
        std::fs::create_dir(&legacy).unwrap();
        let legacy_original = b"# Earlier original\n";
        let legacy_name = format!(
            "{}.bak",
            Sha256::digest(legacy_original)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        std::fs::write(legacy.join(&legacy_name), legacy_original).unwrap();
        std::fs::write(
            project
                .path()
                .join("docs/.kronn-document-optimization.json"),
            "{}\n",
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
                connection_id: None,
                tier: Some(ModelTier::Reasoning),
                kind: None,
                custom_prompt: None,
                resume_run_id: Some("before-repair".into()),
                retry_steps: None,
                redo_steps: None,
                accept_unverified_agents: None,
                fresh: None,
            }),
        )
        .await
        .into_response();
        let stream = sse_body(response).await;
        let done = sse_events(&stream, "done").pop().expect("terminal outcome");
        // KT-1370: an invented path the agent still leaves after its last
        // attempt is demoted to a flagged backticked path: the run completes.
        assert_eq!(done["status"], "complete", "{stream}");
        assert_eq!(done["steps_to_redo"], json!([]), "{stream}");
        assert!(
            done["discussion_id"].is_string(),
            "validation requires a clean gate: {stream}"
        );
        assert_eq!(
            done["unverified_references"],
            u8::from(!correct),
            "{stream}"
        );
        if !correct {
            assert_eq!(
                sse_events(&stream, "step_retry").len(),
                2,
                "exactly two corrective retries: {stream}"
            );
            let requests = requests.lock().unwrap();
            let asked: Vec<String> = requests
                .iter()
                .map(|r| {
                    r["messages"][1]["content"]
                        .to_string()
                        .chars()
                        .take(160)
                        .collect()
                })
                .collect();
            assert_eq!(
                requests.len(),
                4,
                "one tool response and three bounded attempts: {asked:#?}"
            );
        }
        assert_eq!(
            sse_events(&stream, "step_start").len(),
            1,
            "only the owning recovery step reruns"
        );
        let repaired =
            std::fs::read_to_string(project.path().join("docs/tech-debt/TD-repair.md")).unwrap();
        // The rewrite that dropped the human section stays (KT-1079): the
        // doc waits for its owner to reconfirm, with the human text as before.
        assert!(
            !repaired.contains(human.trim_end()),
            "never reverted: {repaired:?}"
        );
        let provenance = crate::core::doc_provenance::report(project.path(), true).unwrap();
        let marked = provenance
            .docs
            .iter()
            .find(|d| d.doc == "docs/tech-debt/TD-repair.md")
            .unwrap_or_else(|| panic!("{provenance:?}"));
        assert!(marked.changes[0]
            .before
            .as_deref()
            .unwrap()
            .contains("Human decision remains."));
        assert_eq!(repaired.contains("invented.rs"), !correct);
        if !correct {
            assert!(
                repaired.contains("`invented.rs` (unverified reference)"),
                "{repaired:?}"
            );
            assert!(
                !repaired.contains("[src: file: invented.rs"),
                "{repaired:?}"
            );
        }
        assert!(
            std::fs::read_dir(
                project
                    .path()
                    .join(crate::core::audit_state::CITATION_ORIGINALS_DIR)
            )
            .unwrap()
            .any(
                |entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()
                    == format!("# Finding\n[src: file: invented.rs:1]\n{human}")
            ),
            "the original survives correction"
        );
        assert_eq!(
            std::fs::read(
                project
                    .path()
                    .join(crate::core::audit_state::CITATION_ORIGINALS_DIR)
                    .join(&legacy_name)
            )
            .unwrap(),
            legacy_original,
            "the legacy original moved with its content"
        );
        assert!(project
            .path()
            .join(crate::core::audit_state::REPORT_FILE)
            .is_file());
        assert_git_shows_only_docs(project.path());
        assert!(requests.lock().unwrap()[0]
            .to_string()
            .contains("targeted correction required"));
        servers.push(server);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn http_start_failure_during_documentary_retry_persists_error_and_prior_usage() {
    assert_http_failure_is_persisted(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn http_failure_after_write_persists_status_and_preserves_partial_files_without_replay() {
    assert_http_failure_is_persisted(true).await;
}

#[cfg(unix)]
async fn assert_http_failure_is_persisted(after_write: bool) {
    use axum::response::IntoResponse;
    let server = MockServer::builder().start().await;
    let requests = Arc::new(Mutex::new(0));
    let seen = requests.clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            let mut seen = seen.lock().unwrap();
            *seen += 1;
            if *seen == 1 {
                ResponseTemplate::new(200).set_body_string(sse(&[
                    if after_write {
                        tool_calls(&[("write_file", json!({
                            "path":"docs/tech-debt/TD-partial.md",
                            "content":"# Partial finding\nWork completed before the provider failure.\n",
                        }))])
                    } else {
                        text("The documentation is unchanged.")
                    },
                    // OpenRouter states the response's cost in `usage.cost`.
                    json!({"choices":[],"usage":{"prompt_tokens":13,"completion_tokens":7,"cost":0.000321}})
                        .to_string(),
                ]))
            } else {
                ResponseTemplate::new(if after_write { 429 } else { 401 })
                    .set_body_json(json!({"error":{"message":"provider unavailable: private-provider-body"}}))
            }
        })
        .mount(&server)
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
    std::fs::write(
        project.path().join("docs/tech-debt/TD-repair.md"),
        "# Finding\n[src: file: invented.rs:1]\n",
    )
    .unwrap();
    commit_fixture(project.path());
    let total =
        crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len() as u32;
    state
        .db
        .with_conn(move |conn| {
            use crate::db::audit_runs as runs;
            let now = chrono::Utc::now() - chrono::Duration::hours(1);
            runs::insert_running(
                conn,
                "before-unavailable",
                PROJECT_ID,
                "Full",
                "LiteLlm",
                now,
            )?;
            for step in 1..=total {
                runs::insert_audit_step_start(conn, "before-unavailable", step, "doc", now)?;
                runs::finalize_audit_step(
                    conn,
                    "before-unavailable",
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
            runs::update_last_completed_step(conn, "before-unavailable", total)?;
            runs::mark_interrupted(conn, "before-unavailable", "documentary gate failed")
        })
        .await
        .unwrap();
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.into()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::LiteLlm,
            connection_id: None,
            tier: Some(ModelTier::Reasoning),
            kind: None,
            custom_prompt: None,
            resume_run_id: Some("before-unavailable".into()),
            retry_steps: None,
            redo_steps: None,
            accept_unverified_agents: None,
            fresh: None,
        }),
    )
    .await
    .into_response();
    let stream = sse_body(response).await;
    let done = sse_events(&stream, "done").pop().expect("terminal outcome");
    assert_eq!(done["status"], "interrupted", "{stream}");
    assert_eq!(done["steps_to_redo"], json!([1]), "{stream}");
    assert!(!done["discussion_id"].is_string());
    assert_eq!(
        sse_events(&stream, "step_retry").len(),
        usize::from(!after_write)
    );
    assert_eq!(
        sse_events(&stream, "step_error").len(),
        usize::from(!after_write)
    );
    assert_eq!(*requests.lock().unwrap(), 2);
    let step_done = sse_events(&stream, "step_done");
    assert_eq!(step_done.len(), 1);
    assert_eq!(
        step_done[0]["tokens"], 20,
        "usage survives a failed retry: {stream}"
    );
    let run_id = done["audit_run_id"].as_str().unwrap().to_owned();
    let steps = state
        .db
        .with_conn(move |conn| crate::db::audit_runs::list_audit_steps(conn, &run_id))
        .await
        .unwrap();
    let failed = steps.iter().find(|s| s.step_index == 1).unwrap();
    assert!(!failed.cli_success);
    assert!(
        failed.ended_at.is_some(),
        "a terminal launch error is finalized"
    );
    assert_eq!(failed.step_tokens, Some(20));
    assert_eq!(
        failed.cost_usd_micros,
        Some(321),
        "the provider-reported cost of the attempts that ran survives the failure"
    );
    assert_eq!(step_done[0]["cost_usd_micros"], 321, "{stream}");
    let warning = failed.step_warning.as_deref().unwrap();
    assert!(
        warning.contains(if after_write { "429" } else { "401" }),
        "{warning}"
    );
    if after_write {
        assert!(
            warning.contains("rate limit") && warning.contains("Partial files are preserved"),
            "{warning}"
        );
        assert!(!warning.contains("private-provider-body"));
        let warnings = sse_events(&stream, "step_warning");
        assert!(warnings
            .iter()
            .any(|w| w["reason"].as_str().is_some_and(|r| r.contains("429"))));
        assert_eq!(
            std::fs::read_to_string(project.path().join("docs/tech-debt/TD-partial.md")).unwrap(),
            "# Partial finding\nWork completed before the provider failure.\n"
        );
    }
    assert_eq!(
        crate::api::audit::full::already_succeeded_step_indices(&steps).len(),
        total as usize - 1
    );
    assert_git_shows_only_docs(project.path());
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
            "docs/AGENTS.md",
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
            "docs/AGENTS.md",
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
            "docs/AGENTS.md",
            None,
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
async fn partial_audit_stream(
    state: &AppState,
    project: &Path,
    agent: AgentType,
    connection: Option<&str>,
) -> String {
    use axum::response::IntoResponse;
    let path = project.to_string_lossy().into_owned();
    let project_id = format!("proj-partial-{}", uuid::Uuid::new_v4());
    let row: crate::models::Project = serde_json::from_value(json!({
        "id": project_id, "name": "partial", "path": path,
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
        .position(|step| {
            crate::api::audit::partial_selectable(
                step,
                &crate::core::docs_location::DocsRoot::default(),
            )
        })
        .expect("a refreshable section")
        + 1;
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(project_id.clone()),
        axum::Json(crate::models::PartialAuditRequest {
            agent,
            connection_id: connection.map(str::to_string),
            tier: None,
            steps: vec![step],
            accept_unverified_agents: None,
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
    // Vibe has no file tools: refused, with the one message the Full launch uses.
    let state = litellm_state("http://127.0.0.1:1").await;
    let refused = partial_audit_stream(
        &state,
        tempfile::tempdir().unwrap().path(),
        AgentType::Vibe,
        None,
    )
    .await;
    assert!(refused.contains("cannot run audits"), "{refused}");
    assert!(
        refused.contains("Ollama") && refused.contains("LiteLLM") && refused.contains("NVIDIA"),
        "{refused}"
    );

    // KT-980 — a Custom agent needs its named connection: refused without one,
    // by name, before anything runs.
    let custom = partial_audit_stream(
        &state,
        tempfile::tempdir().unwrap().path(),
        AgentType::Custom,
        None,
    )
    .await;
    assert!(custom.contains("requires a connection_id"), "{custom}");
    let unknown = partial_audit_stream(
        &state,
        tempfile::tempdir().unwrap().path(),
        AgentType::Custom,
        Some("missing"),
    )
    .await;
    assert!(unknown.contains("was not found"), "{unknown}");

    // LiteLLM and NVIDIA pass the gate and reach the HTTP launcher: with no tier
    // model configured they fail on the launcher's own remedy, never on the gate.
    let mut config = crate::core::config::default_config();
    config.agents.lite_llm.base_url = Some("http://127.0.0.1:1".into());
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let admitted = partial_audit_stream(
        &state,
        tempfile::tempdir().unwrap().path(),
        AgentType::LiteLlm,
        None,
    )
    .await;
    assert!(!admitted.contains("cannot run audits"), "{admitted}");
    assert!(
        admitted.contains("No LiteLLM model configured"),
        "{admitted}"
    );
    let nvidia = partial_audit_stream(
        &state,
        tempfile::tempdir().unwrap().path(),
        AgentType::Nvidia,
        None,
    )
    .await;
    assert!(!nvidia.contains("cannot run audits"), "{nvidia}");
    assert!(nvidia.contains("No NVIDIA model configured"), "{nvidia}");
}

#[tokio::test]
async fn a_named_connection_audit_calls_that_connection_with_its_tier_model() {
    // KT-980 — an OpenRouter-style connection is a `Custom` agent: the audit
    // must reach the connection's own endpoint with the model of the tier.
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("stop here"))
        .mount(&server)
        .await;
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let now = chrono::Utc::now();
    let connection = crate::models::ExternalApiConnection {
        id: "conn-or".into(),
        display_name: "OpenRouter".into(),
        mention_alias: "openrouter".into(),
        endpoint: Some(format!("{}/v1", server.uri())),
        credential_slug: "openrouter".into(),
        origin_preset: crate::models::ExternalApiConnectionPreset::OpenRouter,
        economy_model: Some("or/economy".into()),
        default_model: Some("or/default".into()),
        reasoning_model: Some("or/reasoning".into()),
        created_at: now,
        updated_at: now,
        image_model: None,
        video_model: None,
        media_endpoint: None,
    };
    state
        .db
        .with_conn(move |conn| crate::db::external_api_connections::insert(conn, &connection))
        .await
        .unwrap();
    let project = tempfile::tempdir().unwrap();

    let body =
        partial_audit_stream(&state, project.path(), AgentType::Custom, Some("conn-or")).await;

    assert!(!body.contains("cannot run audits"), "{body}");
    let requests = server.received_requests().await.expect("requests");
    assert!(
        !requests.is_empty(),
        "the connection's endpoint was never called: {body}"
    );
    let sent: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["model"], "or/reasoning", "{sent}");
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
        .filter(|(_, step)| {
            crate::api::audit::partial_selectable(
                step,
                &crate::core::docs_location::DocsRoot::default(),
            )
        })
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
                connection_id: None,
                tier: None,
                kind: None,
                custom_prompt: None,
                resume_run_id: None,
                retry_steps: None,
                redo_steps: None,
                accept_unverified_agents: None,
                fresh: None,
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
                connection_id: None,
                tier: None,
                steps: refreshable_steps(),
                accept_unverified_agents: None,
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

/// KT-1278 — over ACP the context and the step are one user message; a step
/// that ends on a reference block (versions, briefing, rules, linked repos) is
/// answered as a question. Read on what the agent was ACTUALLY handed by the two
/// real pipelines (retries included: the scripted agent writes nothing, so every
/// step runs its gates' attempts): each turn ends on one step's own directive,
/// and the step numbers it names are those of the chain.
#[cfg(unix)]
#[tokio::test]
async fn every_step_prompt_the_full_and_partial_pipelines_hand_the_agent_ends_on_its_directive() {
    let docs_root = crate::core::docs_location::DocsRoot::default_root();
    let steps = crate::api::audit::steps_for(crate::models::AuditKind::Full, &docs_root);
    let directives: Vec<String> = steps
        .iter()
        .enumerate()
        .map(|(i, step)| crate::api::audit::step_closing_directive(i + 1, step, &docs_root))
        .collect();
    for pipeline in ["full", "partial"] {
        for linked in [false, true] {
            let prompts = step_prompts_of(pipeline, linked).await;
            assert!(prompts.len() >= expected_step_count(pipeline), "{pipeline}");
            let mut covered = std::collections::BTreeSet::new();
            for turn in &prompts {
                // The recorder logs the prompt, then the CLI arguments on ONE
                // line: the prompt ends where that line starts, and nothing but
                // the arguments may follow the directive.
                let prompt = match turn.rfind("--print --output-format") {
                    Some(at) => {
                        assert!(!turn[at..].contains('\n'), "{pipeline}: one argument line");
                        &turn[..at]
                    }
                    None => turn.as_str(),
                };
                let step = directives
                    .iter()
                    .position(|d| prompt.ends_with(d.as_str()))
                    .unwrap_or_else(|| {
                        panic!(
                            "{pipeline} (linked={linked}): a step prompt does not end on its directive:\n…{}",
                            &prompt[prompt.len().saturating_sub(500)..]
                        )
                    });
                covered.insert(step);
                assert_eq!(
                    prompt.matches("=== RUN THIS STEP NOW ===").count(),
                    1,
                    "{pipeline}: exactly one directive"
                );
            }
            let wanted: std::collections::BTreeSet<usize> = if pipeline == "full" {
                (0..steps.len()).collect()
            } else {
                refreshable_steps().into_iter().map(|n| n - 1).collect()
            };
            assert_eq!(
                covered, wanted,
                "{pipeline}: every step was handed to the agent"
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
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some("run-prev".into()),
            retry_steps: None,
            redo_steps: None,
            accept_unverified_agents: None,
            fresh: None,
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
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some("run-prev".into()),
            retry_steps: None,
            redo_steps: None,
            accept_unverified_agents: None,
            fresh: None,
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

#[test]
fn a_step_cost_sums_its_attempts_and_one_silent_attempt_makes_it_unknown() {
    assert_eq!(StepCost::default().usd_micros(), None, "no attempt ran");
    let two = StepCost::default()
        .with_attempt(Some(120))
        .with_attempt(Some(300));
    assert_eq!(two.usd_micros(), Some(420), "attempts are summed");
    assert_eq!(
        StepCost::default().with_attempt(Some(0)).usd_micros(),
        Some(0),
        "a reported zero is a real zero"
    );
    assert_eq!(two.with_attempt(None).usd_micros(), None);
    assert_eq!(
        StepCost::default()
            .with_attempt(None)
            .with_attempt(Some(5))
            .usd_micros(),
        None,
        "a later report does not make the silent attempt free"
    );
    assert_eq!(
        StepCost::Known(u64::MAX).with_attempt(Some(1)).usd_micros(),
        Some(u64::MAX)
    );
}

/// KT-1313 — a Claude step stopped between turns holds every turn it consumed,
/// the final line replaces the sum, and no reading ever lowers the counter.
#[test]
fn a_stream_step_sums_its_turns_and_its_final_line_replaces_the_sum() {
    use crate::db::audit_runs::StepTokens;
    let agent = AgentType::ClaudeCode;
    let turn = |input: u64, output: u64| {
        StepTokens {
            input: Some(input),
            output: Some(output),
            cache_read: None,
            cache_write: None,
            accounting: Default::default(),
        }
        .inclusive_for(&agent)
    };
    let mut turns = StreamTurns::default();
    let mut usage = StepTokens::UNKNOWN;
    let mut seen = Vec::new();
    for reading in [turn(300, 30), turn(500, 47), turn(200, 16)] {
        usage = turns.fold(usage, reading, false);
        seen.push(usage.total().unwrap());
    }
    assert_eq!(seen, [330, 877, 1093], "a smaller turn still adds");
    assert_eq!((usage.input, usage.output), (Some(1000), Some(93)));

    // The final line states the whole run: it replaces the sum.
    let done = turns.fold(usage, turn(1000, 100), true);
    assert_eq!(done.total(), Some(1100));
    // …unless it counts less than what was already seen.
    assert_eq!(turns.fold(done, turn(10, 1), true), done);

    assert!(runner::is_claude_result_line(
        r#"{"type":"result","usage":{"input_tokens":1}}"#
    ));
    assert!(!runner::is_claude_result_line(
        r#"{"type":"stream_event","event":{"type":"message_delta"}}"#
    ));
    assert!(!runner::is_claude_result_line("not json"));
}

#[test]
fn the_runs_live_token_counter_never_goes_down() {
    let mut run = RunTokens::default();
    assert_eq!(run.total(), None);
    assert_eq!(run.with(630), 630);
    assert_eq!(run.with(877), 877);
    // A stopped step whose tokens were never added, then a smaller reading.
    assert_eq!(run.with(346), 877, "the shown total holds");
    run.add(Some(877));
    assert_eq!(run.total(), Some(877));
    assert_eq!(run.with(0), 877);
    assert_eq!(run.with(100), 977);
}

#[test]
fn the_run_model_is_the_observed_one_else_the_configured_one_labelled() {
    let observed = vec!["model-a".to_string(), "modèle-b".to_string()];
    assert_eq!(
        run_model_label(&observed, Some("cfg")).as_deref(),
        Some("model-a / modèle-b")
    );
    assert_eq!(
        run_model_label(&[], Some(" cfg-model ")).as_deref(),
        Some("cfg-model (configured)")
    );
    assert_eq!(run_model_label(&[], Some("  ")), None);
    assert_eq!(run_model_label(&[], None), None);
}

/// KT-950 DoD (fixture, fake clock) — ten logical minutes of tool turns without
/// one line of text: every turn moves the tool chip, the call count and the
/// tokens at the next tick; the ticks in between, a provider still thinking,
/// move nothing, so silence is never dressed up as progress.
#[tokio::test(start_paused = true)]
async fn minutes_of_tool_turns_without_text_move_the_counters_at_each_tick() {
    let http = runner::ToolActivityProbe::scripted();
    let mut watch = StepActivityWatch::new(
        AuditActivityProbe::new(http.clone(), None),
        AgentType::LiteLlm,
    );
    let mut tick = tokio::time::interval(ACTIVITY_TICK);
    let started = tokio::time::Instant::now();
    const TURNS: u64 = 200;
    for turn in 1..=TURNS {
        // The provider takes three ticks per tool turn.
        for _ in 0..2 {
            tick.tick().await;
            assert_eq!(watch.tool_moved(), None, "turn {turn}: nothing happened");
            assert_eq!(watch.tokens_moved(), None, "turn {turn}: nothing happened");
        }
        http.record_turn(90, 10, Some("read_file"));
        tick.tick().await;
        assert_eq!(watch.tool_moved(), Some(("Read".to_string(), turn as u32)));
        let tokens = watch.tokens_moved().expect("the turn's usage shows");
        assert_eq!(tokens.total(), Some(100 * turn));
    }
    assert!(started.elapsed() >= ACTIVITY_TICK * 3 * (TURNS as u32 - 1));
    assert!(
        started.elapsed() >= Duration::from_secs(590),
        "ten logical minutes"
    );
}

/// AGT-11 — one probe reads both channels: an HTTP agent's run, an ACP agent's
/// activity sink.
#[test]
fn the_activity_probe_reads_the_acp_sink_when_the_http_run_is_silent() {
    let (sink, rx) = tokio::sync::watch::channel(None);
    let http = runner::ToolActivityProbe::scripted();
    let probe = AuditActivityProbe::new(http.clone(), Some(rx));
    assert_eq!(probe.tool(), None);
    crate::agents::activity::tool_started(Some(&sink), crate::models::ActivityCategory::Execute);
    crate::agents::activity::tool_started(Some(&sink), crate::models::ActivityCategory::Read);
    assert_eq!(probe.tool(), Some(("Read".to_string(), 2)));
    http.record_turn(1, 1, Some("write_file"));
    assert_eq!(probe.tool(), Some(("Edit".to_string(), 1)));
}

fn stream_line(event: serde_json::Value) -> runner::StreamJsonEvent {
    runner::parse_claude_stream_line(
        &serde_json::json!({ "type": "stream_event", "event": event }).to_string(),
    )
}

/// A CLI's stream-json feeds the details panel and the chip by category only:
/// a tool's name, its input and the prose never reach them.
#[test]
fn a_cli_stream_feeds_the_recent_actions_by_category_only() {
    use crate::agents::activity::tests::SENTINEL;
    let mut feed = StepRecentFeed::new(None);
    assert_eq!(
        feed.moved(),
        None,
        "nothing to show before the first action"
    );
    let tool = |feed: &mut StepRecentFeed, name: &str, input: serde_json::Value| {
        feed.on_stream_event(&stream_line(serde_json::json!({
            "type": "content_block_start", "content_block": { "type": "tool_use", "name": name }
        })));
        feed.on_stream_event(&stream_line(serde_json::json!({
            "type": "content_block_delta", "delta": { "type": "input_json_delta", "partial_json": input.to_string() }
        })));
        feed.on_stream_event(&stream_line(
            serde_json::json!({ "type": "content_block_stop" }),
        ));
    };
    feed.on_stream_event(&stream_line(serde_json::json!({
        "type": "content_block_delta", "delta": { "type": "text_delta", "text": SENTINEL }
    })));
    tool(
        &mut feed,
        "Read",
        serde_json::json!({ "file_path": SENTINEL }),
    );
    tool(
        &mut feed,
        "Grep",
        serde_json::json!({ "pattern": SENTINEL, "path": SENTINEL }),
    );
    tool(
        &mut feed,
        "Bash",
        serde_json::json!({ "command": format!("mysql -p{SENTINEL}") }),
    );
    tool(&mut feed, SENTINEL, serde_json::json!({}));
    tool(
        &mut feed,
        &format!("mcp__{SENTINEL}__x"),
        serde_json::json!({}),
    );

    // The drift pipeline's chip reads the category each start returns.
    assert_eq!(
        feed.on_stream_event(&stream_line(serde_json::json!({
            "type": "content_block_start", "content_block": { "type": "tool_use", "name": "Bash" }
        }))),
        Some(crate::models::ActivityCategory::Execute)
    );
    assert_eq!(
        feed.on_stream_event(&stream_line(
            serde_json::json!({ "type": "content_block_stop" })
        )),
        None
    );
    let shown = feed.moved().expect("the actions show");
    let categories: Vec<_> = shown.entries.iter().map(|e| e.category).collect();
    use crate::models::ActivityCategory::*;
    assert_eq!(categories, [Execute, Mcp, Other, Execute, Search, Read]);
    let all = serde_json::to_string(&shown).unwrap();
    assert!(
        !all.contains("hunter2") && !all.contains("SENTINEL"),
        "{all}"
    );
    assert_eq!(feed.moved(), None, "unchanged, nothing new to send");
}

/// An HTTP agent's run feeds the same panel and its chip label through its
/// probe, recorded the way its tool loop records each call.
#[test]
fn an_http_run_feeds_the_recent_actions_and_its_label_by_category_only() {
    use crate::agents::activity::tests::SENTINEL;
    let http = runner::ToolActivityProbe::scripted();
    let mut feed = StepRecentFeed::new(Some(http.clone()));
    assert_eq!(feed.moved(), None);
    for turn in 0..20 {
        http.record_http_call(&ToolCall {
            id: format!("c{turn}"),
            name: "read_file".into(),
            arguments: json!({ "path": SENTINEL }),
        });
    }
    http.record_http_call(&ToolCall {
        id: "x".into(),
        name: SENTINEL.into(),
        arguments: json!({ "command": SENTINEL }),
    });
    let shown = feed.moved().expect("the run's calls show");
    assert_eq!(
        shown.entries.len(),
        crate::agents::activity::RECENT_MAX_ENTRIES
    );
    assert_eq!(
        shown.entries[0].category,
        crate::models::ActivityCategory::Other
    );
    assert_eq!(
        shown.entries[1].category,
        crate::models::ActivityCategory::Read
    );
    assert_eq!(
        http.read(),
        Some((crate::models::ActivityCategory::Other, 21))
    );
    let all = format!(
        "{} {:?}",
        serde_json::to_string(&shown).unwrap(),
        http.read()
    );
    assert!(
        !all.contains("hunter2") && !all.contains("SENTINEL"),
        "{all}"
    );
}

/// KT-1000 — the version-source cascade reads only what the launch establishes.
#[test]
fn version_reach_follows_what_each_launch_mounts() {
    let claude = AgentType::ClaudeCode;
    assert_eq!(
        version_reach(true, false, true, &AgentType::LiteLlm),
        VersionReach {
            mcp: None,
            web: false
        }
    );
    assert_eq!(
        version_reach(false, true, true, &claude),
        VersionReach {
            mcp: Some(true),
            web: false
        }
    );
    assert_eq!(
        version_reach(false, false, true, &claude),
        VersionReach {
            mcp: Some(false),
            web: true
        }
    );
    assert_eq!(
        version_reach(false, false, false, &claude),
        VersionReach {
            mcp: Some(false),
            web: false
        }
    );
    assert_eq!(
        version_reach(false, false, true, &AgentType::Codex),
        VersionReach {
            mcp: None,
            web: false
        }
    );
}

#[tokio::test]
async fn version_source_inputs_read_the_mcp_config_the_agent_is_handed() {
    use crate::core::version_sources::{select_version_source, VersionSource};
    let state = litellm_state("http://127.0.0.1:1").await;
    let project = tempfile::tempdir().unwrap();
    let path = project.path();
    let path_str = path.to_string_lossy().to_string();
    let stdio =
        r#"{"mcpServers":{"context7":{"command":"npx","args":["-y","@upstash/context7-mcp"]}}}"#;
    let remote =
        r#"{"mcpServers":{"context7":{"type":"http","url":"https://mcp.context7.com/mcp"}}}"#;

    let acp = AuditAgentLauncher::with_route(
        &state,
        &AgentType::ClaudeCode,
        crate::acp::AcpProductionRoute::AdaptedAcp,
    )
    .await;
    let direct = AuditAgentLauncher::with_route(
        &state,
        &AgentType::ClaudeCode,
        crate::acp::AcpProductionRoute::DirectCliMigration,
    )
    .await;
    let http = AuditAgentLauncher::new(&state, &AgentType::LiteLlm).await;
    let pick = |l: &AuditAgentLauncher, watch| {
        select_version_source(l.version_source_inputs(
            &AgentType::ClaudeCode,
            path,
            &path_str,
            watch,
        ))
    };

    std::fs::write(path.join(".mcp.json"), stdio).unwrap();
    assert_eq!(pick(&acp, true), VersionSource::Context7);
    assert_eq!(pick(&direct, true), VersionSource::Context7);
    assert_eq!(
        select_version_source(http.version_source_inputs(
            &AgentType::LiteLlm,
            path,
            &path_str,
            true
        )),
        VersionSource::DependencyWatch
    );

    std::fs::write(path.join(".mcp.json"), remote).unwrap();
    assert_eq!(
        pick(&acp, true),
        VersionSource::DependencyWatch,
        "an ACP session drops url-only servers"
    );
    assert_eq!(pick(&direct, false), VersionSource::Context7);

    std::fs::remove_file(path.join(".mcp.json")).unwrap();
    assert_eq!(pick(&direct, false), VersionSource::Web);
    assert_eq!(pick(&acp, false), VersionSource::None);
}

/// KT-1320 — the connection a resumed run had may have been deleted since: the
/// resume runs on the agent's default connection and says so, it is not refused.
#[cfg(unix)]
#[tokio::test]
async fn a_resume_whose_connection_was_deleted_falls_back_with_a_visible_note() {
    resume_after_the_run_connection_was_deleted(None).await;
}

/// KT-1320 — the UI echoes the run's connection id back on a resume with an
/// untouched picker: the server still recognises it as the run's own choice.
#[cfg(unix)]
#[tokio::test]
async fn a_resume_echoing_the_deleted_connection_of_its_run_falls_back_too() {
    resume_after_the_run_connection_was_deleted(Some("deleted-conn")).await;
}

/// KT-1320 — the same fallback on an HTTP provider: the retried step reaches the
/// agent's default endpoint on the run's economy model, not the reasoning one.
#[cfg(unix)]
#[tokio::test]
async fn a_resume_whose_connection_was_deleted_reaches_the_default_provider_on_the_run_tier() {
    let prose = text("The documentation is unchanged.");
    let (server, requests) = provider(prose.clone(), prose).await;
    let state = litellm_state(&server.uri()).await;
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;
    seed_run_on_a_deleted_connection(&state, project.path(), "LiteLlm").await;

    let stream = resume_run_gone(&state, AgentType::LiteLlm, Some("deleted-conn")).await;

    assert!(!stream.contains("event: error"), "{stream}");
    assert!(
        sse_events(&stream, "warning").iter().any(|w| w["message"]
            .as_str()
            .is_some_and(|m| m.contains("deleted-conn") && m.contains("no longer exists"))),
        "{stream}"
    );
    let seen = requests.lock().unwrap().clone();
    assert!(
        !seen.is_empty(),
        "the retried step reached the default endpoint: {stream}"
    );
    assert!(
        seen.iter().all(|body| body["model"] == "economy-model"),
        "the run's economy tier picks the model: {seen:?}"
    );
    let run = latest_run(&state).await;
    assert_eq!(run.tier.as_deref(), Some("economy"));
    assert_eq!(run.connection_id, None);
}

#[cfg(unix)]
async fn resume_after_the_run_connection_was_deleted(requested_connection: Option<&str>) {
    let tools = tempfile::tempdir().unwrap();
    let (fixture, _log) = recording_claude(tools.path());
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;
    let _route = route_claude(project.path(), &fixture);
    seed_run_on_a_deleted_connection(&state, project.path(), "ClaudeCode").await;

    let stream = resume_run_gone(&state, AgentType::ClaudeCode, requested_connection).await;

    assert!(!stream.contains("event: error"), "{stream}");
    let warnings = sse_events(&stream, "warning");
    assert!(
        warnings.iter().any(|w| w["message"]
            .as_str()
            .is_some_and(|m| m.contains("deleted-conn") && m.contains("no longer exists"))),
        "{stream}"
    );
    let run = latest_run(&state).await;
    assert_ne!(run.id, "run-gone");
    assert_eq!(
        run.tier.as_deref(),
        Some("economy"),
        "the tier is still the run's"
    );
    assert_eq!(run.connection_id, None);
}

/// An interrupted economy run on `agent` and on a connection that no longer
/// exists, whose step 5 is the one left to retry.
#[cfg(unix)]
async fn seed_run_on_a_deleted_connection(state: &AppState, project: &Path, agent: &'static str) {
    std::fs::create_dir_all(project.join("docs/conventions")).unwrap();
    std::fs::write(project.join("docs/AGENTS.md"), "# Audited project\n").unwrap();
    std::fs::write(
        project.join("docs/conventions/agents-md-format-v1.md"),
        "# AGENTS.md format\n",
    )
    .unwrap();
    let total =
        crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len() as u32;
    state
        .db
        .with_conn(move |conn| {
            use crate::db::audit_runs as runs;
            let started = chrono::Utc::now() - chrono::Duration::hours(1);
            runs::insert_running(conn, "run-gone", PROJECT_ID, "Full", agent, started)?;
            runs::set_run_launch(conn, "run-gone", ModelTier::Economy, Some("deleted-conn"))?;
            for step in 1..=total {
                let label = format!("docs/step-{step}.md");
                runs::insert_audit_step_start(conn, "run-gone", step, &label, started)?;
                runs::finalize_audit_step(
                    conn,
                    "run-gone",
                    step,
                    started,
                    10,
                    &runs::StepTokens::UNKNOWN,
                    None,
                    step != 5,
                    (step == 5).then_some("Mac went to sleep"),
                    false,
                )?;
                if step != 5 {
                    runs::update_last_completed_step(conn, "run-gone", step.min(4))?;
                }
            }
            runs::mark_interrupted(conn, "run-gone", "warned steps: [5]")
        })
        .await
        .unwrap();
}

/// Resumes `run-gone` the way the UI does, naming no tier, and returns the stream.
#[cfg(unix)]
async fn resume_run_gone(
    state: &AppState,
    agent: AgentType,
    requested_connection: Option<&str>,
) -> String {
    use axum::response::IntoResponse;
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent,
            connection_id: requested_connection.map(str::to_string),
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some("run-gone".into()),
            retry_steps: None,
            redo_steps: None,
            fresh: None,
            accept_unverified_agents: None,
        }),
    )
    .await
    .into_response();
    sse_body(response).await
}

#[cfg(unix)]
async fn latest_run(state: &AppState) -> crate::models::AuditRun {
    state
        .db
        .with_conn(|conn| {
            Ok(crate::db::audit_runs::list_recent(conn, PROJECT_ID, 1)?
                .into_iter()
                .next())
        })
        .await
        .unwrap()
        .expect("the resumed run")
}

/// KT-1318 — a full audit writes a lot of files and builds: below the
/// configured free space it is refused before anything is recorded or written,
/// with the number and the setting named.
#[cfg(unix)]
#[tokio::test]
async fn a_full_audit_under_disk_pressure_is_refused_before_it_creates_anything() {
    use axum::response::IntoResponse;
    // A petabyte: more than any test host has free, whatever its disk.
    let _policy = crate::core::disk_usage::TestDiskPolicy::set(1_000_000, 6);
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;

    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: None,
            retry_steps: None,
            redo_steps: None,
            accept_unverified_agents: None,
            fresh: None,
        }),
    )
    .await
    .into_response();
    let stream = sse_body(response).await;

    let refusal = sse_events(&stream, "error").pop().expect("a refusal");
    let reason = refusal["error"].as_str().unwrap_or_default();
    assert!(
        reason.contains("refusing to start a full audit"),
        "{reason}"
    );
    assert!(
        reason.contains("server.disk_launch_min_free_gib") && reason.contains("1000000 GiB"),
        "{reason}"
    );
    assert!(
        sse_events(&stream, "step_start").is_empty(),
        "no step started: {stream}"
    );
    let runs: i64 = state
        .db
        .with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM audit_runs", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(runs, 0, "no audit run was recorded");
    assert!(
        !project.path().join("docs").exists(),
        "nothing was written into the project"
    );
}
