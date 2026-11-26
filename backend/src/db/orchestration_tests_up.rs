//! KT-317 — persistence + state-machine tests (DoD-6): constraints, forbidden
//! transitions, idempotence, concurrency, saga-checkpoint resume, lineage. No
//! Git integration (that is KT-320).

use super::*;
use rusqlite::{params, Connection};

use crate::models::{
    saga_resume_action, AgentType, BlockedReasonCode, CampaignWorkerSelection,
    CancellationCleanupPolicy, ExecutionRecoveryAction, ExecutionTimeoutKind,
    LaunchSingleTaskInput, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor,
    OrchestrationControlState, OrchestrationResiliencePolicy, OrchestrationRunInput,
    OrchestrationRunKind, PlanningActorKind, SagaResumeAction, TaskExecutionStatus,
    TaskWorkerScope, ValidationSpec,
};

const DISC: &str = "disc-parent";

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at) \
         VALUES ('p1', 'P', '/p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO discussions (id, title, created_at, updated_at) \
         VALUES (?1, 'D', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![DISC],
    )
    .unwrap();
    conn
}

fn seed_task(conn: &Connection, task_id: &str, number: i64) {
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
         VALUES (?1, ?2, 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![task_id, number],
    )
    .unwrap();
}

fn backend_actor() -> OrchestrationActor {
    OrchestrationActor {
        kind: PlanningActorKind::Backend,
        id: Some("orchestrator".into()),
        session_id: None,
        source_message_id: None,
        token_declared: false,
    }
}

/// Sets a process-wide timeout variable for one test and puts it back on drop
/// (also on panic). Only used under `serial(task_timeout_env)`.
struct TimeoutEnv(&'static str);

impl TimeoutEnv {
    fn set(name: &'static str, value: &str) -> Self {
        crate::core::child_env::set_var(name, value);
        TimeoutEnv(name)
    }
}

impl Drop for TimeoutEnv {
    fn drop(&mut self) {
        crate::core::child_env::remove_var(self.0);
    }
}

// ── KT-1433: Docker elevation card ───────────────────────────────────────

fn launch_in_project(conn: &Connection, task_id: &str, number: i64) -> String {
    seed_task(conn, task_id, number);
    let mut input = LaunchSingleTaskInput::new(task_id, DISC);
    input.project_id = Some("p1".into());
    launch_single_task(conn, &input, &backend_actor())
        .unwrap()
        .execution
        .id
}

fn elevation_fixture(conn: &Connection) -> (String, String) {
    let exec = launch_in_project(conn, "task-1433", 1433);
    post_docker_elevation_card(conn, &exec).unwrap();
    let card: String = conn
        .query_row(
            "SELECT id FROM messages WHERE id LIKE 'orch-elevation:docker:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    (exec, card)
}

#[test]
fn kronn_posts_a_docker_elevation_card_with_three_scopes() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    let (key, payload): (String, String) = conn
        .query_row(
            "SELECT question_key, payload_json FROM discussion_questions WHERE source_message_id = ?1",
            [&card],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(key, format!("kronn-elevation-docker.{exec}"));
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let ids: Vec<&str> = payload["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["deny", "discussion", "project"]);
    let kind: String = conn
        .query_row("SELECT kind FROM messages WHERE id = ?1", [&card], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(kind, "orchestration");
}

#[test]
fn a_project_elevation_opens_docker_to_the_project_and_never_narrows_full_access() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    assert_eq!(
        record_docker_elevation(&conn, &card, "project", "Romu").unwrap(),
        Some(exec.clone())
    );
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Docker
    );
    crate::db::projects::set_validations_access(
        &conn,
        "p1",
        crate::models::ValidationsAccess::Full,
    )
    .unwrap();
    record_docker_elevation(&conn, &card, "project", "Romu").unwrap();
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Full
    );
}

#[test]
fn a_discussion_elevation_grants_only_that_discussion() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    assert_eq!(
        record_docker_elevation(&conn, &card, "discussion", "Romu").unwrap(),
        Some(exec)
    );
    assert!(
        crate::db::projects::discussion_capability_granted(&conn, DISC, "validations_docker")
            .unwrap()
    );
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Sandbox,
        "the project is untouched"
    );
}

#[test]
fn a_refusal_or_a_look_alike_card_grants_nothing() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    assert_eq!(
        record_docker_elevation(&conn, &card, "deny", "Romu").unwrap(),
        None
    );
    // A message with the prefix but without the orchestration kind is not
    // Kronn's request.
    conn.execute("UPDATE messages SET kind = NULL WHERE id = ?1", [&card])
        .unwrap();
    assert_eq!(
        record_docker_elevation(&conn, &card, "project", "Romu").unwrap(),
        None
    );
    assert_eq!(
        record_docker_elevation(&conn, &format!("question:{exec}:0"), "project", "Romu").unwrap(),
        None
    );
    assert!(
        !crate::db::projects::discussion_capability_granted(&conn, DISC, "validations_docker")
            .unwrap()
    );
    assert_eq!(
        crate::db::projects::validations_access(&conn, "p1").unwrap(),
        crate::models::ValidationsAccess::Sandbox
    );
}

/// The elevation cards of the room, oldest first: (question id, message,
/// stored key, state).
fn elevation_cards(conn: &Connection) -> Vec<(String, String, String, String)> {
    conn.prepare(
        "SELECT id, source_message_id, question_key, state FROM discussion_questions
          WHERE discussion_id = ?1 ORDER BY created_at, id",
    )
    .unwrap()
    .query_map([DISC], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// A human answers a card, then what the answer route does with it: the
/// execution to resume, if any.
fn answer_elevation(conn: &Connection, question_id: &str, choice: &str) -> Option<String> {
    let question = crate::db::discussion_questions::answer(
        conn,
        DISC,
        question_id,
        &crate::db::discussion_questions::AnswerDiscussionQuestionRequest {
            selected_option_ids: vec![choice.into()],
            item_answers: vec![],
            text: None,
            idempotency_key: format!("answer-{question_id}"),
        },
        "Romu",
        None,
    )
    .unwrap();
    record_docker_elevation(conn, &question.source_message_id, choice, "Romu").unwrap()
}

/// KT-2087 — two executions of one room held on Docker at once each get their
/// own card, both answerable, and each answer resumes its own execution.
#[test]
fn two_executions_held_on_docker_in_one_room_each_get_an_answerable_card() {
    let conn = setup();
    let first = launch_in_project(&conn, "task-2087-a", 2087);
    let second = launch_in_project(&conn, "task-2087-b", 2088);
    post_docker_elevation_card(&conn, &first).unwrap();
    post_docker_elevation_card(&conn, &second).unwrap();

    let cards = elevation_cards(&conn);
    assert_eq!(cards.len(), 2, "{cards:?}");
    for ((_, message, key, state), exec) in cards.iter().zip([&first, &second]) {
        assert_eq!(message, &format!("{DOCKER_ELEVATION_PREFIX}{exec}:0"));
        assert_eq!(key, &format!("kronn-elevation-docker.{exec}"));
        assert_eq!(state, "pending");
    }
    assert_eq!(
        answer_elevation(&conn, &cards[1].0, "discussion"),
        Some(second)
    );
    assert_eq!(answer_elevation(&conn, &cards[0].0, "project"), Some(first));
}

/// KT-2087 — refused on Docker again after its card was answered, the same
/// execution gets a new round of its own card, answerable like the first.
#[test]
fn a_second_docker_refusal_after_an_answer_opens_a_new_round_of_the_card() {
    let conn = setup();
    let (exec, _card) = elevation_fixture(&conn);
    let first = elevation_cards(&conn);
    assert_eq!(answer_elevation(&conn, &first[0].0, "deny"), None);

    post_docker_elevation_card(&conn, &exec).unwrap();
    let cards = elevation_cards(&conn);
    let key = format!("kronn-elevation-docker.{exec}");
    let rounds: Vec<(&str, &str, &str)> = cards
        .iter()
        .map(|(_, message, key, state)| (message.as_str(), key.as_str(), state.as_str()))
        .collect();
    assert_eq!(
        rounds,
        [
            (
                format!("{DOCKER_ELEVATION_PREFIX}{exec}:0").as_str(),
                key.as_str(),
                "answered"
            ),
            (
                format!("{DOCKER_ELEVATION_PREFIX}{exec}:1").as_str(),
                format!("{key}~2").as_str(),
                "pending"
            ),
        ]
    );
    assert_eq!(
        answer_elevation(&conn, &cards[1].0, "discussion"),
        Some(exec)
    );
}

/// KT-2087 — refused again while its card still waits, the execution gets no
/// second message: the waiting card answers for it. Once it is answered, a new
/// refusal opens a new round.
#[test]
fn a_docker_refusal_while_the_card_waits_posts_nothing_more() {
    let conn = setup();
    let (exec, card) = elevation_fixture(&conn);
    post_docker_elevation_card(&conn, &exec).unwrap();
    post_docker_elevation_card(&conn, &exec).unwrap();

    let posted = |conn: &Connection| -> Vec<String> {
        conn.prepare("SELECT id FROM messages WHERE id LIKE 'orch-elevation:docker:%' ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(posted(&conn), std::slice::from_ref(&card));
    let cards = elevation_cards(&conn);
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].3, "pending");

    assert_eq!(answer_elevation(&conn, &cards[0].0, "deny"), None);
    post_docker_elevation_card(&conn, &exec).unwrap();
    let second = format!("{DOCKER_ELEVATION_PREFIX}{exec}:1");
    assert_eq!(posted(&conn), [card, second.clone()]);
    let cards = elevation_cards(&conn);
    assert_eq!(cards.len(), 2, "{cards:?}");
    assert_eq!(
        (
            cards[1].1.as_str(),
            cards[1].2.as_str(),
            cards[1].3.as_str()
        ),
        (
            second.as_str(),
            format!("kronn-elevation-docker.{exec}~2").as_str(),
            "pending"
        )
    );
}

/// An agent's message carrying a card under `key`.
fn agent_card_message(message_id: &str, key: &str) -> DiscussionMessage {
    let card = serde_json::json!({"version": 1, "key": key, "question": "Autoriser Docker ?",
        "options": [{"id": "deny", "label": "Refuser"}, {"id": "project", "label": "Autoriser"}]});
    DiscussionMessage {
        id: message_id.into(),
        role: MessageRole::Agent,
        channel: MessageChannel::Main,
        content: format!("```kronn-question\n{card}\n```"),
        agent_type: Some(AgentType::Codex),
        timestamp: Utc::now(),
        tokens_used: 0,
        session_tokens_at_message: None,
        recovered_partial: false,
        auth_mode: None,
        model_tier: None,
        model: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        lint_report: None,
        target_agent: None,
        reply_to_message_id: None,
        author_cli_ordinal: None,
        session_continuity: None,
        kind: Default::default(),
    }
}

/// An agent's message carrying a card under `key`: the reason it was refused,
/// if it was.
fn agent_card_under(conn: &Connection, message_id: &str, key: &str) -> Option<String> {
    let message = agent_card_message(message_id, key);
    crate::db::discussions::insert_message(conn, DISC, &message).unwrap();
    let report: Option<String> = conn
        .query_row(
            "SELECT lint_report FROM messages WHERE id = ?1",
            [message_id],
            |row| row.get(0),
        )
        .unwrap();
    let report: crate::core::anti_halluc::LintReport = serde_json::from_str(&report?).unwrap();
    assert_eq!(report.invalid_cards.len(), 1, "{:?}", report.invalid_cards);
    Some(report.invalid_cards[0].reason.clone())
}

/// KT-2085 — a Docker elevation key is Kronn's: an answer only grants on
/// Kronn's own message, so an agent's card under it is refused, first or
/// after Kronn's card, and never blocks Kronn's next round.
#[test]
fn an_agent_card_under_a_docker_elevation_key_is_refused() {
    let conn = setup();
    let other = launch_in_project(&conn, "task-2085-other", 2085);
    let posted_first = agent_card_under(
        &conn,
        "agent-first",
        &format!("kronn-elevation-docker.{other}"),
    )
    .expect("posted before Kronn's card, it is refused");
    assert!(
        posted_first.contains("Docker elevation cards Kronn posts")
            && posted_first.contains("under any key")
            && !posted_first.contains("new key"),
        "{posted_first}"
    );

    let (exec, card) = elevation_fixture(&conn);
    let key = format!("kronn-elevation-docker.{exec}");
    let while_waiting = agent_card_under(&conn, "agent-waiting", &key).unwrap();
    assert!(
        while_waiting.starts_with(&format!(
            "key `{key}` is still waiting for an answer in message {card}"
        )) && while_waiting.contains("Docker elevation cards Kronn posts")
            && !while_waiting.contains("new key"),
        "{while_waiting}"
    );
    let cards = elevation_cards(&conn);
    assert_eq!(answer_elevation(&conn, &cards[0].0, "deny"), None);
    let after_answer = agent_card_under(&conn, "agent-after", &key).unwrap();
    assert!(
        after_answer.contains("Docker elevation cards Kronn posts")
            && !after_answer.contains("new key"),
        "{after_answer}"
    );
    // An id that looks like Kronn's is not enough: the message must be Kronn's.
    let look_alike = format!("{DOCKER_ELEVATION_PREFIX}{other}:0");
    assert!(agent_card_under(&conn, &look_alike, &key).is_some());

    // Kronn asks again on its own message: a new round, answerable as before.
    post_docker_elevation_card(&conn, &exec).unwrap();
    let rounds: Vec<(String, String, String)> = elevation_cards(&conn)
        .into_iter()
        .map(|(_, message, key, state)| (message, key, state))
        .collect();
    assert_eq!(
        rounds,
        [
            (card, key.clone(), "answered".to_string()),
            (
                format!("{DOCKER_ELEVATION_PREFIX}{exec}:1"),
                format!("{key}~2"),
                "pending".to_string()
            ),
        ]
    );
}

/// KT-2085 — an agent cannot take an execution's elevation key before Kronn:
/// its card is refused, and Kronn's card for that execution is still round 1.
#[test]
fn an_agent_posting_an_execution_elevation_key_first_never_takes_its_first_round() {
    let conn = setup();
    let exec = launch_in_project(&conn, "task-2085-first", 2086);
    let key = format!("kronn-elevation-docker.{exec}");
    let refused = agent_card_under(&conn, "agent-first", &key).expect("refused");
    assert!(
        refused.starts_with(&format!(
            "key `{key}` is reserved to the Docker elevation cards Kronn posts"
        )),
        "{refused}"
    );
    assert!(elevation_cards(&conn).is_empty());

    post_docker_elevation_card(&conn, &exec).unwrap();
    let cards = elevation_cards(&conn);
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(
        (
            cards[0].1.as_str(),
            cards[0].2.as_str(),
            cards[0].3.as_str()
        ),
        (
            format!("{DOCKER_ELEVATION_PREFIX}{exec}:0").as_str(),
            key.as_str(),
            "pending"
        )
    );
    assert_eq!(
        answer_elevation(&conn, &cards[0].0, "discussion"),
        Some(exec)
    );
}

/// KT-2085 — the whole `kronn-elevation-docker` key prefix is Kronn's: the old
/// fixed key and a dashed look-alike from an agent are refused like the dotted
/// form (the UI styles them as elevation cards, their answer grants nothing),
/// while Kronn's own message is recorded under any of them.
#[test]
fn every_key_under_the_docker_elevation_prefix_is_refused_to_an_agent() {
    let conn = setup();
    for (message_id, key) in [
        ("agent-fixed", "kronn-elevation-docker"),
        ("agent-dashed", "kronn-elevation-docker-check"),
    ] {
        let refused = agent_card_under(&conn, message_id, key).expect("refused");
        assert!(
            refused.starts_with(&format!(
                "key `{key}` is reserved to the Docker elevation cards Kronn posts"
            )) && refused.contains("under any key")
                && !refused.contains("new key"),
            "{refused}"
        );
    }
    assert!(elevation_cards(&conn).is_empty());

    let exec = launch_in_project(&conn, "task-2085-fixed", 2087);
    let mut kronn = agent_card_message(
        &format!("{DOCKER_ELEVATION_PREFIX}{exec}:0"),
        "kronn-elevation-docker",
    );
    kronn.role = MessageRole::User;
    kronn.agent_type = None;
    kronn.kind = crate::models::MessageKind::Orchestration;
    crate::db::discussions::insert_message(&conn, DISC, &kronn).unwrap();
    let invalid = crate::db::discussion_questions::ingest_message_questions(
        &conn,
        DISC,
        &kronn.id,
        &kronn.content,
    )
    .unwrap();
    assert!(invalid.is_empty(), "{invalid:?}");
    let cards = elevation_cards(&conn);
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(
        (cards[0].1.as_str(), cards[0].2.as_str()),
        (kronn.id.as_str(), "kronn-elevation-docker")
    );
}

/// KT-2087 — the message and its card are written together: a card that
/// cannot be recorded leaves no message behind.
#[test]
fn an_elevation_card_that_cannot_be_recorded_leaves_no_message() {
    let conn = setup();
    let exec = launch_in_project(&conn, "task-1433", 1433);
    conn.execute_batch("ALTER TABLE discussion_questions RENAME TO discussion_questions_away")
        .unwrap();

    assert!(post_docker_elevation_card(&conn, &exec).is_err());
    let posted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE id LIKE 'orch-elevation:docker:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(posted, 0);
}
