use super::*;
use crate::db::discussions::*;

use crate::db::migrations;

use chrono::Utc;

use rusqlite::{params, Connection};

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
        connection_id: None,
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
        summary_strategy: crate::models::SummaryStrategy::OnDemand,
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
        session_continuity: None,
        kind: Default::default(),
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

// KT-1330 — the orchestrator's room posts carry a typed kind; ordinary
// messages do not; legacy rows (NULL column) fall back to the id prefix;
// and the agent-facing read keeps every message either way.
#[test]
fn orchestrator_posts_carry_the_orchestration_kind_through_the_db() {
    use crate::models::MessageKind;
    let conn = test_conn();
    insert_discussion(&conn, &make_discussion("d-kind")).unwrap();
    let orch = crate::api::orchestration::orchestrator_message(
        "orch-review-request:exec-1:0".into(),
        "review please".into(),
    );
    assert_eq!(orch.kind, MessageKind::Orchestration);
    insert_message(&conn, "d-kind", &orch).unwrap();
    insert_message(
        &conn,
        "d-kind",
        &make_message("m-human", MessageRole::User, None),
    )
    .unwrap();

    let stored: Option<String> = conn
        .query_row(
            "SELECT kind FROM messages WHERE id = 'orch-review-request:exec-1:0'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored.as_deref(), Some("orchestration"));

    let messages = list_messages(&conn, "d-kind").unwrap();
    assert_eq!(messages.len(), 2, "display-only: no message is dropped");
    let kind_of = |id: &str| messages.iter().find(|m| m.id == id).unwrap().kind;
    assert_eq!(
        kind_of("orch-review-request:exec-1:0"),
        MessageKind::Orchestration
    );
    assert_eq!(kind_of("m-human"), MessageKind::Standard);
}
