#[cfg(test)]
mod tests {
    use crate::db::discussions::*;
    use crate::db::migrations;
    use chrono::Utc;
    use rusqlite::Connection;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

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
            agent_running: false,
            id: id.into(),
            project_id: None,
            title: format!("Discussion {}", id),
            agent: AgentType::ClaudeCode,
            language: "en".into(),
            participants: vec![AgentType::ClaudeCode],
            messages: vec![],
            message_count: 0,
            non_system_message_count: 0,
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
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
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
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
            id: id.into(),
            role,
            channel: crate::models::MessageChannel::Main,
            content: format!("Content of {}", id),
            agent_type: agent,
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: None,
            duration_ms: None,
            target_agent: None,
            reply_to_message_id: None,
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

    /// After a reload nothing live remains, so the list row is the only thing
    /// left to tell a queued agent from a working one. The flag is read by
    /// column position and a miss is swallowed as `false` — the very state that
    /// used to show an hourglass over a running job — so pin both ends.
    #[test]
    fn list_tells_a_running_agent_from_a_queued_one() {
        use crate::db::agent_dispatch::{self, NewAgentDispatchJob};

        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();
        insert_message(&conn, "d1", &make_message("m1", MessageRole::User, None)).unwrap();

        agent_dispatch::enqueue(
            &conn,
            NewAgentDispatchJob {
                id: "job1",
                discussion_id: "d1",
                trigger_message_id: "m1",
                trigger_sort_order: 1,
                dedupe_key: "d1:m1",
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();

        // Waiting for a slot: not working yet.
        assert!(!list_discussions(&conn).unwrap()[0].agent_running);

        agent_dispatch::claim(&conn, "job1").unwrap().unwrap();

        // Claimed by a worker, but still waiting behind the provider capacity
        // gate: it must remain queued in the UI.
        assert!(!list_discussions(&conn).unwrap()[0].agent_running);

        assert!(agent_dispatch::mark_agent_started(&conn, "job1").unwrap());

        // Provider invocation started: the same row says so, with no stream
        // or in-memory registry involved.
        assert!(list_discussions(&conn).unwrap()[0].agent_running);
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
        )
        .unwrap();
        for run_id in ["run-x", "run-y"] {
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, started_at) VALUES (?1, 'wf-x', ?2)",
                rusqlite::params![run_id, ts],
            )
            .unwrap();
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
        assert!(list_discussions_by_run(&conn, "run-does-not-exist")
            .unwrap()
            .is_empty());
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
        let updated =
            update_discussion(&conn, "nonexistent", Some("Title"), None, None, None).unwrap();
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
        insert_message(
            &conn,
            "d1",
            &make_message("m2", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();
        insert_message(&conn, "d1", &make_message("m3", MessageRole::System, None)).unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 2);

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, MessageRole::User));
        assert_eq!(messages[0].id, "m1");
    }

    /// KT-58 — a structured `@agent` mention used to vanish into the dispatch
    /// job: the message row kept no trace of who it was addressed to, so no
    /// reader could tell "names @codex" from "awaits @codex". The target is now
    /// stamped by the same transaction that enqueues the job.
    #[test]
    fn targeted_dispatch_records_the_target_on_the_message() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-target")).unwrap();

        insert_message_with_targeted_dispatch(
            &conn,
            "d-target",
            &make_message("m-mention", MessageRole::User, None),
            "job-1",
            &AgentType::Codex,
        )
        .unwrap();
        // An ordinary message must stay untargeted — otherwise every mention
        // would look pending.
        insert_message(
            &conn,
            "d-target",
            &make_message("m-plain", MessageRole::User, None),
        )
        .unwrap();

        let messages = list_messages(&conn, "d-target").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].target_agent, Some(AgentType::Codex));
        assert_eq!(messages[1].target_agent, None);
    }

    #[test]
    fn deleted_tail_does_not_reuse_message_sequence() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-sequence")).unwrap();

        assert_eq!(
            insert_message(
                &conn,
                "d-sequence",
                &make_message("m1", MessageRole::User, None)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            insert_message(
                &conn,
                "d-sequence",
                &make_message("m2", MessageRole::Agent, Some(AgentType::Codex))
            )
            .unwrap(),
            2
        );
        assert_eq!(delete_last_agent_messages(&conn, "d-sequence").unwrap(), 1);
        assert_eq!(
            insert_message(
                &conn,
                "d-sequence",
                &make_message("m3", MessageRole::Agent, Some(AgentType::Codex))
            )
            .unwrap(),
            3
        );
    }

    #[test]
    fn concurrent_writers_allocate_distinct_message_sequences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sequence.db");
        let conn = Connection::open(&path).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
            .unwrap();
        migrations::run(&conn).unwrap();
        insert_discussion(&conn, &make_discussion("d-concurrent-sequence")).unwrap();
        drop(conn);

        let writers = 8;
        let barrier = Arc::new(Barrier::new(writers));
        let handles = (0..writers)
            .map(|idx| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_timeout(Duration::from_secs(5)).unwrap();
                    barrier.wait();
                    insert_message(
                        &conn,
                        "d-concurrent-sequence",
                        &make_message(&format!("concurrent-{idx}"), MessageRole::User, None),
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();

        let mut allocated = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        allocated.sort_unstable();
        assert_eq!(allocated, (1..=writers as i64).collect::<Vec<_>>());

        let conn = Connection::open(path).unwrap();
        let stored = list_messages(&conn, "d-concurrent-sequence").unwrap();
        assert_eq!(stored.len(), writers);
    }

    #[test]
    fn delete_last_agent_messages_preserves_earlier_agent_messages() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        // User -> Agent -> User -> Agent (only the last Agent after last User should go)
        insert_message(&conn, "d1", &make_message("m1", MessageRole::User, None)).unwrap();
        insert_message(
            &conn,
            "d1",
            &make_message("m2", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();
        insert_message(&conn, "d1", &make_message("m3", MessageRole::User, None)).unwrap();
        insert_message(
            &conn,
            "d1",
            &make_message("m4", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 1); // Only m4

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[1].id, "m2");
        assert_eq!(messages[2].id, "m3");
    }

    #[test]
    fn delete_last_agent_messages_no_user_messages_is_a_safe_noop() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();

        // Only agent messages, no user messages
        insert_message(
            &conn,
            "d1",
            &make_message("m1", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();
        insert_message(&conn, "d1", &make_message("m2", MessageRole::System, None)).unwrap();

        let deleted = delete_last_agent_messages(&conn, "d1").unwrap();
        assert_eq!(deleted, 0);

        let messages = list_messages(&conn, "d1").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[1].id, "m2");
    }

    #[test]
    fn silent_crash_retry_deletes_only_its_reply_and_preserves_its_job() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-agent-room")).unwrap();

        let trigger_order = insert_message(
            &conn,
            "d-agent-room",
            &make_message("agent-trigger", MessageRole::Agent, Some(AgentType::Codex)),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-agent-room",
            &make_message("room-context", MessageRole::System, None),
        )
        .unwrap();

        crate::db::agent_dispatch::enqueue(
            &conn,
            crate::db::agent_dispatch::NewAgentDispatchJob {
                id: "dispatch-1",
                discussion_id: "d-agent-room",
                trigger_message_id: "agent-trigger",
                trigger_sort_order: trigger_order,
                dedupe_key: "agent-room-dispatch-1",
                agent_override: Some(&AgentType::ClaudeCode),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        crate::db::agent_dispatch::claim(&conn, "dispatch-1")
            .unwrap()
            .unwrap();

        insert_message(
            &conn,
            "d-agent-room",
            &make_message(
                "failed-reply",
                MessageRole::Agent,
                Some(AgentType::ClaudeCode),
            ),
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET agent_dispatch_job_id = 'dispatch-1' WHERE id = 'failed-reply'",
            [],
        )
        .unwrap();

        assert!(crate::db::agent_dispatch::retry_after(
            &conn,
            "dispatch-1",
            5,
            "silent_agent_crash"
        )
        .unwrap());
        assert_eq!(
            delete_dispatch_reply_messages(&conn, "d-agent-room", "dispatch-1").unwrap(),
            1
        );

        let messages = list_messages(&conn, "d-agent-room").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "agent-trigger");
        assert_eq!(messages[1].id, "room-context");
        let job = crate::db::agent_dispatch::get(&conn, "dispatch-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            job.status,
            crate::db::agent_dispatch::DispatchStatus::Pending
        );
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

    #[test]
    fn atomic_revision_tombstones_tail_and_enqueues_exactly_one_job() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-revise")).unwrap();
        let mut user = make_message("u-revise", MessageRole::User, None);
        user.content = "before".into();
        insert_message(&conn, "d-revise", &user).unwrap();
        insert_message(
            &conn,
            "d-revise",
            &make_message("a-revise", MessageRole::Agent, Some(AgentType::Codex)),
        )
        .unwrap();
        update_summary_cache(&conn, "d-revise", "stale summary", 2).unwrap();
        let expected_revision = list_messages(&conn, "d-revise").unwrap()[0]
            .timestamp
            .to_rfc3339();

        let targets = [MessageTarget::agent(AgentType::Codex)];
        let first_dispatches = [UserDispatchSpec {
            job_id: "dispatch-revise-1",
            agent_override: Some(&AgentType::Codex),
            dedupe_key: None,
        }];
        let first = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-revise",
                message_id: "u-revise",
                content: "after",
                expected_revision: &expected_revision,
                idempotency_key: "revise-key-1",
                targets: &targets,
                dispatches: &first_dispatches,
            },
        )
        .unwrap();
        assert!(!first.receipt.duplicate);
        assert_eq!(
            first.claimed_dispatch.as_ref().map(|job| job.id.as_str()),
            Some("dispatch-revise-1")
        );
        assert_eq!(
            first.claimed_dispatch.as_ref().map(|job| job.status),
            Some(crate::db::agent_dispatch::DispatchStatus::Running)
        );

        let projected = list_messages(&conn, "d-revise").unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, "u-revise");
        assert_eq!(projected[0].content, "after");
        let tombstones: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_tombstones
                 WHERE discussion_id = 'd-revise' AND id = 'a-revise' AND sort_order = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstones, 1);
        let message_count: i64 = conn
            .query_row(
                "SELECT message_count FROM discussions WHERE id = 'd-revise'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(message_count, 1);
        let summary: Option<String> = conn
            .query_row(
                "SELECT summary_cache FROM discussions WHERE id = 'd-revise'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(summary.is_none());

        let duplicate_dispatches = [UserDispatchSpec {
            job_id: "dispatch-revise-2",
            agent_override: Some(&AgentType::Codex),
            dedupe_key: None,
        }];
        let duplicate = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-revise",
                message_id: "u-revise",
                content: "after",
                expected_revision: &expected_revision,
                idempotency_key: "revise-key-1",
                targets: &targets,
                dispatches: &duplicate_dispatches,
            },
        )
        .unwrap();
        assert!(duplicate.receipt.duplicate);
        assert!(duplicate.claimed_dispatch.is_none());
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE dedupe_key = 'revision:revise-key-1:Codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_revision_events
                 WHERE idempotency_key = 'revise-key-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(jobs, 1);
        assert_eq!(events, 1);
    }

    #[test]
    fn atomic_revision_accepts_api_utc_timestamp_as_the_same_cas_revision() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-revise-api-timestamp")).unwrap();
        let mut user = make_message("u-revise-api-timestamp", MessageRole::User, None);
        user.content = "before".into();
        insert_message(&conn, "d-revise-api-timestamp", &user).unwrap();

        let stored_revision: String = conn
            .query_row(
                "SELECT timestamp FROM messages WHERE id = 'u-revise-api-timestamp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored_revision.ends_with("+00:00"));
        let api_revision = chrono::DateTime::parse_from_rfc3339(&stored_revision)
            .unwrap()
            .with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        assert!(api_revision.ends_with('Z'));

        let revised = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-revise-api-timestamp",
                message_id: "u-revise-api-timestamp",
                content: "after",
                expected_revision: &api_revision,
                idempotency_key: "api-timestamp-revision",
                targets: &[],
                dispatches: &[],
            },
        )
        .unwrap();

        assert!(!revised.receipt.duplicate);
        assert_eq!(
            list_messages(&conn, "d-revise-api-timestamp").unwrap()[0].content,
            "after"
        );
    }

    #[test]
    fn atomic_revision_without_local_dispatch_uses_cas_and_no_job() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-peer-revise")).unwrap();
        let mut user = make_message("u-peer", MessageRole::User, None);
        user.content = "before".into();
        insert_message(&conn, "d-peer-revise", &user).unwrap();
        let expected_revision = list_messages(&conn, "d-peer-revise").unwrap()[0]
            .timestamp
            .to_rfc3339();

        let first = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-peer-revise",
                message_id: "u-peer",
                content: "first edit",
                expected_revision: &expected_revision,
                idempotency_key: "peer-revision-1",
                targets: &[],
                dispatches: &[],
            },
        )
        .unwrap();
        assert!(first.claimed_dispatch.is_none());
        assert!(first.receipt.dispatch_job_id.is_none());

        let conflict = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-peer-revise",
                message_id: "u-peer",
                content: "divergent edit",
                expected_revision: &expected_revision,
                idempotency_key: "peer-revision-2",
                targets: &[],
                dispatches: &[],
            },
        )
        .unwrap_err();
        assert!(matches!(conflict, ReviseMessageError::Conflict { .. }));
        let jobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(jobs, 0);
        assert_eq!(
            list_messages(&conn, "d-peer-revise").unwrap()[0].content,
            "first edit"
        );
    }

    #[test]
    fn atomic_revision_replaces_plural_targets_and_enqueues_each_once() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-plural-revise")).unwrap();
        let mut user = make_message("u-plural-revise", MessageRole::User, None);
        user.content = "before".into();
        insert_message(&conn, "d-plural-revise", &user).unwrap();
        let expected_revision = list_messages(&conn, "d-plural-revise").unwrap()[0]
            .timestamp
            .to_rfc3339();
        let targets = [
            MessageTarget::agent(AgentType::Codex),
            MessageTarget::agent(AgentType::ClaudeCode),
        ];
        let dispatches = [
            UserDispatchSpec {
                job_id: "plural-revise-codex",
                agent_override: Some(&AgentType::Codex),
                dedupe_key: None,
            },
            UserDispatchSpec {
                job_id: "plural-revise-claude",
                agent_override: Some(&AgentType::ClaudeCode),
                dedupe_key: None,
            },
        ];

        let outcome = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-plural-revise",
                message_id: "u-plural-revise",
                content: "@codex after @claude",
                expected_revision: &expected_revision,
                idempotency_key: "plural-revision-key",
                targets: &targets,
                dispatches: &dispatches,
            },
        )
        .unwrap();

        assert_eq!(
            outcome.claimed_dispatch.as_ref().map(|job| job.id.as_str()),
            Some("plural-revise-codex"),
        );
        assert_eq!(
            list_message_targets(&conn, "u-plural-revise").unwrap(),
            targets,
        );
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE trigger_message_id = 'u-plural-revise'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(jobs, 2);
        assert_eq!(
            crate::db::agent_dispatch::get(&conn, "plural-revise-claude")
                .unwrap()
                .unwrap()
                .status,
            crate::db::agent_dispatch::DispatchStatus::Pending,
        );
    }

    #[test]
    fn stale_revision_dispatch_is_cancelled_when_native_agent_was_disabled() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-disabled-revise")).unwrap();
        let mut user = make_message("u-disabled-revise", MessageRole::User, None);
        user.content = "before".into();
        insert_message(&conn, "d-disabled-revise", &user).unwrap();
        conn.execute(
            "UPDATE discussions SET no_agent = 1 WHERE id = 'd-disabled-revise'",
            [],
        )
        .unwrap();
        let expected_revision = list_messages(&conn, "d-disabled-revise").unwrap()[0]
            .timestamp
            .to_rfc3339();

        let targets = [MessageTarget::agent(AgentType::Codex)];
        let dispatches = [UserDispatchSpec {
            job_id: "dispatch-disabled-revise",
            agent_override: Some(&AgentType::Codex),
            dedupe_key: None,
        }];
        let revised = revise_message_with_dispatch(
            &conn,
            ReviseMessageParams {
                discussion_id: "d-disabled-revise",
                message_id: "u-disabled-revise",
                content: "after",
                expected_revision: &expected_revision,
                idempotency_key: "disabled-revision",
                targets: &targets,
                dispatches: &dispatches,
            },
        )
        .unwrap();

        assert!(revised.claimed_dispatch.is_none());
        let job = crate::db::agent_dispatch::get(&conn, "dispatch-disabled-revise")
            .unwrap()
            .unwrap();
        assert_eq!(
            job.status,
            crate::db::agent_dispatch::DispatchStatus::Cancelled
        );
        assert_eq!(job.last_error.as_deref(), Some("agent_disabled"));
        assert_eq!(
            conn.query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'd-disabled-revise'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn remote_revision_converges_by_content_hash_and_is_idempotent() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-remote-revise")).unwrap();
        let mut user = make_message("u-remote", MessageRole::User, None);
        user.content = "same content on both mirrors".into();
        insert_message(&conn, "d-remote-revise", &user).unwrap();
        insert_message(
            &conn,
            "d-remote-revise",
            &make_message("a-remote", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();

        let previous_content_hash = content_hash("same content on both mirrors");
        let event = MessageRevisionEvent {
            id: "remote-event".into(),
            discussion_id: "d-remote-revise".into(),
            target_message_id: "u-remote".into(),
            previous_content_hash,
            // Deliberately unrelated to the mirror's timestamp: federation
            // CAS is content-hash based because wire timestamps are millis.
            expected_revision: "sender-opaque-revision".into(),
            revision: Utc::now().to_rfc3339(),
            content: "remote edit".into(),
            target_agent: Some(AgentType::Codex),
            idempotency_key: "remote-key".into(),
            sort_order: 99,
            dispatch_job_id: None,
            created_at: Utc::now(),
        };

        assert!(apply_remote_message_revision(&conn, &event).unwrap());
        assert!(!apply_remote_message_revision(&conn, &event).unwrap());
        let messages = list_messages(&conn, "d-remote-revise").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "remote edit");
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_revision_events
                 WHERE idempotency_key = 'remote-key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // update_message_tokens
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_message_tokens_sets_values() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d1")).unwrap();
        insert_message(
            &conn,
            "d1",
            &make_message("m1", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();

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

        for agent in &[
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::CopilotCli,
        ] {
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
        let raw: String = conn
            .query_row(
                "SELECT agent FROM discussions WHERE id = 'd-format-check'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            raw, "CopilotCli",
            "DB string for CopilotCli must be 'CopilotCli'"
        );
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
        assert_eq!(
            loaded.agent,
            AgentType::Custom,
            "Unknown agent strings should map to Custom"
        );
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

        let updated =
            update_discussion_skill_ids(&conn, "d1", &["security-auditor".into()]).unwrap();
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
        let updated =
            update_discussion_agent(&conn, "agent-switch", &AgentType::GeminiCli).unwrap();
        assert!(updated);

        let after = get_discussion(&conn, "agent-switch").unwrap().unwrap();
        assert!(matches!(after.agent, AgentType::GeminiCli));
        let pending: Option<String> = conn
            .query_row(
                "SELECT pending_agent_handoff_from FROM discussions WHERE id = ?1",
                ["agent-switch"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending.as_deref(), Some("ClaudeCode"));
        assert!(
            after.messages.is_empty(),
            "switching must not create a message"
        );
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
        assert!(
            after.summary_cache.is_none(),
            "Summary should be invalidated after agent switch"
        );
    }

    #[test]
    fn successive_agent_switches_collapse_and_switching_back_cancels_handoff() {
        let conn = test_conn();
        let disc = make_discussion("switch-msg");
        insert_discussion(&conn, &disc).unwrap();

        update_discussion_agent(&conn, "switch-msg", &AgentType::Codex).unwrap();
        update_discussion_agent(&conn, "switch-msg", &AgentType::Kiro).unwrap();
        let pending: Option<String> = conn
            .query_row(
                "SELECT pending_agent_handoff_from FROM discussions WHERE id = ?1",
                ["switch-msg"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending.as_deref(), Some("ClaudeCode"));

        update_discussion_agent(&conn, "switch-msg", &AgentType::ClaudeCode).unwrap();
        let pending: Option<String> = conn
            .query_row(
                "SELECT pending_agent_handoff_from FROM discussions WHERE id = ?1",
                ["switch-msg"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, None);
    }

    #[test]
    fn first_user_message_consumes_pending_agent_handoff() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("handoff-msg")).unwrap();
        update_discussion_agent(&conn, "handoff-msg", &AgentType::Codex).unwrap();

        let msg = make_message("handoff-user", MessageRole::User, None);
        let stored =
            match insert_user_message_with_agent_handoff(&conn, "handoff-msg", &msg).unwrap() {
                InsertUserMessageOutcome::Inserted { message, .. } => message,
                other => panic!("expected inserted message, got {other:?}"),
            };
        assert!(stored
            .content
            .starts_with("<!-- KRONN_AGENT_HANDOFF: ClaudeCode -> Codex."));
        assert!(stored.content.ends_with("Content of handoff-user"));

        let loaded = get_discussion(&conn, "handoff-msg").unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, stored.content);
        let pending: Option<String> = conn
            .query_row(
                "SELECT pending_agent_handoff_from FROM discussions WHERE id = ?1",
                ["handoff-msg"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, None);

        let second = make_message("handoff-user-2", MessageRole::User, None);
        let stored_second =
            match insert_user_message_with_agent_handoff(&conn, "handoff-msg", &second).unwrap() {
                InsertUserMessageOutcome::Inserted { message, .. } => message,
                other => panic!("expected inserted message, got {other:?}"),
            };
        assert_eq!(stored_second.content, "Content of handoff-user-2");
    }

    #[test]
    fn duplicate_user_message_is_idempotent_and_does_not_consume_handoff() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("dedupe-msg")).unwrap();
        let msg = make_message("stable-client-id", MessageRole::User, None);

        let first = insert_user_message_with_agent_handoff(&conn, "dedupe-msg", &msg).unwrap();
        assert!(matches!(
            first,
            InsertUserMessageOutcome::Inserted { sort_order: 1, .. }
        ));

        update_discussion_agent(&conn, "dedupe-msg", &AgentType::Codex).unwrap();
        let duplicate = insert_user_message_with_agent_handoff(&conn, "dedupe-msg", &msg).unwrap();
        assert!(matches!(
            duplicate,
            InsertUserMessageOutcome::Duplicate { sort_order: 1 }
        ));
        assert_eq!(list_messages(&conn, "dedupe-msg").unwrap().len(), 1);

        let pending: Option<String> = conn
            .query_row(
                "SELECT pending_agent_handoff_from FROM discussions WHERE id = ?1",
                ["dedupe-msg"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending.as_deref(), Some("ClaudeCode"));
    }

    #[test]
    fn note_is_idempotent_visible_and_never_consumes_or_dispatches() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("note-msg")).unwrap();
        update_discussion_agent(&conn, "note-msg", &AgentType::Codex).unwrap();

        let mut note = make_message("stable-note-id", MessageRole::User, None);
        note.channel = crate::models::MessageChannel::Note;
        note.content = "Décision hors contexte".into();

        let inserted = insert_note_message(&conn, "note-msg", &note).unwrap();
        assert!(matches!(
            inserted,
            InsertUserMessageOutcome::Inserted {
                sort_order: 1,
                dispatch_job: None,
                ..
            }
        ));
        let duplicate = insert_note_message(&conn, "note-msg", &note).unwrap();
        assert!(matches!(
            duplicate,
            InsertUserMessageOutcome::Duplicate { sort_order: 1 }
        ));

        assert_eq!(count_notes(&conn, "note-msg").unwrap(), 1);
        let notes = list_notes(&conn, "note-msg", 0, 10).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, 1);
        assert_eq!(notes[0].1.content, "Décision hors contexte");
        assert!(matches!(
            notes[0].1.channel,
            crate::models::MessageChannel::Note
        ));
        assert_eq!(list_messages(&conn, "note-msg").unwrap().len(), 1);

        let pending: Option<String> = conn
            .query_row(
                "SELECT pending_agent_handoff_from FROM discussions WHERE id = ?1",
                ["note-msg"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending.as_deref(), Some("ClaudeCode"));
        let (awaiting_agent, dispatch_count): (bool, i64) = (
            conn.query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'note-msg'",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            conn.query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = 'note-msg'",
                [],
                |row| row.get(0),
            )
            .unwrap(),
        );
        assert!(!awaiting_agent);
        assert_eq!(dispatch_count, 0);
    }

    #[test]
    fn user_message_and_dispatch_job_commit_atomically() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("dispatch-msg")).unwrap();
        let msg = make_message("dispatch-user", MessageRole::User, None);

        let outcome = insert_user_message_with_dispatch(
            &conn,
            "dispatch-msg",
            &msg,
            "dispatch-job",
            Some(&AgentType::Codex),
        )
        .unwrap();
        let claimed = match outcome {
            InsertUserMessageOutcome::Inserted {
                sort_order: 1,
                dispatch_job: Some(job),
                ..
            } => job,
            other => panic!("expected atomically claimed dispatch, got {other:?}"),
        };
        assert_eq!(claimed.id, "dispatch-job");

        let job = crate::db::agent_dispatch::get(&conn, "dispatch-job")
            .unwrap()
            .expect("dispatch job");
        assert_eq!(job.trigger_message_id, "dispatch-user");
        assert_eq!(job.trigger_sort_order, 1);
        assert_eq!(job.agent_override, Some(AgentType::Codex));
        assert_eq!(
            job.status,
            crate::db::agent_dispatch::DispatchStatus::Running
        );
        let awaiting: i64 = conn
            .query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'dispatch-msg'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(awaiting, 1);

        let queued = insert_user_message_with_dispatch(
            &conn,
            "dispatch-msg",
            &make_message("dispatch-user-2", MessageRole::User, None),
            "dispatch-job-2",
            None,
        )
        .unwrap();
        assert!(matches!(
            queued,
            InsertUserMessageOutcome::Inserted {
                dispatch_job: None,
                sort_order: 2,
                ..
            }
        ));
        assert_eq!(
            crate::db::agent_dispatch::get(&conn, "dispatch-job-2")
                .unwrap()
                .unwrap()
                .status,
            crate::db::agent_dispatch::DispatchStatus::Pending
        );
    }

    #[test]
    fn dispatch_insert_failure_rolls_back_the_user_message() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("dispatch-source")).unwrap();
        insert_discussion(&conn, &make_discussion("dispatch-target")).unwrap();
        insert_user_message_with_dispatch(
            &conn,
            "dispatch-source",
            &make_message("dispatch-source-user", MessageRole::User, None),
            "shared-job-id",
            None,
        )
        .unwrap();

        let error = insert_user_message_with_dispatch(
            &conn,
            "dispatch-target",
            &make_message("must-rollback", MessageRole::User, None),
            "shared-job-id",
            None,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("UNIQUE constraint failed: agent_dispatch_jobs.id"));
        assert!(
            list_messages(&conn, "dispatch-target").unwrap().is_empty(),
            "the accepted message must roll back with its dispatch obligation"
        );
    }

    #[test]
    fn reused_message_id_outside_same_discussion_user_is_rejected() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("collision-a")).unwrap();
        insert_discussion(&conn, &make_discussion("collision-b")).unwrap();

        let cross_disc = make_message("cross-disc-id", MessageRole::User, None);
        insert_message(&conn, "collision-a", &cross_disc).unwrap();
        let err =
            insert_user_message_with_agent_handoff(&conn, "collision-b", &cross_disc).unwrap_err();
        assert!(err.to_string().contains("already belongs"));

        let role_collision = make_message(
            "role-collision-id",
            MessageRole::Agent,
            Some(AgentType::Codex),
        );
        insert_message(&conn, "collision-a", &role_collision).unwrap();
        let user_with_same_id = make_message("role-collision-id", MessageRole::User, None);
        let err = insert_user_message_with_agent_handoff(&conn, "collision-a", &user_with_same_id)
            .unwrap_err();
        assert!(err.to_string().contains("already belongs"));
    }

    #[test]
    fn pending_partial_rejects_new_message_without_consuming_sequence() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("partial-msg")).unwrap();
        set_partial_response(&conn, "partial-msg", Some("recover me"), None).unwrap();

        let outcome = insert_user_message_with_agent_handoff(
            &conn,
            "partial-msg",
            &make_message("blocked-user", MessageRole::User, None),
        )
        .unwrap();
        assert!(matches!(outcome, InsertUserMessageOutcome::PartialPending));
        assert!(list_messages(&conn, "partial-msg").unwrap().is_empty());

        set_partial_response(&conn, "partial-msg", None, None).unwrap();
        let inserted = insert_user_message_with_agent_handoff(
            &conn,
            "partial-msg",
            &make_message("accepted-user", MessageRole::User, None),
        )
        .unwrap();
        assert!(matches!(
            inserted,
            InsertUserMessageOutcome::Inserted { sort_order: 1, .. }
        ));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Context files CRUD
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn insert_and_list_context_files() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-ctx")).unwrap();

        insert_context_file(
            &conn,
            "cf1",
            "d-ctx",
            "notes.txt",
            "text/plain",
            100,
            "Hello world",
            None,
        )
        .unwrap();
        insert_context_file(
            &conn, "cf2", "d-ctx", "data.csv", "text/csv", 200, "a,b\n1,2", None,
        )
        .unwrap();

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
        assert_eq!(count_pending_context_files(&conn, "d-count").unwrap(), 0);

        insert_context_file(
            &conn,
            "cf1",
            "d-count",
            "a.txt",
            "text/plain",
            10,
            "A",
            None,
        )
        .unwrap();
        assert_eq!(count_context_files(&conn, "d-count").unwrap(), 1);
        assert_eq!(count_pending_context_files(&conn, "d-count").unwrap(), 1);

        insert_context_file(
            &conn,
            "cf2",
            "d-count",
            "b.txt",
            "text/plain",
            10,
            "B",
            None,
        )
        .unwrap();
        assert_eq!(count_context_files(&conn, "d-count").unwrap(), 2);
        assert_eq!(count_pending_context_files(&conn, "d-count").unwrap(), 2);

        insert_message(
            &conn,
            "d-count",
            &make_message("m-count", MessageRole::Agent, Some(AgentType::Codex)),
        )
        .unwrap();
        assert_eq!(
            link_pending_context_files_to_message(&conn, "d-count", "m-count").unwrap(),
            2
        );

        // Historical message attachments remain durable and count toward the
        // discussion inventory, but no longer occupy the composer staging cap.
        assert_eq!(count_context_files(&conn, "d-count").unwrap(), 2);
        assert_eq!(count_pending_context_files(&conn, "d-count").unwrap(), 0);
    }

    #[test]
    fn delete_context_file_removes_it() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-del")).unwrap();
        insert_context_file(
            &conn,
            "cf1",
            "d-del",
            "test.txt",
            "text/plain",
            50,
            "Test",
            None,
        )
        .unwrap();

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
        insert_context_file(
            &conn,
            "cf1",
            "d-a",
            "test.txt",
            "text/plain",
            50,
            "Test",
            None,
        )
        .unwrap();

        // Try deleting from wrong discussion
        let deleted = delete_context_file(&conn, "d-b", "cf1").unwrap();
        assert!(
            !deleted,
            "Should return false when file doesn't belong to discussion"
        );

        // File should still exist in d-a
        assert_eq!(count_context_files(&conn, "d-a").unwrap(), 1);
    }

    #[test]
    fn get_context_files_for_prompt_text_only() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-prompt")).unwrap();
        insert_context_file(
            &conn,
            "cf1",
            "d-prompt",
            "code.rs",
            "text/plain",
            100,
            "fn main() {}",
            None,
        )
        .unwrap();
        insert_context_file(
            &conn,
            "cf2",
            "d-prompt",
            "data.sql",
            "text/plain",
            50,
            "SELECT 1",
            None,
        )
        .unwrap();

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
        insert_context_file(
            &conn,
            "cf1",
            "d-img",
            "screenshot.png",
            "image/png",
            5000,
            "[Image: screenshot.png]",
            Some("/tmp/screenshot.png"),
        )
        .unwrap();

        let entries = get_context_files_for_prompt(&conn, "d-img").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filename, "screenshot.png");
        assert_eq!(
            entries[0].disk_path,
            Some("/tmp/screenshot.png".to_string())
        );
    }

    #[test]
    fn context_files_cascade_on_discussion_delete() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-cascade")).unwrap();
        insert_context_file(
            &conn,
            "cf1",
            "d-cascade",
            "file.txt",
            "text/plain",
            10,
            "X",
            None,
        )
        .unwrap();

        // Delete the discussion
        conn.execute("DELETE FROM discussions WHERE id = 'd-cascade'", [])
            .unwrap();

        // Context files should be gone (CASCADE)
        assert_eq!(count_context_files(&conn, "d-cascade").unwrap(), 0);
    }

    #[test]
    fn context_file_with_disk_path() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-disk")).unwrap();
        insert_context_file(
            &conn,
            "cf1",
            "d-disk",
            "chart.png",
            "image/png",
            50000,
            "[Image]",
            Some("/project/.kronn/context-files/abc_chart.png"),
        )
        .unwrap();

        let files = list_context_files(&conn, "d-disk").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].disk_path,
            Some("/project/.kronn/context-files/abc_chart.png".to_string())
        );
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
        insert_context_file(
            &conn,
            "cf1",
            "d-pend",
            "shot.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/shot.png"),
        )
        .unwrap();

        // Uploaded but not yet sent → no message_id.
        let files = list_context_files(&conn, "d-pend").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].message_id, None,
            "an upload is pending until a message is sent"
        );
    }

    #[test]
    fn link_pending_pins_only_unattached_files_and_returns_count() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-link")).unwrap();
        // Two pending uploads + one already attached to an older message.
        insert_context_file(
            &conn,
            "cf1",
            "d-link",
            "a.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/a.png"),
        )
        .unwrap();
        insert_context_file(
            &conn,
            "cf2",
            "d-link",
            "b.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/b.png"),
        )
        .unwrap();
        insert_context_file(
            &conn,
            "cf3",
            "d-link",
            "old.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/old.png"),
        )
        .unwrap();
        link_pending_context_files_to_message(&conn, "d-link", "msg-old").unwrap();

        // Two MORE pending uploads arrive, then the user sends a new message.
        insert_context_file(
            &conn,
            "cf4",
            "d-link",
            "c.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/c.png"),
        )
        .unwrap();
        insert_context_file(
            &conn,
            "cf5",
            "d-link",
            "d.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/d.png"),
        )
        .unwrap();
        let n = link_pending_context_files_to_message(&conn, "d-link", "msg-new").unwrap();

        assert_eq!(
            n, 2,
            "only the two still-pending files get pinned to the new message"
        );
        let on_new = list_context_files_for_message(&conn, "msg-new").unwrap();
        assert_eq!(
            on_new.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["cf4", "cf5"]
        );
        // The earlier batch stays on its original message — never re-pinned.
        let on_old = list_context_files_for_message(&conn, "msg-old").unwrap();
        assert_eq!(on_old.len(), 3);
    }

    #[test]
    fn link_pending_is_a_no_op_when_nothing_is_pending() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-noop")).unwrap();
        insert_context_file(
            &conn,
            "cf1",
            "d-noop",
            "a.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/a.png"),
        )
        .unwrap();
        link_pending_context_files_to_message(&conn, "d-noop", "msg-1").unwrap();

        // A second send with no new uploads links nothing.
        let n = link_pending_context_files_to_message(&conn, "d-noop", "msg-2").unwrap();
        assert_eq!(n, 0);
        assert!(list_context_files_for_message(&conn, "msg-2")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn link_pending_is_scoped_to_one_discussion() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-x")).unwrap();
        insert_discussion(&conn, &make_discussion("d-y")).unwrap();
        insert_context_file(
            &conn,
            "cfx",
            "d-x",
            "x.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/x.png"),
        )
        .unwrap();
        insert_context_file(
            &conn,
            "cfy",
            "d-y",
            "y.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/y.png"),
        )
        .unwrap();

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
        insert_context_file(
            &conn,
            "old1",
            "d-legacy",
            "spec.pdf",
            "application/pdf",
            10,
            "ref",
            None,
        )
        .unwrap();
        insert_context_file(
            &conn, "old2", "d-legacy", "data.csv", "text/csv", 10, "a,b", None,
        )
        .unwrap();
        insert_context_file(
            &conn,
            "pinned",
            "d-legacy",
            "shot.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/s.png"),
        )
        .unwrap();
        link_pending_context_files_to_message(&conn, "d-legacy", "msg-real").unwrap(); // pins old1, old2, pinned

        // Re-create the "uploaded before the column" case: two fresh NULL rows.
        insert_context_file(
            &conn,
            "legacyA",
            "d-legacy",
            "a.txt",
            "text/plain",
            1,
            "A",
            None,
        )
        .unwrap();
        insert_context_file(
            &conn,
            "legacyB",
            "d-legacy",
            "b.txt",
            "text/plain",
            1,
            "B",
            None,
        )
        .unwrap();

        // Apply the exact backfill migration SQL.
        conn.execute_batch(include_str!("sql/067_context_files_backfill_legacy.sql"))
            .unwrap();

        // The NULL rows are now the inert sentinel — NOT pending.
        let a: Option<String> = conn
            .query_row(
                "SELECT message_id FROM context_files WHERE id='legacyA'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let b: Option<String> = conn
            .query_row(
                "SELECT message_id FROM context_files WHERE id='legacyB'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a.as_deref(), Some("__legacy_disc_wide__"));
        assert_eq!(b.as_deref(), Some("__legacy_disc_wide__"));
        // The already-pinned files keep their real message id.
        let p: Option<String> = conn
            .query_row(
                "SELECT message_id FROM context_files WHERE id='old1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(p.as_deref(), Some("msg-real"));

        // Crucially: a later send links NOTHING — legacy files are no longer
        // pending, so they can't be vacuumed into a new message.
        let n = link_pending_context_files_to_message(&conn, "d-legacy", "msg-next").unwrap();
        assert_eq!(
            n, 0,
            "backfilled legacy files must not attach to the next message"
        );
        assert!(list_context_files_for_message(&conn, "msg-next")
            .unwrap()
            .is_empty());
        // ...and they stay disc-wide context (still listed for the discussion).
        assert_eq!(list_context_files(&conn, "d-legacy").unwrap().len(), 5);
    }

    #[test]
    fn list_for_message_returns_message_id_on_each_row() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-roundtrip")).unwrap();
        insert_context_file(
            &conn,
            "cf1",
            "d-roundtrip",
            "a.png",
            "image/png",
            10,
            "[Image]",
            Some("/tmp/a.png"),
        )
        .unwrap();
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
        insert_message(
            &conn,
            "d-mix",
            &make_message("a1", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();
        for i in 0..6 {
            insert_message(
                &conn,
                "d-mix",
                &make_message(&format!("s{i}"), MessageRole::System, None),
            )
            .unwrap();
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
        insert_message(
            &conn,
            "d-clean",
            &make_message("u1", MessageRole::User, None),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-clean",
            &make_message("a1", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-clean",
            &make_message("u2", MessageRole::User, None),
        )
        .unwrap();

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
        assert!(
            any > recent,
            "anchor must be reception time (~now), got {any}"
        );
        let user = last_user_message_at(&conn, "d-anchor").unwrap().unwrap();
        assert!(
            user > recent,
            "lease anchor must renew on reception, got {user}"
        );
    }

    #[test]
    fn pacing_anchors_follow_the_newest_row_by_sort_order() {
        // The ordering axis is sort_order (the event log), never a MAX()
        // over clocks — received_at values are skewed by hand to prove it.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-order")).unwrap();
        insert_message(
            &conn,
            "d-order",
            &make_message("m1", MessageRole::User, None),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-order",
            &make_message("m2", MessageRole::User, None),
        )
        .unwrap();

        let older = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "UPDATE messages SET received_at = ?1 WHERE id = 'm2'",
            rusqlite::params![older],
        )
        .unwrap();

        // m1 has the LARGER received_at, but m2 is the newest event.
        let any = last_message_at(&conn, "d-order").unwrap().unwrap();
        assert_eq!(
            any.to_rfc3339(),
            older,
            "anchor follows sort_order, not MAX(received_at)"
        );
    }

    #[test]
    fn last_user_message_at_skips_agent_rows_and_empty_discs() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-roles")).unwrap();
        assert!(last_message_at(&conn, "d-roles").unwrap().is_none());
        assert!(last_user_message_at(&conn, "d-roles").unwrap().is_none());

        insert_message(
            &conn,
            "d-roles",
            &make_message("u1", MessageRole::User, None),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-roles",
            &make_message("a1", MessageRole::Agent, Some(AgentType::Codex)),
        )
        .unwrap();
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
        assert!(
            any_anchor > user_anchor,
            "any-role anchor follows the newest (Agent) row"
        );
    }

    // ── reconcile_awaiting_agents (boot recovery of owed runs) ──

    #[test]
    fn reconcile_marks_a_queued_owed_disc_and_clears_the_flag() {
        // A batch child (or a human msg) that was owed an agent which never
        // started: last message is the User prompt, no partial. Reconcile
        // appends an interrupted notice, clears the flag, returns the id.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-owed")).unwrap();
        insert_message(
            &conn,
            "d-owed",
            &make_message("u1", MessageRole::User, None),
        )
        .unwrap();
        set_awaiting_agent(&conn, "d-owed", true).unwrap();

        let marked = reconcile_awaiting_agents(&conn).unwrap();
        assert_eq!(marked, vec!["d-owed".to_string()]);
        // A notice was appended → the disc now has 2 messages, last is Agent.
        let disc = get_discussion(&conn, "d-owed").unwrap().unwrap();
        assert_eq!(disc.messages.len(), 2);
        assert!(matches!(disc.messages[1].role, MessageRole::Agent));
        // The notice speaks to BOTH readers: the human ("Relancez") and
        // the relaunched agent (system-marker brief) — dropping either
        // regresses a live failure (agent floundering on its own marker).
        assert!(disc.messages[1].content.contains("Relancez"));
        assert!(disc.messages[1].content.contains("marqueur système"));
        // Flag cleared → a second reconcile is a no-op.
        assert!(reconcile_awaiting_agents(&conn).unwrap().is_empty());
    }

    #[test]
    fn reconcile_leaves_durable_dispatch_jobs_for_the_worker() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-durable")).unwrap();
        insert_user_message_with_dispatch(
            &conn,
            "d-durable",
            &make_message("u-durable", MessageRole::User, None),
            "j-durable",
            None,
        )
        .unwrap();

        assert!(
            reconcile_awaiting_agents(&conn).unwrap().is_empty(),
            "durable work is resumed by the dispatcher, not marked interrupted"
        );
        let disc = get_discussion(&conn, "d-durable").unwrap().unwrap();
        assert_eq!(disc.messages.len(), 1);
        assert!(disc.awaiting_agent);
        assert_eq!(
            crate::db::agent_dispatch::get(&conn, "j-durable")
                .unwrap()
                .unwrap()
                .status,
            crate::db::agent_dispatch::DispatchStatus::Running
        );
    }

    #[test]
    fn reconcile_skips_a_disc_already_answered() {
        // Flag left set but the agent DID answer (last message is Agent):
        // no notice, no re-flag, just housekeeping-clear the stale flag.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-done")).unwrap();
        insert_message(
            &conn,
            "d-done",
            &make_message("u1", MessageRole::User, None),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-done",
            &make_message("a1", MessageRole::Agent, Some(AgentType::ClaudeCode)),
        )
        .unwrap();
        set_awaiting_agent(&conn, "d-done", true).unwrap();

        let marked = reconcile_awaiting_agents(&conn).unwrap();
        assert!(
            marked.is_empty(),
            "an answered disc must not be marked interrupted"
        );
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
        insert_message(
            &conn,
            "d-partial",
            &make_message("u1", MessageRole::User, None),
        )
        .unwrap();
        set_awaiting_agent(&conn, "d-partial", true).unwrap();
        set_partial_response(&conn, "d-partial", Some("half a reply"), None).unwrap();

        let marked = reconcile_awaiting_agents(&conn).unwrap();
        assert!(
            marked.is_empty(),
            "a disc with a live partial is left to partial recovery"
        );
        // The partial is still there for recover_partial_responses to convert.
        let disc = get_discussion(&conn, "d-partial").unwrap().unwrap();
        assert_eq!(
            disc.messages.len(),
            1,
            "no notice, no conversion — recovery owns it"
        );
    }

    #[test]
    fn recover_partial_response_restores_agent_and_model_provenance() {
        // KT-37 — a mid-stream checkpoint carries the agent + attempted model,
        // so the recovered message is attributed instead of anonymous +
        // model-less.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-prov")).unwrap();
        set_partial_response(
            &conn,
            "d-prov",
            Some("half an answer"),
            Some((&AgentType::Ollama, Some("qwen3:32b"))),
        )
        .unwrap();

        let recovered = recover_partial_responses(&conn).unwrap();
        assert_eq!(recovered, vec!["d-prov".to_string()]);

        let disc = get_discussion(&conn, "d-prov").unwrap().unwrap();
        let msg = disc.messages.last().unwrap();
        assert_eq!(msg.role, MessageRole::Agent);
        assert!(msg.content.contains("half an answer"));
        assert_eq!(msg.agent_type, Some(AgentType::Ollama));
        assert_eq!(msg.model.as_deref(), Some("qwen3:32b"));
        // The checkpoint is cleared after recovery.
        assert!(!has_pending_partial(&conn, "d-prov").unwrap());
    }

    #[test]
    fn recover_partial_response_without_provenance_stays_anonymous() {
        // Legacy pre-089 checkpoints have no agent/model — recovery degrades
        // gracefully, never invents one.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-legacy")).unwrap();
        set_partial_response(&conn, "d-legacy", Some("legacy text"), None).unwrap();

        recover_partial_responses(&conn).unwrap();
        let disc = get_discussion(&conn, "d-legacy").unwrap().unwrap();
        let msg = disc.messages.last().unwrap();
        assert_eq!(msg.agent_type, None);
        assert_eq!(msg.model, None);
    }

    #[test]
    fn reconcile_ignores_unflagged_discs() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-plain")).unwrap();
        insert_message(
            &conn,
            "d-plain",
            &make_message("u1", MessageRole::User, None),
        )
        .unwrap();
        // No set_awaiting_agent → flag stays 0 (default).
        assert!(reconcile_awaiting_agents(&conn).unwrap().is_empty());
    }

    #[test]
    fn plural_targets_commit_once_with_one_durable_job_per_agent() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-plural-targets")).unwrap();
        let mut message = make_message("u-plural-targets", MessageRole::User, None);
        message.content = "@codex confronte @claude".into();
        message.target_agent = Some(AgentType::Codex);
        let targets = [
            MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Economy),
            MessageTarget::agent(AgentType::ClaudeCode).with_tier(ModelTier::Reasoning),
        ];
        let dispatches = [
            UserDispatchSpec {
                job_id: "plural-codex",
                agent_override: Some(&AgentType::Codex),
                dedupe_key: None,
            },
            UserDispatchSpec {
                job_id: "plural-claude",
                agent_override: Some(&AgentType::ClaudeCode),
                dedupe_key: None,
            },
        ];

        let inserted = insert_user_message_with_dispatches(
            &conn,
            "d-plural-targets",
            &message,
            &targets,
            &dispatches,
            false,
        )
        .unwrap();
        let InsertUserMessageOutcome::Inserted { dispatch_job, .. } = inserted else {
            panic!("first acceptance must insert");
        };
        assert_eq!(
            dispatch_job.as_ref().map(|job| job.id.as_str()),
            Some("plural-codex"),
        );
        assert_eq!(
            list_message_targets(&conn, "u-plural-targets").unwrap(),
            targets,
        );
        assert_eq!(
            crate::db::agent_dispatch::get(&conn, "plural-codex")
                .unwrap()
                .unwrap()
                .status,
            crate::db::agent_dispatch::DispatchStatus::Running,
        );
        assert_eq!(
            crate::db::agent_dispatch::get(&conn, "plural-claude")
                .unwrap()
                .unwrap()
                .status,
            crate::db::agent_dispatch::DispatchStatus::Pending,
        );

        let duplicate = insert_user_message_with_dispatches(
            &conn,
            "d-plural-targets",
            &message,
            &targets,
            &[
                UserDispatchSpec {
                    job_id: "plural-codex-retry",
                    agent_override: Some(&AgentType::Codex),
                    dedupe_key: None,
                },
                UserDispatchSpec {
                    job_id: "plural-claude-retry",
                    agent_override: Some(&AgentType::ClaudeCode),
                    dedupe_key: None,
                },
            ],
            false,
        )
        .unwrap();
        assert!(matches!(
            duplicate,
            InsertUserMessageOutcome::Duplicate { .. }
        ));
        let job_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
             WHERE trigger_message_id = 'u-plural-targets'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_count, 2);
    }

    #[test]
    fn deferred_human_turn_is_durable_pending_and_idempotent() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-deferred-turn")).unwrap();
        let message = make_message("u-deferred-turn", MessageRole::User, None);
        let dispatches = [UserDispatchSpec {
            job_id: "deferred-job",
            agent_override: None,
            dedupe_key: None,
        }];

        let inserted = insert_user_message_with_pending_dispatches(
            &conn,
            "d-deferred-turn",
            &message,
            &[],
            &dispatches,
            false,
        )
        .unwrap();
        let InsertUserMessageOutcome::Inserted { dispatch_job, .. } = inserted else {
            panic!("first deferred acceptance must insert");
        };
        assert!(
            dispatch_job.is_none(),
            "the request must not claim a runner"
        );
        assert_eq!(
            crate::db::agent_dispatch::get(&conn, "deferred-job")
                .unwrap()
                .unwrap()
                .status,
            crate::db::agent_dispatch::DispatchStatus::Pending,
        );

        let duplicate = insert_user_message_with_pending_dispatches(
            &conn,
            "d-deferred-turn",
            &message,
            &[],
            &[UserDispatchSpec {
                job_id: "deferred-job-retry",
                agent_override: None,
                dedupe_key: None,
            }],
            false,
        )
        .unwrap();
        assert!(matches!(
            duplicate,
            InsertUserMessageOutcome::Duplicate { .. }
        ));
        let job_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE trigger_message_id = 'u-deferred-turn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_count, 1, "retrying the UUID must not duplicate the job");
    }

    #[test]
    fn discussion_targets_preserve_message_scope_order_and_tiers() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-routing-receipts")).unwrap();
        insert_discussion(&conn, &make_discussion("d-routing-other")).unwrap();

        insert_message(
            &conn,
            "d-routing-receipts",
            &make_message("u-routing-one", MessageRole::User, None),
        )
        .unwrap();
        replace_message_targets(
            &conn,
            "u-routing-one",
            &[
                MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Economy),
                MessageTarget::agent(AgentType::ClaudeCode).with_tier(ModelTier::Reasoning),
            ],
        )
        .unwrap();

        insert_message(
            &conn,
            "d-routing-receipts",
            &make_message("u-routing-two", MessageRole::User, None),
        )
        .unwrap();
        let cli_session_id = crate::db::discussion_sessions::create_session(
            &conn,
            "d-routing-receipts",
            "ClaudeCode",
            Some("routing-cli"),
            "peer",
        )
        .unwrap();
        replace_message_targets(
            &conn,
            "u-routing-two",
            &[MessageTarget::cli(AgentType::ClaudeCode, cli_session_id)],
        )
        .unwrap();

        insert_message(
            &conn,
            "d-routing-other",
            &make_message("u-routing-other", MessageRole::User, None),
        )
        .unwrap();
        replace_message_targets(
            &conn,
            "u-routing-other",
            &[MessageTarget::agent(AgentType::Ollama).with_tier(ModelTier::Default)],
        )
        .unwrap();

        let receipts = list_discussion_message_targets(&conn, "d-routing-receipts").unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(
            receipts.get("u-routing-one"),
            Some(&vec![
                MessageTarget::agent(AgentType::Codex).with_tier(ModelTier::Economy),
                MessageTarget::agent(AgentType::ClaudeCode).with_tier(ModelTier::Reasoning),
            ]),
        );
        assert_eq!(
            receipts.get("u-routing-two"),
            Some(&vec![MessageTarget::cli(
                AgentType::ClaudeCode,
                cli_session_id,
            )]),
        );
        assert!(!receipts.contains_key("u-routing-other"));
    }

    #[test]
    fn native_fallback_is_marked_in_the_user_acceptance_transaction() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-native-fallback")).unwrap();
        crate::db::discussion_sessions::create_session(
            &conn,
            "d-native-fallback",
            "ClaudeCode",
            Some("cli-lapsed"),
            "peer",
        )
        .unwrap();
        let message = make_message("u-native-fallback", MessageRole::User, None);

        let outcome = insert_user_message_with_dispatches(
            &conn,
            "d-native-fallback",
            &message,
            &[],
            &[],
            true,
        )
        .unwrap();
        assert!(matches!(outcome, InsertUserMessageOutcome::Inserted { .. }));

        let marked: i64 = conn
            .query_row(
                "SELECT native_fallback FROM messages WHERE id = ?1",
                [&message.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);
    }

    fn handoff_discussion(id: &str) -> Discussion {
        let mut discussion = make_discussion(id);
        discussion.participants = vec![
            AgentType::ClaudeCode,
            AgentType::Ollama,
            AgentType::Codex,
            AgentType::LiteLlm,
        ];
        discussion
    }

    fn agent_reply(id: &str, agent: AgentType, parent: &str) -> DiscussionMessage {
        let mut message = make_message(id, MessageRole::Agent, Some(agent));
        message.reply_to_message_id = Some(parent.into());
        message
    }

    #[test]
    fn native_agent_handoff_admits_local_and_one_paid_target_atomically() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-agent-handoff")).unwrap();
        insert_message(
            &conn,
            "d-agent-handoff",
            &make_message("u-agent-handoff", MessageRole::User, None),
        )
        .unwrap();
        let response = agent_reply("a-agent-handoff", AgentType::ClaudeCode, "u-agent-handoff");

        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-agent-handoff",
            &response,
            true,
            Some("source-job"),
            &AgentType::ClaudeCode,
            &[AgentType::Ollama, AgentType::Codex, AgentType::LiteLlm],
            true,
            Some(1),
        )
        .unwrap();

        assert_eq!(
            outcome.dispatched_agents,
            vec![AgentType::Ollama, AgentType::Codex]
        );
        assert_eq!(
            list_message_targets(&conn, "a-agent-handoff").unwrap(),
            vec![
                MessageTarget::agent(AgentType::Ollama),
                MessageTarget::agent(AgentType::Codex),
            ]
        );
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE trigger_message_id = 'a-agent-handoff'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(jobs, 2);
        let provenance: (i64, Option<String>) = conn
            .query_row(
                "SELECT agent_run_succeeded, agent_dispatch_job_id
                 FROM messages WHERE id = 'a-agent-handoff'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(provenance, (1, Some("source-job".into())));
    }

    #[test]
    fn native_agent_message_wakes_a_joined_cli_peer_it_mentions_by_alias() {
        // KT-330 — the observed bug: a native principal that mentions a joined
        // CLI peer by its room alias (`@claude-cli`) produced NO target, so the
        // peer's disc_wait_for_peer never woke. Now the native path resolves the
        // alias to a Cli target on the persisted message — no native dispatch,
        // just a durable wake target the joined peer reads.
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-cli-wake")).unwrap();
        let cli_pk = crate::db::discussion_sessions::create_session(
            &conn,
            "d-cli-wake",
            "ClaudeCode",
            Some("cli-sess-1"),
            "peer",
        )
        .unwrap();
        insert_message(
            &conn,
            "d-cli-wake",
            &make_message("u-cli-wake", MessageRole::User, None),
        )
        .unwrap();
        let mut response = agent_reply("a-cli-wake", AgentType::Codex, "u-cli-wake");
        // Native Codex principal names the joined Claude CLI peer in prose; no
        // handoff marker, so `candidate_agents` is empty — the mention is a CLI
        // wake, never a native spawn.
        response.content = "Merci @claude-cli, peux-tu relire ce diff ?".into();

        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-cli-wake",
            &response,
            true,
            None,
            &AgentType::Codex,
            &[],
            true,
            Some(1),
        )
        .unwrap();

        assert!(
            outcome.dispatched_agents.is_empty(),
            "a CLI mention must not spawn a native agent"
        );
        assert_eq!(
            list_message_targets(&conn, "a-cli-wake").unwrap(),
            vec![MessageTarget::cli(AgentType::ClaudeCode, cli_pk)],
            "the joined CLI peer must get a durable Cli wake target"
        );
    }

    #[test]
    fn native_agent_message_persists_dynamic_connection_mention_target() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-connection-wake")).unwrap();
        insert_message(
            &conn,
            "d-connection-wake",
            &make_message("u-connection-wake", MessageRole::User, None),
        )
        .unwrap();
        let now = Utc::now();
        crate::db::external_api_connections::insert(
            &conn,
            &ExternalApiConnection {
                id: "groq-primary".into(),
                display_name: "Groq primary".into(),
                mention_alias: "groq".into(),
                endpoint: Some("https://api.example.test".into()),
                credential_slug: "groq-primary-credential".into(),
                origin_preset: ExternalApiConnectionPreset::Other,
                economy_model: None,
                default_model: Some("model".into()),
                reasoning_model: None,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        let mut response = agent_reply("a-connection-wake", AgentType::Codex, "u-connection-wake");
        response.content = "@groq please inspect the persisted target.".into();

        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-connection-wake",
            &response,
            true,
            None,
            &AgentType::Codex,
            &[],
            true,
            Some(1),
        )
        .unwrap();

        assert!(outcome.dispatched_agents.is_empty());
        assert_eq!(
            list_message_targets(&conn, "a-connection-wake").unwrap(),
            vec![MessageTarget::agent(AgentType::Custom).with_connection("groq-primary")]
        );
    }

    #[test]
    fn native_agent_handoff_respects_global_discussion_and_attachment_guards() {
        for (id, globally_enabled, discussion_disabled, no_agent, run_succeeded) in [
            ("global-off", false, false, false, true),
            ("discussion-off", true, true, false, true),
            ("native-agent-off", true, false, true, true),
            ("failed-run", true, false, false, false),
        ] {
            let conn = test_conn();
            insert_discussion(&conn, &handoff_discussion(id)).unwrap();
            insert_message(
                &conn,
                id,
                &make_message(&format!("u-{id}"), MessageRole::User, None),
            )
            .unwrap();
            if discussion_disabled {
                set_disc_agent_handoffs_disabled(&conn, id, true).unwrap();
            }
            if no_agent {
                set_disc_no_agent(&conn, id, true).unwrap();
            }
            let response = agent_reply(
                &format!("a-{id}"),
                AgentType::ClaudeCode,
                &format!("u-{id}"),
            );
            let outcome = insert_native_agent_message_with_handoffs(
                &conn,
                id,
                &response,
                run_succeeded,
                None,
                &AgentType::ClaudeCode,
                &[AgentType::Codex],
                globally_enabled,
                Some(1),
            )
            .unwrap();
            assert!(outcome.dispatched_agents.is_empty());
        }

        let conn = test_conn();
        let mut discussion = handoff_discussion("unattached");
        discussion
            .participants
            .retain(|agent| *agent != AgentType::Codex);
        insert_discussion(&conn, &discussion).unwrap();
        insert_message(
            &conn,
            "unattached",
            &make_message("u-unattached", MessageRole::User, None),
        )
        .unwrap();
        let response = agent_reply("a-unattached", AgentType::ClaudeCode, "u-unattached");
        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "unattached",
            &response,
            true,
            None,
            &AgentType::ClaudeCode,
            &[AgentType::Codex],
            true,
            Some(1),
        )
        .unwrap();
        assert!(outcome.dispatched_agents.is_empty());
    }

    #[test]
    fn native_agent_handoff_budget_is_shared_by_the_root_user_turn() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-budget")).unwrap();
        insert_message(
            &conn,
            "d-budget",
            &make_message("u-budget", MessageRole::User, None),
        )
        .unwrap();
        let first = agent_reply("a-budget-1", AgentType::ClaudeCode, "u-budget");
        let first_outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-budget",
            &first,
            true,
            None,
            &AgentType::ClaudeCode,
            &[AgentType::Codex],
            true,
            Some(1),
        )
        .unwrap();
        assert_eq!(first_outcome.dispatched_agents, vec![AgentType::Codex]);
        assert_eq!(
            agent_handoff_paid_count_for_reply(&conn, "d-budget", Some("a-budget-1")).unwrap(),
            1
        );

        let second = agent_reply("a-budget-2", AgentType::Ollama, "u-budget");
        let second_outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-budget",
            &second,
            true,
            None,
            &AgentType::Ollama,
            &[AgentType::LiteLlm],
            true,
            Some(1),
        )
        .unwrap();
        assert!(second_outcome.dispatched_agents.is_empty());
    }

    #[test]
    fn native_agent_handoff_does_not_relaunch_a_root_turn_target() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-root-targets")).unwrap();
        insert_message(
            &conn,
            "d-root-targets",
            &make_message("u-root-targets", MessageRole::User, None),
        )
        .unwrap();
        replace_message_targets(
            &conn,
            "u-root-targets",
            &[
                MessageTarget::discussion_agent(AgentType::ClaudeCode),
                MessageTarget::agent(AgentType::Codex),
            ],
        )
        .unwrap();
        let response = agent_reply("a-root-targets", AgentType::ClaudeCode, "u-root-targets");

        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-root-targets",
            &response,
            true,
            None,
            &AgentType::ClaudeCode,
            &[AgentType::Codex, AgentType::LiteLlm],
            true,
            None,
        )
        .unwrap();

        assert_eq!(outcome.dispatched_agents, vec![AgentType::LiteLlm]);
        assert_eq!(
            native_agents_scheduled_for_root_turn(&conn, "d-root-targets", Some("a-root-targets"),)
                .unwrap(),
            vec![AgentType::ClaudeCode, AgentType::Codex, AgentType::LiteLlm,],
        );

        let sibling_response =
            agent_reply("a-root-targets-sibling", AgentType::Codex, "u-root-targets");
        let duplicate_handoff = insert_native_agent_message_with_handoffs(
            &conn,
            "d-root-targets",
            &sibling_response,
            true,
            None,
            &AgentType::Codex,
            &[AgentType::LiteLlm],
            true,
            None,
        )
        .unwrap();
        assert!(duplicate_handoff.dispatched_agents.is_empty());
    }

    #[test]
    fn unlimited_paid_budget_admits_every_attached_paid_target() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-unlimited-budget")).unwrap();
        insert_message(
            &conn,
            "d-unlimited-budget",
            &make_message("u-unlimited-budget", MessageRole::User, None),
        )
        .unwrap();
        let response = agent_reply(
            "a-unlimited-budget",
            AgentType::ClaudeCode,
            "u-unlimited-budget",
        );

        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-unlimited-budget",
            &response,
            true,
            None,
            &AgentType::ClaudeCode,
            &[AgentType::Codex, AgentType::LiteLlm],
            true,
            None,
        )
        .unwrap();

        assert_eq!(
            outcome.dispatched_agents,
            vec![AgentType::Codex, AgentType::LiteLlm]
        );
    }

    #[test]
    fn zero_paid_budget_still_allows_the_bounded_local_target() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-local-only")).unwrap();
        insert_message(
            &conn,
            "d-local-only",
            &make_message("u-local-only", MessageRole::User, None),
        )
        .unwrap();
        let response = agent_reply("a-local-only", AgentType::ClaudeCode, "u-local-only");
        let outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-local-only",
            &response,
            true,
            None,
            &AgentType::ClaudeCode,
            &[AgentType::Codex, AgentType::Ollama],
            true,
            Some(0),
        )
        .unwrap();

        assert_eq!(outcome.dispatched_agents, vec![AgentType::Ollama]);
    }

    #[test]
    fn native_agent_handoff_stops_after_two_generated_hops() {
        let conn = test_conn();
        insert_discussion(&conn, &handoff_discussion("d-depth")).unwrap();
        insert_message(
            &conn,
            "d-depth",
            &make_message("u-depth", MessageRole::User, None),
        )
        .unwrap();
        let first = agent_reply("a-depth-1", AgentType::ClaudeCode, "u-depth");
        insert_native_agent_message_with_handoffs(
            &conn,
            "d-depth",
            &first,
            true,
            None,
            &AgentType::ClaudeCode,
            &[AgentType::Ollama],
            true,
            Some(2),
        )
        .unwrap();
        let second = agent_reply("a-depth-2", AgentType::Ollama, "a-depth-1");
        let second_outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-depth",
            &second,
            true,
            None,
            &AgentType::Ollama,
            &[AgentType::Codex],
            true,
            Some(2),
        )
        .unwrap();
        assert_eq!(second_outcome.dispatched_agents, vec![AgentType::Codex]);
        let third = agent_reply("a-depth-3", AgentType::Codex, "a-depth-2");
        let third_outcome = insert_native_agent_message_with_handoffs(
            &conn,
            "d-depth",
            &third,
            true,
            None,
            &AgentType::Codex,
            &[AgentType::LiteLlm],
            true,
            Some(2),
        )
        .unwrap();
        assert!(third_outcome.dispatched_agents.is_empty());
    }

    #[test]
    fn latest_main_user_message_ignores_notes_and_agent_replies() {
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-latest-user")).unwrap();
        insert_message(
            &conn,
            "d-latest-user",
            &make_message("u-main-1", MessageRole::User, None),
        )
        .unwrap();
        let mut note = make_message("u-note", MessageRole::User, None);
        note.channel = crate::models::MessageChannel::Note;
        insert_message(&conn, "d-latest-user", &note).unwrap();
        insert_message(
            &conn,
            "d-latest-user",
            &make_message("a-latest", MessageRole::Agent, Some(AgentType::Codex)),
        )
        .unwrap();
        insert_message(
            &conn,
            "d-latest-user",
            &make_message("u-main-2", MessageRole::User, None),
        )
        .unwrap();

        assert_eq!(
            latest_main_user_message_id(&conn, "d-latest-user").unwrap(),
            Some("u-main-2".into())
        );
        assert_eq!(latest_main_user_message_id(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn within_tx_returns_each_job_once_and_respects_dedupe_key() {
        // KT-318 T2 — the composable core returns the jobs it enqueues (so the
        // orchestrator can bind their exact ids), enqueues each spec EXACTLY
        // once, and an explicit orchestration key overrides the caller-scoped
        // `peer:` scheme while `None` preserves it verbatim.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-within-tx")).unwrap();
        let message = make_message(
            "m-within-tx",
            MessageRole::Agent,
            Some(AgentType::ClaudeCode),
        );
        let targets = [MessageTarget::agent(AgentType::ClaudeCode)];
        let dispatches = [
            UserDispatchSpec {
                job_id: "orch-native",
                agent_override: Some(&AgentType::ClaudeCode),
                dedupe_key: Some("orch-dispatch:exec-1:0"),
            },
            UserDispatchSpec {
                job_id: "peer-native",
                agent_override: Some(&AgentType::Codex),
                dedupe_key: None,
            },
        ];

        let tx = conn.unchecked_transaction().unwrap();
        let (_sort_order, jobs) = insert_message_with_targets_and_dispatches_within_tx(
            &tx,
            "d-within-tx",
            &message,
            &targets,
            &dispatches,
            None,
        )
        .unwrap();
        tx.commit().unwrap();

        // One job per spec, index-aligned — the caller can bind exact ids.
        assert_eq!(
            jobs.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
            vec!["orch-native", "peer-native"],
        );
        // Exactly one row per spec: no double-enqueue.
        let job_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE trigger_message_id = 'm-within-tx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_count, 2, "no double-enqueue");
        // Explicit orchestration key wins.
        let orch_key: String = conn
            .query_row(
                "SELECT dedupe_key FROM agent_dispatch_jobs WHERE id = 'orch-native'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orch_key, "orch-dispatch:exec-1:0");
        // `None` preserves the historical caller-scoped scheme.
        let peer_key: String = conn
            .query_row(
                "SELECT dedupe_key FROM agent_dispatch_jobs WHERE id = 'peer-native'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            peer_key.starts_with("peer:m-within-tx"),
            "None keeps the peer scheme, got {peer_key}"
        );
    }

    #[test]
    fn within_tx_writes_are_invisible_until_commit() {
        // KT-318 T2 — nothing the core writes is observable until the caller
        // commits; a rolled-back provisioning attempt leaves no message and no
        // dispatch, so the dispatcher and `wait_for_peer` never see a half-turn.
        let conn = test_conn();
        insert_discussion(&conn, &make_discussion("d-rollback")).unwrap();
        let message = make_message(
            "m-rollback",
            MessageRole::Agent,
            Some(AgentType::ClaudeCode),
        );
        let dispatches = [UserDispatchSpec {
            job_id: "rollback-job",
            agent_override: Some(&AgentType::ClaudeCode),
            dedupe_key: Some("orch-dispatch:exec-2:0"),
        }];

        {
            let tx = conn.unchecked_transaction().unwrap();
            insert_message_with_targets_and_dispatches_within_tx(
                &tx,
                "d-rollback",
                &message,
                &[MessageTarget::agent(AgentType::ClaudeCode)],
                &dispatches,
                None,
            )
            .unwrap();
            // `tx` dropped without commit → SQLite rolls the whole attempt back.
        }

        assert!(
            crate::db::agent_dispatch::get(&conn, "rollback-job")
                .unwrap()
                .is_none(),
            "rollback leaves no dispatch job"
        );
        let msg_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = 'm-rollback')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!msg_exists, "rollback leaves no message");
    }
}
