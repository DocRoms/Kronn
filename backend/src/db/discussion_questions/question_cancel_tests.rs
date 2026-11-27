//! KT-1955 — a human refuses a pending card without a message: the card
//! closes (state `cancelled`) and nothing else happens. Every test of this
//! module is selected by `cargo test --lib question_cancel`.

use super::tests::{
    active_step_cards, card, database, decline_request, dispatch_count, finished_step_cards,
    finished_step_question, insert, payload, request,
};
use super::*;

/// « Refuser » with the field left empty.
fn blank(key: &str) -> DeclineDiscussionQuestionRequest {
    DeclineDiscussionQuestionRequest {
        idempotency_key: key.into(),
        reason: None,
    }
}

/// The silent close as every client reaches it: `decline` without a message.
fn cancel(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    author: &str,
) -> Result<DiscussionQuestion, AnswerError> {
    decline(
        conn,
        discussion_id,
        question_id,
        &blank("blank-1"),
        author,
        None,
    )
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// Everything a resolution could have started: messages, targets, dispatches
/// and workflow runs.
fn side_effects(conn: &Connection) -> (i64, i64, i64, i64) {
    (
        count(conn, "SELECT COUNT(*) FROM messages"),
        count(conn, "SELECT COUNT(*) FROM message_targets"),
        dispatch_count(conn),
        count(conn, "SELECT COUNT(*) FROM workflow_runs"),
    )
}

#[test]
fn question_cancel_closes_the_card_with_who_and_when_and_starts_nothing() {
    let conn = database();
    // A native agent's card: answering or refusing it would enqueue a turn.
    insert(&conn, "m", payload());
    let before = side_effects(&conn);
    let started = Utc::now();

    let q = cancel(&conn, "d", "question:m:0", "Romu").unwrap();

    assert_eq!(q.state, DiscussionQuestionState::Cancelled);
    assert_eq!(q.answer, None, "a cancelled card carries no answer");
    let cancellation = q.cancellation.as_ref().expect("who and when are kept");
    assert_eq!(cancellation.author_pseudo, "Romu");
    let at = chrono::DateTime::parse_from_rfc3339(&cancellation.cancelled_at).unwrap();
    assert!(at >= started - chrono::Duration::seconds(1), "{at}");
    assert_eq!(q.updated_at, cancellation.cancelled_at);
    assert_eq!(
        side_effects(&conn),
        before,
        "no message, no target, no dispatch, no run: nothing reaches the agent"
    );
    // The read model says the same after a reload.
    assert_eq!(list(&conn, "d").unwrap().questions[0], q);
}

#[test]
fn question_cancel_leaves_the_pending_count_of_the_room_and_of_the_list() {
    let conn = database();
    insert(&conn, "m1", card("first"));
    insert(&conn, "m2", card("second"));
    let discussion = crate::db::discussions::get_discussion(&conn, "d")
        .unwrap()
        .unwrap();
    let listed = |conn: &Connection| {
        with_pending_counts(conn, vec![discussion.clone()]).unwrap()[0].pending_question_count
    };
    assert_eq!(list(&conn, "d").unwrap().pending_count, 2);
    assert_eq!(listed(&conn), 2);

    cancel(&conn, "d", "question:m1:0", "Romu").unwrap();
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1, "room banner");
    assert_eq!(listed(&conn), 1, "list badge");
    let monitored = crate::db::discussion_monitor::preview(&conn, "d")
        .unwrap()
        .unwrap();
    assert_eq!(monitored.pending_question_count, 1, "monitor badge");

    cancel(&conn, "d", "question:m2:0", "Romu").unwrap();
    assert_eq!(list(&conn, "d").unwrap().pending_count, 0);
    assert_eq!(listed(&conn), 0);
}

#[test]
fn question_cancel_never_resumes_the_workflow_parked_on_the_card() {
    let mut resumable = payload();
    resumable["resume"] = serde_json::json!({
        "workflow_id": "resume-workflow",
        "variables": {"ticket": "KT-1955"}
    });
    let conn = finished_step_question(resumable);
    conn.execute(
        "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
         VALUES ('resume-workflow', 'Resume', '{}', '[]', 'now', 'now')",
        [],
    )
    .unwrap();
    let before = side_effects(&conn);

    cancel(&conn, "d", "question:step-question:0", "Romu").unwrap();

    assert!(
        claim_resume(&conn, "d", "question:step-question:0")
            .unwrap()
            .is_none(),
        "a cancelled card claims no resume run"
    );
    let q = list(&conn, "d").unwrap().questions.remove(0);
    let resume = q.resume.expect("the declared resume is still shown");
    assert_eq!(
        resume.state,
        DiscussionQuestionResumeState::Pending,
        "the workflow stays parked on the cancelled decision"
    );
    assert_eq!(resume.run_id, None);
    assert_eq!(
        side_effects(&conn),
        before,
        "the ended step's room agent is not woken either"
    );
}

#[test]
fn question_cancel_is_idempotent_and_closes_the_card_to_answers_and_refusals() {
    let conn = database();
    insert(&conn, "m", payload());
    let first = cancel(&conn, "d", "question:m:0", "Romu").unwrap();
    // A second click, or a second tab: the same card, unchanged.
    assert_eq!(cancel(&conn, "d", "question:m:0", "Alex").unwrap(), first);
    assert_eq!(
        decline(&conn, "d", "question:m:0", &blank("blank-2"), "Alex", None).unwrap(),
        first,
        "another key is still the same blank refusal"
    );

    assert!(matches!(
        answer(&conn, "d", "question:m:0", &request(), "Romu", None),
        Err(AnswerError::Conflict)
    ));
    assert!(matches!(
        decline(&conn, "d", "question:m:0", &decline_request(), "Romu", None),
        Err(AnswerError::Conflict)
    ));
    assert!(matches!(
        comment(
            &conn,
            "d",
            "question:m:0",
            &CommentDiscussionQuestionRequest {
                idempotency_key: "c-1".into(),
                text: "Encore là ?".into(),
            },
            "Romu",
            None,
        ),
        Err(AnswerError::Conflict)
    ));
    assert_eq!(dispatch_count(&conn), 0);
    assert_eq!(list(&conn, "d").unwrap().questions[0], first);
}

#[test]
fn question_cancel_refuses_a_decided_card_or_a_card_of_another_room() {
    let conn = database();
    conn.execute(
        "INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES ('other','O','Codex','now','now')",
        [],
    )
    .unwrap();
    insert(&conn, "answered", card("answered"));
    insert(&conn, "declined", card("declined"));
    answer(&conn, "d", "question:answered:0", &request(), "Romu", None).unwrap();
    decline(
        &conn,
        "d",
        "question:declined:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();

    for id in ["question:answered:0", "question:declined:0"] {
        assert!(
            matches!(cancel(&conn, "d", id, "Romu"), Err(AnswerError::Conflict)),
            "{id}: a decision the agent received is not rewritten"
        );
    }
    assert!(matches!(
        cancel(&conn, "other", "question:answered:0", "Romu"),
        Err(AnswerError::NotFound)
    ));
    assert!(matches!(
        cancel(&conn, "d", "question:missing:0", "Romu"),
        Err(AnswerError::NotFound)
    ));
    let states = list(&conn, "d")
        .unwrap()
        .questions
        .into_iter()
        .map(|q| (q.state, q.cancellation))
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            (DiscussionQuestionState::Answered, None),
            (DiscussionQuestionState::Declined, None),
        ]
    );
}

/// What `disc_question_list({key})` relays to an agent: the card's JSON as the
/// list returns it.
#[test]
fn question_cancel_reads_back_by_key_as_cancelled_with_no_answer() {
    let conn = database();
    insert(&conn, "m", payload());
    cancel(&conn, "d", "question:m:0", "Romu").unwrap();

    let listed = serde_json::to_value(list(&conn, "d").unwrap()).unwrap();
    let card = listed["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["key"] == "quota-policy")
        .unwrap();
    assert_eq!(card["state"], "cancelled");
    assert!(card["answer"].is_null(), "{card}");
    assert_eq!(card["cancellation"]["author_pseudo"], "Romu");
    assert!(card["cancellation"]["cancelled_at"].is_string());
    assert_eq!(listed["pending_count"], 0);
}

#[test]
fn question_cancel_is_named_in_the_room_agent_recap_of_its_siblings() {
    // The step that asked has ended; one card is cancelled, the other is then
    // answered. The answer wakes the room agent (that is the answer's doing),
    // and the recap tells it the sibling was cancelled, not answered.
    let conn = finished_step_cards(&[card("first"), card("second")]);
    cancel(&conn, "d", "question:step-question:0", "Romu").unwrap();
    assert_eq!(dispatch_count(&conn), 0, "the cancel itself wakes nobody");

    answer(
        &conn,
        "d",
        "question:step-question:1",
        &AnswerDiscussionQuestionRequest {
            selected_option_ids: vec!["retry".into()],
            item_answers: vec![],
            text: None,
            idempotency_key: "answer-1".into(),
        },
        "Romu",
        None,
    )
    .unwrap();
    assert_eq!(dispatch_count(&conn), 1);
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id='question-answer:question:step-question:1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        content.contains("- Question first ? → refusée sans message par un humain"),
        "{content}"
    );
}

#[test]
fn question_cancel_survives_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kronn.db");
    {
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES ('d','D','Codex','now','now')",
            [],
        )
        .unwrap();
        insert(&conn, "m", payload());
        cancel(&conn, "d", "question:m:0", "Romu").unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    crate::db::migrations::run(&conn).unwrap();
    let q = list(&conn, "d").unwrap().questions.remove(0);
    assert_eq!(q.state, DiscussionQuestionState::Cancelled);
    assert_eq!(q.cancellation.unwrap().author_pseudo, "Romu");
}

/// The table rebuild keeps every row and column written before it, the
/// resume bookkeeping included.
#[test]
fn question_cancel_migration_keeps_every_existing_card() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run_through(&conn, "301_workflow_runs_shadow_tokens").unwrap();
    conn.execute_batch(
        "INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES ('d','D','Codex','now','now');
         INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
             VALUES ('m', 'd', 'Agent', 'x', 'now', 1);
         INSERT INTO discussion_questions
             (id, discussion_id, source_message_id, fence_index, question_key, payload_json,
              state, answer_json, answer_idempotency_key, requester_connection_id,
              created_at, updated_at, resume_state, resume_run_id, resume_error)
         VALUES
             ('q0', 'd', 'm', 0, 'k0', '{}', 'pending', NULL, NULL, NULL, 't0', 't0', NULL, NULL, NULL),
             ('q1', 'd', 'm', 1, 'k1', '{}', 'declined', '{\"a\":1}', 'idem', 'conn', 't1', 't2',
              'failed', 'run-1', 'boom');",
    )
    .unwrap();

    crate::db::migrations::run(&conn).unwrap();

    let row: (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT state, answer_json, resume_state, resume_run_id, resume_error
               FROM discussion_questions WHERE id='q1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "declined".into(),
            Some("{\"a\":1}".into()),
            Some("failed".into()),
            Some("run-1".into()),
            Some("boom".into())
        )
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM discussion_questions WHERE state='pending'"
        ),
        1
    );
    let indexes = count(
        &conn,
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND name IN
           ('idx_discussion_questions_pending', 'idx_discussion_questions_resume_run')",
    );
    assert_eq!(indexes, 2, "both indexes are rebuilt");
    conn.execute(
        "UPDATE discussion_questions SET state='cancelled', cancelled_by='Romu', cancelled_at='t3' WHERE id='q0'",
        [],
    )
    .expect("the new state is accepted");
    assert!(conn
        .execute(
            "UPDATE discussion_questions SET state='void' WHERE id='q0'",
            []
        )
        .is_err());
}

#[test]
fn question_cancel_counts_an_empty_or_whitespace_message_as_blank() {
    let conn = database();
    for (id, reason) in [("m1", ""), ("m2", "   \n\t  ")] {
        insert(&conn, id, card(id));
        let before = side_effects(&conn);
        let q = decline(
            &conn,
            "d",
            &format!("question:{id}:0"),
            &DeclineDiscussionQuestionRequest {
                idempotency_key: format!("blank-{id}"),
                reason: Some(reason.into()),
            },
            "Romu",
            None,
        )
        .unwrap();
        assert_eq!(q.state, DiscussionQuestionState::Cancelled, "{reason:?}");
        assert_eq!(q.answer, None);
        assert_eq!(side_effects(&conn), before, "{reason:?} woke someone");
    }
    assert!(is_blank_refusal(&blank("k")));
    assert!(!is_blank_refusal(&decline_request()));
}

#[test]
fn question_cancel_leaves_a_refusal_with_a_message_unchanged() {
    // The message is what the agent must receive: published, targeted,
    // dispatched, and the card reads `declined` with the reason.
    let conn = database();
    insert(&conn, "m", payload());
    let q = decline(&conn, "d", "question:m:0", &decline_request(), "Romu", None).unwrap();
    assert_eq!(q.state, DiscussionQuestionState::Declined);
    assert_eq!(q.cancellation, None);
    assert_eq!(dispatch_count(&conn), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE id='question-decline:question:m:0'"
        ),
        1
    );
    // And a later blank refusal does not rewrite it.
    assert!(matches!(
        cancel(&conn, "d", "question:m:0", "Romu"),
        Err(AnswerError::Conflict)
    ));
}

fn answer_card(conn: &Connection, index: usize, option: &str) {
    answer(
        conn,
        "d",
        &format!("question:step-question:{index}"),
        &AnswerDiscussionQuestionRequest {
            selected_option_ids: vec![option.into()],
            item_answers: vec![],
            text: None,
            idempotency_key: format!("answer-{index}"),
        },
        "Romu",
        None,
    )
    .unwrap();
}

fn blank_refuse_card(conn: &Connection, index: usize, key: &str) -> DiscussionQuestion {
    decline(
        conn,
        "d",
        &format!("question:step-question:{index}"),
        &blank(key),
        "Romu",
        None,
    )
    .unwrap()
}

fn relay_messages(conn: &Connection) -> i64 {
    count(
        conn,
        "SELECT COUNT(*) FROM messages WHERE id LIKE 'question-relay:%'",
    )
}

/// KT-1401 — the step that asked has ended: the earlier answers wait for the
/// last card. Refusing that card without a message must not lose them.
#[test]
fn question_cancel_of_the_last_card_of_an_ended_step_relays_the_earlier_answers_once() {
    let conn = finished_step_cards(&[card("first"), card("second")]);
    answer_card(&conn, 0, "retry");
    assert_eq!(
        dispatch_count(&conn),
        0,
        "the first answer waits for the last card"
    );

    let q = blank_refuse_card(&conn, 1, "blank-1");

    assert_eq!(q.state, DiscussionQuestionState::Cancelled);
    assert_eq!(q.answer, None, "the refusal itself carries no answer");
    assert!(
        q.cancellation.as_ref().unwrap().relayed_answers,
        "the card knows the earlier answers were relayed"
    );
    assert_eq!(relay_messages(&conn), 1);
    assert_eq!(dispatch_count(&conn), 1, "one wake for the earlier answers");
    let (agent, trigger): (String, String) = conn
        .query_row(
            "SELECT agent_override_json, trigger_message_id FROM agent_dispatch_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(agent.contains("Codex"), "{agent}");
    assert_eq!(trigger, "question-relay:question:step-question:1");
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&trigger],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        content.contains("- Question first ? → Réarmer"),
        "{content}"
    );
    assert!(
        content.contains("- Question second ? → refusée sans message par un humain"),
        "{content}"
    );
    assert!(content.contains("l'agent de la room reprend"), "{content}");
    let targets = crate::db::discussions::list_message_targets(&conn, &trigger).unwrap();
    assert_eq!(targets[0].agent_type, crate::models::AgentType::Codex);
    // The read model says the same after a reload.
    assert_eq!(list(&conn, "d").unwrap().questions[1], q);
}

#[test]
fn question_cancel_repeated_on_the_last_card_relays_the_earlier_answers_only_once() {
    let conn = finished_step_cards(&[card("first"), card("second")]);
    answer_card(&conn, 0, "retry");
    let first = blank_refuse_card(&conn, 1, "blank-1");
    let again = blank_refuse_card(&conn, 1, "blank-1");
    let other_tab = blank_refuse_card(&conn, 1, "blank-2");
    assert_eq!(again, first);
    assert_eq!(other_tab, first);
    assert_eq!(relay_messages(&conn), 1, "still one message");
    assert_eq!(dispatch_count(&conn), 1, "still one dispatch");
}

#[test]
fn question_cancel_of_every_card_of_an_ended_step_wakes_nobody() {
    let conn = finished_step_cards(&[card("first"), card("second")]);
    let before = side_effects(&conn);
    blank_refuse_card(&conn, 0, "blank-0");
    let last = blank_refuse_card(&conn, 1, "blank-1");
    assert!(!last.cancellation.unwrap().relayed_answers);
    assert_eq!(
        side_effects(&conn),
        before,
        "no earlier answer to relay: no message, no dispatch"
    );
}

#[test]
fn question_cancel_of_the_last_card_relays_a_sibling_refused_with_a_message() {
    let conn = finished_step_cards(&[card("first"), card("second")]);
    decline(
        &conn,
        "d",
        "question:step-question:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();
    assert_eq!(dispatch_count(&conn), 0, "the ended step reads nothing now");

    blank_refuse_card(&conn, 1, "blank-1");

    assert_eq!(dispatch_count(&conn), 1);
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id='question-relay:question:step-question:1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        content.contains("- Question first ? → refusée"),
        "{content}"
    );
}

#[test]
fn question_cancel_while_the_step_still_runs_wakes_nobody() {
    // Each answer already reached the step that asked: nothing waits on the
    // last card, so refusing it without a message stays silent.
    let conn = active_step_cards(&[card("first"), card("second")]);
    answer_card(&conn, 0, "retry");
    let before = side_effects(&conn);

    let q = blank_refuse_card(&conn, 1, "blank-1");

    assert_eq!(q.state, DiscussionQuestionState::Cancelled);
    assert!(!q.cancellation.unwrap().relayed_answers);
    assert_eq!(relay_messages(&conn), 0);
    assert_eq!(
        side_effects(&conn),
        before,
        "the blank refusal wakes nobody"
    );
}
