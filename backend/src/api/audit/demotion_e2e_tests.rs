//! KT-1370 — after its last attempt a step that still holds citation markers
//! neither the agent nor the exact fixes could repair does not interrupt the
//! run: the markers become flagged backticked paths and the step is done with a
//! warning that lists them. Real Full and revalidation pipelines through the
//! scripted `claude` of the parent modules, on the docroms string (step 8 of
//! `fresh_full`: a text file cited without a line, three attempts running).
use super::*;

const AGENTS: &str = "docs/AGENTS.md";
const MONOLOG: &str = "config/packages/prod/monolog.yaml";
const FLAGGED: &str = "`config/packages/prod/monolog.yaml` (unverified reference)";

fn write_monolog(project: &Path) {
    let path = project.join(MONOLOG);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "monolog:\n    handlers: {}\n").unwrap();
}

/// The project index (step 1) gets three prompts, its three attempts; the rest
/// of the chain follows. The agent leaves `line` in the index on every attempt.
fn arm_index_step(tools: &Path, project: &Path, line: &str) {
    arm_index_step_with(tools, project, line, "");
}

/// As `arm_index_step`, the agent also runs `extra` (shell) before its edit.
fn arm_index_step_with(tools: &Path, project: &Path, line: &str, extra: &str) {
    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let chain: Vec<&str> = chain.iter().map(|s| s.target_file).collect();
    assert_eq!(chain[0], AGENTS);
    let mut targets = vec![chain[0]; 3];
    targets.extend_from_slice(&chain[1..]);
    arm(tools, "fill", &targets);
    let index = project.join(AGENTS);
    after_each_step(
        tools,
        &format!(
            "{AGENTS}) {extra} grep -qF 'KT1370' '{p}' || printf '\\n<!-- KT1370 -->\\n%s\\n' '{line}' >> '{p}' ;;",
            p = index.display()
        ),
    );
}

fn of_step(body: &str, event: &str, step: u64) -> Vec<Value> {
    sse_events(body, event)
        .into_iter()
        .filter(|e| e["step"] == step)
        .collect()
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_full_run_demotes_the_marker_it_could_not_repair_and_completes() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    write_monolog(project);
    arm_index_step(
        tools,
        project,
        &format!("Logs go to stderr [src: file: {MONOLOG}] in prod."),
    );
    let body = run_full(state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;

    // Three attempts, as before; then the last one is demoted, not failed.
    let retries = of_step(&body, "step_retry", 1);
    assert_eq!(retries.len(), 2, "{body}");
    assert!(retries
        .iter()
        .all(|r| r["reason"] == "documentary_blockers"));
    let demoted = of_step(&body, "documentary_demoted", 1);
    assert_eq!(demoted.len(), 1, "{body}");
    assert_eq!(demoted[0]["references"].as_array().unwrap().len(), 1);
    assert_eq!(demoted[0]["references"][0]["doc"], AGENTS);
    assert_eq!(
        demoted[0]["references"][0]["marker"],
        format!("[src: file: {MONOLOG}]")
    );
    assert!(of_step(&body, "step_warning", 1).is_empty(), "{body}");
    let step_done = of_step(&body, "step_done", 1).pop().unwrap();
    assert_eq!(step_done["success"], true, "{step_done}");
    assert_eq!(step_done["unverified_references"], 1, "{step_done}");

    // The run completes and says how many references are unverified.
    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(done["unverified_references"], 1, "{done}");
    let index = std::fs::read_to_string(project.join(AGENTS)).unwrap();
    assert!(
        index.contains(&format!("Logs go to stderr {FLAGGED} in prod.")),
        "{index}"
    );
    assert!(!index.contains("[src: file: config/packages"), "{index}");
    assert!(
        crate::core::document_optimization::analyze(project)
            .unwrap()
            .blocking_diagnostics()
            .next()
            .is_none(),
        "the demoted references are no longer citations"
    );

    // The step is done with a warning listing each reference; the run keeps them.
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(state, &run_id).await;
    let first = steps.iter().find(|s| s.step_index == 1).unwrap();
    assert!(first.cli_success, "{first:?}");
    let warning = first.step_warning.as_deref().expect("step warning");
    assert!(warning.contains(AGENTS), "{warning}");
    assert!(
        warning.contains(&format!("[src: file: {MONOLOG}]")),
        "{warning}"
    );
    assert_ne!(first.target_state.as_deref(), Some("partial"), "{first:?}");
    let saved: Value = serde_json::from_str(run.documentary_notes.as_deref().unwrap_or("null"))
        .unwrap_or_else(|_| panic!("{:?}", run.documentary_notes));
    assert_eq!(saved["unverified"][0]["step"], 1, "{saved}");
    assert_eq!(saved["unverified"][0]["doc"], AGENTS, "{saved}");
    assert!(prompt
        .expect("validation prompt")
        .contains(&format!("[src: file: {MONOLOG}]")));
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_secret_file_is_never_left_as_text_with_a_line() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    std::fs::write(project.join(".env"), "TOKEN=hunter2\n").unwrap();
    arm_index_step(
        tools,
        project,
        "Set the token [src: file: .env:1 the token line] first.",
    );
    let body = run_full(state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;

    assert_eq!(done["status"], "complete", "{body}");
    let demoted = of_step(&body, "documentary_demoted", 1);
    assert_eq!(demoted.len(), 1, "{body}");
    let index = std::fs::read_to_string(project.join(AGENTS)).unwrap();
    assert!(index.contains("Set the token  first."), "{index}");
    assert!(!index.contains(".env"), "{index}");
    assert!(!index.contains("token line"), "{index}");
    let listed = demoted[0]["references"].to_string();
    assert!(!listed.contains(".env"), "{listed}");
    assert!(!listed.contains("token line"), "{listed}");
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(state, &run_id).await;
    let recorded = format!(
        "{:?}{:?}{}",
        steps.iter().map(|s| &s.step_warning).collect::<Vec<_>>(),
        run.documentary_notes,
        prompt.unwrap_or_default()
    );
    assert!(!recorded.contains(".env"), "{recorded}");
    assert!(!recorded.contains("token line"), "{recorded}");
}

/// Every file under `dir` whose bytes hold `needle`, except the secret itself.
fn files_holding(dir: &Path, needle: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            if entry.file_name() != ".git" {
                files_holding(&path, needle, found);
            }
        } else if kind.is_file() && entry.file_name() != ".env" {
            let bytes = std::fs::read(&path).unwrap_or_default();
            if String::from_utf8_lossy(&bytes).contains(needle) {
                found.push(path);
            }
        }
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_secret_marker_survives_nowhere_after_a_demotion() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    std::fs::write(project.join(".env"), "TOKEN=hunter2\n").unwrap();
    // The comma turns the comment into a would-be path; the encoded name is an
    // alias of `.env`. Both are removed with the marker, never copied anywhere.
    // A link whose NAME holds a space resolves to a secret too; the agent makes
    // it (a link present before the run cannot be snapshotted).
    std::fs::create_dir_all(project.join(".ssh")).unwrap();
    std::fs::write(project.join(".ssh/synthetic-key"), "SYNTHETIC_KEY_BODY\n").unwrap();
    let alias = format!(
        "mkdir -p '{p}/refs'; ln -sfn ../.ssh/synthetic-key '{p}/refs/key view.txt';",
        p = project.display()
    );
    arm_index_step_with(
        tools,
        project,
        "Set it [src: file: .env:1 safe_word, SYNTHETIC_COMMENT_7731] and \
         [src: file: %2eenv:1 SYNTHETIC_COMMENT_7731] and \
         [src: file: refs/key view.txt:1 SYNTHETIC_COMMENT_7731] and \
         [src: file: \"refs/key view.txt:1\" SYNTHETIC_COMMENT_7731] first.",
        &alias,
    );
    let body = run_full(state, None).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("no done: {body}"));
    validation_settled(state, &done).await;

    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(done["unverified_references"], 4, "{done}");
    let mut found = Vec::new();
    files_holding(project, "SYNTHETIC_COMMENT_7731", &mut found);
    assert!(
        found.is_empty(),
        "the comment survives in {found:?} (docs root and audit state)"
    );
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(state, &run_id).await;
    let shown: Vec<_> = sse_events(&body, "documentary_demoted")
        .iter()
        .map(|e| e["references"].to_string())
        .collect();
    let recorded = format!(
        "{shown:?}{:?}{:?}{}",
        steps.iter().map(|s| &s.step_warning).collect::<Vec<_>>(),
        run.documentary_notes,
        prompt.unwrap_or_default()
    );
    assert!(!recorded.contains("SYNTHETIC_COMMENT_7731"), "{recorded}");
    assert!(!recorded.contains(".env"), "{recorded}");
    assert!(!recorded.contains("key view"), "{recorded}");
    assert!(
        recorded.contains("[src: file: <secret file>]"),
        "{recorded}"
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_failure_that_is_not_a_citation_marker_still_interrupts() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    write_monolog(project);
    // Besides the marker, a link to a page that exists nowhere: a documentary
    // blocker that is not a citation marker, so nothing is demoted.
    arm_index_step(
        tools,
        project,
        &format!("Logs [src: file: {MONOLOG}] and [the page](nowhere/gone.md)."),
    );
    let body = run_full(state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;

    assert_eq!(of_step(&body, "step_retry", 1).len(), 2, "{body}");
    assert!(
        sse_events(&body, "documentary_demoted").is_empty(),
        "{body}"
    );
    assert!(!of_step(&body, "step_warning", 1).is_empty(), "{body}");
    let step_done = of_step(&body, "step_done", 1).pop().unwrap();
    assert_eq!(step_done["success"], false, "{step_done}");
    assert_eq!(done["status"], "interrupted", "{body}");
    assert_eq!(done["unverified_references"], 0, "{done}");
    // The marker stays a marker: the agent's text is not touched.
    let index = std::fs::read_to_string(project.join(AGENTS)).unwrap();
    assert!(
        index.contains(&format!("[src: file: {MONOLOG}]")),
        "{index}"
    );
    assert!(!index.contains("(unverified reference)"), "{index}");
}

/// A validated repository whose rules cite `src/legacy.rs` (deleted afterwards,
/// which sends the document to an agent) and, as the agent leaves it, a text
/// file without a line.
async fn revalidation_holding(extra: &str) -> Validated {
    let fx = Validated::new(|project| {
        std::fs::write(
            project.join("src/legacy.rs"),
            "fn legacy_entry_point() {}\n",
        )
        .unwrap();
        append(
            project,
            RULES,
            "\n- The legacy entry point survives [src: file: src/legacy.rs:1]\n",
        );
    })
    .await;
    std::fs::remove_file(fx.project().join("src/legacy.rs")).unwrap();
    write_monolog(fx.project());
    append(fx.project(), RULES, extra);
    reread_plain_docs(fx.project());
    fx
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_revalidation_demotes_what_its_agent_left_stale_and_completes() {
    let fx = revalidation_holding(&format!(
        "\n- Logs go to stderr [src: file: {MONOLOG}] in prod.\n"
    ))
    .await;
    arm(fx.h.tools.path(), "noop", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");

    let retries = sse_events(&body, "step_retry");
    assert_eq!(
        retries.len(),
        crate::api::audit::anti_hallu_enforce::MAX_ATTEMPTS - 1,
        "{body}"
    );
    let demoted = sse_events(&body, "documentary_demoted");
    assert_eq!(demoted.len(), 1, "{body}");
    let listed = demoted[0]["references"].to_string();
    assert!(listed.contains(MONOLOG), "{listed}");
    assert!(listed.contains("src/legacy.rs"), "{listed}");
    assert!(sse_events(&body, "step_warning").is_empty(), "{body}");
    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(done["unverified_references"], 2, "{done}");

    let rules = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(
        rules.contains(&format!("Logs go to stderr {FLAGGED} in prod.")),
        "{rules}"
    );
    assert!(
        rules.contains("`src/legacy.rs` (unverified reference)"),
        "{rules}"
    );
    assert!(!rules.contains("[src: file: src/legacy.rs"), "{rules}");
    assert!(rules.contains("[src: file: src/kept.rs:2]"), "{rules}");

    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (_, steps, _) = persisted(&fx.h.state, &run_id).await;
    let step = steps.first().expect("the revalidated step");
    assert!(step.cli_success, "{step:?}");
    let warning = step.step_warning.as_deref().expect("step warning");
    assert!(
        warning.contains(&format!("[src: file: {MONOLOG}]")),
        "{warning}"
    );
    assert!(warning.contains("[src: file: src/legacy.rs]"), "{warning}");
    // Revalidating again reports the references as they are: no marker is left
    // to fail on, and the flag is not read by the gate.
    assert!(
        crate::core::document_optimization::analyze(fx.project())
            .unwrap()
            .blocking_diagnostics()
            .next()
            .is_none(),
        "the flag must not fail the gate"
    );
}

/// Puts the anti-hallucination mode back to what the other tests expect.
struct EnforceMode;
impl EnforceMode {
    fn on() -> Self {
        crate::core::anti_halluc::set_mode("enforce");
        Self
    }
}
impl Drop for EnforceMode {
    fn drop(&mut self) {
        crate::core::anti_halluc::set_mode(crate::core::anti_halluc::DEFAULT_MODE_STR);
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_partial_step_demotes_what_the_enforce_gate_still_refuses_and_completes() {
    let fx = Validated::new(|_| {}).await;
    append(
        fx.project(),
        RULES,
        "\n- Errors are mapped nowhere [src: file: src/kept.rs:999]\n",
    );
    let _mode = EnforceMode::on();
    arm(fx.h.tools.path(), "noop", &[RULES; 3]);
    let body = fx.partial(vec![fx.step_of(RULES) as usize]).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");

    assert_eq!(sse_events(&body, "step_retry").len(), 2, "{body}");
    let demoted = sse_events(&body, "documentary_demoted");
    assert_eq!(demoted.len(), 1, "{body}");
    assert_eq!(
        demoted[0]["references"][0]["marker"], "[src: file: src/kept.rs]",
        "{body}"
    );
    assert!(sse_events(&body, "step_warning").is_empty(), "{body}");
    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(done["unverified_references"], 1, "{done}");
    let rules = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(
        rules.contains("Errors are mapped nowhere `src/kept.rs` (unverified reference)"),
        "{rules}"
    );
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, _) = persisted(&fx.h.state, &run_id).await;
    // The timeline's notes block reads the run: the partial pipeline records
    // the unverified references there, as the Full one does.
    let saved: Value = serde_json::from_str(run.documentary_notes.as_deref().unwrap_or("null"))
        .unwrap_or_else(|_| panic!("{:?}", run.documentary_notes));
    assert_eq!(
        saved["unverified"].as_array().map(Vec::len),
        Some(1),
        "{saved}"
    );
    assert_eq!(saved["unverified"][0]["doc"], RULES, "{saved}");
    assert_eq!(
        saved["unverified"][0]["marker"], "[src: file: src/kept.rs]",
        "{saved}"
    );
    let step = steps.first().expect("the refreshed step");
    assert!(step.cli_success, "{step:?}");
    assert!(
        step.step_warning
            .as_deref()
            .is_some_and(|w| w.contains("[src: file: src/kept.rs]")),
        "{step:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_revalidation_left_with_another_blocker_still_interrupts() {
    let fx = revalidation_holding(&format!(
        "\n- Logs [src: file: {MONOLOG}] and [the page](nowhere/gone.md).\n"
    ))
    .await;
    arm(fx.h.tools.path(), "noop", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");

    assert!(
        sse_events(&body, "documentary_demoted").is_empty(),
        "{body}"
    );
    assert_eq!(done["status"], "interrupted", "{body}");
    assert_eq!(done["unverified_references"], 0, "{done}");
    let rules = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(
        rules.contains(&format!("[src: file: {MONOLOG}]")),
        "{rules}"
    );
    assert!(!rules.contains("(unverified reference)"), "{rules}");
}
