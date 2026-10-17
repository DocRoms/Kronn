//! KT-1312 — what follows an interrupted audit, through the HTTP/SSE routes:
//! revalidation once every failed step is accepted, a resume that never re-runs
//! an accepted step, fresh fingerprints for every doc a run chain rewrote, and a
//! failed doc that stays in the plan with the badge raised.
use super::*;

async fn accept(
    state: &AppState,
    run_id: &str,
    step: u64,
    override_unsound: bool,
) -> crate::models::ApiResponse<crate::api::audit::AcceptAuditStepResponse> {
    crate::api::audit::accept_audit_step(
        axum::extract::State(state.clone()),
        axum::extract::Path((PROJECT_ID.to_string(), run_id.to_string(), step as u32)),
        None,
        axum::Json(crate::api::audit::AcceptAuditStepRequest {
            reason: Some("kept as it stands".into()),
            override_unsound: override_unsound.then_some(true),
        }),
    )
    .await
    .0
}

async fn resume(state: &AppState, run_id: &str, redo_steps: Option<Vec<u32>>) -> String {
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some(run_id.to_string()),
            retry_steps: None,
            redo_steps,
            fresh: None,
        }),
    )
    .await
    .into_response();
    sse_body(response).await
}

fn run_id_of(done: &Value) -> String {
    done["audit_run_id"].as_str().unwrap().to_string()
}

/// A Full audit of a validated project whose `fail`-th step prompt ends in a
/// CLI error: the run ends Interrupted with that one step to redo. In `fill`
/// mode every other step rewrites its doc; in `noop` mode none does.
async fn interrupted_by(fx: &Validated, fail: u64, mode: &str) -> (String, Value) {
    arm(fx.h.tools.path(), mode, &fx.chain);
    fail_steps(fx.h.tools.path(), &[fail as u32]);
    let body = run_full(&fx.h.state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{body}");
    assert_eq!(done["steps_to_redo"], json!([fail]), "{body}");
    (run_id_of(&done), done)
}

fn badge(fx: &Validated) -> crate::core::citation_drift::SourcesState {
    crate::core::checksums::check_drift(fx.project()).sources_state
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_interrupted_audit_with_every_failed_step_accepted_can_be_revalidated() {
    let fx = Validated::new(|_| {}).await;
    let rules = fx.step_of(RULES);
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    let (interrupted, _) = interrupted_by(&fx, rules, "noop").await;
    // The step that failed is the one that owns the stale citation.
    assert_eq!(fx.plan().await.agent_steps, vec![rules as usize]);

    // While a failed step is neither retried nor accepted, the refusal stands.
    arm(fx.h.tools.path(), "revalidate", &[RULES]);
    let refused = fx.revalidate().await;
    let error = sse_events(&refused, "error")
        .pop()
        .unwrap_or_else(|| panic!("{refused}"));
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("accept its failed steps"),
        "{refused}"
    );
    assert!(started_steps(&refused).is_empty(), "{refused}");

    let accepted = accept(&fx.h.state, &interrupted, rules, false).await;
    assert!(accepted.success, "{:?}", accepted.error);
    assert_eq!(accepted.data.unwrap().remaining_failed, Vec::<u32>::new());

    // Nothing is left to resume but the finalization: the revalidation starts.
    let body = fx.revalidate().await;
    assert!(sse_events(&body, "error").is_empty(), "{body}");
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["revalidation"], true, "{body}");
    assert_eq!(started_steps(&body), vec![rules], "{body}");
}

/// A document that still holds a blocker its failed step could not clear: the
/// step is retried `MAX_ATTEMPTS` times, one scripted line per attempt.
fn ghost_chain(writer: &str, chain: &[&'static str]) -> Vec<&'static str> {
    let mut targets = Vec::new();
    for step in chain {
        let attempts = if *step == writer {
            crate::api::audit::anti_hallu_enforce::MAX_ATTEMPTS
        } else {
            1
        };
        targets.extend(std::iter::repeat_n(*step, attempts));
    }
    targets
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_resume_never_reruns_an_accepted_step_unless_the_human_asks() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    let chain: Vec<&'static str> =
        crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
            .iter()
            .map(|s| s.target_file)
            .collect();
    let writer = chain.iter().position(|t| *t == RULES).unwrap() as u64 + 1;

    // The writer step leaves a document no step targets, citing a file that
    // does not exist: its gate fails, and so does the final documentary gate.
    arm(tools, "fill", &ghost_chain(RULES, &chain));
    let root = project.display();
    after_each_step(
        tools,
        &format!(
            "{RULES}) printf '# Extra\\nGhost [src: file: ghost-a.rs:1]\\n' > '{root}/docs/extra-a.md' ;;"
        ),
    );
    let run = run_full(state, None).await;
    let done = sse_events(&run, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "interrupted", "{run}");
    assert_eq!(done["steps_to_redo"], json!([writer]), "{run}");
    let interrupted = run_id_of(&done);

    // Accepted as it stands: the blocker needs the explicit override.
    let refused = accept(state, &interrupted, writer, false).await;
    assert!(!refused.success);
    assert!(refused
        .error
        .unwrap()
        .starts_with(crate::api::audit::OVERRIDE_REQUIRED_PREFIX));
    let accepted = accept(state, &interrupted, writer, true).await;
    assert!(accepted.success, "{:?}", accepted.error);
    assert_eq!(accepted.data.unwrap().remaining_failed, Vec::<u32>::new());

    // The resume leaves the accepted step alone, though its document still holds
    // the blocker: no agent runs.
    std::fs::write(tools.join("steps.log"), "").unwrap();
    std::fs::remove_file(tools.join("after")).unwrap();
    arm(tools, "fill", &chain);
    let resumed = resume(state, &interrupted, None).await;
    assert!(sse_events(&resumed, "error").is_empty(), "{resumed}");
    assert!(started_steps(&resumed).is_empty(), "{resumed}");
    assert!(
        prompt_kinds(tools).is_empty(),
        "no agent ran: {:?}",
        prompt_kinds(tools)
    );
    let skipped: Vec<u64> = sse_events(&resumed, "step_skipped")
        .iter()
        .map(|e| e["step"].as_u64().unwrap())
        .collect();
    assert!(skipped.contains(&writer), "{resumed}");
    let done = sse_events(&resumed, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    let still = run_id_of(&done);

    // The human asks: only then does the accepted step run again.
    arm(tools, "noop", &[RULES]);
    let redone = resume(state, &still, Some(vec![writer as u32])).await;
    assert_eq!(started_steps(&redone), vec![writer], "{redone}");
}

/// The gate still holds the line on the document: fixed by hand, the accepted
/// step lets the resume complete without running anything.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_resume_of_an_accepted_step_finalizes_once_its_document_is_fixed_by_hand() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    let chain: Vec<&'static str> =
        crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
            .iter()
            .map(|s| s.target_file)
            .collect();
    let writer = chain.iter().position(|t| *t == RULES).unwrap() as u64 + 1;
    arm(tools, "fill", &ghost_chain(RULES, &chain));
    let root = project.display();
    after_each_step(
        tools,
        &format!(
            "{RULES}) printf '# Extra\\nGhost [src: file: ghost-a.rs:1]\\n' > '{root}/docs/extra-a.md' ;;"
        ),
    );
    let run = run_full(state, None).await;
    let done = sse_events(&run, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    let interrupted = run_id_of(&done);
    assert!(accept(state, &interrupted, writer, true).await.success);

    std::fs::remove_file(tools.join("after")).unwrap();
    std::fs::remove_file(project.join("docs/extra-a.md")).unwrap();
    std::fs::write(tools.join("steps.log"), "").unwrap();
    arm(tools, "fill", &chain);
    settle_on_validation(tools);
    let resumed = resume(state, &interrupted, None).await;
    let done = sse_events(&resumed, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "complete", "{resumed}");
    assert!(started_steps(&resumed).is_empty(), "{resumed}");
    let (run, _, _) = persisted(state, &run_id_of(&done)).await;
    assert_eq!(outcomes(&run)["accepted"], json!([writer]));
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn every_doc_rewritten_across_a_resumed_chain_gets_fresh_fingerprints() {
    let fx = Validated::new(|_| {}).await;
    let last = fx.chain.len() as u64;
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::NeedsReverification
    );
    let before = fx.record_of(RULES);

    // The run rewrites the rules doc, then its last step fails.
    let (interrupted, _) = interrupted_by(&fx, last, "fill").await;
    assert_ne!(
        fx.record_of(RULES),
        before,
        "the interrupted run rewrote it"
    );
    assert_eq!(
        fx.record_of(RULES).reason,
        crate::core::citation_drift::RecordReason::Audit
    );

    // The failed step accepted, the resume finds nothing to run: the chain ends
    // with the fingerprints of the docs its first run rewrote.
    assert!(accept(&fx.h.state, &interrupted, last, false).await.success);
    std::fs::write(fx.h.tools.path().join("steps.log"), "").unwrap();
    arm(fx.h.tools.path(), "noop", &[]);
    settle_on_validation(fx.h.tools.path());
    let resumed = resume(&fx.h.state, &interrupted, None).await;
    let done = sse_events(&resumed, "done").pop().expect("terminal done");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(done["status"], "complete", "{resumed}");
    assert!(started_steps(&resumed).is_empty(), "{resumed}");
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::Clean
    );
    assert_eq!(badge(&fx), crate::core::citation_drift::SourcesState::Clean);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_doc_whose_step_failed_stays_in_the_plan_and_keeps_the_badge_raised() {
    let fx = Validated::new(|_| {}).await;
    let rules = fx.step_of(RULES);
    assert_eq!(badge(&fx), crate::core::citation_drift::SourcesState::Clean);

    // The step writes a link to a page that does not exist: the documentary
    // gate blocks it, relevance does not read Markdown links.
    arm(fx.h.tools.path(), "noop", &ghost_chain(RULES, &fx.chain));
    let root = fx.project().display();
    after_each_step(
        fx.h.tools.path(),
        &format!("{RULES}) printf '\\nSee [the guide](absent.md).\\n' >> '{root}/{RULES}' ;;"),
    );
    let body = run_full(&fx.h.state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{body}");
    assert_eq!(done["steps_to_redo"], json!([rules]), "{body}");

    // The doc keeps its partial banner...
    let content = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(
        crate::api::audit::step_guard::has_partial_marker(&content),
        "{content}"
    );
    // ... so the badge stays raised and the next plan still queues its step.
    assert_eq!(
        badge(&fx),
        crate::core::citation_drift::SourcesState::NeedsReverification
    );
    let plan = fx.plan().await;
    assert_eq!(
        plan.sources_state,
        crate::core::citation_drift::SourcesState::NeedsReverification
    );
    assert_eq!(plan.agent_steps, vec![rules as usize], "{plan:?}");
    assert_eq!(
        section(&plan, RULES).action,
        crate::models::RevalidationAction::Agent
    );
}
