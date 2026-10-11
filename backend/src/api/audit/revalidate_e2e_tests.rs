//! KT-1168 — « Revalider l'audit » end to end: real Full audit and
//! validation, then the code moves and the revalidation runs on the scripted
//! agent. Deterministic first (re-anchor, human to-dos), then a bounded agent
//! pass through the partial pipeline, resumable by relaunching it.
use super::*;

const RULES: &str = "docs/coding-rules.md";
const TESTING: &str = "docs/testing-quality.md";
const GLOSSARY: &str = "docs/glossary.md";
const HUMAN_RULE: &str = "<!-- kronn:section name=\"team-rule\" owner=\"human\" -->\n\
     ## Team rule\n\nHuman-owned rule: reviews happen on Tuesday.\n\
     <!-- kronn:section:end -->\n";

fn source_lines() -> Vec<String> {
    (1..=12)
        .map(|i| format!("fn cited_function_{i}() {{}}"))
        .collect()
}

fn write_source(project: &Path, lines: &[String]) {
    std::fs::write(project.join("src/kept.rs"), lines.join("\n") + "\n").unwrap();
}

fn append(project: &Path, doc: &str, text: &str) {
    let path = project.join(doc);
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str(text);
    std::fs::write(&path, content).unwrap();
}

/// A repository audited by the Full pipeline and validated by the human, its
/// docs citing `src/kept.rs`: the fingerprints are recorded.
struct Validated {
    h: Harness,
    chain: Vec<&'static str>,
}

impl Validated {
    async fn new(extra: impl FnOnce(&Path)) -> Self {
        let h = harness().await;
        let project = h.project.path().to_path_buf();
        let project = project.as_path();
        std::fs::create_dir_all(project.join("src")).unwrap();
        write_source(project, &source_lines());
        let chain: Vec<&'static str> =
            crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
                .iter()
                .map(|s| s.target_file)
                .collect();
        arm(h.tools.path(), "fill", &chain);
        settle_on_validation(h.tools.path());
        let body = run_full(&h.state, None).await;
        let done = sse_events(&body, "done").pop().expect("terminal done");
        validation_settled(&h.state, &done).await;
        assert_eq!(done["status"], "complete", "{body}");
        append(
            project,
            RULES,
            "\n## Verified facts\n\n\
             - Configuration loads in one function [src: file: src/kept.rs:2]\n\
             - Errors are mapped in one place [src: file: src/kept.rs:3]\n\n",
        );
        append(project, RULES, HUMAN_RULE);
        extra(project);
        let fx = Self { h, chain };
        let validated = fx.validate().await;
        assert!(validated.success, "{:?}", validated.error);
        assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
        assert_eq!(
            crate::core::citation_drift::check(project).sources_state,
            crate::core::citation_drift::SourcesState::Clean
        );
        std::fs::write(fx.h.tools.path().join("steps.log"), "").unwrap();
        fx
    }

    fn project(&self) -> &Path {
        self.h.project.path()
    }

    fn step_of(&self, target: &str) -> u64 {
        self.chain.iter().position(|t| *t == target).unwrap() as u64 + 1
    }

    fn edit_source(&self, edit: impl FnOnce(&mut Vec<String>)) {
        let mut lines: Vec<String> = std::fs::read_to_string(self.project().join("src/kept.rs"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        edit(&mut lines);
        write_source(self.project(), &lines);
    }

    fn audit_status(&self) -> crate::models::AiAuditStatus {
        crate::core::scanner::detect_audit_status(&self.project().display().to_string())
    }

    async fn validate(&self) -> crate::models::ApiResponse<crate::models::AiAuditStatus> {
        crate::api::audit::validate_audit(
            axum::extract::State(self.h.state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
        )
        .await
        .0
    }

    async fn plan(&self) -> crate::models::RevalidationPlan {
        let response = crate::api::audit::revalidation_plan(
            axum::extract::State(self.h.state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
        )
        .await
        .0;
        assert!(response.success, "{:?}", response.error);
        response.data.unwrap()
    }

    async fn revalidate(&self) -> String {
        let response = crate::api::audit::revalidate_audit(
            axum::extract::State(self.h.state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
            axum::Json(crate::models::RevalidateAuditRequest {
                agent: AgentType::ClaudeCode,
                connection_id: None,
                tier: None,
            }),
        )
        .await
        .into_response();
        sse_body(response).await
    }

    async fn partial(&self, steps: Vec<usize>) -> String {
        let response = crate::api::audit::drift::partial_audit(
            axum::extract::State(self.h.state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
            axum::Json(crate::models::PartialAuditRequest {
                agent: AgentType::ClaudeCode,
                connection_id: None,
                tier: None,
                steps,
            }),
        )
        .await
        .into_response();
        sse_body(response).await
    }

    fn record_of(&self, doc: &str) -> crate::core::citation_drift::DocFingerprints {
        crate::core::citation_drift::read_strict(self.project())
            .unwrap()
            .expect("citations recorded")
            .docs
            .remove(doc)
            .unwrap_or_else(|| panic!("no record for {doc}"))
    }

    async fn run_count(&self) -> usize {
        self.h
            .state
            .db
            .with_conn(|conn| Ok(crate::db::audit_runs::list_recent(conn, PROJECT_ID, 100)?.len()))
            .await
            .unwrap()
    }

    fn sources_state(&self) -> crate::core::citation_drift::SourcesState {
        crate::core::citation_drift::check(self.project()).sources_state
    }

    fn prompts(&self) -> Vec<(String, bool)> {
        prompt_kinds(self.h.tools.path())
    }
}

fn section<'a>(
    plan: &'a crate::models::RevalidationPlan,
    doc: &str,
) -> &'a crate::models::RevalidationSection {
    plan.sections
        .iter()
        .find(|s| s.doc == doc)
        .unwrap_or_else(|| panic!("no section {doc}: {plan:?}"))
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_changed_cited_line_is_corrected_by_the_agent_and_the_badge_goes_back_to_clean() {
    let fx = Validated::new(|_| {}).await;
    let rules_step = fx.step_of(RULES);
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::NeedsReverification
    );

    // Deterministic first: Kronn names the citation and the step to run.
    let plan = fx.plan().await;
    assert_eq!(plan.agent_steps, vec![rules_step as usize]);
    let rules = section(&plan, RULES);
    assert_eq!(rules.action, crate::models::RevalidationAction::Agent);
    assert_eq!(rules.broken.len(), 1);
    assert_eq!(rules.broken[0].cite, "src/kept.rs:3");
    assert_eq!(
        rules.broken[0].state,
        crate::core::citation_drift::CitationState::Changed
    );

    let before = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    arm(fx.h.tools.path(), "revalidate", &[RULES]);

    // A Stop that lands before the agent: the run ends Cancelled, nothing
    // was rewritten, so the validation it revoked comes back.
    fx.h.state
        .audit_tracker
        .lock()
        .unwrap()
        .cancelled
        .insert(PROJECT_ID.to_string());
    let body = fx.revalidate().await;
    assert_eq!(sse_events(&body, "cancelled").len(), 1, "{body}");
    assert!(sse_events(&body, "done").is_empty(), "{body}");
    assert!(fx.prompts().is_empty(), "{:?}", fx.prompts());
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
    assert_eq!(
        std::fs::read_to_string(fx.project().join(RULES)).unwrap(),
        before
    );

    // « Reprendre » relaunches the same action.
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(done["revalidation"], true, "{body}");
    assert_eq!(started_steps(&body), vec![rules_step], "{body}");

    // One bounded agent prompt: the skill and Kronn's findings for this doc.
    assert_eq!(fx.prompts().len(), 1, "{:?}", fx.prompts());
    let prompt = std::fs::read_to_string(fx.h.tools.path().join("prompt.1")).unwrap();
    assert!(prompt.contains("RE-VERIFICATION MODE"), "{prompt}");
    assert!(prompt.contains("# Audit revalidation"), "{prompt}");
    assert!(
        prompt.contains("- `[src: file: src/kept.rs:3]` — changed"),
        "{prompt}"
    );

    // The stale claim is corrected; the true one and the human rule stay
    // byte-identical, though the agent tried to rewrite the rule.
    let after = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(after.contains("- Revalidated against the current code [src: file: src/kept.rs:3]\n"));
    assert!(!after.contains("Errors are mapped in one place"), "{after}");
    assert!(after.contains("- Configuration loads in one function [src: file: src/kept.rs:2]\n"));
    assert!(after.contains(HUMAN_RULE), "{after}");
    assert_ne!(after, before);

    // Fresh fingerprints for the rewritten doc: the badge is clean again.
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::Clean
    );
    assert!(fx.plan().await.sections.is_empty());
    // The rewrite goes through the human's validation like any audit change.
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Audited);
    validation_settled(&fx.h.state, &done).await;
    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_moved_cited_line_is_re_anchored_without_an_agent() {
    let fx = Validated::new(|_| {}).await;
    fx.edit_source(|lines| {
        lines.insert(0, "// licence header".into());
        lines.insert(0, "// generated banner".into());
    });
    let plan = fx.plan().await;
    assert!(plan.agent_steps.is_empty(), "{plan:?}");
    assert_eq!(plan.reanchor_count, 2);
    assert_eq!(
        section(&plan, RULES).action,
        crate::models::RevalidationAction::Reanchor
    );
    let runs = fx.run_count().await;

    arm(fx.h.tools.path(), "revalidate", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "clean", "{body}");
    assert_eq!(done["reanchored"], 2, "{body}");
    let reanchored = sse_events(&body, "reanchored");
    assert_eq!(reanchored.len(), 1, "{body}");
    assert_eq!(reanchored[0]["doc"], RULES);

    // No agent, no run row, the validation untouched.
    assert!(fx.prompts().is_empty(), "{:?}", fx.prompts());
    assert_eq!(fx.run_count().await, runs);
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
    let after = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(after.contains("[src: file: src/kept.rs:4]\n"), "{after}");
    assert!(after.contains("[src: file: src/kept.rs:5]\n"), "{after}");
    assert!(after.contains(HUMAN_RULE));
    let report = crate::core::citation_drift::check(fx.project());
    assert_eq!(report.moved_count, 0);
    assert_eq!(report.broken_count, 0);
    assert!(fx.plan().await.sections.is_empty());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_human_section_is_listed_not_edited_and_a_true_doc_gives_the_validation_back() {
    let fx = Validated::new(|project| {
        append(
            project,
            GLOSSARY,
            "\n<!-- kronn:section name=\"owner-notes\" owner=\"human\" -->\n\
             ## Owner notes\n\nThe sixth helper names the term [src: file: src/kept.rs:6].\n\
             <!-- kronn:section:end -->\n",
        );
    })
    .await;
    fx.edit_source(|lines| lines[5] = "fn helper_renamed_by_a_commit() {}".into());

    let plan = fx.plan().await;
    assert!(plan.agent_steps.is_empty(), "{plan:?}");
    let glossary = section(&plan, GLOSSARY);
    assert_eq!(glossary.action, crate::models::RevalidationAction::Human);
    assert_eq!(glossary.human.len(), 1);
    assert_eq!(glossary.human[0].section, "owner-notes");
    assert_eq!(glossary.human[0].citation.cite, "src/kept.rs:6");
    assert!(glossary.broken.is_empty());

    let before = std::fs::read(fx.project().join(GLOSSARY)).unwrap();
    arm(fx.h.tools.path(), "revalidate", &[GLOSSARY]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "todo", "{body}");
    assert!(fx.prompts().is_empty(), "{:?}", fx.prompts());
    assert_eq!(std::fs::read(fx.project().join(GLOSSARY)).unwrap(), before);
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);

    // The owner checks it: "Vérifié, inchangé" settles the section.
    let acknowledged = crate::api::audit::acknowledge_citations(
        axum::extract::State(fx.h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::AcknowledgeCitationsRequest {
            doc: GLOSSARY.into(),
        }),
    )
    .await
    .0;
    assert!(acknowledged.success, "{:?}", acknowledged.error);
    assert_eq!(std::fs::read(fx.project().join(GLOSSARY)).unwrap(), before);
    assert!(fx.plan().await.sections.is_empty());

    // An agent that finds the doc still true rewrites nothing: the run ends
    // no_change, the validation it revoked comes back, and the section is
    // left for the human to acknowledge.
    fx.edit_source(|lines| lines[1] = "fn config_loader_renamed() {}".into());
    arm(fx.h.tools.path(), "noop", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "no_change", "{body}");
    assert_eq!(fx.prompts().len(), 1);
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
    assert_eq!(
        section(&fx.plan().await, RULES).action,
        crate::models::RevalidationAction::Agent
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_interrupted_revalidation_resumes_on_the_sections_left() {
    let fx = Validated::new(|project| {
        append(
            project,
            TESTING,
            "\n- The suite starts from one helper [src: file: src/kept.rs:5]\n",
        );
    })
    .await;
    let (rules_step, testing_step) = (fx.step_of(RULES), fx.step_of(TESTING));
    fx.edit_source(|lines| {
        lines[2] = "fn error_mapper_moved_away() {}".into();
        lines[4] = "fn suite_helper_rewritten() {}".into();
    });
    let plan = fx.plan().await;
    assert_eq!(
        plan.agent_steps,
        vec![rules_step as usize, testing_step as usize]
    );

    // The first agent step fails: the run stops Interrupted, the other doc
    // is corrected and recorded.
    arm(fx.h.tools.path(), "revalidate", &[RULES, TESTING]);
    fail_steps(fx.h.tools.path(), &[1]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "interrupted", "{body}");
    assert_eq!(done["failed_steps"], json!([rules_step]), "{body}");
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, _, _) = persisted(&fx.h.state, &run_id).await;
    assert_eq!(run.status, "Interrupted");
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Audited);
    let left = fx.plan().await;
    assert_eq!(left.agent_steps, vec![rules_step as usize], "{left:?}");
    assert!(left.sections.iter().all(|s| s.doc != TESTING), "{left:?}");

    // « Reprendre »: the same action, recomputed from the fingerprints, runs
    // the section left only, and the badge ends clean.
    std::fs::write(fx.h.tools.path().join("steps.log"), "").unwrap();
    arm(fx.h.tools.path(), "revalidate", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(started_steps(&body), vec![rules_step], "{body}");
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::Clean
    );
    validation_settled(&fx.h.state, &done).await;
    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn revalidating_before_any_fingerprint_is_refused_with_a_reason() {
    let h = harness().await;
    let response = crate::api::audit::revalidate_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::RevalidateAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let error = sse_events(&body, "error").pop().expect("an error event");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("validate the audit first"),
        "{body}"
    );
    assert!(
        crate::core::bridge_token::route_for("POST", "/api/projects/{id}/revalidate-audit")
            .is_none()
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_dead_citation_is_named_before_the_agent_and_never_ends_unchanged() {
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
    let plan = fx.plan().await;
    let rules = section(&plan, RULES);
    assert_eq!(rules.action, crate::models::RevalidationAction::Agent);
    assert_eq!(
        rules.broken[0].state,
        crate::core::citation_drift::CitationState::FileMissing
    );

    // An agent that leaves the dead citation in place is told again, then
    // the step fails: never `unchanged` while a citation does not resolve.
    arm(fx.h.tools.path(), "noop", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "interrupted", "{body}");
    let retries = sse_events(&body, "step_retry");
    assert_eq!(
        retries.len(),
        crate::api::audit::anti_hallu_enforce::MAX_ATTEMPTS - 1,
        "{body}"
    );
    assert!(retries
        .iter()
        .all(|r| r["reason"] == "revalidation_stale_target"));
    let first = std::fs::read_to_string(fx.h.tools.path().join("prompt.1")).unwrap();
    assert!(
        first.contains("- `docs/coding-rules.md`: citation path does not exist `src/legacy.rs`"),
        "{first}"
    );
    // Nothing was rewritten: the validation revoked at launch is back.
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);

    // An agent that follows the finding removes the claim in one attempt.
    std::fs::write(fx.h.tools.path().join("steps.log"), "").unwrap();
    arm(fx.h.tools.path(), "reverify", &[RULES]);
    let body = fx.revalidate().await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    assert_eq!(fx.prompts().len(), 1, "{:?}", fx.prompts());
    let after = std::fs::read_to_string(fx.project().join(RULES)).unwrap();
    assert!(!after.contains("src/legacy.rs"), "{after}");
    assert!(after.contains(HUMAN_RULE), "{after}");
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::Clean
    );
}

/// KT-1187 — the citation broken under a doc edited by hand.
fn broken_by_hand(fx: &Validated) -> Vec<(String, crate::core::citation_drift::CitationState)> {
    let report = crate::core::citation_drift::check(fx.project());
    let rules = report.section(RULES).expect("rules recorded");
    assert!(rules.doc_changed, "{rules:?}");
    rules
        .citations
        .iter()
        .filter(|c| c.state.is_broken())
        .map(|c| (c.cite.clone(), c.state))
        .collect()
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_partial_refresh_never_acknowledges_a_doc_edited_by_hand() {
    use crate::core::citation_drift::{CitationState, RecordReason, SourcesState};
    let fx = Validated::new(|_| {}).await;
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    append(fx.project(), RULES, "\nA note added by hand.\n");

    // A refresh of another doc leaves the hand-edited one to re-verify.
    arm(fx.h.tools.path(), "fill", &[GLOSSARY]);
    let body = fx.partial(vec![fx.step_of(GLOSSARY) as usize]).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(
        broken_by_hand(&fx),
        [("src/kept.rs:3".to_string(), CitationState::Changed)]
    );
    assert_eq!(fx.sources_state(), SourcesState::NeedsReverification);
    assert_eq!(fx.record_of(RULES).reason, RecordReason::Validation);
    assert_eq!(fx.record_of(GLOSSARY).reason, RecordReason::Audit);

    // A refresh that rewrites it records its citations afresh.
    arm(fx.h.tools.path(), "fill", &[RULES]);
    let body = fx.partial(vec![fx.step_of(RULES) as usize]).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(fx.record_of(RULES).reason, RecordReason::Audit);
    assert_eq!(fx.sources_state(), SourcesState::Clean);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_full_audit_re_fingerprints_only_the_docs_its_steps_rewrote() {
    use crate::core::citation_drift::{CitationState, RecordReason, SourcesState};
    let fx = Validated::new(|_| {}).await;
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    append(fx.project(), RULES, "\nA note added by hand.\n");

    // Every step ends `unchanged`: no doc was rewritten by the run.
    arm(fx.h.tools.path(), "noop", &fx.chain);
    let body = run_full(&fx.h.state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(
        broken_by_hand(&fx),
        [("src/kept.rs:3".to_string(), CitationState::Changed)]
    );
    assert_eq!(fx.record_of(RULES).reason, RecordReason::Validation);

    // Every step rewrites its target: each is recorded afresh.
    arm(fx.h.tools.path(), "fill", &fx.chain);
    let body = run_full(&fx.h.state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{body}");
    validation_settled(&fx.h.state, &done).await;
    assert_eq!(fx.record_of(RULES).reason, RecordReason::Audit);
    assert_eq!(fx.sources_state(), SourcesState::Clean);
}
