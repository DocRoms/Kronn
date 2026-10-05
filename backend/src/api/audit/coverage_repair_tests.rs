//! Real Full-audit recovery through HTTP tools; malformed coverage must stay
//! blocking while the model gets bounded feedback and can preserve prior work.
use super::*;
use axum::response::IntoResponse;
use sha2::{Digest, Sha256};

const TARGET: &str = "docs/inconsistencies-tech-debt.md";
const HUMAN: &str = "<!-- kronn:section name=\"decisions\" owner=\"human\" -->\nHuman decision stays unchanged.\n<!-- kronn:section:end -->\n";

fn matrix(valid: bool) -> String {
    let mut content = format!("# Debt\n\n{HUMAN}\n{}\n## Dimension coverage\n\n| Dimension | Outcome | Evidence / reason |\n|---|---|---|\n", "Established project evidence.\n".repeat(200));
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
        if dimension == "Accessibility" && !valid {
            content.push_str("| Accessibility | N/A: backend has no UI |\n");
        } else {
            content.push_str(&format!("| {dimension} | scanned — nothing substantiable | Reviewed the repository sources |\n"));
        }
    }
    content
}

struct TemplatesEnv(Option<std::ffi::OsString>);
impl Drop for TemplatesEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("KRONN_TEMPLATES_DIR", value),
            None => std::env::remove_var("KRONN_TEMPLATES_DIR"),
        }
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn full_resume_repairs_coverage_with_bounded_feedback_and_preserves_prior_work() {
    // 0 repairs its second attempt, 1 never repairs, 2 returns 429 after writing.
    for scenario in 0..3 {
        let templates = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(templates.path().join("docs")).unwrap();
        std::fs::write(
            templates.path().join(TARGET),
            "Template baseline.\n".repeat(100),
        )
        .unwrap();
        let _restore = TemplatesEnv(std::env::var_os("KRONN_TEMPLATES_DIR"));
        std::env::set_var("KRONN_TEMPLATES_DIR", templates.path());
        let project = tempfile::tempdir().unwrap();
        let initialized = crate::core::cmd::git_cmd()
            .args(["init", "--quiet"])
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        std::fs::write(project.path().join("README.md"), "# Coverage fixture\n").unwrap();
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
            format!("# Project\n{}", "Established documentation.\n".repeat(100)),
        )
        .unwrap();
        std::fs::write(project.path().join(TARGET), matrix(false)).unwrap();
        std::fs::write(
            project.path().join("docs/tech-debt/TD-preserved.md"),
            "# Prior finding\nPreserve this work.\n",
        )
        .unwrap();
        let server = MockServer::start().await;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let attempts = Arc::new(Mutex::new(0usize));
        let (seen, launched) = (requests.clone(), attempts.clone());
        let target = project.path().join(TARGET);
        Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                // Full audit starts a detached validation discussion. Its calls
                // must neither mutate the fixture nor count as audit retries.
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
                if after_tool && scenario == 2 {
                    return ResponseTemplate::new(429).set_body_string("rate limit");
                }
                let frame = if after_tool {
                    text("The index is ready.")
                } else {
                    let mut attempt = launched.lock().unwrap();
                    *attempt += 1;
                    let digest: String = Sha256::digest(std::fs::read(&target).unwrap())
                        .iter().map(|byte| format!("{byte:02x}")).collect();
                    tool_calls(&[
                        ("read_file", json!({"path":TARGET})),
                        ("write_file", json!({"path":TARGET,"expected_sha256":digest,"content":matrix(scenario == 0 && *attempt > 1)})),
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
                runs::insert_running(conn, "before-coverage", PROJECT_ID, "Full", "LiteLlm", now)?;
                for step in 1..=total {
                    runs::insert_audit_step_start(conn, "before-coverage", step, "doc", now)?;
                    runs::finalize_audit_step(
                        conn,
                        "before-coverage",
                        step,
                        now,
                        10,
                        &runs::StepTokens::UNKNOWN,
                        None,
                        step != 8,
                        None,
                        false,
                    )?;
                }
                runs::update_last_completed_step(conn, "before-coverage", total - 1)?;
                runs::mark_interrupted(conn, "before-coverage", "coverage incomplete")
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
                    resume_run_id: Some("before-coverage".into()),
                }),
            )
            .await
            .into_response(),
        )
        .await;
        let expected_attempts = [2, 3, 1][scenario];
        assert_eq!(
            *attempts.lock().unwrap(),
            expected_attempts,
            "scenario {scenario}: {stream}"
        );
        assert_eq!(
            requests.lock().unwrap().len(),
            expected_attempts * 2,
            "never replay a failed provider request"
        );
        assert_eq!(
            sse_events(&stream, "step_retry").len(),
            expected_attempts - 1
        );
        let first_prompt = requests.lock().unwrap()[0].to_string();
        assert!(
            first_prompt.contains("evidence/reason cell is empty"),
            "resume carries the recomputed cause"
        );
        let done = sse_events(&stream, "done").pop().unwrap();
        assert_eq!(
            done["status"],
            if scenario == 0 {
                "complete"
            } else {
                "interrupted"
            },
            "{stream}"
        );
        let run_id = done["audit_run_id"].as_str().unwrap().to_owned();
        let steps = state
            .db
            .with_conn(move |conn| crate::db::audit_runs::list_audit_steps(conn, &run_id))
            .await
            .unwrap();
        let step = steps.iter().find(|step| step.step_index == 8).unwrap();
        assert_eq!(step.cli_success, scenario == 0);
        assert_eq!(
            step.step_tokens,
            Some(if scenario == 2 {
                18
            } else {
                expected_attempts as u64 * 36
            })
        );
        if scenario == 2 {
            assert!(step.step_warning.as_deref().unwrap().contains("429"));
        }
        let index = std::fs::read_to_string(project.path().join(TARGET)).unwrap();
        assert!(index.contains(HUMAN));
        assert_eq!(
            std::fs::read_to_string(project.path().join("docs/tech-debt/TD-preserved.md")).unwrap(),
            "# Prior finding\nPreserve this work.\n"
        );
    }
}
