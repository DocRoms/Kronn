//! KT-1323 — the detector disposition gate gets the same bounded retry as every
//! other Step-8 gate, with feedback naming the signals; signals in generated
//! or vendored files do not force a TD. Real Full-audit resume through HTTP tools.
use super::*;
use axum::response::IntoResponse;
use sha2::{Digest, Sha256};

const TARGET: &str = "docs/inconsistencies-tech-debt.md";
const BLANK: &str = "<a href=\"https://example.org\" target=\"_blank\">out</a>\n";

fn index(disposed: &str) -> String {
    let mut content = format!(
        "# Debt\n\n{}\n## Dimension coverage\n\n| Dimension | Outcome | Evidence / reason |\n|---|---|---|\n",
        "Established project evidence.\n".repeat(200)
    );
    for dimension in [
        "Dependencies",
        "Security",
        "Code quality",
        "Scalability",
        "Maintainability",
        "Accessibility",
        "Observability",
        "Compliance",
        "Performance",
        "Documentation drift",
    ] {
        let evidence = if dimension == "Dependencies" {
            "Reviewed the manifests; source used: dependency watch dated 2026-10-04"
        } else {
            "Reviewed the repository sources"
        };
        content.push_str(&format!(
            "| {dimension} | scanned — nothing substantiable | {evidence} |\n"
        ));
    }
    content.push_str(&format!("\n{disposed}\n"));
    content
}

struct TemplatesEnv(Option<std::ffi::OsString>);
impl Drop for TemplatesEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", value),
            None => crate::core::child_env::remove_var("KRONN_TEMPLATES_DIR"),
        }
    }
}

/// Keywords that dispose of the community-file signals the fixture raises, so
/// a run only fails on the file under test.
const OTHER_ANCHORS: &str = "security readme";

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn detector_gate_retries_with_feedback_and_skips_generated_files() {
    // 0: repaired on the 2nd attempt; 1: never repaired (fails after the
    // budget); 2: the only `_blank` hits are in generated/scaffolding files.
    for scenario in 0..3 {
        let templates = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(templates.path().join("docs")).unwrap();
        std::fs::write(
            templates.path().join(TARGET),
            "Template baseline.\n".repeat(100),
        )
        .unwrap();
        let _restore = TemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
        crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", templates.path());
        let project = tempfile::tempdir().unwrap();
        let initialized = crate::core::cmd::git_cmd()
            .args(["init", "--quiet"])
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(initialized.status.success());
        std::fs::write(project.path().join("README.md"), "# Detector fixture\n").unwrap();
        std::fs::create_dir_all(project.path().join("docs/tech-debt")).unwrap();
        std::fs::create_dir_all(project.path().join("docs/conventions")).unwrap();
        std::fs::write(
            project.path().join("docs/conventions/agents-md-format-v1.md"),
            "# Format\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("docs/AGENTS.md"),
            format!("# Project\n{}", "Established documentation.\n".repeat(100)),
        )
        .unwrap();
        let hit = if scenario == 2 {
            "sass/generated/icons.html"
        } else {
            "src/page.html"
        };
        std::fs::create_dir_all(project.path().join(hit).parent().unwrap()).unwrap();
        std::fs::write(project.path().join(hit), BLANK).unwrap();
        if scenario == 2 {
            std::fs::create_dir_all(project.path().join("web")).unwrap();
            std::fs::write(project.path().join("web/config.php"), "<?php // installer\n").unwrap();
        }
        std::fs::write(project.path().join(TARGET), index(OTHER_ANCHORS)).unwrap();
        let server = MockServer::builder().start().await;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let attempts = Arc::new(Mutex::new(0usize));
        let (seen, launched) = (requests.clone(), attempts.clone());
        let target = project.path().join(TARGET);
        Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let is_audit = body["messages"].as_array().unwrap().iter().any(|message| {
                    message["content"].as_str().is_some_and(|content|
                        content.contains(crate::api::audit::PROMPT_PREAMBLE))
                });
                if !is_audit {
                    return ResponseTemplate::new(200).set_body_string(sse(&[
                        text("Validation fixture complete."),
                    ]));
                }
                let after_tool = body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
                seen.lock().unwrap().push(body);
                let frame = if after_tool {
                    text("The index is ready.")
                } else {
                    let mut attempt = launched.lock().unwrap();
                    *attempt += 1;
                    let digest: String = Sha256::digest(std::fs::read(&target).unwrap())
                        .iter().map(|byte| format!("{byte:02x}")).collect();
                    let disposed = if scenario == 0 && *attempt > 1 {
                        format!("{OTHER_ANCHORS} src/page.html")
                    } else {
                        OTHER_ANCHORS.to_string()
                    };
                    tool_calls(&[
                        ("read_file", json!({"path":TARGET})),
                        ("write_file", json!({"path":TARGET,"expected_sha256":digest,"content":index(&disposed)})),
                    ])
                };
                ResponseTemplate::new(200).set_body_string(sse(&[frame,
                    json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}).to_string()]))
            }).mount(&server).await;
        let state = litellm_state(&server.uri()).await;
        project_among_others(&state, project.path(), false).await;
        let total =
            crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full).len() as u32;
        state
            .db
            .with_conn(move |conn| {
                use crate::db::audit_runs as runs;
                let now = chrono::Utc::now() - chrono::Duration::hours(1);
                runs::insert_running(conn, "before-detector", PROJECT_ID, "Full", "LiteLlm", now)?;
                for step in 1..=total {
                    runs::insert_audit_step_start(conn, "before-detector", step, "doc", now)?;
                    runs::finalize_audit_step(
                        conn, "before-detector", step, now, 10,
                        &runs::StepTokens::UNKNOWN, None, step != 8, None, false,
                    )?;
                }
                runs::update_last_completed_step(conn, "before-detector", total - 1)?;
                runs::mark_interrupted(conn, "before-detector", "detector signals undisposed")
            })
            .await
            .unwrap();
        let stream = sse_body(
            crate::api::audit::full::full_audit(
                axum::extract::State(state.clone()),
                axum::extract::Path(PROJECT_ID.into()),
                axum::Json(crate::models::LaunchAuditRequest {
                    agent: AgentType::LiteLlm,
                    connection_id: None,
                    tier: Some(ModelTier::Reasoning),
                    kind: None,
                    custom_prompt: None,
                    resume_run_id: Some("before-detector".into()),
                    retry_steps: None,
                    redo_steps: None,
                    fresh: None,
                }),
            )
            .await
            .into_response(),
        )
        .await;
        let expected_attempts = [2, 3, 1][scenario];
        assert_eq!(*attempts.lock().unwrap(), expected_attempts, "scenario {scenario}: {stream}");
        let retries = sse_events(&stream, "step_retry");
        assert_eq!(retries.len(), expected_attempts - 1, "scenario {scenario}: {stream}");
        assert!(retries.iter().all(|r| r["reason"] == "detector_disposition"));
        if scenario < 2 {
            // The retry prompt names the signal still to dispose of.
            let second = requests.lock().unwrap()[2].to_string();
            assert!(second.contains("Detector signals"), "{second}");
            assert!(second.contains("src/page.html"), "{second}");
        } else {
            let first = requests.lock().unwrap()[0].to_string();
            assert!(!first.contains("sass/generated/icons.html"), "generated file not flagged");
            assert!(!first.contains("web/config.php"), "scaffolding not flagged");
        }
        let done = sse_events(&stream, "done").pop().unwrap();
        let run_id = done["audit_run_id"].as_str().unwrap().to_owned();
        let steps = state
            .db
            .with_conn(move |conn| crate::db::audit_runs::list_audit_steps(conn, &run_id))
            .await
            .unwrap();
        let step = steps.iter().find(|step| step.step_index == 8).unwrap();
        if scenario == 1 {
            assert!(!step.cli_success, "{stream}");
            assert!(
                step.step_warning
                    .as_deref()
                    .unwrap()
                    .contains(crate::api::audit::step_guard::DETECTOR_FRAGMENT),
                "{:?}",
                step.step_warning
            );
            assert_eq!(done["status"], "interrupted");
        } else {
            assert!(step.cli_success, "scenario {scenario}: {stream}");
            assert_eq!(done["status"], "complete", "{stream}");
        }
    }
}
