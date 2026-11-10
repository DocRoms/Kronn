//! KT-1133 — the release gate of the audit-run states, end to end on the HTTP
//! agent. The real Full pipeline and the real templates run against a mocked
//! OpenAI-wire provider (a loopback listener) that plays the model: the n-th
//! step turn works on the n-th line of `targets`, fills the placeholders of that
//! document through the real bounded `write_file` tool, or answers in prose
//! when the repository is already up to date. A step turn whose number is in
//! `fail` is refused by the provider. Scenarios: first audit, second audit with an
//! agent that changes nothing, a failed step retried alone, a failed step
//! accepted by a human, and a restart in the middle of a run.
use super::*;
use regex_lite::Regex;

struct Script {
    mode: String,
    targets: Vec<String>,
    counter: usize,
    fail: Vec<usize>,
    settle: bool,
    /// `(step turn, target, mode)` of every step turn since the last `arm`.
    turns: Vec<(usize, String, String)>,
}

type Shared = Arc<Mutex<Script>>;

/// What the model makes of a document it was told to fill: every placeholder
/// settled, or a line added when there is nothing left to fill.
fn filled(content: &str) -> String {
    let outcome = Regex::new(r"\{\{[A-Z0-9_]*_OUTCOME\}\}").unwrap();
    let any = Regex::new(r"\{\{[A-Z0-9_]*\}\}").unwrap();
    let next = outcome.replace_all(content, "scanned — nothing substantiable");
    let next = next.replace("{{DEP_EVIDENCE}}", "not verified: no source");
    let next = any
        .replace_all(&next, "verified in the fixture repository")
        .into_owned();
    if next == content {
        format!("{next}\nChecked by the audit: scanned — nothing substantiable.\n")
    } else {
        next
    }
}

/// The documents the validation settles by hand: leftover unknowns and TODOs.
fn settle_docs(project: &Path) {
    let any = Regex::new(r"\{\{[A-Z0-9_]*\}\}").unwrap();
    let todo = Regex::new(r"<!-- TODO:[^>]*-->").unwrap();
    let mut pending = vec![project.join("docs")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "tech-debt") {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let content = std::fs::read_to_string(&path).unwrap();
                let next = any.replace_all(&content, "settled by the validation");
                let next = todo.replace_all(&next, "");
                std::fs::write(&path, next.as_bytes()).unwrap();
            }
        }
    }
}

/// The step the coding-rules document leaves to its sibling: the agent settles
/// the commit conventions it names, beside its own target.
fn settle_commit_conventions(project: &Path, target: &str) {
    if Path::new(target)
        .file_name()
        .is_none_or(|n| n != "coding-rules.md")
    {
        return;
    }
    let commits = project
        .join(Path::new(target).parent().unwrap())
        .join("workflow/commits.md");
    let Ok(content) = std::fs::read_to_string(&commits) else {
        return;
    };
    let fill = Regex::new(r"<!-- Fill:[^>]*-->").unwrap();
    let any = Regex::new(r"\{\{[A-Z0-9_]*\}\}").unwrap();
    let kept: String = content
        .lines()
        .filter(|line| !line.contains("TEMPLATE FILE"))
        .map(|line| format!("{line}\n"))
        .collect();
    let next = fill.replace_all(&kept, "");
    let next = any.replace_all(&next, "Not formalized - ask before assuming.");
    std::fs::write(commits, next.as_bytes()).unwrap();
}

fn respond(script: &Shared, project: &Path, request: &wiremock::Request) -> ResponseTemplate {
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let messages = body["messages"].as_array().unwrap();
    let said = |needle: &str| {
        messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains(needle))
        })
    };
    let after_tool = messages.iter().any(|message| message["role"] == "tool");
    let usage =
        json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}).to_string();
    let reply =
        |frame: String| ResponseTemplate::new(200).set_body_string(sse(&[frame, usage.clone()]));
    let mut script = script.lock().unwrap();

    // The detached validation discussion: it settles the documents' unknowns
    // when asked, then ends on the terminal signal.
    if !said(crate::api::audit::PROMPT_PREAMBLE) {
        if !after_tool && script.settle {
            settle_docs(project);
        }
        return reply(text("KRONN:VALIDATION_COMPLETE"));
    }
    if after_tool {
        return reply(text("The document is written."));
    }
    script.counter += 1;
    let n = script.counter;
    let target = script.targets.get(n - 1).cloned().unwrap_or_default();
    let mode = script.mode.clone();
    script.turns.push((n, target.clone(), mode.clone()));
    if script.fail.contains(&n) {
        // A refused key is permanent: a 5xx is retried by the runner and would not fail the step.
        return ResponseTemplate::new(401).set_body_string("invalid api key");
    }
    if mode == "noop" || target.is_empty() || target == "REVIEW" {
        return reply(text("The documents are up to date; nothing to change."));
    }
    let path = project.join(&target);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    settle_commit_conventions(project, &target);
    reply(tool_calls(&[(
        "write_file",
        json!({"path": target, "content": filled(&current)}),
    )]))
}

struct Recette {
    server: MockServer,
    script: Shared,
    state: AppState,
    project: tempfile::TempDir,
    targets: Vec<&'static str>,
    _restore: TemplatesEnv,
}

fn http_state(db: Arc<crate::db::Database>, endpoint: &str) -> AppState {
    let mut config = crate::core::config::default_config();
    config.agents.lite_llm.base_url = Some(endpoint.to_string());
    config.agents.model_tiers.lite_llm.reasoning = Some("audit-model".into());
    config.agents.model_tiers.lite_llm.economy = Some("economy-model".into());
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

impl Recette {
    /// A blank git repository registered as the project, the real templates, and
    /// the mocked provider.
    async fn blank() -> Self {
        let restore = TemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
        crate::core::child_env::set_var(
            "KRONN_TEMPLATES_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates"),
        );
        let project = tempfile::tempdir().unwrap();
        let initialized = crate::core::cmd::git_cmd()
            .args(["init", "--quiet"])
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(initialized.status.success());
        std::fs::write(project.path().join("README.md"), "# Fixture\n").unwrap();
        let targets: Vec<&'static str> =
            crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
                .iter()
                .map(|step| step.target_file)
                .collect();
        let script = Arc::new(Mutex::new(Script {
            mode: "fill".into(),
            targets: Vec::new(),
            counter: 0,
            fail: Vec::new(),
            settle: false,
            turns: Vec::new(),
        }));
        // Not from wiremock's pool: a pooled server keeps its address across tests.
        let server = MockServer::builder().start().await;
        let (shared, root) = (script.clone(), project.path().to_path_buf());
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| respond(&shared, &root, request))
            .mount(&server)
            .await;
        let state = http_state(
            Arc::new(crate::db::Database::open_in_memory().unwrap()),
            &server.uri(),
        );
        project_among_others(&state, project.path(), false).await;
        Self {
            server,
            script,
            state,
            project,
            targets,
            _restore: restore,
        }
    }

    /// The first audit of the blank repository: every step writes its document.
    async fn audited() -> Self {
        let fx = Self::blank().await;
        fx.arm("fill", &fx.targets.clone());
        let first = fx.full(None, None).await;
        let done = sse_events(&first, "done").pop().expect("terminal done");
        validation_settled(&fx.state, &done).await;
        assert_eq!(done["status"], "complete", "{first}");
        fx
    }

    /// Point the model at `targets` in `mode`, from its next step turn.
    fn arm(&self, mode: &str, targets: &[&str]) {
        let mut script = self.script.lock().unwrap();
        script.mode = mode.into();
        script.targets = targets.iter().map(|t| t.to_string()).collect();
        script.counter = 0;
        script.fail.clear();
        script.turns.clear();
    }

    /// Make the listed step turns (their order in this run) be refused by the provider.
    fn fail_turns(&self, turns: &[usize]) {
        self.script.lock().unwrap().fail = turns.to_vec();
    }

    /// Let the validation turn settle what a human validation would.
    fn settle_on_validation(&self) {
        self.script.lock().unwrap().settle = true;
    }

    fn turns(&self) -> Vec<(usize, String, String)> {
        self.script.lock().unwrap().turns.clone()
    }

    fn step_of(&self, target: &str) -> u32 {
        self.targets.iter().position(|t| *t == target).unwrap() as u32 + 1
    }

    fn total(&self) -> u32 {
        self.targets.len() as u32
    }

    async fn full(&self, resume_run_id: Option<String>, retry_steps: Option<Vec<u32>>) -> String {
        launch_http_full(&self.state, resume_run_id, retry_steps).await
    }

    async fn accept(
        &self,
        run_id: &str,
        step: u32,
        reason: &str,
    ) -> crate::models::ApiResponse<crate::api::audit::AcceptAuditStepResponse> {
        crate::api::audit::accept_audit_step(
            axum::extract::State(self.state.clone()),
            axum::extract::Path((PROJECT_ID.to_string(), run_id.to_string(), step)),
            None,
            None,
            axum::Json(crate::api::audit::AcceptAuditStepRequest {
                reason: Some(reason.to_string()),
                override_unsound: None,
            }),
        )
        .await
        .0
    }

    async fn validate(&self) -> crate::models::ApiResponse<crate::models::AiAuditStatus> {
        crate::api::audit::validate_audit(
            axum::extract::State(self.state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
        )
        .await
        .0
    }

    fn validated(&self) -> bool {
        crate::core::scanner::detect_audit_status(&self.project.path().display().to_string())
            == crate::models::AiAuditStatus::Validated
    }
}

async fn launch_http_full(
    state: &AppState,
    resume_run_id: Option<String>,
    retry_steps: Option<Vec<u32>>,
) -> String {
    use axum::response::IntoResponse;
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::LiteLlm,
            connection_id: None,
            tier: Some(ModelTier::Reasoning),
            kind: None,
            custom_prompt: None,
            resume_run_id,
            retry_steps,
            redo_steps: None,
            accept_unverified_agents: None,
            fresh: None,
        }),
    )
    .await
    .into_response();
    sse_body(response).await
}

fn done_of(stream: &str) -> Value {
    sse_events(stream, "done").pop().expect("terminal done")
}

fn started(stream: &str) -> Vec<u64> {
    sse_events(stream, "step_start")
        .iter()
        .map(|event| event["step"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn scenario_1_a_first_audit_of_a_blank_repo_completes_without_a_failed_step() {
    let fx = Recette::blank().await;
    fx.arm("fill", &fx.targets.clone());
    let stream = fx.full(None, None).await;
    let done = done_of(&stream);
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "complete", "{stream}");
    assert_eq!(done["steps_to_redo"], json!([]), "{stream}");
    assert!(!stream.contains("event: error"), "{stream}");
    assert!(sse_events(&stream, "step_warning").is_empty(), "{stream}");
    assert!(sse_events(&stream, "step_unchanged").is_empty(), "{stream}");

    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(&fx.state, &run_id).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(run.last_completed_step, fx.total());
    let recorded = outcomes(&run);
    assert_eq!(recorded["failed"], json!([]));
    assert_eq!(recorded["unchanged"], json!([]));
    assert_eq!(steps.len() as u32, fx.total());
    for step in &steps {
        assert!(step.cli_success, "{step:?}");
        assert!(step.step_warning.is_none(), "{step:?}");
        assert!(!step.unchanged, "{step:?}");
    }
    assert!(prompt.is_some_and(|p| !p.is_empty()));

    // Every documentary step went through the model: one turn each.
    assert_eq!(fx.turns().len() as u32, fx.total());
    let baseline = crate::core::checksums::read_checksums_file_strict(fx.project.path())
        .unwrap()
        .expect("the run wrote its drift baseline");
    assert!(baseline
        .mappings
        .iter()
        .any(|m| m.ai_file == "docs/AGENTS.md"));
    let agents = std::fs::read_to_string(fx.project.path().join("docs/AGENTS.md")).unwrap();
    assert!(
        !Regex::new(r"\{\{[A-Z0-9_]+\}\}").unwrap().is_match(&agents),
        "{agents}"
    );
    drop(fx.server);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn scenario_2_a_second_audit_that_changes_nothing_completes_with_unchanged_steps() {
    let fx = Recette::audited().await;
    let review = fx
        .targets
        .iter()
        .position(|t| *t == "REVIEW")
        .map(|i| i as u32 + 1);
    let documents: Vec<u32> = (1..=fx.total()).filter(|s| Some(*s) != review).collect();

    fx.arm("noop", &fx.targets.clone());
    let stream = fx.full(None, None).await;
    let done = done_of(&stream);
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "complete", "{stream}");
    assert_eq!(done["steps_to_redo"], json!([]), "{stream}");
    assert!(sse_events(&stream, "step_warning").is_empty(), "{stream}");
    let unchanged: Vec<u64> = sse_events(&stream, "step_unchanged")
        .iter()
        .map(|event| event["step"].as_u64().unwrap())
        .collect();
    assert_eq!(
        unchanged,
        documents.iter().map(|s| *s as u64).collect::<Vec<_>>(),
        "{stream}"
    );
    for step_done in sse_events(&stream, "step_done") {
        let expected = if Some(step_done["step"].as_u64().unwrap() as u32) == review {
            "succeeded"
        } else {
            "unchanged"
        };
        assert_eq!(step_done["success"], true, "{step_done}");
        assert_eq!(step_done["outcome"], expected, "{step_done}");
    }

    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(&fx.state, &run_id).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(run.last_completed_step, fx.total());
    let recorded = outcomes(&run);
    assert_eq!(recorded["unchanged"], json!(documents));
    assert_eq!(recorded["failed"], json!([]));
    for step in &steps {
        assert!(step.cli_success, "{step:?}");
        assert_eq!(step.unchanged, Some(step.step_index) != review, "{step:?}");
        assert!(step.step_warning.is_none(), "{step:?}");
    }
    // The validation discussion lists the unchanged steps apart.
    let prompt = prompt.expect("a Completed run has its validation discussion");
    let section = prompt
        .split("## ÉTAPES INCHANGÉES")
        .nth(1)
        .unwrap_or_else(|| panic!("no unchanged section: {prompt}"));
    let section = section.split("\n## ").next().unwrap();
    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    for step in &documents {
        let file = crate::api::audit::full::step_file_label(&chain[*step as usize - 1]);
        assert!(
            section.contains(&format!("{step}/{} — `{file}`", fx.total())),
            "{section}"
        );
    }
}

/// A second audit in which one step is refused by the provider: it ends
/// Interrupted on that step alone, every other step `unchanged`.
async fn audit_with_a_failed_step(fx: &Recette, target: &str) -> (u32, String) {
    let step = fx.step_of(target);
    fx.arm("noop", &fx.targets.clone());
    fx.fail_turns(&[step as usize]);
    let stream = fx.full(None, None).await;
    let done = done_of(&stream);
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{stream}");
    assert_eq!(done["steps_to_redo"], json!([step]), "{stream}");
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, _) = persisted(&fx.state, &run_id).await;
    assert_eq!(run.status, "Interrupted");
    assert_eq!(outcomes(&run)["failed"], json!([step]));
    let failed = steps.iter().find(|s| s.step_index == step).unwrap();
    assert!(!failed.cli_success, "{failed:?}");
    assert!(failed.step_warning.is_some(), "{failed:?}");
    (step, run_id)
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn scenario_3_a_failed_step_retried_alone_completes_the_run_and_validates() {
    let fx = Recette::audited().await;
    let (rules, interrupted) = audit_with_a_failed_step(&fx, "docs/coding-rules.md").await;

    // The retry runs that step only: the model sees one step turn.
    fx.arm("fill", &["docs/coding-rules.md"]);
    fx.settle_on_validation();
    let stream = fx.full(Some(interrupted), Some(vec![rules])).await;
    let done = done_of(&stream);
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "complete", "{stream}");
    assert_eq!(started(&stream), vec![rules as u64], "{stream}");
    assert_eq!(fx.turns().len(), 1, "{:?}", fx.turns());
    assert_eq!(
        sse_events(&stream, "step_skipped").len() as u32,
        fx.total() - 1,
        "{stream}"
    );
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, _, prompt) = persisted(&fx.state, &run_id).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(run.last_completed_step, fx.total());
    assert_eq!(outcomes(&run)["failed"], json!([]));
    assert!(prompt.is_some());

    // Validated, with no global audit after the failure.
    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert!(fx.validated());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn scenario_4_a_failed_step_accepted_by_a_human_completes_the_run_and_validates() {
    let fx = Recette::audited().await;
    let (rules, interrupted) = audit_with_a_failed_step(&fx, "docs/coding-rules.md").await;

    let accepted = fx
        .accept(&interrupted, rules, "  kept as written\nby hand ")
        .await;
    assert!(accepted.success, "{:?}", accepted.error);
    let accepted = accepted.data.unwrap();
    assert_eq!(accepted.step, rules);
    assert_eq!(accepted.remaining_failed, Vec::<u32>::new());
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    let row = steps.iter().find(|s| s.step_index == rules).unwrap();
    assert_eq!(row.accepted_by.as_deref(), Some("operator"));
    assert_eq!(
        row.accepted_reason.as_deref(),
        Some("kept as written by hand")
    );
    assert!(!row.cli_success, "the failure stays on record");

    // The resume only finalizes: no step turn reaches the model.
    fx.arm("noop", &[]);
    fx.settle_on_validation();
    let stream = fx.full(Some(interrupted), None).await;
    let done = done_of(&stream);
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "complete", "{stream}");
    assert!(started(&stream).is_empty(), "{stream}");
    assert!(fx.turns().is_empty(), "{:?}", fx.turns());
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, _, prompt) = persisted(&fx.state, &run_id).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(outcomes(&run)["accepted"], json!([rules]));
    assert_eq!(outcomes(&run)["failed"], json!([]));
    let prompt = prompt.expect("a Completed run has its validation discussion");
    let section = prompt
        .split("## ÉTAPES ACCEPTÉES")
        .nth(1)
        .unwrap_or_else(|| panic!("no accepted section: {prompt}"));
    let section = section.split("\n## ").next().unwrap();
    assert!(section.contains(&format!("{rules}/{} — `docs/coding-rules.md`", fx.total())));
    assert!(section.contains("kept as written by hand"), "{section}");

    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert!(fx.validated());
}

/// Scenario 5, backend half: what the UI reads after Kronn died in the middle
/// of a run — the timeline and the resumable run — is exact, and the action it
/// offers (the resume) ends Completed on the steps that were left.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn scenario_5_a_restart_in_the_middle_of_a_run_shows_an_exact_state_and_resumes() {
    let fx = Recette::blank().await;
    let data_dir = tempfile::tempdir().unwrap();
    let db_path = data_dir.path().join("kronn.db");
    let boot = |path: &std::path::PathBuf| {
        Arc::new(crate::db::Database::open_path_for_backend_boot(path).unwrap())
    };

    // First life of Kronn: a Full audit runs; step 4 is the one running when
    // the process dies.
    let first = http_state(boot(&db_path), &fx.server.uri());
    project_among_others(&first, fx.project.path(), false).await;
    fx.arm("fill", &fx.targets.clone());
    let stream = launch_http_full(&first, None, None).await;
    let done = done_of(&stream);
    validation_settled(&first, &done).await;
    assert_eq!(done["status"], "complete", "{stream}");
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let killed_at = 4_u32;
    {
        let run_id = run_id.clone();
        first
            .db
            .with_conn(move |conn| {
                // What a SIGKILL leaves behind: the row still Running, the
                // in-flight step started and never ended, the later ones never
                // recorded, no validation discussion yet.
                conn.execute(
                    "UPDATE audit_runs SET status = 'Running', ended_at = NULL, duration_ms = NULL,
                            report_path = NULL, last_completed_step = ?2,
                            validation_discussion_id = NULL, step_outcomes_json = NULL
                     WHERE id = ?1",
                    rusqlite::params![run_id, killed_at - 1],
                )?;
                conn.execute(
                    "DELETE FROM audit_run_steps WHERE audit_run_id = ?1 AND step_index >= ?2",
                    rusqlite::params![run_id, killed_at],
                )?;
                crate::db::audit_runs::insert_audit_step_start(
                    conn,
                    &run_id,
                    killed_at,
                    "docs/in-flight.md",
                    chrono::Utc::now(),
                )
            })
            .await
            .unwrap();
    }
    drop(first);

    // Second life: opening the database the way the backend boots settles it.
    let second = http_state(boot(&db_path), &fx.server.uri());
    let timeline = crate::api::audit::audit_timeline(
        axum::extract::State(second.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0
    .data
    .expect("the timeline answers");
    let latest = &timeline.runs[0];
    assert_eq!(latest.id, run_id);
    assert_eq!(
        latest.status, "Interrupted",
        "nothing reads Running: {latest:?}"
    );
    assert_eq!(latest.last_completed_step, killed_at - 1);
    assert!(!timeline.lease_orphaned, "no lease is left to release");
    let recorded: Vec<(u32, bool)> = timeline
        .steps
        .iter()
        .map(|step| (step.step_index, step.ended_at.is_some()))
        .collect();
    let mut expected: Vec<(u32, bool)> = (1..killed_at).map(|s| (s, true)).collect();
    expected.push((killed_at, false));
    let mut recorded_sorted = recorded.clone();
    recorded_sorted.sort_unstable();
    assert_eq!(
        recorded_sorted, expected,
        "finished steps kept, the killed one open"
    );

    let resumable = crate::api::audit::audit_latest_resumable(
        axum::extract::State(second.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0
    .data
    .expect("the resumable run answers");
    let resumable = serde_json::to_value(resumable).expect("a run to resume after the restart");
    assert_eq!(resumable["id"], run_id.as_str(), "{resumable}");
    assert_eq!(
        resumable["last_completed_step"],
        killed_at - 1,
        "{resumable}"
    );
    assert_eq!(
        resumable["steps_to_redo"],
        json!((killed_at..=fx.total()).collect::<Vec<_>>()),
        "{resumable}"
    );
    assert_eq!(resumable["from_repository"], false, "{resumable}");

    // The action it offers: resume. Only the steps that were left reach the model.
    fx.arm("fill", &fx.targets[(killed_at as usize - 1)..]);
    fx.settle_on_validation();
    let resumed = launch_http_full(&second, Some(run_id.clone()), None).await;
    let done = done_of(&resumed);
    validation_settled(&second, &done).await;
    assert_eq!(done["status"], "complete", "{resumed}");
    assert_eq!(
        started(&resumed),
        (killed_at as u64..=fx.total() as u64).collect::<Vec<_>>(),
        "{resumed}"
    );
    let (run, _, prompt) = persisted(&second, done["audit_run_id"].as_str().unwrap()).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(run.last_completed_step, fx.total());
    assert!(prompt.is_some(), "the validation discussion was created");
}
