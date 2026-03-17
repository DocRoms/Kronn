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
            summary_cache: None,
            summary_up_to_msg_idx: None,
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
            model_tier: None,
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

        for agent in &[AgentType::ClaudeCode, AgentType::Codex, AgentType::Vibe, AgentType::GeminiCli] {
            let id = format!("d-{:?}", agent);
            let mut disc = make_discussion(&id);
            disc.agent = agent.clone();
            insert_discussion(&conn, &disc).unwrap();

            let loaded = get_discussion(&conn, &id).unwrap().unwrap();
            assert_eq!(loaded.agent, *agent);
        }
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
}
