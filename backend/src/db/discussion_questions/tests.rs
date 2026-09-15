use super::*;
use crate::models::{DiscussionMessage, MessageChannel, MessageRole, MessageTargetKind};

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute("INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES ('d','D','Codex','now','now')", []).unwrap();
    conn
}

fn payload() -> serde_json::Value {
    serde_json::json!({"version":1,"key":"quota-policy","question":"Quel choix pour écarter le blocage ?",
        "options":[{"id":"retry","label":"Réarmer"},{"id":"wait","label":"Attendre"}],
        "recommended_option_ids":["retry"],"task_ref":"KT-593"})
}

fn add_connection(conn: &Connection, id: &str) {
    conn.execute("INSERT INTO external_api_connections (id,display_name,mention_alias,credential_slug,origin_preset) VALUES (?1,?1,?1,?1,'other')", [id]).unwrap();
}

fn message(id: &str, content: String) -> DiscussionMessage {
    DiscussionMessage {
        id: id.into(),
        role: MessageRole::Agent,
        channel: MessageChannel::Main,
        content,
        agent_type: Some(crate::models::AgentType::Codex),
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
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
    }
}

fn insert(conn: &Connection, id: &str, value: serde_json::Value) {
    crate::db::discussions::insert_message(
        conn,
        "d",
        &message(id, format!("```kronn-question\n{value}\n```")),
    )
    .unwrap();
}

fn request() -> AnswerDiscussionQuestionRequest {
    AnswerDiscussionQuestionRequest {
        selected_option_ids: vec!["retry".into()],
        text: None,
        idempotency_key: "answer-1".into(),
    }
}

#[test]
fn question_spec_is_bounded_and_rejects_unknown_or_duplicate_choices() {
    assert!(parse_spec(&payload().to_string()).is_some());
    for field in ["version", "key", "question"] {
        let mut value = payload();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            parse_spec(&value.to_string()).is_none(),
            "{field} is required"
        );
    }
    let mut value = payload();
    value["options"][1]["id"] = "retry".into();
    assert!(parse_spec(&value.to_string()).is_none());
    value = payload();
    value["recommended_option_ids"] = serde_json::json!(["missing"]);
    assert!(parse_spec(&value.to_string()).is_none());
    value = payload();
    value["question"] = "é".repeat(1001).into();
    assert!(parse_spec(&value.to_string()).is_none());
    value = payload();
    value["run_command"] = "anything".into();
    assert!(parse_spec(&value.to_string()).is_none());
}

#[test]
fn invalid_version_types_do_not_persist_and_corrected_same_key_does() {
    let conn = database();
    for (index, version) in [
        serde_json::json!("1"),
        serde_json::json!(true),
        serde_json::Value::Null,
        serde_json::json!(1.5),
        serde_json::json!(2),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = payload();
        value["version"] = version;
        insert(&conn, &format!("invalid-version-{index}"), value);
        assert!(list(&conn, "d").unwrap().questions.is_empty());
    }
    insert(&conn, "corrected-version", payload());
    let result = list(&conn, "d").unwrap();
    assert_eq!(result.pending_count, 1);
    assert_eq!(result.questions.len(), 1);
    assert_eq!(result.questions[0].source_message_id, "corrected-version");
    assert_eq!(result.questions[0].key, "quota-policy");
}

#[test]
fn only_real_agent_main_fences_persist_and_same_key_deduplicates() {
    let conn = database();
    let fence = format!("```kronn-question\n{}\n```", payload());
    for (id, role, channel) in [
        ("user", MessageRole::User, MessageChannel::Main),
        ("note", MessageRole::Agent, MessageChannel::Note),
    ] {
        let mut msg = message(id, fence.clone());
        msg.role = role;
        msg.channel = channel;
        crate::db::discussions::insert_message(&conn, "d", &msg).unwrap();
    }
    crate::db::discussions::insert_message(
        &conn,
        "d",
        &message("example", format!("````markdown\n{fence}\n````")),
    )
    .unwrap();
    crate::db::discussions::insert_message(
        &conn,
        "d",
        &message("incomplete", format!("```kronn-question\n{}", payload())),
    )
    .unwrap();
    assert_eq!(list(&conn, "d").unwrap().pending_count, 0);
    insert(&conn, "m", payload());
    insert(&conn, "replay", payload());
    let list = list(&conn, "d").unwrap();
    assert_eq!(list.questions.len(), 1);
    assert_eq!(list.questions[0].source_message_id, "m");
    assert_eq!(list.pending_count, 1);
}

#[test]
fn answer_replay_is_exact_and_routes_once_to_the_native_requester() {
    let conn = database();
    insert(&conn, "m", payload());
    let q = answer(&conn, "d", "question:m:0", &request(), "Romu", None).unwrap();
    assert_eq!(q.state, DiscussionQuestionState::Answered);
    assert_eq!(q.answer.as_ref().unwrap().author_pseudo, "Romu");
    assert_eq!(
        answer(&conn, "d", &q.id, &request(), "Romu", None).unwrap(),
        q
    );
    let mut changed = request();
    changed.selected_option_ids = vec!["wait".into()];
    assert!(matches!(
        answer(&conn, "d", &q.id, &changed, "Romu", None),
        Err(AnswerError::Conflict)
    ));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM messages WHERE role='User'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let targets =
        crate::db::discussions::list_message_targets(&conn, &q.answer.unwrap().message_id).unwrap();
    assert_eq!(targets[0].agent_type, crate::models::AgentType::Codex);
    assert_eq!(list(&conn, "d").unwrap().pending_count, 0);
}

#[test]
fn native_reply_uses_source_dispatch_connection_not_the_changed_room_default() {
    let conn = database();
    add_connection(&conn, "default-at-creation");
    add_connection(&conn, "actual-requester-connection");
    conn.execute(
        "UPDATE discussions SET agent='Custom', connection_id='default-at-creation' WHERE id='d'",
        [],
    )
    .unwrap();
    let mut source = message(
        "custom-source",
        format!("```kronn-question\n{}\n```", payload()),
    );
    source.agent_type = Some(crate::models::AgentType::Custom);
    let order = crate::db::discussions::insert_message(&conn, "d", &source).unwrap();
    crate::db::agent_dispatch::enqueue_with_connection(
        &conn,
        crate::db::agent_dispatch::NewAgentDispatchJob {
            id: "source-dispatch",
            discussion_id: "d",
            trigger_message_id: "custom-source",
            trigger_sort_order: order,
            dedupe_key: "source-dispatch",
            agent_override: Some(&crate::models::AgentType::Custom),
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
        Some("actual-requester-connection"),
    )
    .unwrap();
    conn.execute(
        "UPDATE messages SET agent_dispatch_job_id='source-dispatch' WHERE id='custom-source'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE discussions SET agent='Codex', connection_id=NULL WHERE id='d'",
        [],
    )
    .unwrap();
    let q = answer(
        &conn,
        "d",
        "question:custom-source:0",
        &request(),
        "Human",
        None,
    )
    .unwrap();
    let target = crate::db::discussions::list_message_targets(&conn, &q.answer.unwrap().message_id)
        .unwrap()
        .remove(0);
    assert_eq!(target.agent_type, crate::models::AgentType::Custom);
    assert_eq!(
        target.connection_id.as_deref(),
        Some("actual-requester-connection")
    );
    assert_eq!(conn.query_row("SELECT connection_id FROM agent_dispatch_jobs WHERE dedupe_key='question-answer:question:custom-source:0'", [], |row| row.get::<_, String>(0)).unwrap(), "actual-requester-connection");
}

#[test]
fn peer_provider_does_not_inherit_the_room_connection() {
    let conn = database();
    add_connection(&conn, "unrelated-default");
    conn.execute(
        "UPDATE discussions SET agent='Custom', connection_id='unrelated-default' WHERE id='d'",
        [],
    )
    .unwrap();
    insert(&conn, "m", payload());
    let q = answer(&conn, "d", "question:m:0", &request(), "Human", None).unwrap();
    let target = crate::db::discussions::list_message_targets(&conn, &q.answer.unwrap().message_id)
        .unwrap()
        .remove(0);
    assert_eq!(target.agent_type, crate::models::AgentType::Codex);
    assert_eq!(target.connection_id, None);
}

#[test]
fn free_text_replies_to_exact_cli_even_when_it_is_offline() {
    let conn = database();
    insert(&conn, "m", payload());
    conn.execute("INSERT INTO discussion_sessions (id,disc_id,agent_type,session_id,role,status,joined_at) VALUES (41,'d','Codex','cli-test','peer','left','now')", []).unwrap();
    crate::db::discussions::set_message_cli_author(&conn, "m", 41).unwrap();
    let request = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec![],
        text: Some("  Une troisième réponse 中文  ".into()),
        idempotency_key: "free".into(),
    };
    let q = answer(&conn, "d", "question:m:0", &request, "Romu", None).unwrap();
    let a = q.answer.unwrap();
    assert_eq!(a.text.as_deref(), Some("Une troisième réponse 中文"));
    let targets = crate::db::discussions::list_message_targets(&conn, &a.message_id).unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].kind, MessageTargetKind::Cli);
    assert_eq!(targets[0].cli_session_id, Some(41));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn invalid_cross_room_or_empty_answers_do_not_resolve_the_question() {
    let conn = database();
    insert(&conn, "m", payload());
    assert!(matches!(
        answer(&conn, "another", "question:m:0", &request(), "Romu", None),
        Err(AnswerError::NotFound)
    ));
    for selected in [
        vec![],
        vec!["unknown"],
        vec!["retry", "wait"],
        vec!["retry", "retry"],
    ] {
        let request = AnswerDiscussionQuestionRequest {
            selected_option_ids: selected.into_iter().map(str::to_string).collect(),
            text: Some("  ".into()),
            idempotency_key: "bad".into(),
        };
        assert!(matches!(
            answer(&conn, "d", "question:m:0", &request, "Romu", None),
            Err(AnswerError::Invalid(_))
        ));
    }
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM messages WHERE role='User'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn failed_receipt_rolls_back_answer_and_dispatch() {
    let conn = database();
    insert(&conn, "m", payload());
    conn.execute_batch("CREATE TRIGGER refuse_answer BEFORE INSERT ON messages WHEN NEW.role='User' BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
    assert!(answer(&conn, "d", "question:m:0", &request(), "Romu", None).is_err());
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn questions_and_answers_survive_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("questions.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute("INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES ('d','D','Codex','now','now')", []).unwrap();
        insert(&conn, "m", payload());
    }
    {
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        assert_eq!(list(&conn, "d").unwrap().pending_count, 1);
        answer(&conn, "d", "question:m:0", &request(), "Romu", None).unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        list(&conn, "d").unwrap().questions[0].state,
        DiscussionQuestionState::Answered
    );
}

/// The shape that created a phantom arbitration on 2026-09-15: a failed Codex
/// run whose raw output — folded into `kronn:context` as "technical details" —
/// carried the system prompt, and with it the documented example fence.
#[test]
fn a_question_inside_folded_context_is_not_ingested() {
    let content = "⛔ **Limite du plan atteinte.**\n\n\
<!-- kronn:context title=\"détails techniques\" -->\n\
Complete minimal example (replace key/question with this real decision):\n\
```kronn-question\n\
{\"version\":1,\"key\":\"decision-key\",\"question\":\"Which option should we use?\"}\n\
```\n\
<!-- /kronn:context -->\n";
    // Without the fix this exact content yielded one live fence — that is the
    // bug, and asserting it here keeps the test from passing for a wrong reason.
    assert_eq!(
        question_fences(content).len(),
        1,
        "the raw message must still contain the example fence"
    );
    let stripped = strip_context_blocks(content);
    assert!(
        !stripped.contains("kronn-question"),
        "folded context must be dropped: {stripped}"
    );
    assert!(
        stripped.contains("Limite du plan"),
        "the agent's own text must survive"
    );
    assert!(question_fences(&stripped).is_empty());
}

#[test]
fn a_real_question_outside_any_context_block_still_ingests() {
    let content = "Voici les options.\n\
```kronn-question\n\
{\"version\":1,\"key\":\"real-key\",\"question\":\"On garde quoi ?\"}\n\
```\n";
    assert_eq!(question_fences(&strip_context_blocks(content)).len(), 1);
}

#[test]
fn a_real_question_survives_alongside_a_context_block() {
    let content = "```kronn-question\n\
{\"version\":1,\"key\":\"real-key\",\"question\":\"On garde quoi ?\"}\n\
```\n\
<!-- kronn:context title=\"details\" -->\n\
```kronn-question\n\
{\"version\":1,\"key\":\"decision-key\",\"question\":\"Which option should we use?\"}\n\
```\n\
<!-- /kronn:context -->\n";
    let fences = question_fences(&strip_context_blocks(content));
    assert_eq!(fences.len(), 1, "only the agent's own question may survive");
    assert!(fences[0].contains("real-key"));
}

#[test]
fn an_unclosed_context_block_swallows_the_rest() {
    // A truncated message fails closed: a missed question beats a phantom one
    // that nothing in the schema can ever clear.
    let content = "ok\n<!-- kronn:context title=\"x\" -->\n```kronn-question\n{}\n```\n";
    assert!(!strip_context_blocks(content).contains("kronn-question"));
}

fn decline_request() -> DeclineDiscussionQuestionRequest {
    DeclineDiscussionQuestionRequest {
        idempotency_key: "decline-1".into(),
        reason: Some("La question n'a plus d'objet.".into()),
    }
}

#[test]
fn declining_resolves_the_question_and_reaches_the_asker() {
    // A refusal is a decision: it must leave `pending` AND be delivered, or the
    // agent waits forever on an answer that is never coming.
    let conn = database();
    insert(&conn, "m", payload());
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1);

    let q = decline(&conn, "d", "question:m:0", &decline_request(), "Romu", None).unwrap();
    assert_eq!(q.state, DiscussionQuestionState::Declined);
    let record = q.answer.as_ref().expect("a refusal is recorded like an answer");
    assert!(record.selected_option_ids.is_empty(), "nothing was chosen");
    assert_eq!(record.text.as_deref(), Some("La question n'a plus d'objet."));
    assert_eq!(record.author_pseudo, "Romu");
    assert_eq!(list(&conn, "d").unwrap().pending_count, 0);

    // Same delivery path as an answer: one message, one dispatch, right target.
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let targets =
        crate::db::discussions::list_message_targets(&conn, &record.message_id).unwrap();
    assert_eq!(targets[0].agent_type, crate::models::AgentType::Codex);
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&record.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(content.starts_with("Arbitrage refusé —"), "{content}");
    assert!(content.contains("La question n'a plus d'objet."));
}

#[test]
fn a_refusal_without_a_reason_still_tells_the_agent_what_to_do() {
    // A human owes no justification, but an empty refusal must not read as a
    // shrug: the agent needs to know it may proceed without the decision.
    let conn = database();
    insert(&conn, "m", payload());
    let request = DeclineDiscussionQuestionRequest {
        idempotency_key: "decline-bare".into(),
        reason: None,
    };
    let q = decline(&conn, "d", "question:m:0", &request, "Romu", None).unwrap();
    assert_eq!(q.state, DiscussionQuestionState::Declined);
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&q.answer.unwrap().message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(content.contains("Ne bloque pas sur cette question"), "{content}");
}

#[test]
fn declining_replays_on_the_same_key_and_refuses_to_overwrite_a_decision() {
    let conn = database();
    insert(&conn, "m", payload());
    let q = decline(&conn, "d", "question:m:0", &decline_request(), "Romu", None).unwrap();
    // Replay of the same key is the client retrying, not a conflict.
    assert_eq!(
        decline(&conn, "d", &q.id, &decline_request(), "Romu", None).unwrap(),
        q
    );
    // A different key on an already-refused question IS a conflict.
    let other = DeclineDiscussionQuestionRequest {
        idempotency_key: "decline-2".into(),
        reason: None,
    };
    assert!(matches!(
        decline(&conn, "d", &q.id, &other, "Romu", None),
        Err(AnswerError::Conflict)
    ));
    // And exactly one dispatch was ever enqueued.
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn an_answered_question_cannot_be_refused_afterwards() {
    // The agent already received that decision; refusing now would rewrite it.
    let conn = database();
    insert(&conn, "m", payload());
    let q = answer(&conn, "d", "question:m:0", &request(), "Romu", None).unwrap();
    assert!(matches!(
        decline(&conn, "d", &q.id, &decline_request(), "Romu", None),
        Err(AnswerError::Conflict)
    ));
    assert_eq!(
        list(&conn, "d").unwrap().questions[0].state,
        DiscussionQuestionState::Answered
    );
}

#[test]
fn a_refused_question_cannot_then_be_answered() {
    let conn = database();
    insert(&conn, "m", payload());
    let q = decline(&conn, "d", "question:m:0", &decline_request(), "Romu", None).unwrap();
    assert!(matches!(
        answer(&conn, "d", &q.id, &request(), "Romu", None),
        Err(AnswerError::Conflict)
    ));
}
