//! KT-999 — the existing-documentation block through the real Full and partial
//! pipelines, on the scripted agent: the pipelines read the setting and hand the
//! block to the steps that use it (and only to them), and the step prompt still
//! ends on its closing directive.
use super::*;
use crate::core::docs_location::DocsRoot;
use crate::models::{BriefingDocLocation, BriefingDocState, BriefingDocsAnswers};

const BLOCK: &str = "## Existing documentation (from the briefing)";
const STEP_8: &str = "docs/inconsistencies-tech-debt.md";

/// Replace the project's briefing with one that lists an obsolete and a
/// reference location.
async fn document_the_briefing(h: &Harness) {
    let location = |path: &str, state, date: &str| BriefingDocLocation {
        path: path.into(),
        state,
        last_commit: Some(date.into()),
        ..BriefingDocLocation::default()
    };
    let section = crate::api::audit::briefing_docs::render_section(&BriefingDocsAnswers {
        locations: vec![
            location("CHANGELOG.md", BriefingDocState::Obsolete, "2021-04-09"),
            location("apidoc", BriefingDocState::Reference, "2026-09-30"),
        ],
    })
    .unwrap();
    let notes = format!("{BRIEFING_MARKER}\n\n{section}");
    h.state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, PROJECT_ID, Some(&notes))
        })
        .await
        .unwrap();
}

async fn enable_the_setting(h: &Harness, on: bool) {
    h.state.config.write().await.audit_briefing_docs = on;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn the_full_pipeline_hands_the_block_to_steps_1_and_8_only_when_the_setting_is_on() {
    let _restore = crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&[
        crate::api::audit::briefing_docs::ENV_SWITCH,
    ]);
    crate::core::child_env::remove_var(crate::api::audit::briefing_docs::ENV_SWITCH);
    let h = harness().await;
    document_the_briefing(&h).await;
    enable_the_setting(&h, true).await;
    let root = DocsRoot::default_root();
    let steps = crate::api::audit::steps_for(crate::models::AuditKind::Full, &root);
    let prompts = audited_with(&h, &root).await;
    assert_eq!(prompts.len(), steps.len());

    let mut reached = Vec::new();
    for (index, (prompt, step)) in prompts.iter().zip(&steps).enumerate() {
        if prompt.contains(BLOCK) {
            reached.push(step.target_file);
        }
        // The closing directive stays last, after the block.
        let directive = crate::api::audit::step_closing_directive(index + 1, step, &root);
        assert!(prompt.ends_with(&directive), "step {}", index + 1);
    }
    assert_eq!(
        reached,
        vec!["docs/AGENTS.md", STEP_8, "docs/inconsistencies-api.md"]
    );
    let debt = &prompts[steps.iter().position(|s| s.target_file == STEP_8).unwrap()];
    assert!(
        debt.contains("`CHANGELOG.md` — documentation folder; obsolete; last commit 2021-04-09"),
        "{debt}"
    );
    assert!(debt.contains("never list it under `## Reconciliation candidates`"));
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn the_full_pipeline_keeps_the_block_out_when_the_setting_is_off() {
    let _restore = crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&[
        crate::api::audit::briefing_docs::ENV_SWITCH,
    ]);
    crate::core::child_env::remove_var(crate::api::audit::briefing_docs::ENV_SWITCH);
    let h = harness().await;
    document_the_briefing(&h).await;
    let root = DocsRoot::default_root();
    let prompts = audited_with(&h, &root).await;
    assert!(!prompts.is_empty());
    for prompt in &prompts {
        assert!(!prompt.contains("Existing documentation"), "{prompt}");
        assert!(!prompt.contains("CHANGELOG.md"), "{prompt}");
    }
}

/// A partial refresh of step 8 under `setting`/`env`, and the prompt it sent.
async fn partial_step_8_prompt(setting: bool, env: bool) -> String {
    let _restore = crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&[
        crate::api::audit::briefing_docs::ENV_SWITCH,
    ]);
    if env {
        crate::core::child_env::set_var(crate::api::audit::briefing_docs::ENV_SWITCH, "1");
    } else {
        crate::core::child_env::remove_var(crate::api::audit::briefing_docs::ENV_SWITCH);
    }
    let h = harness().await;
    document_the_briefing(&h).await;
    enable_the_setting(&h, setting).await;
    let root = DocsRoot::default_root();
    let steps = crate::api::audit::steps_for(crate::models::AuditKind::Full, &root);
    let step = steps.iter().position(|s| s.target_file == STEP_8).unwrap() + 1;
    arm(h.tools.path(), "fill", &[STEP_8]);
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![step],
            accept_unverified_agents: None,
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&h.state, &done).await;
    assert_eq!(started_steps(&body), vec![step as u64], "{body}");
    let prompt = std::fs::read_to_string(h.tools.path().join("prompt.1")).unwrap();
    let directive = crate::api::audit::step_closing_directive(step, &steps[step - 1], &root);
    assert!(
        prompt.ends_with(&directive),
        "…{}",
        &prompt[prompt.len().saturating_sub(400)..]
    );
    prompt
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn the_partial_pipeline_reads_the_setting_and_the_bench_switch() {
    let off = partial_step_8_prompt(false, false).await;
    assert!(
        !off.contains("Existing documentation") && !off.contains("CHANGELOG.md"),
        "{off}"
    );

    for (setting, env) in [(true, false), (false, true)] {
        let on = partial_step_8_prompt(setting, env).await;
        assert!(on.contains(BLOCK), "setting {setting} env {env}:\n{on}");
        assert!(
            on.contains("`apidoc` — documentation folder; reference;"),
            "{on}"
        );
        assert!(
            on.contains("whatever the scope or the dimension of this audit"),
            "{on}"
        );
    }
}

/// A Full audit filled by the scripted agent; the step prompts in order.
async fn audited_with(h: &Harness, root: &DocsRoot) -> Vec<String> {
    let targets: Vec<&str> = crate::api::audit::steps_for(crate::models::AuditKind::Full, root)
        .iter()
        .map(|s| s.target_file)
        .collect();
    arm(h.tools.path(), "fill", &targets);
    settle_on_validation(h.tools.path());
    let body = run_full(&h.state, None).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("no terminal done: {body}"));
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");
    (1..=targets.len())
        .filter_map(|n| std::fs::read_to_string(h.tools.path().join(format!("prompt.{n}"))).ok())
        .collect()
}
