#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rusqlite::Connection;
    use crate::db::migrations;
    use crate::db::discussions::*;

    /// Create an in-memory database with all migrations applied
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn make_discussion(id: &str) -> Discussion {
        let now = Utc::now();
        Discussion {
            awaiting_agent: false,
            id: id.into(),
            project_id: None,
            title: format!("Discussion {}", id),
            agent: AgentType::ClaudeCode,
            language: "en".into(),
            participants: vec![AgentType::ClaudeCode],
            messages: vec![],
            message_count: 0, non_system_message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_message(id: &str, role: MessageRole, agent: Option<AgentType>) -> DiscussionMessage {
        DiscussionMessage {
            model: None,
            lint_report: None,
            id: id.into(),
            role,
            content: format!("Content of {}", id),
            agent_type: agent,
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None, source_msg_id: None, duration_ms: None,
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // insert_discussion + list_discussions
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn insert_and_list_returns_inserted() {
        let conn = test_conn();
        let disc = make_discussion("d1");
        insert_discussion(&conn, &disc).unwrap();

        let all = list_discussions(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "d1");
        assert_eq!(all[0].title, "Discussion d1");
        assert!(!all[0].archived);
    }

    #[test]
    fn list_returns_multiple_ordered_by_updated_at() {
        let conn = test_conn();
        // Insert two discussions; the second one will have a later updated_at
        let d1 = make_discussion("d1");
        insert_discussion(&conn, &d1).unwrap();

        let d2 = make_discussion("d2");
        insert_discussion(&conn, &d2).unwrap();

        // Update d1 so it becomes most recent
        update_discussion(&conn, "d1", Some("Updated Title"), None, None, None).unwrap();

        let all = list_discussions(&conn).unwrap();
        assert_eq!(all.len(), 2);
        // d1 was updated more recently, should be first (ORDER BY updated_at DESC)
        assert_eq!(all[0].id, "d1");
        assert_eq!(all[1].id, "d2");
    }

    #[test]
    fn list_discussions_by_run_filters_and_orders_by_created_at() {
        let conn = test_conn();
        let base = Utc::now();
        let ts = base.to_rfc3339();

        // discussions.workflow_run_id has a FK to workflow_runs(id), which in
        // turn FK's to workflows(id) — seed the parents so the inserts are
        // FK-valid (foreign_keys=ON in test_conn).
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES ('wf-x', 'Test WF', '{}', '[]', ?1, ?1)",
            rusqlite::params![ts],
        ).unwrap();
        for run_id in ["run-x", "run-y"] {
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, started_at) VALUES (?1, 'wf-x', ?2)",
                rusqlite::params![run_id, ts],
            ).unwrap();
        }

        // Two children of run-x (inserted newest-first to prove ASC sort),
        // one child of a different run, one with no run at all.
        let mut a = make_discussion("child-a");
        a.workflow_run_id = Some("run-x".into());
        a.created_at = base + chrono::Duration::seconds(10);
        insert_discussion(&conn, &a).unwrap();

        let mut b = make_discussion("child-b");
        b.workflow_run_id = Some("run-x".into());
        b.created_at = base; // earlier → should sort first
        insert_discussion(&conn, &b).unwrap();

        let mut other = make_discussion("child-other");
        other.workflow_run_id = Some("run-y".into());
        insert_discussion(&conn, &other).unwrap();

        let orphan = make_discussion("orphan"); // workflow_run_id = None
        insert_discussion(&conn, &orphan).unwrap();

        let run_x = list_discussions_by_run(&conn, "run-x").unwrap();
        assert_eq!(run_x.len(), 2, "only the two run-x children should match");
        // ORDER BY created_at ASC → b (base) before a (base+10s).
        assert_eq!(run_x[0].id, "child-b");
        assert_eq!(run_x[1].id, "child-a");

        // Unknown run → empty, never an error.
        assert!(list_discussions_by_run(&conn, "run-does-not-exist").unwrap().is_empty());
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // insert_message + get_discussion
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn get_discussion_includes_messages() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let msg = make_message("m1", MessageRole::User, None);
        insert_message(&conn, "d1", &msg).unwrap();

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(disc.messages.len(), 1);
        assert_eq!(disc.messages[0].content, "Content of m1");
        assert!(matches!(disc.messages[0].role, MessageRole::User));
    }

    #[test]
    fn get_discussion_not_found_returns_none() {
        let conn = test_conn();
        let result = get_discussion(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // update_discussion — title change
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_discussion_title() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let updated = update_discussion(&conn, "d1", Some("New Title"), None, None, None).unwrap();
        assert!(updated);

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(disc.title, "New Title");
    }

    #[test]
    fn update_discussion_title_nonexistent_returns_false() {
        let conn = test_conn();
        let updated = update_discussion(&conn, "nonexistent", Some("Title"), None, None, None).unwrap();
        assert!(!updated);
    }

    #[test]
    fn update_discussion_no_fields_returns_false() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();
        let updated = update_discussion(&conn, "d1", None, None, None, None).unwrap();
        assert!(!updated);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // update_discussion — archive
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_discussion_archive() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let updated = update_discussion(&conn, "d1", None, Some(true), None, None).unwrap();
        assert!(updated);

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert!(disc.archived);
    }

    #[test]
    fn update_discussion_unarchive() {
        let conn = test_conn();
        let mut disc = make_discussion("d1");
        disc.archived = true;
        insert_discussion(&conn, &disc).unwrap();

        update_discussion(&conn, "d1", None, Some(false), None, None).unwrap();

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert!(!disc.archived);
    }

    #[test]
    fn update_discussion_title_and_archive_together() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        update_discussion(&conn, "d1", Some("Archived Disc"), Some(true), None, None).unwrap();

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(disc.title, "Archived Disc");
        assert!(disc.archived);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // delete_discussion
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn delete_discussion_removes_it() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let deleted = delete_discussion(&conn, "d1").unwrap();
        assert!(deleted);

        let all = list_discussions(&conn).unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn delete_discussion_nonexistent_returns_false() {
        let conn = test_conn();
        let deleted = delete_discussion(&conn, "nonexistent").unwrap();
        assert!(!deleted);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // delete_last_agent_messages
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn delete_last_agent_messages_removes_agent_and_system() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        // User -> Agent -> System (trailing non-user messages)
        insert_message(&conn, "d1", &make_message("m1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d1", &make_message("m2", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();
        insert_message(&conn, "d1", &make_message("m3", MessageRole::System, None)).unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 2);

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, MessageRole::User));
        assert_eq!(messages[0].id, "m1");
    }

    #[test]
    fn delete_last_agent_messages_preserves_earlier_agent_messages() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        // User -> Agent -> User -> Agent (only the last Agent after last User should go)
        insert_message(&conn, "d1", &make_message("m1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d1", &make_message("m2", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();
        insert_message(&conn, "d1", &make_message("m3", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d1", &make_message("m4", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 1); // Only m4

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[1].id, "m2");
        assert_eq!(messages[2].id, "m3");
    }

    #[test]
    fn delete_last_agent_messages_no_user_messages_deletes_all() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        // Only agent messages, no user messages
        insert_message(&conn, "d1", &make_message("m1", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();
        insert_message(&conn, "d1", &make_message("m2", MessageRole::System, None)).unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 2);

        let messages = list_messages(&conn, "d1").unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn delete_last_agent_messages_nothing_to_delete() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        // Only a user message at the end
        insert_message(&conn, "d1", &make_message("m1", MessageRole::User, None)).unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 0);

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // edit_last_user_message
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn edit_last_user_message_updates_content() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();
        insert_message(&conn, "d1", &make_message("m1", MessageRole::User, None)).unwrap();

        let edited = edit_last_user_message(&conn, "d1", "new content").unwrap();
        assert!(edited);

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages[0].content, "new content");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // update_message_tokens
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_message_tokens_sets_values() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();
        insert_message(&conn, "d1", &make_message("m1", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();

        update_message_tokens(&conn, "m1", 2500, Some("override")).unwrap();

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages[0].tokens_used, 2500);
        assert_eq!(messages[0].auth_mode, Some("override".into()));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // update_discussion_participants
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_participants() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let new_participants = vec![AgentType::ClaudeCode, AgentType::Codex];
        update_discussion_participants(&conn, "d1", &new_participants).unwrap();

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(disc.participants.len(), 2);
        assert_eq!(disc.participants[0], AgentType::ClaudeCode);
        assert_eq!(disc.participants[1], AgentType::Codex);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Message ordering
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn messages_maintain_insertion_order() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        for i in 1..=10 {
            let msg = make_message(&format!("m{}", i), MessageRole::User, None);
            insert_message(&conn, "d1", &msg).unwrap();
        }

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 10);
        for (i, msg) in messages.iter().enumerate() {
            assert_eq!(msg.id, format!("m{}", i + 1));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Agent type round-trip
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn agent_type_round_trips_through_db() {
        let conn = test_conn();

        for agent in &[AgentType::ClaudeCode, AgentType::Codex, AgentType::Vibe, AgentType::GeminiCli, AgentType::Kiro, AgentType::CopilotCli] {
            let id = format!("d-{:?}", agent);
            let mut disc = make_discussion(&id);
            disc.agent = agent.clone();
            insert_discussion(&conn, &disc).unwrap();

            let loaded = get_discussion(&conn, &id).unwrap().unwrap();
            assert_eq!(loaded.agent, *agent);
        }
    }

    #[test]
    fn agent_type_db_string_format_is_stable() {
        // Ensure the DB string representation never changes (would break existing data)
        let conn = test_conn();
        let mut disc = make_discussion("d-format-check");
        disc.agent = AgentType::CopilotCli;
        insert_discussion(&conn, &disc).unwrap();

        // Read raw string from DB to verify format
        let raw: String = conn.query_row(
            "SELECT agent FROM discussions WHERE id = 'd-format-check'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(raw, "CopilotCli", "DB string for CopilotCli must be 'CopilotCli'");
    }

    #[test]
    fn unknown_agent_type_in_db_becomes_custom() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
             VALUES ('d-unknown', 'test', 'FutureAgent', 'en', '[]', datetime('now'), datetime('now'))",
            [],
        ).unwrap();
        let loaded = get_discussion(&conn, "d-unknown").unwrap().unwrap();
        assert_eq!(loaded.agent, AgentType::Custom, "Unknown agent strings should map to Custom");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // skill_ids persistence
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn insert_discussion_with_skill_ids() {
        let conn = test_conn();
        let mut disc = make_discussion("d1");
        disc.skill_ids = vec!["token-saver".into(), "rust-dev".into()];
        insert_discussion(&conn, &disc).unwrap();

        let loaded = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(loaded.skill_ids, vec!["token-saver", "rust-dev"]);
    }

    #[test]
    fn insert_discussion_empty_skill_ids() {
        let conn = test_conn();
        let disc = make_discussion("d1");
        insert_discussion(&conn, &disc).unwrap();

        let loaded = get_discussion(&conn, "d1").unwrap().unwrap();
        assert!(loaded.skill_ids.is_empty());
    }

    #[test]
    fn update_skill_ids_sets_values() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let updated = update_discussion_skill_ids(&conn, "d1", &["security-auditor".into()]).unwrap();
        assert!(updated);

        let loaded = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(loaded.skill_ids, vec!["security-auditor"]);
    }

    #[test]
    fn update_skill_ids_to_empty() {
        let conn = test_conn();
        let mut disc = make_discussion("d1");
        disc.skill_ids = vec!["token-saver".into()];
        insert_discussion(&conn, &disc).unwrap();

        update_discussion_skill_ids(&conn, "d1", &[]).unwrap();

        let loaded = get_discussion(&conn, "d1").unwrap().unwrap();
        assert!(loaded.skill_ids.is_empty());
    }

    #[test]
    fn list_discussions_includes_skill_ids() {
        let conn = test_conn();
        let mut disc = make_discussion("d1");
        disc.skill_ids = vec!["rust-dev".into()];
        insert_discussion(&conn, &disc).unwrap();

        let all = list_discussions(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].skill_ids, vec!["rust-dev"]);
    }

    #[test]
    fn update_discussion_agent_changes_primary_agent() {
        let conn = test_conn();
        let disc = make_discussion("agent-switch");
        insert_discussion(&conn, &disc).unwrap();

        // Verify initial agent
        let before = get_discussion(&conn, "agent-switch").unwrap().unwrap();
        assert!(matches!(before.agent, AgentType::ClaudeCode));

        // Switch to GeminiCli
        let updated = update_discussion_agent(&conn, "agent-switch", &AgentType::GeminiCli).unwrap();
        assert!(updated);

        let after = get_discussion(&conn, "agent-switch").unwrap().unwrap();
        assert!(matches!(after.agent, AgentType::GeminiCli));
    }

    #[test]
    fn update_discussion_agent_nonexistent_returns_false() {
        let conn = test_conn();
        let updated = update_discussion_agent(&conn, "nonexistent", &AgentType::Vibe).unwrap();
        assert!(!updated);
    }

    #[test]
    fn agent_switch_invalidates_summary_cache() {
        let conn = test_conn();
        let disc = make_discussion("switch-summary");
        insert_discussion(&conn, &disc).unwrap();

        // Set a summary cache
        update_summary_cache(&conn, "switch-summary", "Previous summary text", 5).unwrap();
        let before = get_discussion(&conn, "switch-summary").unwrap().unwrap();
        assert!(before.summary_cache.is_some());

        // Switch agent — caller is responsible for invalidating summary
        update_discussion_agent(&conn, "switch-summary", &AgentType::Kiro).unwrap();
        invalidate_summary_cache(&conn, "switch-summary").unwrap();

        let after = get_discussion(&conn, "switch-summary").unwrap().unwrap();
        assert!(matches!(after.agent, AgentType::Kiro));
        assert!(after.summary_cache.is_none(), "Summary should be invalidated after agent switch");
    }

    #[test]
    fn agent_switch_message_is_inserted() {
        let conn = test_conn();
        let disc = make_discussion("switch-msg");
        insert_discussion(&conn, &disc).unwrap();

        // Simulate the switch message insertion (same as API handler does)
        let msg = DiscussionMessage {
            model: None,
            lint_report: None,
            id: "switch-msg-1".into(),
            role: MessageRole::User,
            content: "[Agent switch: ClaudeCode → Kiro] You are now the primary agent.".into(),
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None, source_msg_id: None, duration_ms: None,
        };
        insert_message(&conn, "switch-msg", &msg).unwrap();

        let loaded = get_discussion(&conn, "switch-msg").unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert!(loaded.messages[0].content.contains("Agent switch"));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Context files CRUD
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn insert_and_list_context_files() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-ctx")).unwrap();

        insert_context_file(&conn, "cf1", "d-ctx", "notes.txt", "text/plain", 100, "Hello world", None).unwrap();
        insert_context_file(&conn, "cf2", "d-ctx", "data.csv", "text/csv", 200, "a,b\n1,2", None).unwrap();

        let files = list_context_files(&conn, "d-ctx").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "notes.txt");
        assert_eq!(files[1].filename, "data.csv");
        assert_eq!(files[0].original_size, 100);
        assert_eq!(files[1].extracted_size, 7); // "a,b\n1,2".len()
    }

    #[test]
    fn count_context_files_accuracy() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-count")).unwrap();

        assert_eq!(count_context_files(&conn, "d-count").unwrap(), 0);

        insert_context_file(&conn, "cf1", "d-count", "a.txt", "text/plain", 10, "A", None).unwrap();
        assert_eq!(count_context_files(&conn, "d-count").unwrap(), 1);

        insert_context_file(&conn, "cf2", "d-count", "b.txt", "text/plain", 10, "B", None).unwrap();
        assert_eq!(count_context_files(&conn, "d-count").unwrap(), 2);
    }

    #[test]
    fn delete_context_file_removes_it() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-del")).unwrap();
        insert_context_file(&conn, "cf1", "d-del", "test.txt", "text/plain", 50, "Test", None).unwrap();

        let deleted = delete_context_file(&conn, "d-del", "cf1").unwrap();
        assert!(deleted, "Should return true when file existed");

        let files = list_context_files(&conn, "d-del").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn delete_context_file_wrong_discussion_returns_false() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-a")).unwrap();
        insert_discussion(&conn, &make_discussion("d-b")).unwrap();
        insert_context_file(&conn, "cf1", "d-a", "test.txt", "text/plain", 50, "Test", None).unwrap();

        // Try deleting from wrong discussion
        let deleted = delete_context_file(&conn, "d-b", "cf1").unwrap();
        assert!(!deleted, "Should return false when file doesn't belong to discussion");

        // File should still exist in d-a
        assert_eq!(count_context_files(&conn, "d-a").unwrap(), 1);
    }

    #[test]
    fn get_context_files_for_prompt_text_only() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-prompt")).unwrap();
        insert_context_file(&conn, "cf1", "d-prompt", "code.rs", "text/plain", 100, "fn main() {}", None).unwrap();
        insert_context_file(&conn, "cf2", "d-prompt", "data.sql", "text/plain", 50, "SELECT 1", None).unwrap();

        let entries = get_context_files_for_prompt(&conn, "d-prompt").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].filename, "code.rs");
        assert_eq!(entries[0].text, "fn main() {}");
        assert!(entries[0].disk_path.is_none());
        assert_eq!(entries[1].filename, "data.sql");
    }

    #[test]
    fn get_context_files_for_prompt_with_image() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-img")).unwrap();
        insert_context_file(&conn, "cf1", "d-img", "screenshot.png", "image/png", 5000, "[Image: screenshot.png]", Some("/tmp/screenshot.png")).unwrap();

        let entries = get_context_files_for_prompt(&conn, "d-img").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filename, "screenshot.png");
        assert_eq!(entries[0].disk_path, Some("/tmp/screenshot.png".to_string()));
    }

    #[test]
    fn context_files_cascade_on_discussion_delete() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-cascade")).unwrap();
        insert_context_file(&conn, "cf1", "d-cascade", "file.txt", "text/plain", 10, "X", None).unwrap();

        // Delete the discussion
        conn.execute("DELETE FROM discussions WHERE id = 'd-cascade'", []).unwrap();

        // Context files should be gone (CASCADE)
        assert_eq!(count_context_files(&conn, "d-cascade").unwrap(), 0);
    }

    #[test]
    fn context_file_with_disk_path() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-disk")).unwrap();
        insert_context_file(&conn, "cf1", "d-disk", "chart.png", "image/png", 50000, "[Image]", Some("/project/.kronn/context-files/abc_chart.png")).unwrap();

        let files = list_context_files(&conn, "d-disk").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].disk_path, Some("/project/.kronn/context-files/abc_chart.png".to_string()));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // 0.8.8 — per-message attachments. A freshly uploaded file is "pending"
    // (message_id NULL); send_message pins every pending file of the disc to
    // the new user message so it renders in that bubble instead of the input.
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn freshly_inserted_context_file_is_pending() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-pend")).unwrap();
        insert_context_file(&conn, "cf1", "d-pend", "shot.png", "image/png", 10, "[Image]", Some("/tmp/shot.png")).unwrap();

        // Uploaded but not yet sent → no message_id.
        let files = list_context_files(&conn, "d-pend").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].message_id, None, "an upload is pending until a message is sent");
    }

    #[test]
    fn link_pending_pins_only_unattached_files_and_returns_count() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-link")).unwrap();
        // Two pending uploads + one already attached to an older message.
        insert_context_file(&conn, "cf1", "d-link", "a.png", "image/png", 10, "[Image]", Some("/tmp/a.png")).unwrap();
        insert_context_file(&conn, "cf2", "d-link", "b.png", "image/png", 10, "[Image]", Some("/tmp/b.png")).unwrap();
        insert_context_file(&conn, "cf3", "d-link", "old.png", "image/png", 10, "[Image]", Some("/tmp/old.png")).unwrap();
        link_pending_context_files_to_message(&conn, "d-link", "msg-old").unwrap();

        // Two MORE pending uploads arrive, then the user sends a new message.
        insert_context_file(&conn, "cf4", "d-link", "c.png", "image/png", 10, "[Image]", Some("/tmp/c.png")).unwrap();
        insert_context_file(&conn, "cf5", "d-link", "d.png", "image/png", 10, "[Image]", Some("/tmp/d.png")).unwrap();
        let n = link_pending_context_files_to_message(&conn, "d-link", "msg-new").unwrap();

        assert_eq!(n, 2, "only the two still-pending files get pinned to the new message");
        let on_new = list_context_files_for_message(&conn, "msg-new").unwrap();
        assert_eq!(on_new.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec!["cf4", "cf5"]);
        // The earlier batch stays on its original message — never re-pinned.
        let on_old = list_context_files_for_message(&conn, "msg-old").unwrap();
        assert_eq!(on_old.len(), 3);
    }

    #[test]
    fn link_pending_is_a_no_op_when_nothing_is_pending() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-noop")).unwrap();
        insert_context_file(&conn, "cf1", "d-noop", "a.png", "image/png", 10, "[Image]", Some("/tmp/a.png")).unwrap();
        link_pending_context_files_to_message(&conn, "d-noop", "msg-1").unwrap();

        // A second send with no new uploads links nothing.
        let n = link_pending_context_files_to_message(&conn, "d-noop", "msg-2").unwrap();
        assert_eq!(n, 0);
        assert!(list_context_files_for_message(&conn, "msg-2").unwrap().is_empty());
    }

    #[test]
    fn link_pending_is_scoped_to_one_discussion() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-x")).unwrap();
        insert_discussion(&conn, &make_discussion("d-y")).unwrap();
        insert_context_file(&conn, "cfx", "d-x", "x.png", "image/png", 10, "[Image]", Some("/tmp/x.png")).unwrap();
        insert_context_file(&conn, "cfy", "d-y", "y.png", "image/png", 10, "[Image]", Some("/tmp/y.png")).unwrap();

        let n = link_pending_context_files_to_message(&conn, "d-x", "msg-x").unwrap();
        assert_eq!(n, 1, "a send in d-x must not touch pending files of d-y");
        // d-y's file is still pending.
        let y = list_context_files(&conn, "d-y").unwrap();
        assert_eq!(y[0].message_id, None);
    }

    #[test]
    fn migration_067_backfill_separates_legacy_files_from_pending() {
        // Reproduce a pre-0.8.8 state: files uploaded before message_id existed
        // (so they're NULL = would be "pending") alongside one already pinned.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-legacy")).unwrap();
        insert_context_file(&conn, "old1", "d-legacy", "spec.pdf", "application/pdf", 10, "ref", None).unwrap();
        insert_context_file(&conn, "old2", "d-legacy", "data.csv", "text/csv", 10, "a,b", None).unwrap();
        insert_context_file(&conn, "pinned", "d-legacy", "shot.png", "image/png", 10, "[Image]", Some("/tmp/s.png")).unwrap();
        link_pending_context_files_to_message(&conn, "d-legacy", "msg-real").unwrap(); // pins old1, old2, pinned

        // Re-create the "uploaded before the column" case: two fresh NULL rows.
        insert_context_file(&conn, "legacyA", "d-legacy", "a.txt", "text/plain", 1, "A", None).unwrap();
        insert_context_file(&conn, "legacyB", "d-legacy", "b.txt", "text/plain", 1, "B", None).unwrap();

        // Apply the exact backfill migration SQL.
        conn.execute_batch(include_str!("sql/067_context_files_backfill_legacy.sql")).unwrap();

        // The NULL rows are now the inert sentinel — NOT pending.
        let a: Option<String> = conn.query_row("SELECT message_id FROM context_files WHERE id='legacyA'", [], |r| r.get(0)).unwrap();
        let b: Option<String> = conn.query_row("SELECT message_id FROM context_files WHERE id='legacyB'", [], |r| r.get(0)).unwrap();
        assert_eq!(a.as_deref(), Some("__legacy_disc_wide__"));
        assert_eq!(b.as_deref(), Some("__legacy_disc_wide__"));
        // The already-pinned files keep their real message id.
        let p: Option<String> = conn.query_row("SELECT message_id FROM context_files WHERE id='old1'", [], |r| r.get(0)).unwrap();
        assert_eq!(p.as_deref(), Some("msg-real"));

        // Crucially: a later send links NOTHING — legacy files are no longer
        // pending, so they can't be vacuumed into a new message.
        let n = link_pending_context_files_to_message(&conn, "d-legacy", "msg-next").unwrap();
        assert_eq!(n, 0, "backfilled legacy files must not attach to the next message");
        assert!(list_context_files_for_message(&conn, "msg-next").unwrap().is_empty());
        // ...and they stay disc-wide context (still listed for the discussion).
        assert_eq!(list_context_files(&conn, "d-legacy").unwrap().len(), 5);
    }

    #[test]
    fn list_for_message_returns_message_id_on_each_row() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-roundtrip")).unwrap();
        insert_context_file(&conn, "cf1", "d-roundtrip", "a.png", "image/png", 10, "[Image]", Some("/tmp/a.png")).unwrap();
        link_pending_context_files_to_message(&conn, "d-roundtrip", "msg-rt").unwrap();

        let per_msg = list_context_files_for_message(&conn, "msg-rt").unwrap();
        assert_eq!(per_msg.len(), 1);
        assert_eq!(per_msg[0].message_id, Some("msg-rt".to_string()));
        // The disc-wide listing now also reflects the link.
        let all = list_context_files(&conn, "d-roundtrip").unwrap();
        assert_eq!(all[0].message_id, Some("msg-rt".to_string()));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // 0.8.7 — non_system_message_count: the unread-badge basis.
    //
    // The streaming layer persists every tool call + every cached-summary
    // breadcrumb as its own `MessageRole::System` message, which inflates
    // `message_count`. The user-facing "messages à lire" badge tracks
    // `non_system_message_count` instead. These tests pin that contract.
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn non_system_message_count_excludes_system_rows() {
        let conn = test_conn();
        let disc = make_discussion("d-mix");
        insert_discussion(&conn, &disc).unwrap();

        // Two real exchanges (User → Agent) + six System breadcrumbs
        // (simulates a workflow run with 6 tool / summary lines per reply).
        insert_message(&conn, "d-mix", &make_message("u1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d-mix", &make_message("a1", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();
        for i in 0..6 {
            insert_message(
                &conn, "d-mix",
                &make_message(&format!("s{i}"), MessageRole::System, None),
            ).unwrap();
        }

        let listed = list_discussions(&conn).unwrap();
        let d = listed.iter().find(|d| d.id == "d-mix").unwrap();
        assert_eq!(d.message_count, 8, "total includes System rows");
        assert_eq!(
            d.non_system_message_count, 2,
            "the badge basis must exclude System rows (1 User + 1 Agent = 2)"
        );

        // get_discussion path populates the field from the loaded messages
        // array (not the SQL subquery) — both code paths must agree.
        let got = get_discussion(&conn, "d-mix").unwrap().unwrap();
        assert_eq!(got.message_count, 8);
        assert_eq!(got.non_system_message_count, 2);
    }

    #[test]
    fn non_system_message_count_is_zero_for_empty_discussion() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-empty")).unwrap();
        let listed = list_discussions(&conn).unwrap();
        let d = listed.iter().find(|d| d.id == "d-empty").unwrap();
        assert_eq!(d.message_count, 0);
        assert_eq!(d.non_system_message_count, 0);
    }

    #[test]
    fn non_system_message_count_equals_message_count_when_no_system_rows() {
        // Sanity guard: a disc with only User+Agent rows must report both
        // counts equal (otherwise the badge would under-count real replies).
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-clean")).unwrap();
        insert_message(&conn, "d-clean", &make_message("u1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d-clean", &make_message("a1", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();
        insert_message(&conn, "d-clean", &make_message("u2", MessageRole::User, None)).unwrap();

        let listed = list_discussions(&conn).unwrap();
        let d = listed.iter().find(|d| d.id == "d-clean").unwrap();
        assert_eq!(d.message_count, 3);
        assert_eq!(d.non_system_message_count, 3);
    }

    #[test]
    fn pacing_anchors_use_the_reception_clock_not_the_authored_timestamp() {
        // Copilot + Codex reviews (PR 118): a federated message arriving
        // stamped 3h in the past must STILL reset the ramp / renew the
        // lease — the contract is about reception on THIS instance
        // (`received_at`, 072), not the author's clock.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-anchor")).unwrap();

        let mut stale_stamped = make_message("m-stale", MessageRole::User, None);
        stale_stamped.timestamp = Utc::now() - chrono::Duration::hours(3);
        insert_message(&conn, "d-anchor", &stale_stamped).unwrap();

        let recent = Utc::now() - chrono::Duration::seconds(60);
        let any = last_message_at(&conn, "d-anchor").unwrap().unwrap();
        assert!(any > recent, "anchor must be reception time (~now), got {any}");
        let user = last_user_message_at(&conn, "d-anchor").unwrap().unwrap();
        assert!(user > recent, "lease anchor must renew on reception, got {user}");
    }

    #[test]
    fn pacing_anchors_follow_the_newest_row_by_sort_order() {
        // The ordering axis is sort_order (the event log), never a MAX()
        // over clocks — received_at values are skewed by hand to prove it.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-order")).unwrap();
        insert_message(&conn, "d-order", &make_message("m1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d-order", &make_message("m2", MessageRole::User, None)).unwrap();

        let older = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "UPDATE messages SET received_at = ?1 WHERE id = 'm2'",
            rusqlite::params![older],
        )
        .unwrap();

        // m1 has the LARGER received_at, but m2 is the newest event.
        let any = last_message_at(&conn, "d-order").unwrap().unwrap();
        assert_eq!(any.to_rfc3339(), older, "anchor follows sort_order, not MAX(received_at)");
    }

    #[test]
    fn last_user_message_at_skips_agent_rows_and_empty_discs() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-roles")).unwrap();
        assert!(last_message_at(&conn, "d-roles").unwrap().is_none());
        assert!(last_user_message_at(&conn, "d-roles").unwrap().is_none());

        insert_message(&conn, "d-roles", &make_message("u1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d-roles", &make_message("a1", MessageRole::Agent, Some(AgentType::Codex))).unwrap();
        let user_received = (Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        conn.execute(
            "UPDATE messages SET received_at = ?1 WHERE id = 'u1'",
            rusqlite::params![user_received],
        )
        .unwrap();

        // The User lease anchor stays on the User row even though an Agent
        // row is newer; the any-role anchor follows the Agent row.
        let user_anchor = last_user_message_at(&conn, "d-roles").unwrap().unwrap();
        assert_eq!(user_anchor.to_rfc3339(), user_received);
        let any_anchor = last_message_at(&conn, "d-roles").unwrap().unwrap();
        assert!(any_anchor > user_anchor, "any-role anchor follows the newest (Agent) row");
    }

    // ── reconcile_awaiting_agents (boot recovery of owed runs) ──

    #[test]
    fn reconcile_marks_a_queued_owed_disc_and_clears_the_flag() {
        // A batch child (or a human msg) that was owed an agent which never
        // started: last message is the User prompt, no partial. Reconcile
        // appends an interrupted notice, clears the flag, returns the id.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-owed")).unwrap();
        insert_message(&conn, "d-owed", &make_message("u1", MessageRole::User, None)).unwrap();
        set_awaiting_agent(&conn, "d-owed", true).unwrap();

        let marked = reconcile_awaiting_agents(&conn).unwrap();
        assert_eq!(marked, vec!["d-owed".to_string()]);
        // A notice was appended → the disc now has 2 messages, last is Agent.
        let disc = get_discussion(&conn, "d-owed").unwrap().unwrap();
        assert_eq!(disc.messages.len(), 2);
        assert!(matches!(disc.messages[1].role, MessageRole::Agent));
        // Flag cleared → a second reconcile is a no-op.
        assert!(reconcile_awaiting_agents(&conn).unwrap().is_empty());
    }

    #[test]
    fn reconcile_skips_a_disc_already_answered() {
        // Flag left set but the agent DID answer (last message is Agent):
        // no notice, no re-flag, just housekeeping-clear the stale flag.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-done")).unwrap();
        insert_message(&conn, "d-done", &make_message("u1", MessageRole::User, None)).unwrap();
        insert_message(&conn, "d-done", &make_message("a1", MessageRole::Agent, Some(AgentType::ClaudeCode))).unwrap();
        set_awaiting_agent(&conn, "d-done", true).unwrap();

        let marked = reconcile_awaiting_agents(&conn).unwrap();
        assert!(marked.is_empty(), "an answered disc must not be marked interrupted");
        let disc = get_discussion(&conn, "d-done").unwrap().unwrap();
        assert_eq!(disc.messages.len(), 2, "no notice appended");
        // Stale flag cleared so it won't be re-scanned next boot.
        assert!(reconcile_awaiting_agents(&conn).unwrap().is_empty());
    }

    #[test]
    fn reconcile_leaves_a_disc_with_a_live_partial_to_partial_recovery() {
        // awaiting=1 AND a partial checkpoint present → excluded by the WHERE
        // (recover_partial_responses owns it). Flag + partial untouched here.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-partial")).unwrap();
        insert_message(&conn, "d-partial", &make_message("u1", MessageRole::User, None)).unwrap();
        set_awaiting_agent(&conn, "d-partial", true).unwrap();
        set_partial_response(&conn, "d-partial", Some("half a reply")).unwrap();

        let marked = reconcile_awaiting_agents(&conn).unwrap();
        assert!(marked.is_empty(), "a disc with a live partial is left to partial recovery");
        // The partial is still there for recover_partial_responses to convert.
        let disc = get_discussion(&conn, "d-partial").unwrap().unwrap();
        assert_eq!(disc.messages.len(), 1, "no notice, no conversion — recovery owns it");
    }

    #[test]
    fn reconcile_ignores_unflagged_discs() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-plain")).unwrap();
        insert_message(&conn, "d-plain", &make_message("u1", MessageRole::User, None)).unwrap();
        // No set_awaiting_agent → flag stays 0 (default).
        assert!(reconcile_awaiting_agents(&conn).unwrap().is_empty());
    }
}
