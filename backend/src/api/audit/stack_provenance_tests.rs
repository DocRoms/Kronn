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

/// The dependency watch the audit injects: `joi` and `jest` of the `JS / TS` manager.
async fn watch_dependencies(state: &AppState) {
    let package = |name: &str| crate::models::DependencyUpdatePackage {
        name: name.into(),
        current: "29.0.0".into(),
        latest: "30.0.0".into(),
        major: true,
        declared_only: false,
    };
    let summary = crate::models::DependencyUpdateSummary {
        managers: vec![crate::models::DependencyManagerUpdate {
            manager: "JS / TS".into(),
            manifest: "package.json".into(),
            status: crate::models::DependencyCheckStatus::UpdatesAvailable,
            outdated: 2,
            major: 2,
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
    if let Some(url) = fetched {
        std::fs::write(tools.join("stream-extra"), fetch_stream(url)).unwrap();
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
