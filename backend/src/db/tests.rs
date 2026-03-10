use chrono::Utc;
use rusqlite::Connection;
use crate::db::migrations;
use crate::models::*;

/// Create an in-memory database with all migrations applied
fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&conn).unwrap();
    conn
}

fn sample_project(id: &str, name: &str) -> Project {
    let now = Utc::now();
    Project {
        id: id.into(),
        name: name.into(),
        path: format!("/tmp/{}", name),
        repo_url: Some("https://github.com/test/repo".into()),
        token_override: None,
        ai_config: AiConfigStatus { detected: false, configs: vec![] },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        created_at: now,
        updated_at: now,
    }
}

fn sample_discussion(id: &str, project_id: Option<&str>) -> Discussion {
    let now = Utc::now();
    Discussion {
        id: id.into(),
        project_id: project_id.map(|s| s.into()),
        title: "Test Discussion".into(),
        agent: AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![AgentType::ClaudeCode],
        messages: vec![],
        archived: false,
        created_at: now,
        updated_at: now,
    }
}

fn sample_message(id: &str, role: MessageRole) -> DiscussionMessage {
    DiscussionMessage {
        id: id.into(),
        role,
        content: format!("Message {}", id),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Migrations
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn migrations_idempotent() {
    let conn = test_db();
    // Running migrations again should not fail
    migrations::run(&conn).unwrap();
    migrations::run(&conn).unwrap();
}

#[test]
fn migrations_create_all_tables() {
    let conn = test_db();
    let tables: Vec<String> = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB '_*' ORDER BY name"
    ).unwrap()
    .query_map([], |r| r.get(0)).unwrap()
    .filter_map(|r| r.ok()).collect();

    assert!(tables.contains(&"projects".into()), "Missing 'projects' table");
    assert!(tables.contains(&"discussions".into()), "Missing 'discussions' table");
    assert!(tables.contains(&"messages".into()), "Missing 'messages' table");
    assert!(tables.contains(&"mcp_servers".into()), "Missing 'mcp_servers' table");
    assert!(tables.contains(&"mcp_configs".into()), "Missing 'mcp_configs' table");
    assert!(tables.contains(&"workflows".into()), "Missing 'workflows' table");
    assert!(tables.contains(&"workflow_runs".into()), "Missing 'workflow_runs' table");
}

// ═══════════════════════════════════════════════════════════════════════════
// Projects CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projects_insert_and_list() {
    let conn = test_db();
    let p = sample_project("p1", "MyProject");
    crate::db::projects::insert_project(&conn, &p).unwrap();

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "MyProject");
    assert_eq!(projects[0].path, "/tmp/MyProject");
}

#[test]
fn projects_get_by_id() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "B")).unwrap();

    let found = crate::db::projects::get_project(&conn, "p2").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "B");

    let missing = crate::db::projects::get_project(&conn, "p999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn projects_delete() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    let deleted = crate::db::projects::delete_project(&conn, "p1").unwrap();
    assert!(deleted);

    let deleted_again = crate::db::projects::delete_project(&conn, "p1").unwrap();
    assert!(!deleted_again);

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    assert!(projects.is_empty());
}

#[test]
fn projects_update_ai_config() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    let new_config = AiConfigStatus {
        detected: true,
        configs: vec![AiConfigType::ClaudeMd, AiConfigType::AiDir],
    };
    crate::db::projects::update_project_ai_config(&conn, "p1", &new_config).unwrap();

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert!(p.ai_config.detected);
    assert_eq!(p.ai_config.configs.len(), 2);
}

#[test]
fn projects_list_ordered_by_name() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Zebra")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "Alpha")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p3", "Middle")).unwrap();

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    assert_eq!(projects[0].name, "Alpha");
    assert_eq!(projects[1].name, "Middle");
    assert_eq!(projects[2].name, "Zebra");
}

// ═══════════════════════════════════════════════════════════════════════════
// Discussions CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn discussions_insert_and_list() {
    let conn = test_db();
    let disc = sample_discussion("d1", None);
    crate::db::discussions::insert_discussion(&conn, &disc).unwrap();

    let discussions = crate::db::discussions::list_discussions(&conn).unwrap();
    assert_eq!(discussions.len(), 1);
    assert_eq!(discussions[0].title, "Test Discussion");
}

#[test]
fn discussions_get_by_id() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    let found = crate::db::discussions::get_discussion(&conn, "d1").unwrap();
    assert!(found.is_some());

    let missing = crate::db::discussions::get_discussion(&conn, "d999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn discussions_delete() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    let deleted = crate::db::discussions::delete_discussion(&conn, "d1").unwrap();
    assert!(deleted);

    let discussions = crate::db::discussions::list_discussions(&conn).unwrap();
    assert!(discussions.is_empty());
}

#[test]
fn discussions_with_project() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Proj")).unwrap();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", Some("p1"))).unwrap();

    let disc = crate::db::discussions::get_discussion(&conn, "d1").unwrap().unwrap();
    assert_eq!(disc.project_id, Some("p1".into()));
}

// ═══════════════════════════════════════════════════════════════════════════
// Messages CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn messages_insert_and_list() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    let msg = sample_message("m1", MessageRole::User);
    crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "Message m1");
    assert!(matches!(messages[0].role, MessageRole::User));
}

#[test]
fn messages_sort_order() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    for i in 1..=5 {
        let msg = sample_message(&format!("m{}", i), MessageRole::User);
        crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();
    }

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages.len(), 5);
    // Check ordering is preserved
    for (i, msg) in messages.iter().enumerate() {
        assert_eq!(msg.content, format!("Message m{}", i + 1));
    }
}

#[test]
fn messages_with_tokens_and_auth_mode() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    let mut msg = sample_message("m1", MessageRole::Agent);
    msg.tokens_used = 1500;
    msg.auth_mode = Some("override".into());
    msg.agent_type = Some(AgentType::ClaudeCode);
    crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages[0].tokens_used, 1500);
    assert_eq!(messages[0].auth_mode, Some("override".into()));
}

#[test]
fn messages_update_tokens() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::Agent)).unwrap();

    crate::db::discussions::update_message_tokens(&conn, "m1", 999, Some("local")).unwrap();

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages[0].tokens_used, 999);
    assert_eq!(messages[0].auth_mode, Some("local".into()));
}

#[test]
fn messages_delete_last_agent() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m3", MessageRole::System)).unwrap();

    let deleted = crate::db::discussions::delete_last_agent_messages(&conn, "d1").unwrap();
    assert_eq!(deleted, 2); // Agent + System messages after last User

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages.len(), 1);
    assert!(matches!(messages[0].role, MessageRole::User));
}

#[test]
fn messages_edit_last_user() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();

    let edited = crate::db::discussions::edit_last_user_message(&conn, "d1", "updated content").unwrap();
    assert!(edited);

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages[0].content, "updated content");
}

#[test]
fn discussions_loaded_with_messages() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();

    let disc = crate::db::discussions::get_discussion(&conn, "d1").unwrap().unwrap();
    assert_eq!(disc.messages.len(), 2);
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mcp_server_upsert_and_list() {
    let conn = test_db();
    let server = McpServer {
        id: "test-server".into(),
        name: "Test".into(),
        description: "A test server".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let servers = crate::db::mcps::list_servers(&conn).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "Test");

    // Upsert again (update)
    let updated = McpServer { name: "Updated Test".into(), ..server };
    crate::db::mcps::upsert_server(&conn, &updated).unwrap();

    let servers = crate::db::mcps::list_servers(&conn).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "Updated Test");
}

#[test]
fn mcp_config_insert_with_projects() {
    let conn = test_db();
    // Create server and projects first
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Sse { url: "http://localhost".into() },
        source: McpSource::Manual,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Proj1")).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "My Config".into(),
        env_keys: vec!["KEY1".into()], env_encrypted: "enc".into(),
        args_override: None, is_global: false, config_hash: "hash1".into(),
        project_ids: vec!["p1".into()],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    let configs = crate::db::mcps::list_configs(&conn).unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].project_ids, vec!["p1".to_string()]);
}

#[test]
fn mcp_configs_for_project_includes_global() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();

    // Global config
    let global = McpConfig {
        id: "cfg-global".into(), server_id: "srv1".into(), label: "Global".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: true, config_hash: "h1".into(),
        project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &global).unwrap();

    // Project-specific config
    let specific = McpConfig {
        id: "cfg-proj".into(), server_id: "srv1".into(), label: "Proj".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, config_hash: "h2".into(),
        project_ids: vec!["p1".into()],
    };
    crate::db::mcps::insert_config(&conn, &specific).unwrap();

    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    assert_eq!(for_p1.len(), 2); // Global + specific
}

#[test]
fn mcp_encrypt_decrypt_env() {
    let secret = crate::core::crypto::generate_secret();
    let mut env = std::collections::HashMap::new();
    env.insert("KEY1".into(), "value1".into());
    env.insert("KEY2".into(), "value2".into());

    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
    assert!(!encrypted.is_empty());

    let decrypted = crate::db::mcps::decrypt_env(&encrypted, &secret).unwrap();
    assert_eq!(decrypted.get("KEY1").unwrap(), "value1");
    assert_eq!(decrypted.get("KEY2").unwrap(), "value2");
}

#[test]
fn mcp_encrypt_empty_env() {
    let secret = crate::core::crypto::generate_secret();
    let env = std::collections::HashMap::new();
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
    assert!(encrypted.is_empty());

    let decrypted = crate::db::mcps::decrypt_env(&encrypted, &secret).unwrap();
    assert!(decrypted.is_empty());
}

#[test]
fn mcp_config_hash_deterministic() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
    };
    let mut env = std::collections::HashMap::new();
    env.insert("K".into(), "V".into());

    let h1 = crate::db::mcps::compute_config_hash(&server, &env, None);
    let h2 = crate::db::mcps::compute_config_hash(&server, &env, None);
    assert_eq!(h1, h2);
}

#[test]
fn mcp_config_hash_differs_on_env() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec![] },
        source: McpSource::Registry,
    };
    let mut env1 = std::collections::HashMap::new();
    env1.insert("K".into(), "V1".into());
    let mut env2 = std::collections::HashMap::new();
    env2.insert("K".into(), "V2".into());

    let h1 = crate::db::mcps::compute_config_hash(&server, &env1, None);
    let h2 = crate::db::mcps::compute_config_hash(&server, &env2, None);
    assert_ne!(h1, h2);
}

// ═══════════════════════════════════════════════════════════════════════════
// Workflows CRUD
// ═══════════════════════════════════════════════════════════════════════════

fn sample_workflow(id: &str) -> Workflow {
    let now = Utc::now();
    Workflow {
        id: id.into(),
        name: "Test Workflow".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps: vec![WorkflowStep {
            name: "step1".into(),
            agent: AgentType::ClaudeCode,
            prompt_template: "Do something".into(),
            mode: StepMode::Normal,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
        }],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        enabled: true,
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn workflows_insert_and_list() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    let workflows = crate::db::workflows::list_workflows(&conn).unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].name, "Test Workflow");
    assert!(workflows[0].enabled);
    assert_eq!(workflows[0].steps.len(), 1);
}

#[test]
fn workflows_get_by_id() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    let found = crate::db::workflows::get_workflow(&conn, "w1").unwrap();
    assert!(found.is_some());

    let missing = crate::db::workflows::get_workflow(&conn, "w999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn workflows_update() {
    let conn = test_db();
    let mut wf = sample_workflow("w1");
    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    wf.name = "Updated".into();
    wf.enabled = false;
    crate::db::workflows::update_workflow(&conn, &wf).unwrap();

    let updated = crate::db::workflows::get_workflow(&conn, "w1").unwrap().unwrap();
    assert_eq!(updated.name, "Updated");
    assert!(!updated.enabled);
}

#[test]
fn workflows_delete() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::delete_workflow(&conn, "w1").unwrap();

    let workflows = crate::db::workflows::list_workflows(&conn).unwrap();
    assert!(workflows.is_empty());
}

// ─── Workflow Runs ───────────────────────────────────────────────────────

fn sample_run(id: &str, workflow_id: &str) -> WorkflowRun {
    let now = Utc::now();
    WorkflowRun {
        id: id.into(),
        workflow_id: workflow_id.into(),
        status: RunStatus::Running,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
    }
}

#[test]
fn workflow_runs_insert_and_list() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();

    let runs = crate::db::workflows::list_runs(&conn, "w1").unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Running);
}

#[test]
fn workflow_runs_update() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let mut run = sample_run("r1", "w1");
    crate::db::workflows::insert_run(&conn, &run).unwrap();

    run.status = RunStatus::Success;
    run.tokens_used = 500;
    run.finished_at = Some(Utc::now());
    run.step_results = vec![StepResult {
        step_name: "step1".into(),
        status: RunStatus::Success,
        output: "Done".into(),
        tokens_used: 500,
        duration_ms: 1234,
        condition_result: None,
    }];
    crate::db::workflows::update_run(&conn, &run).unwrap();

    let updated = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert_eq!(updated.status, RunStatus::Success);
    assert_eq!(updated.tokens_used, 500);
    assert!(updated.finished_at.is_some());
    assert_eq!(updated.step_results.len(), 1);
}

#[test]
fn workflow_runs_count_active() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();

    let mut r2 = sample_run("r2", "w1");
    r2.status = RunStatus::Pending;
    crate::db::workflows::insert_run(&conn, &r2).unwrap();

    let mut r3 = sample_run("r3", "w1");
    r3.status = RunStatus::Success;
    crate::db::workflows::insert_run(&conn, &r3).unwrap();

    let count = crate::db::workflows::count_active_runs(&conn, "w1").unwrap();
    assert_eq!(count, 2); // Running + Pending
}

#[test]
fn workflow_runs_delete_all() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r2", "w1")).unwrap();

    crate::db::workflows::delete_all_runs(&conn, "w1").unwrap();
    let runs = crate::db::workflows::list_runs(&conn, "w1").unwrap();
    assert!(runs.is_empty());
}

#[test]
fn workflow_get_last_run() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r2", "w1")).unwrap();

    let last = crate::db::workflows::get_last_run(&conn, "w1").unwrap();
    assert!(last.is_some());
}

// ─── Tracker processed ──────────────────────────────────────────────────

#[test]
fn tracker_issue_processed() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    assert!(!crate::db::workflows::is_issue_processed(&conn, "w1", "issue-1").unwrap());

    crate::db::workflows::mark_issue_processed(&conn, "w1", "issue-1").unwrap();
    assert!(crate::db::workflows::is_issue_processed(&conn, "w1", "issue-1").unwrap());

    // Marking again should not fail (INSERT OR IGNORE)
    crate::db::workflows::mark_issue_processed(&conn, "w1", "issue-1").unwrap();
}
