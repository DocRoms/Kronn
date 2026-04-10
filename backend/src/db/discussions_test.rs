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
            id: id.into(),
            project_id: None,
            title: format!("Discussion {}", id),
            agent: AgentType::ClaudeCode,
            language: "en".into(),
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

    fn make_message(id: &str, role: MessageRole, agent: Option<AgentType>) -> DiscussionMessage {
        DiscussionMessage {
            id: id.into(),
            role,
            content: format!("Content of {}", id),
            agent_type: agent,
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
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
        update_discussion(&conn, "d1", Some("Updated Title"), None, None).unwrap();

        let all = list_discussions(&conn).unwrap();
        assert_eq!(all.len(), 2);
        // d1 was updated more recently, should be first (ORDER BY updated_at DESC)
        assert_eq!(all[0].id, "d1");
        assert_eq!(all[1].id, "d2");
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

        let updated = update_discussion(&conn, "d1", Some("New Title"), None, None).unwrap();
        assert!(updated);

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(disc.title, "New Title");
    }

    #[test]
    fn update_discussion_title_nonexistent_returns_false() {
        let conn = test_conn();
        let updated = update_discussion(&conn, "nonexistent", Some("Title"), None, None).unwrap();
        assert!(!updated);
    }

    #[test]
    fn update_discussion_no_fields_returns_false() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();
        let updated = update_discussion(&conn, "d1", None, None, None).unwrap();
        assert!(!updated);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // update_discussion — archive
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_discussion_archive() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        let updated = update_discussion(&conn, "d1", None, Some(true), None).unwrap();
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

        update_discussion(&conn, "d1", None, Some(false), None).unwrap();

        let disc = get_discussion(&conn, "d1").unwrap().unwrap();
        assert!(!disc.archived);
    }

    #[test]
    fn update_discussion_title_and_archive_together() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        update_discussion(&conn, "d1", Some("Archived Disc"), Some(true), None).unwrap();

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
            id: "switch-msg-1".into(),
            role: MessageRole::User,
            content: "[Agent switch: ClaudeCode → Kiro] You are now the primary agent.".into(),
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
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
}
