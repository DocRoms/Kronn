use super::*;
use crate::models::{DiscussionMessage, MessageChannel, MessageRole};
use chrono::Utc;

const ROOM: &str = "d-room";
const CHILD: &str = "d-child";

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    for id in [ROOM, CHILD] {
        conn.execute(
            "INSERT INTO discussions (id,title,agent,created_at,updated_at) \
             VALUES (?1,?1,'Codex','now','now')",
            [id],
        )
        .unwrap();
    }
    conn
}

/// A CLI session parked in `room`, as Kronn writes it at offer acceptance.
fn session(conn: &Connection, room: &str, session_id: &str) -> i64 {
    conn.execute(
        "INSERT INTO discussion_sessions (disc_id, agent_type, session_id, role, status, joined_at) \
         VALUES (?1,'ClaudeCode',?2,'peer','active','now')",
        rusqlite::params![room, session_id],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// Make `CHILD` a genuine execution room, the way a delegation does.
///
/// The real parent rows are inserted rather than switching foreign keys off:
/// a fixture the schema would reject proves nothing about the schema.
fn execution_room(conn: &Connection) {
    conn.execute(
        "INSERT INTO orchestration_runs (id, kind, discussion_id, created_at, updated_at) \
         VALUES ('run','single_task',?1,'now','now')",
        [ROOM],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
         VALUES ('task', 619, 'Important messages', 'now', 'now')",
        [],
    )
    .unwrap();
    let worker = session(conn, CHILD, "execution-worker");
    conn.execute(
        "INSERT INTO task_executions (id, orchestration_run_id, task_id, parent_discussion_id, \
             sub_discussion_id, status, worker_target_kind, worker_agent_type, \
             worker_cli_session_id, created_at, updated_at) \
         VALUES ('x','run','task',?1,?2,'Working','cli','ClaudeCode',?3,'now','now')",
        rusqlite::params![ROOM, CHILD, worker],
    )
    .unwrap();
}

fn spec_json(dedup_key: &str) -> String {
    serde_json::json!({
        "version": 1,
        "category": "decision",
        "dedup_key": dedup_key,
        "title": "Cible de la 0.13.0",
        "highlight": "La release part sans KT-610.",
        "impact": "Les captures d'écran arrivent en 0.13.1.",
        "action_required": { "required": false }
    })
    .to_string()
}

fn fenced(body: &str) -> String {
    format!("Décision prise.\n\n```kronn-important\n{body}\n```\n")
}

fn message(id: &str, content: String, role: MessageRole) -> DiscussionMessage {
    DiscussionMessage {
        id: id.into(),
        role,
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

/// Insert a message the way an append does, then ingest its fence.
fn append(
    conn: &Connection,
    room: &str,
    id: &str,
    content: String,
    publisher: &ImportantPublisher,
) -> ImportantIngest {
    append_as(conn, room, id, content, publisher, MessageRole::Agent)
}

fn append_as(
    conn: &Connection,
    room: &str,
    id: &str,
    content: String,
    publisher: &ImportantPublisher,
    role: MessageRole,
) -> ImportantIngest {
    let msg = message(id, content, role);
    crate::db::discussions::insert_message(conn, room, &msg).unwrap();
    ingest_message_important(
        conn,
        room,
        id,
        &msg.content,
        publisher,
        &msg.timestamp.to_rfc3339(),
    )
    .unwrap()
}

fn orchestrator() -> ImportantPublisher {
    ImportantPublisher::Orchestrator("Codex".into())
}

#[test]
fn an_orchestrator_fence_becomes_one_persisted_card() {
    let conn = database();
    let ingest = append(&conn, ROOM, "m1", fenced(&spec_json("kt-619.ship")), &orchestrator());

    assert_eq!(ingest.published, 1);
    let list = list(&conn, ROOM, None).unwrap();
    assert_eq!(list.total, 1);
    let card = &list.items[0];
    assert_eq!(card.category, ImportantCategory::Decision);
    assert_eq!(card.highlight, "La release part sans KT-610.");
    assert_eq!(card.author_kind, ImportantAuthorKind::Orchestrator);
    assert_eq!(card.author_label, "Codex");
    // Server-owned, never taken from the payload.
    assert_eq!(card.schema_version, IMPORTANT_SCHEMA_VERSION);
    assert!(!card.created_at.is_empty());
    assert_eq!(count(&conn, ROOM).unwrap(), 1);
}

#[test]
fn a_worker_is_refused_and_told_so() {
    let conn = database();
    execution_room(&conn);
    let worker = session(&conn, CHILD, "worker-1");

    let publisher = publisher_for_author(&conn, Some(worker), false, "ClaudeCode").unwrap();
    assert_eq!(publisher, ImportantPublisher::Worker);

    let ingest = append(&conn, ROOM, "m1", fenced(&spec_json("kt-619.ship")), &publisher);
    assert_eq!(ingest.refused_worker, 1);
    assert_eq!(ingest.published, 0);
    // The refusal is visible, not a silent drop.
    assert!(!ingest.is_empty());
    assert_eq!(count(&conn, ROOM).unwrap(), 0);
}

#[test]
fn a_worker_targeting_the_parent_room_is_still_refused() {
    let conn = database();
    execution_room(&conn);
    let worker = session(&conn, CHILD, "worker-1");

    // The route bypass: name the principal's room instead of its own. Authority
    // is read from where the session sits, so the target changes nothing.
    let publisher = publisher_for_author(&conn, Some(worker), false, "ClaudeCode").unwrap();
    assert_eq!(publisher, ImportantPublisher::Worker);
    let ingest = append(&conn, ROOM, "m1", fenced(&spec_json("k")), &publisher);
    assert_eq!(ingest.refused_worker, 1);
    assert_eq!(count(&conn, ROOM).unwrap(), 0);
}

#[test]
fn a_worker_claiming_the_human_role_is_still_refused() {
    let conn = database();
    execution_room(&conn);
    let worker = session(&conn, CHILD, "worker-1");

    // The payload bypass: label the turn `User` to be taken for the human.
    // The lineage check runs first and does not consult the claimed role.
    let publisher = publisher_for_author(&conn, Some(worker), true, "Human").unwrap();
    assert_eq!(publisher, ImportantPublisher::Worker);
    let ingest = append_as(
        &conn,
        ROOM,
        "m1",
        fenced(&spec_json("k")),
        &publisher,
        MessageRole::User,
    );
    assert_eq!(ingest.refused_worker, 1);
    assert_eq!(count(&conn, ROOM).unwrap(), 0);
}

#[test]
fn the_human_publishes_even_inside_an_execution_room() {
    let conn = database();
    execution_room(&conn);
    // Not a worker: no CLI session of its own parked in the child.
    let publisher = publisher_for_author(&conn, None, true, "Romu").unwrap();
    assert_eq!(publisher, ImportantPublisher::Human("Romu".into()));

    let ingest = append_as(
        &conn,
        CHILD,
        "m1",
        fenced(&spec_json("k")),
        &publisher,
        MessageRole::User,
    );
    assert_eq!(ingest.published, 1);
    assert_eq!(
        list(&conn, CHILD, None).unwrap().items[0].author_kind,
        ImportantAuthorKind::Human
    );
}

#[test]
fn a_replayed_event_collapses_onto_its_dedup_key() {
    let conn = database();
    let body = fenced(&spec_json("kt-619.ship"));
    assert_eq!(append(&conn, ROOM, "m1", body.clone(), &orchestrator()).published, 1);

    // Same fact, different message: a restart replaying the steering event.
    let again = append(&conn, ROOM, "m2", body, &orchestrator());
    assert_eq!(again.published, 0);
    assert_eq!(again.deduplicated, 1);
    assert_eq!(count(&conn, ROOM).unwrap(), 1);
}

#[test]
fn a_card_cannot_be_attached_to_an_earlier_message() {
    let conn = database();
    let first = message("m1", "ordinary".into(), MessageRole::Agent);
    crate::db::discussions::insert_message(&conn, ROOM, &first).unwrap();
    let second = message("m2", "newer".into(), MessageRole::Agent);
    crate::db::discussions::insert_message(&conn, ROOM, &second).unwrap();

    let spec = parse_spec(&spec_json("k")).unwrap();
    // m1 is now an ordinary, older message. Converting it must fail.
    let refused = publish(
        &conn,
        "important:m1",
        ROOM,
        "m1",
        &spec,
        ImportantAuthorKind::Orchestrator,
        "Codex",
        None,
        "now",
    );
    assert!(refused.is_err());
    assert_eq!(count(&conn, ROOM).unwrap(), 0);
}

#[test]
fn only_the_first_fence_in_a_message_publishes() {
    let conn = database();
    let content = format!(
        "{}\n{}",
        fenced(&spec_json("first")),
        fenced(&spec_json("second"))
    );
    let ingest = append(&conn, ROOM, "m1", content, &orchestrator());
    assert_eq!(ingest.published, 1);
    assert_eq!(ingest.refused_extra, 1);
    assert_eq!(list(&conn, ROOM, None).unwrap().items[0].dedup_key, "first");
}

#[test]
fn a_payload_claiming_server_owned_fields_is_refused_whole() {
    // `deny_unknown_fields` is what keeps author/created_at/schema_version out
    // of the model's reach: one unknown key fails the whole parse.
    for injected in ["author", "created_at", "schema_version", "author_kind"] {
        let mut value: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
        value[injected] = serde_json::json!("forged");
        assert!(
            parse_spec(&value.to_string()).is_none(),
            "{injected} must not be accepted"
        );
    }
}

#[test]
fn version_must_be_the_number_one() {
    let mut value: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
    value["version"] = serde_json::json!("1");
    assert!(parse_spec(&value.to_string()).is_none());
    value["version"] = serde_json::json!(2);
    assert!(parse_spec(&value.to_string()).is_none());
}

#[test]
fn an_explicit_no_action_carries_no_action_fields() {
    let mut value: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
    // required:false with a stray owner means the publisher meant something it
    // did not say; refuse rather than render half an instruction.
    value["action_required"] = serde_json::json!({ "required": false, "owner": "Romu" });
    assert!(parse_spec(&value.to_string()).is_none());

    value["action_required"] = serde_json::json!({ "required": true, "action": "Trancher" });
    assert!(parse_spec(&value.to_string()).is_none(), "owner is required");

    value["action_required"] =
        serde_json::json!({ "required": true, "action": "Trancher", "owner": "Romu" });
    assert!(parse_spec(&value.to_string()).is_some());
}

#[test]
fn empty_blank_and_oversized_fields_are_refused() {
    let mut value: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
    value["highlight"] = serde_json::json!("   ");
    assert!(parse_spec(&value.to_string()).is_none(), "blank highlight");

    value["highlight"] = serde_json::json!("x".repeat(501));
    assert!(parse_spec(&value.to_string()).is_none(), "highlight ceiling");

    let mut over = serde_json::from_str::<serde_json::Value>(&spec_json("k")).unwrap();
    over["impact"] = serde_json::json!("x".repeat(30_000));
    assert!(parse_spec(&over.to_string()).is_none(), "24 000-byte cap");
}

#[test]
fn unicode_counts_characters_for_bounds_and_bytes_for_the_cap() {
    let mut value: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
    // 500 characters is 500 characters, whatever they weigh in UTF-8.
    value["highlight"] = serde_json::json!("é".repeat(500));
    assert!(parse_spec(&value.to_string()).is_some());
    value["highlight"] = serde_json::json!("é".repeat(501));
    assert!(parse_spec(&value.to_string()).is_none());

    let mut emoji: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
    emoji["title"] = serde_json::json!("🚢 Release");
    assert!(parse_spec(&emoji.to_string()).is_some());
}

#[test]
fn a_nested_example_fence_never_publishes() {
    let conn = database();
    let content = format!(
        "Voici comment on écrit une carte :\n\n````markdown\n```kronn-important\n{}\n```\n````\n",
        spec_json("documented-example")
    );
    let ingest = append(&conn, ROOM, "m1", content, &orchestrator());
    assert!(ingest.is_empty(), "an example must stay an example");
    assert_eq!(count(&conn, ROOM).unwrap(), 0);
}

#[test]
fn the_filter_and_the_transcript_agree_on_order() {
    let conn = database();
    for (index, key) in ["a", "b", "c"].iter().enumerate() {
        let mut value: serde_json::Value = serde_json::from_str(&spec_json(key)).unwrap();
        if index == 1 {
            value["category"] = serde_json::json!("blocking_alert");
        }
        append(
            &conn,
            ROOM,
            &format!("m{index}"),
            fenced(&value.to_string()),
            &orchestrator(),
        );
    }
    let all = list(&conn, ROOM, None).unwrap();
    assert_eq!(all.total, 3);
    // Navigation reads this order, so it must be the transcript's.
    let orders: Vec<i64> = all.items.iter().map(|item| item.sort_order).collect();
    let mut sorted = orders.clone();
    sorted.sort_unstable();
    assert_eq!(orders, sorted);

    let alerts = list(&conn, ROOM, Some(ImportantCategory::BlockingAlert)).unwrap();
    assert_eq!(alerts.total, 1);
    assert_eq!(alerts.items[0].dedup_key, "b");
    // The counter is the unfiltered total, so the chip does not lie when a
    // filter is active.
    assert_eq!(count(&conn, ROOM).unwrap(), 3);
}

#[test]
fn cards_survive_a_restart_with_their_source_link() {
    let conn = database();
    append(&conn, ROOM, "m1", fenced(&spec_json("kt-619.ship")), &orchestrator());
    // Re-reading through a fresh statement is what a restart does: the row is
    // the truth, not any in-memory projection.
    let card = get_by_message(&conn, "m1").unwrap().expect("card persisted");
    assert_eq!(card.message_id, "m1");
    assert_eq!(card.dedup_key, "kt-619.ship");
    assert_eq!(card.impact, "Les captures d'écran arrivent en 0.13.1.");
    assert!(!card.action_required.required);
    assert_eq!(get_by_message(&conn, "absent").unwrap(), None);
}

#[test]
fn every_category_round_trips_through_the_database_check() {
    let conn = database();
    let categories = [
        "decision",
        "scope_change",
        "dod_waiver",
        "blocking_alert",
        "human_action_required",
        "accepted_delivery",
    ];
    for (index, name) in categories.iter().enumerate() {
        let mut value: serde_json::Value = serde_json::from_str(&spec_json(name)).unwrap();
        value["category"] = serde_json::json!(name);
        let ingest = append(
            &conn,
            ROOM,
            &format!("m{index}"),
            fenced(&value.to_string()),
            &orchestrator(),
        );
        assert_eq!(ingest.published, 1, "{name} must be a valid category");
    }
    assert_eq!(count(&conn, ROOM).unwrap(), categories.len() as u32);

    let mut invented: serde_json::Value = serde_json::from_str(&spec_json("k")).unwrap();
    invented["category"] = serde_json::json!("very_important");
    assert!(parse_spec(&invented.to_string()).is_none());
}
