// KT-924 — the tools an audit step hands an HTTP agent, and the bounds on them.
use super::*;

async fn audit_executor(project: &std::path::Path) -> std::sync::Arc<dyn ToolExecutor> {
    let state = crate::AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    KronnToolExecutor::audit_arc(state, project.to_path_buf())
}

async fn run(executor: &dyn ToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: name.into(),
            name: name.into(),
            arguments,
        })
        .await
}

#[tokio::test]
async fn an_audit_declares_the_bounded_file_tools_and_nothing_else() {
    let project = tempfile::tempdir().unwrap();
    let executor = audit_executor(project.path()).await;
    let mut declared: Vec<String> = executor
        .catalogue()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
        .collect();
    declared.sort();
    let mut expected: Vec<String> = AUDIT_TOOLS.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(declared, expected, "a tool silently dropped or added");
    for name in &declared {
        assert!(
            crate::api::agent_workspace_tools::TOOL_NAMES.contains(&name.as_str()),
            "`{name}` is not a workspace tool"
        );
    }
    // No shell exists to decline; these are the tools that would reach beyond
    // the project or change history the audit never asked it to touch.
    for excluded in [
        "web_fetch",
        "git_commit",
        "api_call",
        "qa_run",
        "disc_read",
        "task_create",
        "task_exec_launch",
        "media_generate",
    ] {
        assert!(!declared.iter().any(|name| name == excluded), "{excluded}");
    }
}

#[tokio::test]
async fn an_audit_writes_its_deliverable_inside_the_project() {
    let project = tempfile::tempdir().unwrap();
    let executor = audit_executor(project.path()).await;
    let written = run(
        executor.as_ref(),
        "write_file",
        json!({"path": "docs/AGENTS.md", "content": "# Project\n"}),
    )
    .await;
    assert!(written.ok, "{}", written.content);
    assert_eq!(
        std::fs::read_to_string(project.path().join("docs/AGENTS.md")).unwrap(),
        "# Project\n"
    );
    let read = run(
        executor.as_ref(),
        "read_file",
        json!({"path": "docs/AGENTS.md"}),
    )
    .await;
    assert!(
        read.ok && read.content.to_string().contains("# Project"),
        "{}",
        read.content
    );
}

#[tokio::test]
async fn an_audit_cannot_touch_anything_outside_the_project() {
    let outer = tempfile::tempdir().unwrap();
    let project = outer.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(outer.path().join("secret.txt"), "not yours").unwrap();
    let executor = audit_executor(&project).await;

    let absolute = outer.path().join("absolute.md");
    for (tool, arguments) in [
        (
            "write_file",
            json!({"path": "../escape.md", "content": "x"}),
        ),
        (
            "write_file",
            json!({"path": absolute.to_string_lossy(), "content": "x"}),
        ),
        (
            "write_file",
            json!({"path": "docs/../../escape.md", "content": "x"}),
        ),
        ("read_file", json!({"path": "../secret.txt"})),
        ("list_files", json!({"path": ".."})),
    ] {
        let outcome = run(executor.as_ref(), tool, arguments.clone()).await;
        assert!(
            !outcome.ok,
            "{tool} {arguments} must be refused: {}",
            outcome.content
        );
        assert!(
            outcome.content.to_string().contains("refused"),
            "a readable refusal, not an opaque error: {}",
            outcome.content
        );
    }
    assert!(!outer.path().join("escape.md").exists());
    assert!(!absolute.exists());
    assert_eq!(
        std::fs::read_to_string(outer.path().join("secret.txt")).unwrap(),
        "not yours"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_audit_cannot_leave_the_project_through_a_symlink() {
    let outer = tempfile::tempdir().unwrap();
    let project = outer.path().join("project");
    let elsewhere = outer.path().join("elsewhere");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, project.join("docs")).unwrap();
    let executor = audit_executor(&project).await;
    let outcome = run(
        executor.as_ref(),
        "write_file",
        json!({"path": "docs/AGENTS.md", "content": "x"}),
    )
    .await;
    assert!(!outcome.ok, "{}", outcome.content);
    assert!(!elsewhere.join("AGENTS.md").exists());
}

#[tokio::test]
async fn an_audit_refuses_a_tool_it_never_declared_before_any_handler_runs() {
    let project = tempfile::tempdir().unwrap();
    let executor = audit_executor(project.path()).await;
    for name in [
        "web_fetch",
        "git_commit",
        "api_call",
        "task_list",
        "disc_read",
        "qe_run",
        "tools_load",
    ] {
        let outcome = run(
            executor.as_ref(),
            name,
            json!({"url": "http://example.com"}),
        )
        .await;
        assert!(!outcome.ok, "{name}: {}", outcome.content);
        assert!(
            outcome
                .content
                .to_string()
                .contains("not available during an audit"),
            "{name}: {}",
            outcome.content
        );
    }
}
