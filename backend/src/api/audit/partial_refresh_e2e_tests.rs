//! KT-1207 — a stale-sections refresh that re-verifies a section without
//! rewriting it completes, an interrupted one keeps what it did, and a human
//! can acknowledge a stale section without any agent.
use super::*;

const AGENTS: &str = "docs/AGENTS.md";

/// Sections judged by their checksum mapping, as on a project whose citations
/// were never recorded: `README.md` feeds the `docs/AGENTS.md` mapping.
fn checksum_drift_on_agents(project: &Path) {
    std::fs::remove_file(crate::core::citation_drift::citations_path(project)).unwrap();
    std::fs::write(project.join("README.md"), "# Fixture\n\nA new paragraph.\n").unwrap();
    assert!(stale(project).contains(&AGENTS.to_string()));
}

fn stale(project: &Path) -> Vec<String> {
    crate::core::checksums::check_drift(project)
        .stale_sections
        .into_iter()
        .map(|s| s.ai_file)
        .collect()
}

/// The doc's baseline mapping matches its sources as they are now.
fn mapping_is_current(project: &Path, doc: &str) -> bool {
    let file = crate::core::checksums::read_checksums_file_strict(project)
        .unwrap()
        .expect("baseline");
    let mapping = file
        .mappings
        .iter()
        .find(|m| m.ai_file == doc)
        .expect("mapped");
    let patterns: Vec<&str> = mapping.sources.iter().map(String::as_str).collect();
    mapping.checksums == crate::core::checksums::compute_step_checksums(project, &patterns)
}

fn validated_at(project: &Path) -> Option<String> {
    crate::core::kronn_state::read(project).and_then(|s| s.validated_at)
}

async fn run_of(state: &AppState, done: &Value) -> crate::models::AuditRun {
    persisted(state, done["audit_run_id"].as_str().unwrap())
        .await
        .0
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_section_re_verified_unchanged_completes_and_gives_the_validation_back() {
    let fx = Validated::new(|_| {}).await;
    let before = validated_at(fx.project()).expect("validated");
    checksum_drift_on_agents(fx.project());

    arm(fx.h.tools.path(), "noop", &[AGENTS]);
    let body = fx.partial(vec![fx.step_of(AGENTS) as usize]).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "no_change", "{body}");
    assert_eq!(done["discussion_id"], Value::Null, "{body}");
    let run = run_of(&fx.h.state, &done).await;
    assert_eq!(run.status, "Completed");
    assert!(run.validation_discussion_id.is_none());
    // Re-verified: the section is fresh and the validation is back as it was.
    assert!(mapping_is_current(fx.project(), AGENTS));
    assert!(!stale(fx.project()).contains(&AGENTS.to_string()));
    assert_eq!(validated_at(fx.project()), Some(before));
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_unchanged_target_cannot_restore_validation_after_another_doc_changed() {
    let fx = Validated::new(|_| {}).await;
    checksum_drift_on_agents(fx.project());
    arm(fx.h.tools.path(), "noop", &[AGENTS]);
    std::fs::write(
        fx.h.tools.path().join("undo"),
        format!(
            "printf '\\nAgent changed another document.\\n' >> '{}'\n",
            fx.project().join(RULES).display()
        ),
    )
    .unwrap();
    let body = fx.partial(vec![fx.step_of(AGENTS) as usize]).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert!(validated_at(fx.project()).is_none(), "{body}");
    assert!(
        done["discussion_id"].is_string(),
        "changed docs require validation: {body}"
    );
    let provenance = crate::core::doc_provenance::report(fx.project(), false).unwrap();
    assert!(provenance
        .docs
        .iter()
        .any(|d| d.doc == RULES
            && d.state == crate::core::doc_provenance::ProvenanceState::ToReconfirm));
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_reverified_unchanged_section_refreshes_its_citation_fingerprints() {
    let fx = Validated::new(|_| {}).await;
    let validated_before = validated_at(fx.project()).expect("validated");
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    assert!(stale(fx.project()).contains(&RULES.to_string()));
    let doc_before = std::fs::read(fx.project().join(RULES)).unwrap();

    arm(fx.h.tools.path(), "noop", &[RULES]);
    let body = fx.partial(vec![fx.step_of(RULES) as usize]).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "no_change", "{body}");
    assert_eq!(run_of(&fx.h.state, &done).await.status, "Completed");
    assert!(done["discussion_id"].is_null(), "{body}");
    assert!(!stale(fx.project()).contains(&RULES.to_string()));
    assert_eq!(std::fs::read(fx.project().join(RULES)).unwrap(), doc_before);
    assert_eq!(validated_at(fx.project()), Some(validated_before));
    assert_eq!(
        fx.sources_state(),
        crate::core::citation_drift::SourcesState::Clean
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_unvalidated_project_re_verified_unchanged_can_be_validated() {
    let fx = Audited::new().await;
    checksum_drift_on_agents(fx.project.path());
    assert!(validated_at(fx.project.path()).is_none());

    // Partial runs also require unresolved templates to be settled before
    // human validation, even when their selected target was unchanged.
    settle_on_validation(fx.tools.path());
    arm(fx.tools.path(), "noop", &[AGENTS]);
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(fx.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![fx.step_of(AGENTS) as usize],
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "no_change", "{body}");
    let (run, _, prompt) = persisted(&fx.state, done["audit_run_id"].as_str().unwrap()).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(
        run.validation_discussion_id.as_deref(),
        done["discussion_id"].as_str()
    );
    let prompt = prompt.expect("a scoped validation discussion");
    assert!(prompt.contains(&format!("`{AGENTS}`")), "{prompt}");
    assert!(!stale(fx.project.path()).contains(&AGENTS.to_string()));

    // The validate gate no longer refuses: the latest run is Completed.
    validation_settled(&fx.state, &done).await;
    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert!(validated_at(fx.project.path()).is_some());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_refresh_stopped_before_any_write_gives_the_validation_back() {
    let fx = Validated::new(|_| {}).await;
    let before = validated_at(fx.project()).expect("validated");
    arm(fx.h.tools.path(), "noop", &[GLOSSARY]);
    let body = fx
        .stopped_during_step(fx.partial(vec![fx.step_of(GLOSSARY) as usize]))
        .await;
    assert_eq!(sse_events(&body, "cancelled").len(), 1, "{body}");
    assert_eq!(validated_at(fx.project()), Some(before));
    assert_eq!(fx.audit_status(), crate::models::AiAuditStatus::Validated);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_interrupted_refresh_merges_its_done_steps_and_opens_their_validation() {
    let fx = Validated::new(|_| {}).await;
    checksum_drift_on_agents(fx.project());
    let (agents_step, glossary_step) = (fx.step_of(AGENTS), fx.step_of(GLOSSARY));

    arm(fx.h.tools.path(), "fill", &[AGENTS, GLOSSARY]);
    fail_steps(fx.h.tools.path(), &[2]);
    let body = fx
        .partial(vec![agents_step as usize, glossary_step as usize])
        .await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "interrupted", "{body}");
    assert_eq!(done["succeeded_steps"], json!([agents_step]), "{body}");
    assert_eq!(done["failed_steps"], json!([glossary_step]), "{body}");
    let disc = done["discussion_id"].as_str().expect("a scoped validation");

    let (run, _, prompt) = persisted(&fx.h.state, done["audit_run_id"].as_str().unwrap()).await;
    assert_eq!(run.status, "Interrupted");
    assert_eq!(run.validation_discussion_id.as_deref(), Some(disc));
    let prompt = prompt.unwrap();
    let total = fx.chain.len();
    assert!(
        prompt.contains(&format!("- {glossary_step}/{total} — `{GLOSSARY}`")),
        "the failed section is named out of scope: {prompt}"
    );
    assert!(prompt.contains(&format!("`{AGENTS}`")), "{prompt}");
    // The done step's baseline is merged; the run still cannot be validated.
    assert!(mapping_is_current(fx.project(), AGENTS));
    assert!(!stale(fx.project()).contains(&AGENTS.to_string()));
    validation_settled(&fx.h.state, &done).await;
    let refused = fx.validate().await;
    assert!(!refused.success);
    assert!(
        refused
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Interrupted"),
        "{:?}",
        refused.error
    );
}

async fn acknowledge(
    fx: &Validated,
    sections: &[&str],
    agent: bool,
) -> crate::models::ApiResponse<crate::models::DriftCheckResponse> {
    let bridge = agent.then(|| {
        axum::Extension(crate::core::bridge_token::BridgeCaller {
            token_id: "t".into(),
            project: Some(PROJECT_ID.into()),
            own_discussions: vec![],
            own_run: None,
            agent: None,
        })
    });
    crate::api::audit::acknowledge_drift(
        axum::extract::State(fx.h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        bridge,
        axum::Json(crate::models::AcknowledgeDriftRequest {
            sections: sections.iter().map(|s| s.to_string()).collect(),
        }),
    )
    .await
    .0
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn verified_unchanged_freshens_a_checksum_section_for_a_human_only() {
    let fx = Validated::new(|_| {}).await;
    checksum_drift_on_agents(fx.project());
    let docs_before = docs_snapshot(fx.project());
    let manifest = || std::fs::read(fx.project().join("docs/checksums.json")).unwrap();
    let manifest_before = manifest();

    // Never an agent's decision, and never while an audit holds the lease.
    let refused = acknowledge(&fx, &[AGENTS], true).await;
    assert!(!refused.success);
    assert!(
        crate::core::bridge_token::route_for("POST", "/api/projects/{id}/drift/acknowledge")
            .is_none()
    );
    assert!(fx
        .h
        .state
        .audit_tracker
        .lock()
        .unwrap()
        .try_acquire_lease(PROJECT_ID));
    let refused = acknowledge(&fx, &[AGENTS], false).await;
    assert!(!refused.success);
    fx.h.state
        .audit_tracker
        .lock()
        .unwrap()
        .release_lease(PROJECT_ID);
    let refused = acknowledge(&fx, &["docs/not-a-section.md"], false).await;
    assert!(!refused.success);
    assert_eq!(manifest(), manifest_before);

    let acknowledged = acknowledge(&fx, &[AGENTS], false).await;
    assert!(acknowledged.success, "{:?}", acknowledged.error);
    let drift = acknowledged.data.unwrap();
    assert!(drift.stale_sections.iter().all(|s| s.ai_file != AGENTS));
    assert!(!stale(fx.project()).contains(&AGENTS.to_string()));
    assert_eq!(docs_snapshot(fx.project()).len(), docs_before.len());
    for (path, bytes) in &docs_before {
        if path != "docs/checksums.json" && !path.ends_with(".json") {
            assert_eq!(docs_snapshot(fx.project()).get(path), Some(bytes), "{path}");
        }
    }
    // The lease is released once the acknowledgement is written.
    assert!(fx
        .h
        .state
        .audit_tracker
        .lock()
        .unwrap()
        .try_acquire_lease(PROJECT_ID));
    fx.h.state
        .audit_tracker
        .lock()
        .unwrap()
        .release_lease(PROJECT_ID);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn verified_unchanged_also_settles_a_section_judged_by_its_citations() {
    let fx = Validated::new(|_| {}).await;
    fx.edit_source(|lines| lines[2] = "fn renamed_error_mapper() {}".into());
    assert!(stale(fx.project()).contains(&RULES.to_string()));
    let before = std::fs::read(fx.project().join(RULES)).unwrap();

    let acknowledged = acknowledge(&fx, &[RULES], false).await;
    assert!(acknowledged.success, "{:?}", acknowledged.error);
    assert!(!stale(fx.project()).contains(&RULES.to_string()));
    assert_eq!(std::fs::read(fx.project().join(RULES)).unwrap(), before);
    assert_eq!(
        fx.record_of(RULES).reason,
        crate::core::citation_drift::RecordReason::Acknowledged
    );
}
