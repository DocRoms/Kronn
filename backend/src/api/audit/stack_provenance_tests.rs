//! KT-1308 — in the default (warn) mode, the documentary check refuses a
//! `[src: deps: …]` row the dependency watch does not hold and a
//! `[src: url: …]` page the run never fetched, and accepts `not verified`.
//! Real Full pipeline through the scripted `claude` of the parent module.
use super::*;

/// The stack table the scripted agent appends to the project index.
fn stack_table(date: &str, node: &str, jest: &str) -> String {
    format!(
        "\n## Stack currency probe\n\n| Tool | Latest stable | Support end |\n|---|---|---|\n\
         | Node | {node} | not verified: no source |\n\
         | Jest | {jest} | not verified: no source |\n\
         | Date | {date} | not verified: no source |\n"
    )
}

/// The tool call of an agent that fetched `url`, as Claude reports it: the
/// ACP adapter (the default route) reads the complete input of an assistant
/// message.
fn fetch_stream(url: &str) -> String {
    let call = json!({"type": "tool_use", "id": "t1", "name": "WebFetch",
        "input": {"url": url, "prompt": "latest stable"}});
    json!({"type": "assistant", "message": {"content": [call]}}).to_string() + "\n"
}

/// One stream-json line of an assistant message holding `call`.
fn assistant_line(call: Value) -> String {
    json!({"type": "assistant", "message": {"content": [call]}}).to_string() + "\n"
}

/// A Context7 lookup and the result it sent back, naming the page it read.
fn context7_stream(result: &str) -> String {
    let call = json!({"type": "tool_use", "id": "c7", "name": "mcp__context7__query-docs",
        "input": {"libraryId": "/facebook/jest", "query": "latest version"}});
    let result = json!({"type": "tool_result", "tool_use_id": "c7",
        "content": [{"type": "text", "text": result}]});
    assistant_line(call)
        + &(json!({"type": "user", "message": {"content": [result]}}).to_string() + "\n")
}

/// The normal Context7 flow: `resolve-library-id` (its result names a library,
/// no page), then `query-docs` (its result names the pages it read).
fn context7_resolve_then_query_stream(result: &str) -> String {
    let resolve = json!({"type": "tool_use", "id": "c7r",
        "name": "mcp__context7__resolve-library-id",
        "input": {"libraryName": "jest", "query": "latest version"}});
    let resolved = json!({"type": "tool_result", "tool_use_id": "c7r",
        "content": [{"type": "text", "text": "- Title: Jest\n- Context7-compatible library ID: /facebook/jest\n- Code Snippets: 120"}]});
    assistant_line(resolve)
        + &(json!({"type": "user", "message": {"content": [resolved]}}).to_string() + "\n")
        + &context7_stream(result)
}

/// The dependency watch the audit injects: `joi` and `jest` of the `JS / TS` manager.
async fn watch_dependencies(state: &AppState) {
    let package = |name: &str| crate::models::DependencyUpdatePackage {
        name: name.into(),
        current: "29.0.0".into(),
        latest: "30.0.0".into(),
        major: true,
        declared_only: false,
        lockfile: None,
    };
    let summary = crate::models::DependencyUpdateSummary {
        managers: vec![crate::models::DependencyManagerUpdate {
            manager: "JS / TS".into(),
            manifest: "package.json".into(),
            status: crate::models::DependencyCheckStatus::UpdatesAvailable,
            outdated: 2,
            major: 2,
            reason: None,
            packages: vec![package("joi"), package("jest")],
        }],
        total_outdated: 2,
        total_major: 2,
        checked_at: chrono::Utc::now(),
        cached: false,
        monitoring_interval_days: Some(7),
        next_check_at: None,
    };
    state
        .db
        .with_conn(move |conn| {
            crate::db::projects::save_dependency_scan(conn, PROJECT_ID, 1, &summary).map(|_| ())
        })
        .await
        .unwrap();
}

/// What the project index's step (1) emitted: its retries, warnings and outcome.
struct IndexStep {
    retries: Vec<Value>,
    warnings: Vec<Value>,
    success: bool,
}

async fn index_step_with_table(
    h: &Harness,
    table: String,
    fetched: Option<&str>,
    watch: bool,
) -> IndexStep {
    index_step_with_stream(h, table, fetched.map(fetch_stream), watch).await
}

/// `stream`: the tool lines the scripted agent emits besides its writing.
async fn index_step_with_stream(
    h: &Harness,
    table: String,
    stream: Option<String>,
    watch: bool,
) -> IndexStep {
    let (tools, state) = (h.tools.path(), &h.state);
    assert_eq!(
        crate::core::anti_halluc::current_mode(),
        crate::core::anti_halluc::AntiHallucMode::Warn,
        "the point of KT-1308 is the default mode"
    );
    if watch {
        watch_dependencies(state).await;
    }
    std::fs::write(tools.join("stack-table"), table).unwrap();
    if let Some(stream) = stream {
        std::fs::write(tools.join("stream-extra"), stream).unwrap();
    }
    // Every prompt (the attempts of step 1, then the later steps) writes the index.
    arm(tools, "fill", &["docs/AGENTS.md"; 3]);
    let body = run_full(state, None).await;
    let of_step_one = |name: &str| -> Vec<Value> {
        sse_events(&body, name)
            .into_iter()
            .filter(|e| e["step"] == 1)
            .collect()
    };
    IndexStep {
        retries: of_step_one("step_retry"),
        warnings: of_step_one("step_warning"),
        success: of_step_one("step_done")
            .first()
            .is_some_and(|done| done["success"] == true),
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_refuses_an_invented_latest_stable_with_fake_provenance() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let table = stack_table(
        &date,
        &format!("23.0.0 [src: url: https://nodejs.org/en/about/previous-releases] (read {date})"),
        &format!("29.0.0 [src: deps: JS / TS/prettier@{date}]"),
    );
    // The watch holds joi and jest; prettier is invented and nothing was fetched.
    let step = index_step_with_table(&h, table, None, true).await;
    assert!(!step.success, "{:?}", step.warnings);
    assert_eq!(step.retries.len(), 2, "{:?}", step.retries);
    assert!(step
        .retries
        .iter()
        .all(|retry| retry["reason"] == "documentary_blockers"));
    let diagnostics = step.retries[0]["diagnostics"].to_string();
    assert!(diagnostics.contains("unbacked_provenance"), "{diagnostics}");
    assert!(diagnostics.contains("never fetched"), "{diagnostics}");
    assert!(
        diagnostics.contains("not in the dependency watch"),
        "{diagnostics}"
    );
    let reason = step.warnings[0]["reason"].as_str().unwrap();
    assert!(reason.contains("not verified: no source"), "{reason}");
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_accepts_citations_the_run_can_back() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let table = stack_table(
        &date,
        &format!("22.11.0 [src: url: https://nodejs.org/en/about/previous-releases] (read {date})"),
        &format!("30.0.0 [src: deps: JS / TS/jest@{date}]"),
    );
    let fetched = Some("https://nodejs.org/en/about/previous-releases");
    let step = index_step_with_table(&h, table, fetched, true).await;
    assert!(step.retries.is_empty(), "{:?}", step.retries);
    assert!(step.warnings.is_empty(), "{:?}", step.warnings);
    assert!(step.success);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_accepts_not_verified() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let honest = "not verified: no source";
    let step = index_step_with_table(&h, stack_table(&date, honest, honest), None, false).await;
    assert!(step.retries.is_empty(), "{:?}", step.retries);
    assert!(step.warnings.is_empty(), "{:?}", step.warnings);
    assert!(step.success);
}

/// KT-1306 × KT-1308 — the blame ledger is recorded with the same provenance-aware
/// analysis as the gate: an unbacked `deps:` row a step left in a document no step
/// targets is charged to that step, not to nobody.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_unbacked_citation_is_charged_to_the_step_that_wrote_it() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    watch_dependencies(state).await;
    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let writer = chain
        .iter()
        .position(|s| s.target_file == "docs/glossary.md")
        .unwrap() as u32
        + 1;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let attempts = crate::api::audit::anti_hallu_enforce::MAX_ATTEMPTS;
    let mut targets: Vec<&str> = Vec::new();
    for (index, step) in chain.iter().enumerate() {
        let n = if index as u32 + 1 == writer {
            attempts
        } else {
            1
        };
        targets.extend(std::iter::repeat_n(step.target_file, n));
    }
    arm(tools, "fill", &targets);
    let root = project.display();
    super::after_each_step(
        tools,
        &format!(
            "docs/glossary.md) printf '# Extra\\nNode 23.0.0 [src: deps: JS / TS/prettier@{date}]\\n' > '{root}/docs/extra.md' ;;"
        ),
    );
    let run = run_full(state, None).await;
    let done = sse_events(&run, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "interrupted", "{run}");
    let step_done = sse_events(&run, "step_done")
        .into_iter()
        .find(|e| e["step"] == writer)
        .unwrap();
    assert_eq!(step_done["success"], false, "{run}");
    let ledger = std::fs::read_to_string(project.join(".kronn/audit/document-blame.json")).unwrap();
    assert!(
        ledger.contains("unbacked_provenance") && ledger.contains(&format!("\"step\":{writer}")),
        "{ledger}"
    );
}

/// The default route is the ACP adapter, which reports a call as a bounded trace:
/// the page a Context7 result names reaches the check as a URL, not as output.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_accepts_a_page_a_context7_result_named_on_the_acp_route() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let page = "https://jestjs.io/docs/getting-started";
    let table = stack_table(
        &date,
        "not verified: no source",
        &format!("30.0.0 [src: url: {page}] (read {date})"),
    );
    let stream = context7_stream(&format!("### Getting started\nSource: {page}\n"));
    let step = index_step_with_stream(&h, table, Some(stream), false).await;
    assert!(step.retries.is_empty(), "{:?}", step.retries);
    assert!(step.success, "{:?}", step.warnings);
}

/// A page the Context7 result did not name is still invented.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_refuses_a_page_no_context7_result_named() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let table = stack_table(
        &date,
        "not verified: no source",
        &format!("30.0.0 [src: url: https://jestjs.io/blog] (read {date})"),
    );
    let stream = context7_stream("Source: https://jestjs.io/docs/getting-started");
    let step = index_step_with_stream(&h, table, Some(stream), false).await;
    assert!(!step.success, "{:?}", step.warnings);
    assert!(step.retries[0]["diagnostics"]
        .to_string()
        .contains("never fetched"));
}

/// A trace input is cut at a bound: the page a call opened is read before it.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_accepts_a_fetch_whose_input_exceeds_the_trace_bound() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let (fetched, downloaded) = (
        "https://nodejs.org/en/about/previous-releases",
        "https://registry.npmjs.org/jest/latest",
    );
    let long = "x".repeat(4000);
    let fetch = json!({"type": "tool_use", "id": "f1", "name": "WebFetch",
        "input": {"url": fetched, "prompt": long}});
    let shell = json!({"type": "tool_use", "id": "f2", "name": "Bash",
        "input": {"command": format!("echo '{long}' && curl -s {downloaded}")}});
    let stream = assistant_line(fetch) + &assistant_line(shell);
    let table = stack_table(
        &date,
        &format!("22.11.0 [src: url: {fetched}] (read {date})"),
        &format!("30.0.0 [src: url: {downloaded}] (read {date})"),
    );
    let step = index_step_with_stream(&h, table, Some(stream), false).await;
    assert!(step.retries.is_empty(), "{:?}", step.retries);
    assert!(step.success, "{:?}", step.warnings);
}

/// `resolve-library-id` comes first and names no page: it must not switch the
/// run's `url:` check off, or an invented page would pass on the default route.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_still_refuses_an_invented_page_after_resolve_library_id() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let table = stack_table(
        &date,
        "not verified: no source",
        &format!("30.0.0 [src: url: https://jestjs.io/blog/invented] (read {date})"),
    );
    let stream =
        context7_resolve_then_query_stream("Source: https://jestjs.io/docs/getting-started");
    let step = index_step_with_stream(&h, table, Some(stream), false).await;
    assert!(!step.success, "{:?}", step.warnings);
    assert!(step.retries[0]["diagnostics"]
        .to_string()
        .contains("never fetched"));
}

/// The same flow, citing the page the `query-docs` result named.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn warn_mode_accepts_a_page_named_after_resolve_library_id() {
    let h = harness().await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let page = "https://jestjs.io/docs/getting-started";
    let table = stack_table(
        &date,
        "not verified: no source",
        &format!("30.0.0 [src: url: {page}] (read {date})"),
    );
    let stream = context7_resolve_then_query_stream(&format!("Source: {page}"));
    let step = index_step_with_stream(&h, table, Some(stream), false).await;
    assert!(step.retries.is_empty(), "{:?}", step.retries);
    assert!(step.success, "{:?}", step.warnings);
}
