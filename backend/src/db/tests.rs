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
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
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
        message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Default,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
            shared_id: None,
            shared_with: vec![],
        workflow_run_id: None,
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
        model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
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
// Projects — default skills / profile / cascade delete
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projects_update_default_skills() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    let skills = vec!["skill-rust-expert".to_string(), "skill-testing".to_string()];
    let updated = crate::db::projects::update_project_default_skills(&conn, "p1", &skills).unwrap();
    assert!(updated, "update should affect one row");

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(p.default_skill_ids, skills, "Default skills must persist after update");
}

#[test]
fn projects_update_default_skills_to_empty() {
    let conn = test_db();
    let mut proj = sample_project("p1", "A");
    proj.default_skill_ids = vec!["old-skill".into()];
    crate::db::projects::insert_project(&conn, &proj).unwrap();

    crate::db::projects::update_project_default_skills(&conn, "p1", &[]).unwrap();

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert!(p.default_skill_ids.is_empty(), "Skills should be clearable to empty");
}

#[test]
fn projects_update_default_profile() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Set a profile
    let updated = crate::db::projects::update_project_default_profile(&conn, "p1", Some("profile-senior")).unwrap();
    assert!(updated);

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(p.default_profile_id.as_deref(), Some("profile-senior"));

    // Clear the profile
    crate::db::projects::update_project_default_profile(&conn, "p1", None).unwrap();
    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert!(p.default_profile_id.is_none(), "Profile should be clearable to None");
}

#[test]
fn projects_delete_cascade_discussions() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Insert discussions linked to project
    let d1 = sample_discussion("d1", Some("p1"));
    let d2 = sample_discussion("d2", Some("p1"));
    let d3 = sample_discussion("d3", None); // unlinked
    crate::db::discussions::insert_discussion(&conn, &d1).unwrap();
    crate::db::discussions::insert_discussion(&conn, &d2).unwrap();
    crate::db::discussions::insert_discussion(&conn, &d3).unwrap();

    // Cascade delete project discussions
    crate::db::projects::delete_project_discussions(&conn, "p1").unwrap();

    let all = crate::db::discussions::list_discussions(&conn).unwrap();
    assert_eq!(all.len(), 1, "Only unlinked discussion should remain");
    assert_eq!(all[0].id, "d3");
}

#[test]
fn projects_update_nonexistent_returns_false() {
    let conn = test_db();
    let updated = crate::db::projects::update_project_default_skills(&conn, "nonexistent", &["s1".into()]).unwrap();
    assert!(!updated, "Updating nonexistent project should return false");
}

#[test]
fn projects_briefing_notes_set_and_get() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Initially None
    let notes = crate::db::projects::get_project_briefing_notes(&conn, "p1").unwrap();
    assert!(notes.is_none(), "Briefing notes should be None initially");

    // Set notes
    let updated = crate::db::projects::update_project_briefing_notes(
        &conn, "p1", Some("This is a React app with a REST API backend")
    ).unwrap();
    assert!(updated);

    let notes = crate::db::projects::get_project_briefing_notes(&conn, "p1").unwrap();
    assert_eq!(notes.as_deref(), Some("This is a React app with a REST API backend"));

    // Verify it's also returned in get_project
    let project = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(project.briefing_notes.as_deref(), Some("This is a React app with a REST API backend"));
}

#[test]
fn projects_briefing_notes_clear() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Set then clear
    crate::db::projects::update_project_briefing_notes(&conn, "p1", Some("Some notes")).unwrap();
    crate::db::projects::update_project_briefing_notes(&conn, "p1", None).unwrap();

    let notes = crate::db::projects::get_project_briefing_notes(&conn, "p1").unwrap();
    assert!(notes.is_none(), "Briefing notes should be clearable to None");
}

#[test]
fn projects_briefing_notes_nonexistent_project() {
    let conn = test_db();
    let updated = crate::db::projects::update_project_briefing_notes(&conn, "nonexistent", Some("notes")).unwrap();
    assert!(!updated, "Updating briefing notes for nonexistent project should return false");
}

#[test]
fn projects_briefing_notes_persisted_in_insert() {
    let conn = test_db();
    let mut p = sample_project("p1", "WithNotes");
    p.briefing_notes = Some("Pre-filled briefing".into());
    crate::db::projects::insert_project(&conn, &p).unwrap();

    let loaded = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(loaded.briefing_notes.as_deref(), Some("Pre-filled briefing"));
}

#[test]
fn projects_briefing_notes_in_list() {
    let conn = test_db();
    let mut p = sample_project("p1", "A");
    p.briefing_notes = Some("Project A notes".into());
    crate::db::projects::insert_project(&conn, &p).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "B")).unwrap();

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    let a = projects.iter().find(|p| p.id == "p1").unwrap();
    let b = projects.iter().find(|p| p.id == "p2").unwrap();
    assert_eq!(a.briefing_notes.as_deref(), Some("Project A notes"));
    assert!(b.briefing_notes.is_none());
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
        args_override: None, is_global: false, include_general: true, config_hash: "hash1".into(),
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
        args_override: None, is_global: true, include_general: true, config_hash: "h1".into(),
        project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &global).unwrap();

    // Project-specific config
    let specific = McpConfig {
        id: "cfg-proj".into(), server_id: "srv1".into(), label: "Proj".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true, config_hash: "h2".into(),
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
// MCP Config Update & Sync
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mcp_config_update_env_persists() {
    let conn = test_db();
    let secret = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv1".into(), name: "GitHub".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let mut env = std::collections::HashMap::new();
    env.insert("TOKEN".into(), "old-value".into());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "My GitHub".into(),
        env_keys: vec!["TOKEN".into()], env_encrypted: encrypted,
        args_override: None, is_global: false, include_general: true,
        config_hash: "h1".into(), project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Update env with new value
    let mut new_env = std::collections::HashMap::new();
    new_env.insert("TOKEN".into(), "new-secret-value".into());
    let new_encrypted = crate::db::mcps::encrypt_env(&new_env, &secret).unwrap();
    let new_keys = vec!["TOKEN".to_string()];

    let updated = crate::db::mcps::update_config(
        &conn, "cfg1", None, Some(&new_encrypted), Some(&new_keys),
        None, None, None, None,
    ).unwrap();
    assert!(updated, "update_config should return true");

    // Verify the stored encrypted value decrypts to the new value
    let loaded = crate::db::mcps::get_config(&conn, "cfg1").unwrap().unwrap();
    let decrypted = crate::db::mcps::decrypt_env(&loaded.env_encrypted, &secret).unwrap();
    assert_eq!(decrypted.get("TOKEN").unwrap(), "new-secret-value",
        "Updated env value must persist after update_config");
}

#[test]
fn mcp_config_update_nonexistent_returns_false() {
    let conn = test_db();
    let result = crate::db::mcps::update_config(
        &conn, "nonexistent", Some("label"), None, None, None, None, None, None,
    ).unwrap();
    assert!(!result, "Updating a nonexistent config should return false");
}

#[test]
fn mcp_decrypt_wrong_secret_fails() {
    let secret1 = crate::core::crypto::generate_secret();
    let secret2 = crate::core::crypto::generate_secret();

    let mut env = std::collections::HashMap::new();
    env.insert("KEY".into(), "secret-value".into());

    let encrypted = crate::db::mcps::encrypt_env(&env, &secret1).unwrap();
    let result = crate::db::mcps::decrypt_env(&encrypted, &secret2);
    assert!(result.is_err(), "Decrypting with wrong secret must fail");
}

#[test]
fn mcp_config_global_visible_to_all_projects() {
    let conn = test_db();
    let secret = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv1".into(), name: "Sentry".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec![] },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "ProjectA")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "ProjectB")).unwrap();

    // Create a global config with encrypted env
    let mut env = std::collections::HashMap::new();
    env.insert("SENTRY_TOKEN".into(), "tok-123".into());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();

    let config = McpConfig {
        id: "cfg-global".into(), server_id: "srv1".into(), label: "Sentry Global".into(),
        env_keys: vec!["SENTRY_TOKEN".into()], env_encrypted: encrypted.clone(),
        args_override: None, is_global: true, include_general: true,
        config_hash: "h".into(), project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Both projects should see it
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    let for_p2 = crate::db::mcps::configs_for_project(&conn, "p2").unwrap();
    assert_eq!(for_p1.len(), 1, "P1 should see global config");
    assert_eq!(for_p2.len(), 1, "P2 should see global config");

    // Now update the global config's env
    let mut new_env = std::collections::HashMap::new();
    new_env.insert("SENTRY_TOKEN".into(), "tok-456-updated".into());
    let new_encrypted = crate::db::mcps::encrypt_env(&new_env, &secret).unwrap();
    crate::db::mcps::update_config(
        &conn, "cfg-global", None, Some(&new_encrypted), None, None, None, None, None,
    ).unwrap();

    // Both projects should see the UPDATED value
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    let decrypted = crate::db::mcps::decrypt_env(&for_p1[0].env_encrypted, &secret).unwrap();
    assert_eq!(decrypted.get("SENTRY_TOKEN").unwrap(), "tok-456-updated",
        "Global config update must be visible to all projects immediately");
}

#[test]
fn mcp_set_config_projects_relinks() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "P2")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p3", "P3")).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "Test".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true,
        config_hash: "h".into(), project_ids: vec!["p1".into(), "p2".into()],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Verify initial state
    let loaded = crate::db::mcps::get_config(&conn, "cfg1").unwrap().unwrap();
    assert_eq!(loaded.project_ids.len(), 2);

    // Re-link to p2 and p3 (remove p1, add p3)
    crate::db::mcps::set_config_projects(&conn, "cfg1", &["p2".into(), "p3".into()]).unwrap();

    let reloaded = crate::db::mcps::get_config(&conn, "cfg1").unwrap().unwrap();
    assert!(!reloaded.project_ids.contains(&"p1".to_string()), "p1 should be unlinked");
    assert!(reloaded.project_ids.contains(&"p2".to_string()), "p2 should remain");
    assert!(reloaded.project_ids.contains(&"p3".to_string()), "p3 should be added");

    // p1 should no longer see this config
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    assert!(for_p1.is_empty(), "p1 should have no configs after re-link");
}

#[test]
fn mcp_delete_config_removes_project_links() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "Test".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true,
        config_hash: "h".into(), project_ids: vec!["p1".into()],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Delete the config
    let deleted = crate::db::mcps::delete_config(&conn, "cfg1").unwrap();
    assert!(deleted);

    // Project should have no configs
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    assert!(for_p1.is_empty(), "Deleted config should not appear in project configs");
}

#[test]
fn mcp_config_update_global_flag_changes_visibility() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "P2")).unwrap();

    // Create non-global config linked to p1 only
    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "Test".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true,
        config_hash: "h".into(), project_ids: vec!["p1".into()],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // p2 should NOT see it
    let for_p2 = crate::db::mcps::configs_for_project(&conn, "p2").unwrap();
    assert!(for_p2.is_empty(), "Non-global config should not be visible to unlinked project");

    // Promote to global
    crate::db::mcps::update_config(
        &conn, "cfg1", None, None, None, None, Some(true), None, None,
    ).unwrap();

    // Now p2 should see it
    let for_p2 = crate::db::mcps::configs_for_project(&conn, "p2").unwrap();
    assert_eq!(for_p2.len(), 1, "Global config must be visible to all projects");
}

#[test]
fn mcp_config_hash_changes_on_env_update() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
    };

    let mut env_old = std::collections::HashMap::new();
    env_old.insert("TOKEN".into(), "old-value".into());
    let mut env_new = std::collections::HashMap::new();
    env_new.insert("TOKEN".into(), "new-value".into());

    let hash_old = crate::db::mcps::compute_config_hash(&server, &env_old, None);
    let hash_new = crate::db::mcps::compute_config_hash(&server, &env_new, None);

    assert_ne!(hash_old, hash_new,
        "Config hash must change when env values change (dedup detection)");
}

#[test]
fn mcp_config_hash_changes_on_args_override() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
    };
    let env = std::collections::HashMap::new();

    let hash_default = crate::db::mcps::compute_config_hash(&server, &env, None);
    let hash_override = crate::db::mcps::compute_config_hash(
        &server, &env, Some(&vec!["--custom-flag".into()]),
    );

    assert_ne!(hash_default, hash_override,
        "Config hash must differ when args_override is set");
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
            step_type: StepType::default(),
            output_format: StepOutputFormat::default(),
            description: None,
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
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
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
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
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

// ─── Batch runs (Phase 1b) ───────────────────────────────────────────────

/// Build a sample batch run. Caller must insert the placeholder workflow
/// first (or use `insert_batch_run_with_placeholder` below).
fn sample_batch_run(id: &str, qp_id: &str, total: u32) -> WorkflowRun {
    let now = Utc::now();
    WorkflowRun {
        id: id.into(),
        workflow_id: format!("qp:{}", qp_id),
        status: RunStatus::Running,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "batch".into(),
        batch_total: total,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: Some(format!("Cadrage — {}", id)),
    }
}

/// Insert a batch run with its placeholder workflow in one shot.
fn insert_batch_run_with_placeholder(conn: &rusqlite::Connection, id: &str, qp_id: &str, total: u32) {
    crate::db::workflows::ensure_batch_placeholder_workflow(conn, qp_id, "TestQP", None).unwrap();
    let run = sample_batch_run(id, qp_id, total);
    crate::db::workflows::insert_run(conn, &run).unwrap();
}

#[test]
fn batch_run_persists_fields() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br1", "qp-br1", 5);
    let loaded = crate::db::workflows::get_run(&conn, "br1").unwrap().unwrap();
    assert_eq!(loaded.run_type, "batch");
    assert_eq!(loaded.batch_total, 5);
    assert_eq!(loaded.batch_completed, 0);
    assert_eq!(loaded.batch_failed, 0);
    assert_eq!(loaded.batch_name.as_deref(), Some("Cadrage — br1"));
}

#[test]
fn batch_placeholder_workflow_is_hidden_from_list() {
    let conn = test_db();
    // Insert a real workflow AND a batch placeholder
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("real-wf")).unwrap();
    crate::db::workflows::ensure_batch_placeholder_workflow(&conn, "qp-test", "TestQP", None).unwrap();
    // Placeholder is in the DB but filtered out of list_workflows
    let visible = crate::db::workflows::list_workflows(&conn).unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "real-wf");
    // But get_workflow can still fetch it by id if needed (placeholder row exists)
}

#[test]
fn batch_placeholder_is_idempotent() {
    let conn = test_db();
    crate::db::workflows::ensure_batch_placeholder_workflow(&conn, "qp-x", "X", None).unwrap();
    // Second call must not error — INSERT OR IGNORE
    crate::db::workflows::ensure_batch_placeholder_workflow(&conn, "qp-x", "X", None).unwrap();
}

#[test]
fn batch_progress_increments_success_counter() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br2", "qp-br2", 3);

    // First child succeeds — not final yet
    let updated = crate::db::workflows::increment_batch_progress(&conn, "br2", true).unwrap();
    let updated = updated.unwrap();
    assert_eq!(updated.batch_completed, 1);
    assert_eq!(updated.batch_failed, 0);
    assert_eq!(updated.status, RunStatus::Running);

    // Second child succeeds
    let updated = crate::db::workflows::increment_batch_progress(&conn, "br2", true).unwrap().unwrap();
    assert_eq!(updated.batch_completed, 2);
    assert_eq!(updated.status, RunStatus::Running);

    // Third (last) child succeeds → run is marked Success and finished
    let final_run = crate::db::workflows::increment_batch_progress(&conn, "br2", true).unwrap().unwrap();
    assert_eq!(final_run.batch_completed, 3);
    assert_eq!(final_run.status, RunStatus::Success);
    assert!(final_run.finished_at.is_some());
}

#[test]
fn batch_progress_marks_failed_if_all_children_fail() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br3", "qp-br3", 2);

    crate::db::workflows::increment_batch_progress(&conn, "br3", false).unwrap();
    let final_run = crate::db::workflows::increment_batch_progress(&conn, "br3", false).unwrap().unwrap();
    assert_eq!(final_run.batch_failed, 2);
    assert_eq!(final_run.batch_completed, 0);
    // At least one success is needed to mark Success — otherwise Failed
    assert_eq!(final_run.status, RunStatus::Failed);
    assert!(final_run.finished_at.is_some());
}

#[test]
fn batch_progress_marks_success_if_at_least_one_child_succeeds() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br4", "qp-br4", 3);

    crate::db::workflows::increment_batch_progress(&conn, "br4", false).unwrap();
    crate::db::workflows::increment_batch_progress(&conn, "br4", true).unwrap();
    let final_run = crate::db::workflows::increment_batch_progress(&conn, "br4", false).unwrap().unwrap();
    assert_eq!(final_run.batch_completed, 1);
    assert_eq!(final_run.batch_failed, 2);
    // Mixed result — one success is enough to count as "the batch did something"
    assert_eq!(final_run.status, RunStatus::Success);
}

#[test]
fn batch_progress_ignores_linear_runs() {
    let conn = test_db();
    // A plain linear run — increment_batch_progress must be a no-op on it
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("lr1", "w1")).unwrap();
    let result = crate::db::workflows::increment_batch_progress(&conn, "lr1", true).unwrap();
    // The UPDATE is guarded on run_type = 'batch' so nothing was written,
    // and the helper returns None early once it loads the run and sees
    // it's not a batch. The caller treats None as "no-op, no broadcast".
    assert!(result.is_none(), "linear runs must be skipped by batch progress helper");
    // Verify the linear run is untouched in DB
    let unchanged = crate::db::workflows::get_run(&conn, "lr1").unwrap().unwrap();
    assert_eq!(unchanged.batch_completed, 0);
    assert_eq!(unchanged.status, RunStatus::Running);
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

// ═══════════════════════════════════════════════════════════════════════════
// Batch / Performance functions
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projects_get_names_batch() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Alpha")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "Beta")).unwrap();

    let names = crate::db::projects::get_project_names(&conn).unwrap();
    assert_eq!(names.len(), 2);
    assert_eq!(names.get("p1").unwrap(), "Alpha");
    assert_eq!(names.get("p2").unwrap(), "Beta");
}

#[test]
fn projects_get_names_empty() {
    let conn = test_db();
    let names = crate::db::projects::get_project_names(&conn).unwrap();
    assert!(names.is_empty());
}

#[test]
fn discussions_list_does_not_load_messages() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();

    let discussions = crate::db::discussions::list_discussions(&conn).unwrap();
    assert_eq!(discussions.len(), 1);
    assert_eq!(discussions[0].messages.len(), 0, "list_discussions should not load messages");
}

#[test]
fn discussions_list_with_messages_batch_loads() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d2", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();
    crate::db::discussions::insert_message(&conn, "d2", &sample_message("m3", MessageRole::User)).unwrap();

    let discussions = crate::db::discussions::list_discussions_with_messages(&conn).unwrap();
    assert_eq!(discussions.len(), 2);

    let d1 = discussions.iter().find(|d| d.id == "d1").unwrap();
    let d2 = discussions.iter().find(|d| d.id == "d2").unwrap();
    assert_eq!(d1.messages.len(), 2);
    assert_eq!(d2.messages.len(), 1);
}

#[test]
fn workflow_get_last_runs_all_batch() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w2")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r2", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r3", "w2")).unwrap();

    let last_runs = crate::db::workflows::get_last_runs_all(&conn).unwrap();
    assert_eq!(last_runs.len(), 2);
    assert!(last_runs.contains_key("w1"));
    assert!(last_runs.contains_key("w2"));
    assert_eq!(last_runs.get("w2").unwrap().id, "r3");
}

#[test]
fn workflow_get_last_runs_all_empty() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let last_runs = crate::db::workflows::get_last_runs_all(&conn).unwrap();
    assert!(last_runs.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// Workflow multi-step
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn workflow_multi_step_roundtrip() {
    let conn = test_db();
    let now = Utc::now();
    let wf = Workflow {
        id: "wm1".into(),
        name: "Multi-step".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps: vec![
            WorkflowStep {
                step_type: StepType::default(),
                output_format: StepOutputFormat::default(),
                description: None,
                name: "analyze".into(),
                agent: AgentType::ClaudeCode,
                prompt_template: "Analyze this".into(),
                mode: StepMode::Normal,
                mcp_config_ids: vec![],
                agent_settings: None,
                on_result: vec![],
                stall_timeout_secs: None,
                retry: None,
                delay_after_secs: None,
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
            },
            WorkflowStep {
                step_type: StepType::default(),
                output_format: StepOutputFormat::default(),
                description: None,
                name: "fix".into(),
                agent: AgentType::Codex,
                prompt_template: "Fix: {{previous_step.output}}".into(),
                mode: StepMode::Normal,
                mcp_config_ids: vec![],
                agent_settings: None,
                on_result: vec![StepConditionRule {
                    contains: "NO_RESULTS".into(),
                    action: ConditionAction::Stop,
                }],
                stall_timeout_secs: Some(300),
                retry: None,
                delay_after_secs: None,
                skill_ids: vec!["token-saver".into()],
                profile_ids: vec![],
                directive_ids: vec![],
            },
            WorkflowStep {
                step_type: StepType::default(),
                output_format: StepOutputFormat::default(),
                description: None,
                name: "review".into(),
                agent: AgentType::GeminiCli,
                prompt_template: "Review the changes".into(),
                mode: StepMode::Normal,
                mcp_config_ids: vec![],
                agent_settings: None,
                on_result: vec![],
                stall_timeout_secs: None,
                retry: None,
                delay_after_secs: Some(5),
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
            },
        ],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    let loaded = crate::db::workflows::get_workflow(&conn, "wm1").unwrap().unwrap();
    assert_eq!(loaded.steps.len(), 3);
    assert_eq!(loaded.steps[0].name, "analyze");
    assert_eq!(loaded.steps[0].agent, AgentType::ClaudeCode);
    assert_eq!(loaded.steps[1].name, "fix");
    assert_eq!(loaded.steps[1].agent, AgentType::Codex);
    assert_eq!(loaded.steps[1].on_result.len(), 1);
    assert_eq!(loaded.steps[1].on_result[0].contains, "NO_RESULTS");
    assert_eq!(loaded.steps[1].stall_timeout_secs, Some(300));
    assert_eq!(loaded.steps[1].skill_ids, vec!["token-saver"]);
    assert_eq!(loaded.steps[2].name, "review");
    assert_eq!(loaded.steps[2].agent, AgentType::GeminiCli);
    assert_eq!(loaded.steps[2].delay_after_secs, Some(5));
}

#[test]
fn workflow_update_steps_count() {
    let conn = test_db();
    let mut wf = sample_workflow("wu1");
    assert_eq!(wf.steps.len(), 1);
    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    // Add a second step
    wf.steps.push(WorkflowStep {
        step_type: StepType::default(),
        output_format: StepOutputFormat::default(),
        description: None,
        name: "step2".into(),
        agent: AgentType::ClaudeCode,
        prompt_template: "Second step".into(),
        mode: StepMode::Normal,
        mcp_config_ids: vec![],
        agent_settings: None,
        on_result: vec![],
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
    });
    crate::db::workflows::update_workflow(&conn, &wf).unwrap();

    let loaded = crate::db::workflows::get_workflow(&conn, "wu1").unwrap().unwrap();
    assert_eq!(loaded.steps.len(), 2);
    assert_eq!(loaded.steps[1].name, "step2");
    assert_eq!(loaded.steps[1].prompt_template, "Second step");
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP Config — secrets_broken detection
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mcp_config_display_secrets_broken_when_decrypt_fails() {
    let conn = test_db();
    let secret_a = crate::core::crypto::generate_secret();
    let secret_b = crate::core::crypto::generate_secret();

    // Insert a server first
    let server = McpServer {
        id: "srv-broken-test".into(),
        name: "TestServer".into(),
        description: "Test server for secrets_broken".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    // Create env and encrypt with secret_a
    let mut env = std::collections::HashMap::new();
    env.insert("TOKEN".to_string(), "test-value-alpha".to_string());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret_a).unwrap();

    let config = McpConfig {
        id: "cfg-broken-test".into(),
        server_id: "srv-broken-test".into(),
        label: "TestConfig".into(),
        env_keys: vec!["TOKEN".into()],
        env_encrypted: encrypted,
        args_override: None,
        is_global: false,
        include_general: true,
        config_hash: "hash-broken-test".into(),
        project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Decrypt with wrong secret (secret_b) → secrets_broken should be true
    let display = crate::db::mcps::list_configs_display(&conn, Some(&secret_b)).unwrap();
    assert_eq!(display.len(), 1);
    assert!(display[0].secrets_broken, "secrets_broken should be true when decryption fails with wrong key");
}

#[test]
fn mcp_config_display_secrets_ok_when_decrypt_succeeds() {
    let conn = test_db();
    let secret_a = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv-ok-test".into(),
        name: "TestServerOk".into(),
        description: "Test".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let mut env = std::collections::HashMap::new();
    env.insert("SECRET_KEY".to_string(), "value-beta".to_string());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret_a).unwrap();

    let config = McpConfig {
        id: "cfg-ok-test".into(),
        server_id: "srv-ok-test".into(),
        label: "OkConfig".into(),
        env_keys: vec!["SECRET_KEY".into()],
        env_encrypted: encrypted,
        args_override: None,
        is_global: false,
        include_general: true,
        config_hash: "hash-ok-test".into(),
        project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Decrypt with correct secret → secrets_broken should be false
    let display = crate::db::mcps::list_configs_display(&conn, Some(&secret_a)).unwrap();
    assert_eq!(display.len(), 1);
    assert!(!display[0].secrets_broken, "secrets_broken should be false when decryption succeeds");
}

#[test]
fn mcp_config_display_secrets_broken_false_when_no_env() {
    let conn = test_db();
    let secret_a = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv-noenv-test".into(),
        name: "TestServerNoEnv".into(),
        description: "Test".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let config = McpConfig {
        id: "cfg-noenv-test".into(),
        server_id: "srv-noenv-test".into(),
        label: "NoEnvConfig".into(),
        env_keys: vec![],        // no env keys
        env_encrypted: String::new(), // no encrypted data
        args_override: None,
        is_global: false,
        include_general: true,
        config_hash: "hash-noenv-test".into(),
        project_ids: vec![],
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    let display = crate::db::mcps::list_configs_display(&conn, Some(&secret_a)).unwrap();
    assert_eq!(display.len(), 1);
    assert!(!display[0].secrets_broken, "secrets_broken should be false when no env keys exist");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Project path remapping
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn update_project_path_success() {
    let conn = test_db();
    let p = sample_project("p-remap", "RemapTest");
    crate::db::projects::insert_project(&conn, &p).unwrap();

    let updated = crate::db::projects::update_project_path(&conn, "p-remap", "/new/path").unwrap();
    assert!(updated, "update_project_path should return true for existing project");

    let project = crate::db::projects::get_project(&conn, "p-remap").unwrap().unwrap();
    assert_eq!(project.path, "/new/path");
}

#[test]
fn update_project_path_nonexistent() {
    let conn = test_db();
    let updated = crate::db::projects::update_project_path(&conn, "nonexistent", "/any").unwrap();
    assert!(!updated, "update_project_path should return false for nonexistent project");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contact export/import round-trip
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn contacts_export_import_roundtrip() {
    let conn = test_db();
    let now = Utc::now();
    let contact = Contact {
        id: "c1".into(),
        pseudo: "PeerAlpha".into(),
        avatar_email: Some("alpha@test.dev".into()),
        kronn_url: "http://localhost:3140".into(),
        invite_code: "abc123".into(),
        status: "accepted".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::contacts::insert_contact(&conn, &contact).unwrap();

    let contacts = crate::db::contacts::list_contacts(&conn).unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].pseudo, "PeerAlpha");

    // Simulate import: clear and re-insert
    conn.execute_batch("DELETE FROM contacts;").unwrap();
    assert_eq!(crate::db::contacts::list_contacts(&conn).unwrap().len(), 0);

    crate::db::contacts::insert_contact(&conn, &contact).unwrap();
    let reimported = crate::db::contacts::list_contacts(&conn).unwrap();
    assert_eq!(reimported.len(), 1);
    assert_eq!(reimported[0].id, "c1");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Prompts CRUD
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn quick_prompt_crud() {
    let conn = test_db();
    let now = Utc::now();

    // Insert
    let qp = QuickPrompt {
        id: "qp1".into(),
        name: "Analyse ticket".into(),
        icon: "🔍".into(),
        prompt_template: "Analyse le ticket {{ticket}} sur le projet {{project}}".into(),
        variables: vec![
            crate::models::PromptVariable {
                name: "ticket".into(), label: "Ticket".into(), placeholder: "PROJ-123".into(),
                description: Some("Identifiant Jira du ticket à analyser".into()), required: true,
            },
            crate::models::PromptVariable {
                name: "project".into(), label: "Projet".into(), placeholder: "front_euronews".into(),
                description: None, required: true,
            },
        ],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        tier: crate::models::ModelTier::Default,
        description: "Analyse technique d'un ticket Jira pour cadrage".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    // List
    let all = crate::db::quick_prompts::list_quick_prompts(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "Analyse ticket");
    assert_eq!(all[0].variables.len(), 2);
    assert_eq!(all[0].variables[0].name, "ticket");

    // Get
    let found = crate::db::quick_prompts::get_quick_prompt(&conn, "qp1").unwrap();
    assert!(found.is_some());
    let found_qp = found.unwrap();
    assert_eq!(found_qp.prompt_template, "Analyse le ticket {{ticket}} sur le projet {{project}}");
    // v2 fields: description on the QP + per-variable description/required
    assert_eq!(found_qp.description, "Analyse technique d'un ticket Jira pour cadrage");
    assert_eq!(found_qp.variables[0].description.as_deref(), Some("Identifiant Jira du ticket à analyser"));
    assert!(found_qp.variables[0].required);
    assert!(found_qp.variables[1].description.is_none());

    // Update
    let mut updated = qp.clone();
    updated.name = "Analyse ticket v2".into();
    updated.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &updated).unwrap();
    let found2 = crate::db::quick_prompts::get_quick_prompt(&conn, "qp1").unwrap().unwrap();
    assert_eq!(found2.name, "Analyse ticket v2");

    // Delete
    crate::db::quick_prompts::delete_quick_prompt(&conn, "qp1").unwrap();
    let all2 = crate::db::quick_prompts::list_quick_prompts(&conn).unwrap();
    assert_eq!(all2.len(), 0);
}

#[test]
fn quick_prompt_not_found() {
    let conn = test_db();
    let found = crate::db::quick_prompts::get_quick_prompt(&conn, "nonexistent").unwrap();
    assert!(found.is_none());
}

#[test]
fn quick_prompt_variables_roundtrip() {
    let conn = test_db();
    let now = Utc::now();
    let qp = QuickPrompt {
        id: "qp-vars".into(),
        name: "Test vars".into(),
        icon: "🔍".into(),
        prompt_template: "{{#jira}}Ticket {{jira}}, {{/jira}}{{#pr}}PR #{{pr}}{{/pr}}".into(),
        variables: vec![
            crate::models::PromptVariable {
                name: "jira".into(), label: "Ticket Jira".into(), placeholder: "PROJ-123".into(),
                description: None, required: false,
            },
            crate::models::PromptVariable {
                name: "pr".into(), label: "PR".into(), placeholder: "42".into(),
                description: None, required: false,
            },
        ],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec!["security".into()],
        tier: crate::models::ModelTier::Reasoning,
        description: String::new(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    let loaded = crate::db::quick_prompts::get_quick_prompt(&conn, "qp-vars").unwrap().unwrap();

    // Variables preserved
    assert_eq!(loaded.variables.len(), 2);
    assert_eq!(loaded.variables[0].name, "jira");
    assert_eq!(loaded.variables[0].placeholder, "PROJ-123");
    assert_eq!(loaded.variables[1].name, "pr");

    // Skill IDs preserved
    assert_eq!(loaded.skill_ids, vec!["security"]);

    // Tier preserved
    assert_eq!(loaded.tier, crate::models::ModelTier::Reasoning);

    // Template with conditional sections preserved
    assert!(loaded.prompt_template.contains("{{#jira}}"));
    assert!(loaded.prompt_template.contains("{{/pr}}"));
}
