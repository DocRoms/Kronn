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

#[tokio::test]
async fn an_audit_step_cannot_write_the_document_of_another_step() {
    // Run O6: filling docs/AGENTS.md, qwen3.6:35b also wrote repo-map.md,
    // coding-rules.md and testing-quality.md; steps 3 to 5 then found nothing
    // to rewrite and failed.
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("docs/tech-debt")).unwrap();
    let state = crate::AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let step1 = KronnToolExecutor::audit_arc_for_step(
        state.clone(),
        project.path().to_path_buf(),
        Some("docs/AGENTS.md"),
    );
    let foreign = run(
        step1.as_ref(),
        "write_file",
        serde_json::json!({ "path": "./docs/repo-map.md", "content": "x" }),
    )
    .await;
    assert!(!foreign.ok, "{}", foreign.content);
    assert!(
        foreign.content.to_string().contains("audit step 3"),
        "{}",
        foreign.content
    );
    assert!(!project.path().join("docs/repo-map.md").exists());

    let own = run(
        step1.as_ref(),
        "write_file",
        serde_json::json!({ "path": "docs/AGENTS.md", "content": "# filled" }),
    )
    .await;
    assert!(own.ok, "{}", own.content);
    // Files that are no step's target (a TD detail, a sequence) stay writable.
    let detail = run(
        step1.as_ref(),
        "write_file",
        serde_json::json!({ "path": "docs/tech-debt/TD-1.md", "content": "td" }),
    )
    .await;
    assert!(detail.ok, "{}", detail.content);

    // The consolidation step reviews every document.
    let consolidation = KronnToolExecutor::audit_arc_for_step(
        state,
        project.path().to_path_buf(),
        Some("docs/decisions.md"),
    );
    let review = run(
        consolidation.as_ref(),
        "write_file",
        serde_json::json!({ "path": "docs/repo-map.md", "content": "reviewed" }),
    )
    .await;
    assert!(review.ok, "{}", review.content);
}

#[tokio::test]
async fn an_audit_never_reads_a_private_key_into_the_model() {
    // A/B s10 on qwen3.6:35b: told to report key paths "and never open them",
    // one run read `.ssh/<key>` and the key entered its context.
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".ssh")).unwrap();
    std::fs::write(
        project.path().join(".ssh/deploy"),
        "-----BEGIN OPENSSH PRIVATE KEY-----\nS0VZLUNBTkFSWQ==\n-----END OPENSSH PRIVATE KEY-----\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join(".ssh/deploy.pub"),
        "ssh-ed25519 AAAA test\n",
    )
    .unwrap();
    let executor = audit_executor(project.path()).await;
    let key = run(
        executor.as_ref(),
        "read_file",
        serde_json::json!({ "path": ".ssh/deploy" }),
    )
    .await;
    assert!(!key.ok);
    let shown = key.content.to_string();
    assert!(shown.contains("holds a private key"), "{shown}");
    assert!(!shown.contains("S0VZLUNBTkFSWQ"), "{shown}");
    let public = run(
        executor.as_ref(),
        "read_file",
        serde_json::json!({ "path": ".ssh/deploy.pub" }),
    )
    .await;
    assert!(public.ok, "{}", public.content);
}

#[tokio::test]
async fn an_audit_step_rewrites_its_own_document_without_a_receipt() {
    // Run O7, step 8: Kronn rewrote the index between attempts, the model's
    // receipt went stale, the write window had withdrawn read_file, and four
    // writes were refused ("Read it first, then pass its content_sha256").
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("docs")).unwrap();
    std::fs::write(
        project.path().join("docs/inconsistencies-tech-debt.md"),
        "template\n",
    )
    .unwrap();
    std::fs::write(project.path().join("docs/glossary.md"), "other\n").unwrap();
    let state = crate::AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let step8 = KronnToolExecutor::audit_arc_for_step(
        state.clone(),
        project.path().to_path_buf(),
        Some("docs/inconsistencies-tech-debt.md"),
    );
    let own = run(
        step8.as_ref(),
        "write_file",
        serde_json::json!({ "path": "./docs/inconsistencies-tech-debt.md", "content": "filled\n" }),
    )
    .await;
    assert!(own.ok, "{}", own.content);
    assert_eq!(
        std::fs::read_to_string(project.path().join("docs/inconsistencies-tech-debt.md")).unwrap(),
        "filled\n"
    );
    // Any other existing file keeps its receipt requirement (here the
    // consolidation step, free to write every document).
    let consolidation = KronnToolExecutor::audit_arc_for_step(
        state,
        project.path().to_path_buf(),
        Some("docs/decisions.md"),
    );
    let other = run(
        consolidation.as_ref(),
        "write_file",
        serde_json::json!({ "path": "docs/glossary.md", "content": "x" }),
    )
    .await;
    assert!(!other.ok, "{}", other.content);
    assert_eq!(
        std::fs::read_to_string(project.path().join("docs/glossary.md")).unwrap(),
        "other\n"
    );
}
