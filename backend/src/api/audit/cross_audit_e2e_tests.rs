//! KT-1183 — the cross-agent audit through the real `full_audit` pipeline, the
//! real launcher and runner, on a scripted agent standing in for every CLI
//! (no socket). The same pipeline without the option stays single-agent.
use super::*;
use axum::response::IntoResponse;
use std::path::PathBuf;

const SECURITY_INDEX: &str = "docs/inconsistencies-security.md";
const SINGLE_AGENT_INDEX: &str = "# Security findings\n\n| ID | Problem | Area | Severity |\n|----|---------|------|----------|\n| TD-20261011-single | Written by the single agent | src/db.rs | High |\n";

struct TemplatesEnv(Option<std::ffi::OsString>);
impl Drop for TemplatesEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", value),
            None => crate::core::child_env::remove_var("KRONN_TEMPLATES_DIR"),
        }
    }
}

/// One script for every agent: it reads which agent it is from the prompt,
/// answers each cross-agent role from `reply.<role>.<agent>`, and in
/// detection waits until every agent has started (sequential calls would
/// never meet). A plain step prompt writes the single-agent index.
fn scripted_agents(tools: &Path, project: &Path) -> PathBuf {
    let s = tools.display();
    let p = project.display();
    let script = format!(
        r#"prompt=$(cat)
agent=$(printf '%s' "$prompt" | grep -o 'You are agent `[A-Za-z]*`' | head -n 1 | sed 's/.*`\(.*\)`/\1/')
case "$prompt" in
  *"DETECTION MODE"*)
    printf '%s' "$prompt" > '{s}/det.'"$agent"
    touch '{s}/started.'"$agent"
    i=0
    while [ $i -lt 150 ]; do
      n=$(ls '{s}'/started.* 2>/dev/null | wc -l)
      [ "$n" -ge "$(cat '{s}/agents')" ] && break
      sleep 0.1
      i=$((i+1))
    done
    [ "$n" -ge "$(cat '{s}/agents')" ] || touch '{s}/sequential.'"$agent"
    cat '{s}/reply.det.'"$agent"
    exit 0
    ;;
  *"RIGHT OF REPLY"*)
    printf '%s' "$prompt" >> '{s}/rep.'"$agent"
    cat '{s}/reply.rep.'"$agent"
    exit 0
    ;;
  *"CROSS-CHECK. You are agent"*)
    printf '%s' "$prompt" >> '{s}/chk.'"$agent"
    cat '{s}/reply.chk.'"$agent"
    exit 0
    ;;
  *{BRIEFING_MARKER}*)
    echo step >> '{s}/single.count'
    printf '%s' "$prompt" > '{s}/single.prompt'
    mkdir -p '{p}/docs'
    cp '{s}/single.md' '{p}/{SECURITY_INDEX}'
    ;;
  *)
    printf '%s\n' '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"KRONN:VALIDATION_COMPLETE"}}}}}}'
    printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":1}}}}'
    exit 0
    ;;
esac
printf '%s\n' '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"Done."}}}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":1}}}}'"#
    );
    crate::acp::test_support::write_fixture_script(tools, &script)
}

/// What a scripted agent prints to answer `text`.
fn stream(text: &str) -> String {
    format!(
        "{}\n{}\n",
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}}),
        json!({"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"output_tokens":5}})
    )
}

fn reply(tools: &Path, role: &str, agent: &str, text: String) {
    std::fs::write(tools.join(format!("reply.{role}.{agent}")), stream(&text)).unwrap();
}

struct Harness {
    tools: tempfile::TempDir,
    state: AppState,
    project: tempfile::TempDir,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
    _agent_routes: Vec<crate::agents::runner::test_acp_routes::AgentRouteGuard>,
    _restore: TemplatesEnv,
}

async fn harness(briefing: &str) -> Harness {
    let restore = TemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
    crate::core::child_env::set_var(
        "KRONN_TEMPLATES_DIR",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates"),
    );
    let tools = tempfile::tempdir().unwrap();
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;
    let notes = briefing.to_string();
    state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, PROJECT_ID, Some(&notes))
        })
        .await
        .unwrap();
    let initialized = crate::core::cmd::git_cmd()
        .args(["init", "--quiet"])
        .current_dir(project.path())
        .output()
        .unwrap();
    assert!(initialized.status.success());
    std::fs::write(project.path().join("README.md"), "# Fixture\n").unwrap();
    std::fs::create_dir_all(project.path().join("src")).unwrap();
    std::fs::write(
        project.path().join("src/db.rs"),
        "fn a() {}\nfn b() {}\nlet q = format!(\"SELECT * FROM t WHERE id = {}\", id);\nconn.execute(&q);\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join("src/guard.rs"),
        "pub fn check(id: &str) -> bool {\n    id.chars().all(|c| c.is_ascii_digit())\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join("src/web.rs"),
        "let cors = \"*\";\nallow_origin(cors);\n",
    )
    .unwrap();
    std::fs::write(tools.path().join("single.md"), SINGLE_AGENT_INDEX).unwrap();
    let fixture = scripted_agents(tools.path(), project.path());
    let route = route_claude(project.path(), &fixture);
    // One adapter per agent: the detections run at the same time.
    let project_path = project.path().to_string_lossy().into_owned();
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&project_path), &project_path).unwrap();
    let agent_routes = [AgentType::ClaudeCode, AgentType::Codex]
        .iter()
        .map(|agent| {
            crate::agents::runner::test_acp_routes::route_agent(
                &work_dir,
                agent,
                std::sync::Arc::new(crate::acp::ClaudeAcpAdapter::new_with_program(
                    fixture.to_string_lossy(),
                    None,
                    false,
                )),
            )
        })
        .collect();
    Harness {
        tools,
        state,
        project,
        _route: route,
        _agent_routes: agent_routes,
        _restore: restore,
    }
}

async fn run_security(state: &AppState) -> String {
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            kind: Some(crate::models::AuditKind::Security),
            custom_prompt: None,
            resume_run_id: None,
            retry_steps: None,
            redo_steps: None,
            fresh: None,
        }),
    )
    .await
    .into_response();
    sse_body(response).await
}

/// The validation agent runs detached through the same script.
async fn validation_settled(state: &AppState, done: &Value) {
    let Some(id) = done["discussion_id"].as_str().map(str::to_string) else {
        return;
    };
    for _ in 0..600 {
        let id = id.clone();
        let answered = state
            .db
            .with_conn(move |conn| {
                Ok(crate::db::discussions::get_discussion(conn, &id)?
                    .is_some_and(|d| d.messages.len() >= 2))
            })
            .await
            .unwrap();
        if answered {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the validation agent of discussion {id} never answered");
}

fn finding(title: &str, file: &str, line: u32, quote: &str) -> Value {
    json!({"title": title, "category": "security", "severity": "High", "file": file, "line": line,
           "quote": quote, "impact": "impact", "remediation": "fix"})
}

fn block(fence: &str, body: Value) -> String {
    format!("Done.\n```{fence}\n{body}\n```\n")
}

const SQL: &str = "SQL query built with format";
const CORS: &str = "CORS allows every origin";
const INVENTED: &str = "Hardcoded password in config";

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_cross_agent_security_step_runs_end_to_end() {
    let briefing = format!(
        "{BRIEFING_MARKER}\n\n## Kronn options\n{}\n",
        crate::core::cross_audit::briefing_option_line(&[AgentType::ClaudeCode, AgentType::Codex])
    );
    let h = harness(&briefing).await;
    let tools = h.tools.path();
    std::fs::write(tools.join("agents"), "2").unwrap();
    let sql = finding(SQL, "src/db.rs", 3, "format!(\"SELECT * FROM t");
    reply(
        tools,
        "det",
        "ClaudeCode",
        block(
            "kronn-findings",
            json!({"findings": [sql.clone(), finding(INVENTED, "src/config.rs", 7, "password")]}),
        ),
    );
    reply(
        tools,
        "det",
        "Codex",
        block(
            "kronn-findings",
            json!({"findings": [sql, finding(CORS, "src/web.rs", 1, "let cors = \"*\"")]}),
        ),
    );
    // Codex checks the SQL finding (everyone found it) without proof; Claude
    // refutes the CORS one with a proof that verifies, and Codex withdraws.
    reply(
        tools,
        "chk",
        "Codex",
        block(
            "kronn-verdicts",
            json!({"verdicts": [{"id": "F1", "verdict": "refute", "argument": "trust me", "proof": []}]}),
        ),
    );
    reply(
        tools,
        "chk",
        "ClaudeCode",
        block(
            "kronn-verdicts",
            json!({"verdicts": [{"id": "F2", "verdict": "refute", "argument": "digits only",
        "proof": [{"file": "src/guard.rs", "line": 2, "quote": "is_ascii_digit"}]}]}),
        ),
    );
    reply(
        tools,
        "rep",
        "Codex",
        block(
            "kronn-replies",
            json!({"replies": [{"id": "F2", "stance": "withdraw", "argument": "fair"}]}),
        ),
    );

    let body = run_security(&h.state).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("{body}"));
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");

    // Detection in parallel, each agent on its own prompt.
    assert!(
        !tools.join("sequential.ClaudeCode").exists() && !tools.join("sequential.Codex").exists()
    );
    let claude_prompt = std::fs::read_to_string(tools.join("det.ClaudeCode")).unwrap();
    let codex_prompt = std::fs::read_to_string(tools.join("det.Codex")).unwrap();
    assert!(!claude_prompt.contains(CORS) && !codex_prompt.contains(INVENTED));
    // KT-1278 — the prompt the detection agent received ends on its own no-write
    // closing, never on the step's "write the file" directive.
    for prompt in [&claude_prompt, &codex_prompt] {
        assert!(
            prompt.ends_with(crate::core::cross_audit::DETECTION_CLOSING),
            "…{}",
            &prompt[prompt.len().saturating_sub(500)..]
        );
        assert!(
            !prompt.contains("=== RUN THIS STEP NOW ==="),
            "a detection pass must not be told to write the target: {prompt}"
        );
        assert!(
            prompt.find("DETECTION MODE").unwrap()
                < prompt.rfind("=== RUN THIS DETECTION NOW ===").unwrap()
        );
    }
    assert!(
        !tools.join("single.count").exists(),
        "the single-agent step never ran"
    );
    // The false citation never reached a challenger.
    let checks = std::fs::read_to_string(tools.join("chk.ClaudeCode")).unwrap()
        + &std::fs::read_to_string(tools.join("chk.Codex")).unwrap();
    assert!(!checks.contains(INVENTED));

    let summary = sse_events(&body, "cross_audit")
        .pop()
        .unwrap_or_else(|| panic!("{body}"));
    assert_eq!(summary["dropped"], 1, "{summary}");
    assert_eq!(summary["contested"], 1, "{summary}");
    assert_eq!(summary["dismissed"], 1, "{summary}");
    assert_eq!(summary["calls"], 5, "{summary}");
    assert_eq!(sse_events(&body, "cross_audit_start").len(), 1);
    let step_done = sse_events(&body, "step_done").pop().unwrap();
    assert_eq!(step_done["success"], true, "{body}");
    assert_eq!(step_done["cross_audit"], true);

    let index = std::fs::read_to_string(h.project.path().join(SECURITY_INDEX)).unwrap();
    let table = index.split("## Cross-check verdicts").next().unwrap();
    assert!(
        table.contains(SQL) && table.contains("(contested)"),
        "{index}"
    );
    assert!(!table.contains(CORS), "{index}");
    assert!(index
        .split("## Dropped by the citation check")
        .nth(1)
        .unwrap()
        .contains(INVENTED));

    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let rows = h
        .state
        .db
        .with_conn(move |conn| crate::db::audit_cross::list_findings(conn, &run_id))
        .await
        .unwrap();
    let sql_row = &rows.iter().find(|(_, r)| r.title == SQL).unwrap().1;
    assert_eq!(sql_row.detected_by, vec!["ClaudeCode", "Codex"]);
    assert_eq!(
        sql_row.outcome,
        crate::core::cross_audit::Outcome::Contested
    );
    let cors_row = &rows.iter().find(|(_, r)| r.title == CORS).unwrap().1;
    assert_eq!(
        cors_row.outcome,
        crate::core::cross_audit::Outcome::Dismissed
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn without_the_option_the_step_stays_single_agent_byte_for_byte() {
    let h = harness(BRIEFING_MARKER).await;
    let tools = h.tools.path();
    let body = run_security(&h.state).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("{body}"));
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");

    assert_eq!(
        std::fs::read_to_string(tools.join("single.count")).unwrap(),
        "step\n"
    );
    let prompt = std::fs::read_to_string(tools.join("single.prompt")).unwrap();
    assert!(
        !prompt.contains("CROSS-AGENT") && !prompt.contains("kronn-findings"),
        "{prompt}"
    );
    assert!(!tools.join("det.ClaudeCode").exists());
    assert!(
        sse_events(&body, "cross_audit_start").is_empty()
            && sse_events(&body, "cross_audit").is_empty()
    );
    assert!(sse_events(&body, "step_done")
        .iter()
        .all(|e| e.get("cross_audit").is_none()));
    // The agent's own document, untouched by Kronn.
    assert_eq!(
        std::fs::read(h.project.path().join(SECURITY_INDEX)).unwrap(),
        SINGLE_AGENT_INDEX.as_bytes()
    );
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let rows = h
        .state
        .db
        .with_conn(move |conn| {
            Ok((
                crate::db::audit_cross::list_findings(conn, &run_id)?.len(),
                conn.query_row("SELECT COUNT(*) FROM audit_cross_steps", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(rows, (0, 0));
}
