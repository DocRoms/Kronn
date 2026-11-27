use super::*;
use crate::models::{DiscussionMessage, MessageChannel, MessageRole, MessageTargetKind};

pub(super) fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute("INSERT INTO discussions (id,title,agent,created_at,updated_at) VALUES ('d','D','Codex','now','now')", []).unwrap();
    conn
}

pub(super) fn payload() -> serde_json::Value {
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
        session_continuity: None,
        kind: Default::default(),
        model: None,
        lint_report: None,
    }
}

pub(super) fn insert(conn: &Connection, id: &str, value: serde_json::Value) {
    crate::db::discussions::insert_message(
        conn,
        "d",
        &message(id, format!("```kronn-question\n{value}\n```")),
    )
    .unwrap();
}

/// Every text of every row of `table`, joined.
fn whole_table(conn: &Connection, table: &str) -> String {
    let mut statement = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|at| {
                    Ok(match row.get_ref(at)? {
                        rusqlite::types::ValueRef::Text(text) => {
                            String::from_utf8_lossy(text).into_owned()
                        }
                        other => format!("{other:?}"),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
                .map(|cells| cells.join("|"))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .join("\n")
}

/// KT-1656 — the message row is masked once and every consumer of the content
/// reads the masked text: a `kronn-*` fence that carries a credential is still
/// ingested as a card, with the credential masked in the card's own tables.
#[test]
fn a_fence_carrying_a_token_is_still_ingested_with_the_token_masked() {
    const TOKEN: &str = "ghp_R7kQ2mZx9LpV4nB8cT1yW6dH3sF5jG0aNeUo";
    let conn = database();
    let question = serde_json::json!({"version":1,"key":"rotate","question":format!("Rotate {TOKEN} now?"),
        "options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]});
    let action = serde_json::json!({"kind":"quick_prompt","target_id":"missing-target",
        "values":[{"name":"token","value":TOKEN,"provenance":"agent_suggestion","suggested_by":"codex"}]});
    crate::db::discussions::insert_message(
        &conn,
        "d",
        &message(
            "m-token",
            format!(
                "I found {TOKEN} in the file.\n```kronn-question\n{question}\n```\n```kronn-action\n{action}\n```"
            ),
        ),
    )
    .unwrap();

    let stored: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id = 'm-token'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!stored.contains(TOKEN), "{stored}");
    assert!(stored.contains("kronn-question") && stored.contains("kronn-action"));

    let questions = whole_table(&conn, "discussion_questions");
    assert!(
        questions.contains("rotate"),
        "the question card is still ingested: {questions}"
    );
    assert!(questions.contains("REDACTED"), "{questions}");
    let actions = whole_table(&conn, "discussion_actions");
    assert!(
        actions.contains("action:m-token:0"),
        "the action card is still ingested: {actions}"
    );
    for table in [
        "discussion_questions",
        "discussion_actions",
        "planning_proposal_items",
        "messages",
    ] {
        let text = whole_table(&conn, table);
        assert!(!text.contains(TOKEN), "{table} holds the token: {text}");
    }
}

pub(super) fn finished_step_question(value: serde_json::Value) -> Connection {
    finished_step_cards(&[value])
}

/// One message of a workflow step carrying several cards, the step then ended.
pub(super) fn finished_step_cards(values: &[serde_json::Value]) -> Connection {
    let conn = database();
    let session_pk = seed_step_cards(&conn, values);
    end_step(&conn, session_pk);
    conn
}

/// One message of a workflow step carrying several cards, the step still running.
pub(super) fn active_step_cards(values: &[serde_json::Value]) -> Connection {
    let conn = database();
    seed_step_cards(&conn, values);
    conn
}

/// Ends the step [`seed_step_cards`] started: its agent will not read again.
pub(crate) fn end_step(conn: &Connection, session_pk: i64) {
    crate::db::workflow_step_rooms::finish_activity(conn, "source-run", "orchestrate").unwrap();
    crate::db::workflow_step_rooms::revoke(conn, &[session_pk]).unwrap();
    conn.execute(
        "UPDATE workflow_runs SET status='Success', finished_at='now' WHERE id='source-run'",
        [],
    )
    .unwrap();
}

/// In discussion `d`, message `step-question` of a running workflow step
/// carrying `values` as cards. Returns the step's session.
pub(crate) fn seed_step_cards(conn: &Connection, values: &[serde_json::Value]) -> i64 {
    conn.execute(
        "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
         VALUES ('source-workflow', 'Implementation', '{}', '[]', 'now', 'now')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, status, started_at)
         VALUES ('source-run', 'source-workflow', 'Running', 'now')",
        [],
    )
    .unwrap();
    crate::db::workflow_step_rooms::begin_activity(
        conn,
        "source-run",
        "orchestrate",
        "Orchestrate",
        "d",
        "Codex",
    )
    .unwrap();
    let session = crate::db::workflow_step_rooms::join(
        conn,
        "source-run",
        "orchestrate",
        "d",
        "Codex",
        "step-session",
    )
    .unwrap();
    crate::db::discussions::insert_cli_message_with_targets_and_dispatches(
        conn,
        "d",
        &message(
            "step-question",
            values
                .iter()
                .map(|value| format!("```kronn-question\n{value}\n```"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        &[],
        &[],
        session.session_pk,
    )
    .unwrap();
    session.session_pk
}

pub(super) fn request() -> AnswerDiscussionQuestionRequest {
    AnswerDiscussionQuestionRequest {
        selected_option_ids: vec!["retry".into()],
        item_answers: vec![],
        text: None,
        idempotency_key: "answer-1".into(),
    }
}

fn td_fixture(status: &str, severity: &str) -> String {
    format!(
        "---\nname: td-test\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-09-01\n      status: {status}\n      reviewer: Full audit\n---\n\n# TD test\n\n- **Severity**: {severity}\n- **Status**: {status}\n"
    )
}

fn audit_database() -> (Connection, tempfile::TempDir) {
    let conn = database();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("docs/tech-debt")).unwrap();
    std::fs::write(project.path().join("docs/decisions.md"), "# Decisions\n").unwrap();
    let path = project.path().to_string_lossy().into_owned();
    conn.execute(
        "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('p','P',?1,'now','now')",
        [path],
    )
    .unwrap();
    conn.execute("UPDATE discussions SET project_id='p' WHERE id='d'", [])
        .unwrap();
    crate::db::audit_runs::insert_running(&conn, "run", "p", "Full", "Codex", chrono::Utc::now())
        .unwrap();
    crate::db::audit_runs::complete(
        &conn,
        "run",
        chrono::Utc::now(),
        "Completed",
        1,
        0,
        1,
        0,
        0,
        2,
        0,
        80,
        None,
        None,
    )
    .unwrap();
    crate::db::audit_runs::set_validation_discussion(&conn, "run", "d").unwrap();
    (conn, project)
}

#[test]
fn audit_card_answers_update_td_files_and_record_accepted_decisions() {
    let (conn, project) = audit_database();
    let td_dir = project.path().join("docs/tech-debt");
    std::fs::write(
        td_dir.join("TD-20260928-critical.md"),
        td_fixture("Verified in source", "Critical"),
    )
    .unwrap();
    std::fs::write(
        td_dir.join("TD-20260928-medium.md"),
        td_fixture("Inferred", "Medium"),
    )
    .unwrap();
    std::fs::write(
        td_dir.join("TD-20260928-low.md"),
        td_fixture("Inferred", "Low"),
    )
    .unwrap();
    // An accepted ADR an agent wrote after the run: a card does not trust it.
    std::fs::write(
        project.path().join("docs/decisions.md"),
        "# Decisions\n\n## ADR-0001: Medium is fine\n\n- **Status**: accepted\n- **Covers**: medium\n",
    )
    .unwrap();
    let options = serde_json::json!([
        {"id":"confirm","label":"Confirm"},
        {"id":"reject","label":"Reject"},
        {"id":"accept_decision","label":"Accepted decision"},
        {"id":"defer","label":"Defer"}
    ]);
    insert(
        &conn,
        "audit-single",
        serde_json::json!({
            "version": 1,
            "key": "audit-critical",
            "question": "Critical TD outcome?",
            "options": options.clone(),
            "task_ref": "audit-td:TD-20260928-critical"
        }),
    );
    insert(
        &conn,
        "audit-batch",
        serde_json::json!({
            "version": 1,
            "key": "audit-rest",
            "question": "Remaining TD outcomes?",
            "items": [
                {"id":"TD-20260928-medium","label":"Medium TD"},
                {"id":"TD-20260928-low","label":"Low TD"}
            ],
            "options": options,
            "task_ref": "audit-td-batch"
        }),
    );

    let single = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec!["accept_decision".into()],
        item_answers: vec![],
        text: Some("The operational trade-off is intentional.".into()),
        idempotency_key: "audit-single-answer".into(),
    };
    answer(
        &conn,
        "d",
        "question:audit-single:0",
        &single,
        "Human",
        None,
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "the agent resumes only after the last audit card"
    );

    let batch = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec![],
        item_answers: vec![
            DiscussionQuestionItemAnswer {
                item_id: "TD-20260928-medium".into(),
                selected_option_id: "confirm".into(),
            },
            DiscussionQuestionItemAnswer {
                item_id: "TD-20260928-low".into(),
                selected_option_id: "defer".into(),
            },
        ],
        text: None,
        idempotency_key: "audit-batch-answer".into(),
    };
    answer(&conn, "d", "question:audit-batch:0", &batch, "Human", None).unwrap();

    let critical = std::fs::read_to_string(td_dir.join("TD-20260928-critical.md")).unwrap();
    let medium = std::fs::read_to_string(td_dir.join("TD-20260928-medium.md")).unwrap();
    let low = std::fs::read_to_string(td_dir.join("TD-20260928-low.md")).unwrap();
    let decisions = std::fs::read_to_string(project.path().join("docs/decisions.md")).unwrap();
    assert!(
        critical.contains("- **Status**: Accepted decision"),
        "{critical}"
    );
    assert!(critical.contains("status: Accepted decision"));
    assert!(medium.contains("- **Status**: Confirmed by user"));
    assert!(low.contains("- **Status**: Deferred"));
    assert!(decisions.contains("TD-20260928-critical"));
    assert!(decisions.contains("The operational trade-off is intentional."));
    // The card's own ADR settles the TD it accepted, and only that one.
    assert!(
        critical.contains("- **Decision record**: ADR-0002 — Accept `TD-20260928-critical`"),
        "{critical}"
    );
    assert!(!medium.contains("Decision record"), "{medium}");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "the final card resumes the validation exactly once"
    );
}

/// A sheet whose current status lives only in `metadata.audit_history`.
fn history_only_sheet(status: &str) -> String {
    format!(
        "---\nname: td-history\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-09-01\n      status: {status}\n      reviewer: Full audit\n---\n\n# TD history only\n\n- **Severity**: High\n"
    )
}

/// The `audit_history` statuses of a sheet, parsed as strict YAML.
fn strict_statuses(sheet: &str) -> Vec<String> {
    let rest = sheet.strip_prefix("---\n").expect("front matter");
    let end = rest.find("\n---").expect("front matter end");
    let front: serde_yaml::Value = serde_yaml::from_str(&rest[..end])
        .unwrap_or_else(|error| panic!("strict YAML parse failed: {error}\n{sheet}"));
    front["metadata"]["audit_history"]
        .as_sequence()
        .expect("audit_history list")
        .iter()
        .map(|entry| entry["status"].as_str().unwrap().to_string())
        .collect()
}

fn insert_audit_card(conn: &Connection, id: &str, key: &str, td_ids: &[&str]) {
    let options = serde_json::json!([
        {"id":"confirm","label":"Confirm"},
        {"id":"reject","label":"Reject"},
        {"id":"accept_decision","label":"Accepted decision"},
        {"id":"defer","label":"Defer"}
    ]);
    let value = if let [single] = td_ids {
        serde_json::json!({
            "version": 1, "key": key, "question": "TD outcome?", "options": options,
            "task_ref": format!("audit-td:{single}")
        })
    } else {
        let items: Vec<_> = td_ids
            .iter()
            .map(|td| serde_json::json!({"id": td, "label": td}))
            .collect();
        serde_json::json!({
            "version": 1, "key": key, "question": "TD outcomes?", "options": options,
            "items": items, "task_ref": "audit-td-batch"
        })
    };
    insert(conn, id, value);
}

fn batch_answer(items: &[(&str, &str)], key: &str) -> AnswerDiscussionQuestionRequest {
    AnswerDiscussionQuestionRequest {
        selected_option_ids: vec![],
        item_answers: items
            .iter()
            .map(|(item, option)| DiscussionQuestionItemAnswer {
                item_id: (*item).into(),
                selected_option_id: (*option).into(),
            })
            .collect(),
        text: None,
        idempotency_key: key.into(),
    }
}

fn answer_message(conn: &Connection, question: &str) -> String {
    conn.query_row(
        "SELECT content FROM messages WHERE id=?1",
        [format!("question-answer:{question}")],
        |row| row.get(0),
    )
    .unwrap()
}

const INDEX_ONLY_ROWS: &str = "# Index\n\n## Current list\n\n| ID | Problem | Area | Severity | Status |\n|----|---------|------|----------|--------|\n| TD-20261016-legacy | Legacy one-liner | API | High | Open |\n";

#[test]
fn single_card_resolves_a_history_only_sheet_and_an_index_only_td() {
    let (conn, project) = audit_database();
    let docs = project.path().join("docs");
    std::fs::write(
        docs.join("tech-debt/TD-20261016-history.md"),
        history_only_sheet("Verified in source"),
    )
    .unwrap();
    std::fs::write(docs.join("inconsistencies-tech-debt.md"), INDEX_ONLY_ROWS).unwrap();
    insert_audit_card(
        &conn,
        "single-history",
        "k-history",
        &["TD-20261016-history"],
    );
    insert_audit_card(&conn, "single-legacy", "k-legacy", &["TD-20261016-legacy"]);

    let request = |key: &str| AnswerDiscussionQuestionRequest {
        selected_option_ids: vec!["confirm".into()],
        item_answers: vec![],
        text: None,
        idempotency_key: key.into(),
    };
    answer(
        &conn,
        "d",
        "question:single-history:0",
        &request("a"),
        "Human",
        None,
    )
    .unwrap();
    answer(
        &conn,
        "d",
        "question:single-legacy:0",
        &request("b"),
        "Human",
        None,
    )
    .unwrap();

    let history = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-history.md")).unwrap();
    assert!(
        history.contains("- **Status**: Confirmed by user"),
        "{history}"
    );
    assert_eq!(
        strict_statuses(&history),
        ["Verified in source", "Confirmed by user"]
    );
    let legacy = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-legacy.md")).unwrap();
    assert!(
        legacy.contains("- **Status**: Confirmed by user"),
        "{legacy}"
    );
    assert_eq!(strict_statuses(&legacy), ["Confirmed by user"]);
}

#[test]
fn batch_card_resolves_both_shapes_together() {
    let (conn, project) = audit_database();
    let docs = project.path().join("docs");
    std::fs::write(
        docs.join("tech-debt/TD-20261016-history.md"),
        history_only_sheet("Inferred"),
    )
    .unwrap();
    std::fs::write(docs.join("inconsistencies-tech-debt.md"), INDEX_ONLY_ROWS).unwrap();
    insert_audit_card(
        &conn,
        "batch-shapes",
        "k-shapes",
        &["TD-20261016-history", "TD-20261016-legacy"],
    );
    answer(
        &conn,
        "d",
        "question:batch-shapes:0",
        &batch_answer(
            &[
                ("TD-20261016-history", "defer"),
                ("TD-20261016-legacy", "reject"),
            ],
            "shapes",
        ),
        "Human",
        None,
    )
    .unwrap();
    let history = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-history.md")).unwrap();
    assert!(history.contains("- **Status**: Deferred"), "{history}");
    assert_eq!(strict_statuses(&history), ["Inferred", "Deferred"]);
    let legacy = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-legacy.md")).unwrap();
    assert_eq!(strict_statuses(&legacy), ["Rejected"]);
    assert!(
        !answer_message(&conn, "question:batch-shapes:0").contains("could not record"),
        "a fully recorded card shows no error"
    );
}

#[test]
fn one_bad_td_never_blocks_the_others_of_a_batch_and_is_named() {
    let (conn, project) = audit_database();
    let docs = project.path().join("docs");
    let good = docs.join("tech-debt/TD-20261016-good.md");
    std::fs::write(&good, td_fixture("Inferred", "Low")).unwrap();
    std::fs::write(
        docs.join("tech-debt/TD-20261016-history.md"),
        history_only_sheet("Verified in source"),
    )
    .unwrap();
    // Front matter that never closes: nothing can be written to it.
    let broken = docs.join("tech-debt/TD-20261016-broken.md");
    std::fs::write(&broken, "---\nname: broken\n\n# TD broken\n").unwrap();
    // Neither a sheet nor an index row.
    insert_audit_card(
        &conn,
        "batch-mixed",
        "k-mixed",
        &[
            "TD-20261016-good",
            "TD-20261016-broken",
            "TD-20261016-ghost",
            "TD-20261016-history",
        ],
    );
    let question = answer(
        &conn,
        "d",
        "question:batch-mixed:0",
        &batch_answer(
            &[
                ("TD-20261016-good", "confirm"),
                ("TD-20261016-broken", "confirm"),
                ("TD-20261016-ghost", "confirm"),
                ("TD-20261016-history", "confirm"),
            ],
            "mixed",
        ),
        "Human",
        None,
    )
    .unwrap();
    // The card is not closed: it keeps only the TDs that were not recorded.
    assert_eq!(question.state, DiscussionQuestionState::Pending);
    assert!(question.answer.is_none());
    let left: Vec<_> = question.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(left, ["TD-20261016-broken", "TD-20261016-ghost"]);
    assert_eq!(question.context, None, "the reasons are not mixed into it");
    let shown = question.td_errors.as_deref().expect("reasons on the card");
    assert!(shown.contains("could not record 2 of 4"), "{shown}");
    assert!(shown.contains("- TD-20261016-broken:"), "{shown}");
    assert!(shown.contains("- TD-20261016-ghost:"), "{shown}");
    assert!(!shown.contains("- TD-20261016-good:"), "{shown}");
    assert!(!shown.contains("- TD-20261016-history:"), "{shown}");
    let stored = list(&conn, "d").unwrap();
    assert_eq!(stored.pending_count, 1);
    assert_eq!(
        stored.questions[0].items, question.items,
        "durable, not just returned"
    );
    assert_eq!(dispatch_count(&conn), 0, "no resume while a TD is pending");
    assert!(
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE id='question-answer:question:batch-mixed:0'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap()
            == 0,
        "the card has no final answer yet"
    );

    let good_sheet = std::fs::read_to_string(&good).unwrap();
    assert_eq!(
        strict_statuses(&good_sheet),
        ["Inferred", "Confirmed by user"]
    );
    let history = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-history.md")).unwrap();
    assert_eq!(
        strict_statuses(&history),
        ["Verified in source", "Confirmed by user"]
    );
    assert_eq!(
        std::fs::read_to_string(&broken).unwrap(),
        "---\nname: broken\n\n# TD broken\n",
        "the sheet that cannot be written is left as it was"
    );
    assert!(!docs.join("tech-debt/TD-20261016-ghost.md").exists());

    // Repair both, then answer again: only the two left are applied.
    std::fs::write(&broken, td_fixture("Inferred", "Low")).unwrap();
    std::fs::write(docs.join("inconsistencies-tech-debt.md"), GHOST_ROW).unwrap();
    // A stale full answer is refused: the card no longer carries those TDs.
    assert!(answer(
        &conn,
        "d",
        "question:batch-mixed:0",
        &batch_answer(
            &[
                ("TD-20261016-good", "reject"),
                ("TD-20261016-broken", "accept_decision"),
                ("TD-20261016-ghost", "confirm"),
                ("TD-20261016-history", "reject"),
            ],
            "stale",
        ),
        "Human",
        None,
    )
    .is_err());
    let done = answer(
        &conn,
        "d",
        "question:batch-mixed:0",
        &batch_answer(
            &[
                ("TD-20261016-broken", "accept_decision"),
                ("TD-20261016-ghost", "confirm"),
            ],
            "repair",
        ),
        "Human",
        None,
    )
    .unwrap();
    assert_eq!(done.state, DiscussionQuestionState::Answered);
    assert_eq!(done.td_errors, None, "a repaired TD no longer alerts");
    assert_eq!(list(&conn, "d").unwrap().questions[0].td_errors, None);
    assert_eq!(list(&conn, "d").unwrap().pending_count, 0);
    assert_eq!(dispatch_count(&conn), 1, "the validation resumes once");
    assert_eq!(
        strict_statuses(&std::fs::read_to_string(&good).unwrap()),
        ["Inferred", "Confirmed by user"],
        "an applied TD is not written twice"
    );
    assert_eq!(
        strict_statuses(
            &std::fs::read_to_string(docs.join("tech-debt/TD-20261016-history.md")).unwrap()
        ),
        ["Verified in source", "Confirmed by user"]
    );
    let repaired = std::fs::read_to_string(&broken).unwrap();
    // The ADR settle step appends its own history entry after the card's.
    assert_eq!(
        strict_statuses(&repaired)[..2],
        ["Inferred", "Accepted decision"]
    );
    assert_eq!(
        repaired.matches("- **Decision record**: ADR-0001").count(),
        1,
        "{repaired}"
    );
    let ghost = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-ghost.md")).unwrap();
    assert_eq!(strict_statuses(&ghost), ["Confirmed by user"]);
    let decisions = std::fs::read_to_string(docs.join("decisions.md")).unwrap();
    assert_eq!(decisions.matches("## ADR-").count(), 1, "{decisions}");
    // Replaying the final answer is idempotent.
    answer(
        &conn,
        "d",
        "question:batch-mixed:0",
        &batch_answer(
            &[
                ("TD-20261016-broken", "accept_decision"),
                ("TD-20261016-ghost", "confirm"),
            ],
            "repair",
        ),
        "Human",
        None,
    )
    .unwrap();
    assert_eq!(dispatch_count(&conn), 1);
}

/// KT-1348 review — the failure note lives in its own field: the agent's
/// context keeps its ordinary warning and its full length, the reasons survive
/// a context at the limit, and a repair removes the resolved alert while the
/// partial receipt keeps its history.
#[test]
fn the_failure_note_is_its_own_field_and_goes_away_once_repaired() {
    let (conn, project) = audit_database();
    let docs = project.path().join("docs");
    let broken = docs.join("tech-debt/TD-20261016-broken.md");
    std::fs::write(&broken, "---\nname: broken\n\n# TD broken\n").unwrap();
    std::fs::write(
        docs.join("tech-debt/TD-20261016-good.md"),
        td_fixture("Inferred", "Low"),
    )
    .unwrap();
    let prefix = "Audit findings. ⚠ This intentional trade-off must be reviewed with the owner.";
    let full_context = format!("{prefix}{}", "x".repeat(4000 - prefix.chars().count()));
    assert_eq!(full_context.chars().count(), 4000);
    insert(
        &conn,
        "ctx",
        serde_json::json!({
            "version": 1, "key": "k-ctx", "question": "TD outcomes?",
            "context": full_context,
            "options": [{"id":"confirm","label":"Confirm"}],
            "items": [
                {"id": "TD-20261016-good", "label": "good"},
                {"id": "TD-20261016-broken", "label": "broken"}
            ],
            "task_ref": "audit-td-batch"
        }),
    );
    let partial = answer(
        &conn,
        "d",
        "question:ctx:0",
        &batch_answer(
            &[
                ("TD-20261016-good", "confirm"),
                ("TD-20261016-broken", "confirm"),
            ],
            "first",
        ),
        "Human",
        None,
    )
    .unwrap();
    assert_eq!(partial.context.as_deref(), Some(full_context.as_str()));
    let shown = partial.td_errors.as_deref().expect("reasons on the card");
    assert!(shown.contains("- TD-20261016-broken:"), "{shown}");
    assert!(shown.contains("unterminated YAML frontmatter"), "{shown}");

    // A second failure replaces the note instead of stacking on it.
    let again = answer(
        &conn,
        "d",
        "question:ctx:0",
        &batch_answer(&[("TD-20261016-broken", "confirm")], "second"),
        "Human",
        None,
    );
    assert!(again.is_err(), "nothing recordable: refused, card kept");
    let kept = list(&conn, "d").unwrap().questions.remove(0);
    assert_eq!(kept.state, DiscussionQuestionState::Pending);
    assert_eq!(kept.td_errors, partial.td_errors);

    std::fs::write(&broken, td_fixture("Inferred", "Low")).unwrap();
    let done = answer(
        &conn,
        "d",
        "question:ctx:0",
        &batch_answer(&[("TD-20261016-broken", "confirm")], "third"),
        "Human",
        None,
    )
    .unwrap();
    assert_eq!(done.state, DiscussionQuestionState::Answered);
    assert_eq!(done.td_errors, None);
    assert_eq!(done.context.as_deref(), Some(full_context.as_str()));
    let stored = list(&conn, "d").unwrap().questions.remove(0);
    assert_eq!(stored.td_errors, None, "the stored card no longer alerts");
    assert_eq!(stored.context.as_deref(), Some(full_context.as_str()));
    let receipt: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id LIKE 'question-answer:question:ctx:0:partial:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(receipt.contains("- TD-20261016-broken:"), "{receipt}");
}

#[test]
fn an_agent_fence_cannot_write_the_failure_note() {
    let fence = r#"{"version":1,"key":"k","question":"Q?","td_errors":"forged","options":[{"id":"a","label":"A"}]}"#;
    assert!(super::parse_spec(fence).is_err());
}

const GHOST_ROW: &str = "# Index\n\n## Current list\n\n| ID | Problem | Area | Severity | Status |\n|----|---------|------|----------|--------|\n| TD-20261016-ghost | Ghost one-liner | API | High | Open |\n";

pub(super) fn dispatch_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn a_card_where_no_td_can_be_recorded_stays_pending_and_names_each_td() {
    let (conn, _project) = audit_database();
    insert_audit_card(
        &conn,
        "batch-none",
        "k-none",
        &["TD-20261016-ghost-a", "TD-20261016-ghost-b"],
    );
    let error = answer(
        &conn,
        "d",
        "question:batch-none:0",
        &batch_answer(
            &[
                ("TD-20261016-ghost-a", "confirm"),
                ("TD-20261016-ghost-b", "defer"),
            ],
            "none",
        ),
        "Human",
        None,
    )
    .unwrap_err();
    let shown = format!("{error:?}");
    assert!(shown.contains("TD-20261016-ghost-a"), "{shown}");
    assert!(shown.contains("TD-20261016-ghost-b"), "{shown}");
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1);

    // A single card names its TD too.
    insert_audit_card(&conn, "single-ghost", "k-ghost", &["TD-20261016-ghost-c"]);
    let single = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec!["confirm".into()],
        item_answers: vec![],
        text: None,
        idempotency_key: "ghost".into(),
    };
    let error = answer(
        &conn,
        "d",
        "question:single-ghost:0",
        &single,
        "Human",
        None,
    )
    .unwrap_err();
    assert!(
        format!("{error:?}").contains("TD-20261016-ghost-c"),
        "{error:?}"
    );
}

#[test]
fn first_card_in_replacement_validation_relinks_archived_run_and_updates_td() {
    let (conn, project) = audit_database();
    let td_path = project.path().join("docs/tech-debt/TD-20260928-resumed.md");
    std::fs::write(&td_path, td_fixture("Inferred", "High")).unwrap();
    conn.execute("UPDATE discussions SET archived=1 WHERE id='d'", [])
        .unwrap();
    conn.execute(
        "INSERT INTO discussions
         (id, project_id, title, agent, language, created_at, updated_at)
         VALUES ('d-resumed', 'p', 'Validation audit AI', 'Codex', 'fr', 'now', 'now')",
        [],
    )
    .unwrap();
    let options = serde_json::json!([
        {"id":"confirm","label":"Confirm"},
        {"id":"reject","label":"Reject"},
        {"id":"accept_decision","label":"Accepted decision"},
        {"id":"defer","label":"Defer"}
    ]);
    crate::db::discussions::insert_message(
        &conn,
        "d-resumed",
        &message(
            "audit-resumed",
            format!(
                "```kronn-question\n{}\n```",
                serde_json::json!({
                    "version": 1,
                    "key": "audit-resumed-high",
                    "question": "High TD outcome?",
                    "options": options,
                    "task_ref": "audit-td:TD-20260928-resumed"
                })
            ),
        ),
    )
    .unwrap();
    answer(
        &conn,
        "d-resumed",
        "question:audit-resumed:0",
        &AnswerDiscussionQuestionRequest {
            selected_option_ids: vec!["confirm".into()],
            item_answers: vec![],
            text: None,
            idempotency_key: "audit-resumed-answer".into(),
        },
        "Human",
        None,
    )
    .unwrap();

    assert!(std::fs::read_to_string(td_path)
        .unwrap()
        .contains("- **Status**: Confirmed by user"));
    assert_eq!(
        crate::db::audit_runs::get_by_id(&conn, "run")
            .unwrap()
            .unwrap()
            .validation_discussion_id
            .as_deref(),
        Some("d-resumed")
    );
}

#[test]
fn question_spec_is_bounded_and_rejects_unknown_or_duplicate_choices() {
    assert!(parse_spec(&payload().to_string()).is_ok());
    for field in ["version", "key", "question"] {
        let mut value = payload();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            parse_spec(&value.to_string()).is_err(),
            "{field} is required"
        );
    }
    let mut value = payload();
    value["options"][1]["id"] = "retry".into();
    assert!(parse_spec(&value.to_string()).is_err());
    value = payload();
    value["recommended_option_ids"] = serde_json::json!(["missing"]);
    assert!(parse_spec(&value.to_string()).is_err());
    value = payload();
    value["question"] = "".into();
    assert!(parse_spec(&value.to_string()).is_err());
    value = payload();
    value["run_command"] = "anything".into();
    assert!(parse_spec(&value.to_string()).is_err());

    value = payload();
    value["items"] = serde_json::json!([
        {"id":"TD-1","label":"First TD"},
        {"id":"TD-2","label":"Second TD"}
    ]);
    assert!(parse_spec(&value.to_string()).is_ok());
    value["items"] = serde_json::json!([
        {"id":"TD-1","label":"First TD"},
        {"id":"TD-1","label":"Duplicate TD"}
    ]);
    assert!(parse_spec(&value.to_string()).is_err());
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
        item_answers: vec![],
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
            item_answers: vec![],
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

pub(super) fn decline_request() -> DeclineDiscussionQuestionRequest {
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
    let record = q
        .answer
        .as_ref()
        .expect("a refusal is recorded like an answer");
    assert!(record.selected_option_ids.is_empty(), "nothing was chosen");
    assert_eq!(
        record.text.as_deref(),
        Some("La question n'a plus d'objet.")
    );
    assert_eq!(record.author_pseudo, "Romu");
    assert_eq!(list(&conn, "d").unwrap().pending_count, 0);

    // Same delivery path as an answer: one message, one dispatch, right target.
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let targets = crate::db::discussions::list_message_targets(&conn, &record.message_id).unwrap();
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
        reason: Some("Autre raison.".into()),
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

#[test]
fn a_comment_reaches_the_asker_and_leaves_the_question_waiting() {
    let conn = database();
    insert(&conn, "m", payload());
    let remark = CommentDiscussionQuestionRequest {
        idempotency_key: "comment-1".into(),
        text: "Quel est le problème de ce ticket ?".into(),
    };

    let q = comment(&conn, "d", "question:m:0", &remark, "Romu", None).unwrap();
    assert_eq!(
        q.state,
        DiscussionQuestionState::Pending,
        "a comment is not a decision"
    );
    assert!(q.answer.is_none());
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1);

    let message_id = "question-comment:question:m:0:comment-1";
    let (content, reply_to): (String, Option<String>) = conn
        .query_row(
            "SELECT content, reply_to_message_id FROM messages WHERE id=?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        content.contains("Quel est le problème de ce ticket ?"),
        "{content}"
    );
    assert!(
        content.contains("la question reste en attente"),
        "{content}"
    );
    assert_eq!(reply_to.as_deref(), Some("m"));
    // Delivered like an answer: the asker is woken.
    let targets = crate::db::discussions::list_message_targets(&conn, message_id).unwrap();
    assert_eq!(targets[0].agent_type, crate::models::AgentType::Codex);
    let jobs = |conn: &Connection| {
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    assert_eq!(jobs(&conn), 1);

    // A retry with the same key posts nothing more; the question can still be decided.
    comment(&conn, "d", "question:m:0", &remark, "Romu", None).unwrap();
    assert_eq!(jobs(&conn), 1);
    let decided = answer(&conn, "d", "question:m:0", &request(), "Romu", None).unwrap();
    assert_eq!(decided.state, DiscussionQuestionState::Answered);
    // Once decided, a comment is refused rather than reopening it.
    let late = CommentDiscussionQuestionRequest {
        idempotency_key: "comment-2".into(),
        text: "trop tard".into(),
    };
    assert!(matches!(
        comment(&conn, "d", "question:m:0", &late, "Romu", None),
        Err(AnswerError::Conflict)
    ));
}

#[test]
fn a_reply_to_a_finished_step_waits_but_a_comment_wakes_the_room_agent() {
    let warning = "L'étape qui a posé cette question est terminée";

    let answered = finished_step_question(payload());
    let source = list(&answered, "d").unwrap().questions.remove(0);
    let step = source
        .requester_workflow_step
        .expect("the question keeps workflow-step provenance");
    assert_eq!(step.identity.workflow_name, "Implementation");
    assert_eq!(step.identity.step_name, "Orchestrate");
    assert!(!step.active);
    let answer = answer(
        &answered,
        "d",
        "question:step-question:0",
        &request(),
        "Romu",
        None,
    )
    .unwrap();
    let answer_content: String = answered
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [answer.answer.unwrap().message_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(answer_content.contains(warning), "{answer_content}");

    let declined = finished_step_question(payload());
    let refusal = decline(
        &declined,
        "d",
        "question:step-question:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();
    let refusal_content: String = declined
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [refusal.answer.unwrap().message_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(refusal_content.contains(warning), "{refusal_content}");

    let commented = finished_step_question(payload());
    let remark = CommentDiscussionQuestionRequest {
        idempotency_key: "finished-comment".into(),
        text: "Peux-tu préciser ?".into(),
    };
    comment(
        &commented,
        "d",
        "question:step-question:0",
        &remark,
        "Romu",
        None,
    )
    .unwrap();
    let comment_content: String = commented
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            ["question-comment:question:step-question:0:finished-comment"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(comment_content.contains(warning), "{comment_content}");
    assert!(
        comment_content.contains("l'agent de la room répond"),
        "{comment_content}"
    );

    let jobs = |conn: &Connection| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    // A refusal waits for the next run; it wakes nobody.
    assert_eq!(
        jobs(&declined),
        0,
        "a revoked step session has no native dispatch to wake"
    );
    // KT-1401 — answering the last (here: only) pending card wakes the room's agent.
    assert_eq!(
        jobs(&answered),
        1,
        "the room's native agent takes the answer"
    );
    assert!(
        answer_content.contains("l'agent de la room reprend"),
        "{answer_content}"
    );
    // A comment asks for an answer now: the room's own agent takes it.
    assert_eq!(
        jobs(&commented),
        1,
        "the room's native agent answers the comment"
    );
    let (agent, trigger): (String, String) = commented
        .query_row(
            "SELECT agent_override_json, trigger_message_id FROM agent_dispatch_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(agent.contains("Codex"), "{agent}");
    assert_eq!(
        trigger,
        "question-comment:question:step-question:0:finished-comment"
    );
    let question = list(&commented, "d").unwrap().questions.remove(0);
    assert_eq!(
        question.state,
        DiscussionQuestionState::Pending,
        "a comment decides nothing"
    );

    // A room without a native agent keeps the old notice and wakes nobody.
    let quiet = finished_step_question(payload());
    quiet
        .execute("UPDATE discussions SET no_agent=1 WHERE id='d'", [])
        .unwrap();
    comment(
        &quiet,
        "d",
        "question:step-question:0",
        &remark,
        "Romu",
        None,
    )
    .unwrap();
    let quiet_content: String = quiet
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            ["question-comment:question:step-question:0:finished-comment"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        quiet_content.contains("personne ne lira"),
        "{quiet_content}"
    );
    assert_eq!(jobs(&quiet), 0);
}

#[test]
fn a_resolved_step_question_claims_exactly_one_resume_run() {
    let mut resumable = payload();
    resumable["resume"] = serde_json::json!({
        "workflow_id": "resume-workflow",
        "variables": {"ticket": "KT-883"}
    });
    let conn = finished_step_question(resumable);
    conn.execute(
        "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
         VALUES ('resume-workflow', 'Resume', '{}', '[]', 'now', 'now')",
        [],
    )
    .unwrap();
    answer(
        &conn,
        "d",
        "question:step-question:0",
        &request(),
        "Romu",
        None,
    )
    .unwrap();

    let claim = claim_resume(&conn, "d", "question:step-question:0")
        .unwrap()
        .expect("the first resolution caller claims the resume");
    assert_eq!(claim.workflow_id, "resume-workflow");
    assert_eq!(claim.variables["ticket"], "KT-883");
    assert_eq!(claim.source_run_id, "source-run");
    assert!(claim_resume(&conn, "d", "question:step-question:0")
        .unwrap()
        .is_none());
}

pub(super) fn card(key: &str) -> serde_json::Value {
    serde_json::json!({"version":1,"key":key,"question":format!("Question {key} ?"),
        "options":[{"id":"retry","label":"Réarmer"},{"id":"wait","label":"Attendre"}]})
}

fn answer_with(conn: &Connection, index: usize, option: &str) {
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

#[test]
fn answering_the_last_card_of_an_ended_step_wakes_the_room_agent_once_with_every_answer() {
    let conn = finished_step_cards(&[card("first"), card("second"), card("third")]);

    answer_with(&conn, 0, "retry");
    answer_with(&conn, 1, "wait");
    assert_eq!(
        dispatch_count(&conn),
        0,
        "a card still pending: nobody is woken yet"
    );

    answer_with(&conn, 2, "retry");
    assert_eq!(
        dispatch_count(&conn),
        1,
        "one wake for the whole message, not per card"
    );
    let (agent, trigger): (String, String) = conn
        .query_row(
            "SELECT agent_override_json, trigger_message_id FROM agent_dispatch_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(agent.contains("Codex"), "{agent}");
    assert_eq!(trigger, "question-answer:question:step-question:2");
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&trigger],
            |row| row.get(0),
        )
        .unwrap();
    assert!(content.contains("l'agent de la room reprend"), "{content}");
    assert!(content.contains("Question first ? → Réarmer"), "{content}");
    assert!(
        content.contains("Question second ? → Attendre"),
        "{content}"
    );
    let targets = crate::db::discussions::list_message_targets(&conn, &trigger).unwrap();
    assert_eq!(targets[0].agent_type, crate::models::AgentType::Codex);

    // A retry of the last answer does not wake it a second time.
    answer_with(&conn, 2, "retry");
    assert_eq!(dispatch_count(&conn), 1);
}

fn answer_notice(conn: &Connection, index: usize) -> String {
    conn.query_row(
        "SELECT content FROM messages WHERE id=?1",
        [format!("question-answer:question:step-question:{index}")],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn an_earlier_card_of_an_ended_step_says_its_answer_goes_to_the_room_agent_with_the_last_one() {
    let conn = finished_step_cards(&[card("first"), card("second")]);
    answer_with(&conn, 0, "retry");
    assert_eq!(dispatch_count(&conn), 0, "the last answer wakes, not this");
    let early = answer_notice(&conn, 0);
    assert!(
        early.contains(
            "cette réponse sera transmise à l'agent de la room avec la dernière réponse de ce message"
        ),
        "{early}"
    );
    assert!(!early.contains("personne ne lira"), "{early}");

    answer_with(&conn, 1, "wait");
    assert!(answer_notice(&conn, 1).contains("l'agent de la room reprend"));

    // Without a native agent nobody reads it: the old notice stays true.
    let quiet = finished_step_cards(&[card("first"), card("second")]);
    quiet
        .execute("UPDATE discussions SET no_agent=1 WHERE id='d'", [])
        .unwrap();
    answer_with(&quiet, 0, "retry");
    let quiet_notice = answer_notice(&quiet, 0);
    assert!(quiet_notice.contains("personne ne lira"), "{quiet_notice}");
    assert!(!quiet_notice.contains("sera transmise"), "{quiet_notice}");
}

#[test]
fn a_refused_sibling_does_not_block_the_wake_and_a_room_without_agent_wakes_nobody() {
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
    assert_eq!(dispatch_count(&conn), 0);
    answer_with(&conn, 1, "wait");
    assert_eq!(dispatch_count(&conn), 1);
    let content: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id='question-answer:question:step-question:1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(content.contains("Question first ? → refusée"), "{content}");

    let quiet = finished_step_cards(&[card("first")]);
    quiet
        .execute("UPDATE discussions SET no_agent=1 WHERE id='d'", [])
        .unwrap();
    answer_with(&quiet, 0, "retry");
    assert_eq!(dispatch_count(&quiet), 0);
}

fn stored_lint_report(conn: &Connection) -> Option<crate::core::anti_halluc::LintReport> {
    let json: Option<String> = conn
        .query_row(
            "SELECT lint_report FROM messages WHERE discussion_id='d' ORDER BY sort_order LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    json.map(|json| serde_json::from_str(&json).unwrap())
}

#[test]
fn a_label_over_its_bound_is_cut_at_a_word_and_its_full_text_moves_to_the_description() {
    let long_label = format!("{} fin", "mot ".repeat(65));
    assert!(long_label.chars().count() > 250);
    let mut value = payload();
    value["options"][0]["label"] = long_label.clone().into();
    value["options"][0]["description"] = "précision".into();
    let conn = database();
    insert(&conn, "m", value);

    let stored = list(&conn, "d").unwrap().questions.remove(0);
    let option = &stored.options[0];
    assert!(option.label.chars().count() <= 250, "{}", option.label);
    assert!(option.label.ends_with("mot…"), "{}", option.label);
    let description = option.description.as_deref().unwrap();
    assert!(description.starts_with(long_label.trim()), "{description}");
    assert!(description.ends_with("précision"), "{description}");
    assert_eq!(
        stored.options[1].label, "Attendre",
        "other options untouched"
    );
    assert!(
        stored_lint_report(&conn).is_none(),
        "a normalised card is not invalid"
    );
}

#[test]
fn a_question_and_a_context_over_their_bound_are_kept_shortened_not_dropped() {
    let long_question = format!("{} ?", "mot ".repeat(300));
    let mut value = payload();
    value["question"] = long_question.clone().into();
    value["context"] = "c".repeat(4500).into();
    let conn = database();
    insert(&conn, "m", value);

    let stored = list(&conn, "d").unwrap().questions.remove(0);
    assert!(stored.question.chars().count() <= 1000);
    assert!(stored.question.ends_with('…'));
    assert!(stored.context.as_deref().unwrap().chars().count() <= 4000);

    let mut value = payload();
    value["question"] = long_question.clone().into();
    let conn = database();
    insert(&conn, "m", value);
    let stored = list(&conn, "d").unwrap().questions.remove(0);
    assert_eq!(stored.context.as_deref(), Some(long_question.trim()));
}

#[test]
fn a_cut_without_a_word_boundary_is_still_within_the_bound() {
    assert_eq!(cut_at_word("court", 250), "court");
    let cut = cut_at_word(&"é".repeat(300), 250);
    assert_eq!(cut.chars().count(), 250);
    assert!(cut.ends_with('…'));
}

#[test]
fn a_fence_still_invalid_is_reported_on_the_message_never_dropped_silently() {
    let conn = database();
    let mut broken = payload();
    broken["recommended_option_ids"] = serde_json::json!(["missing"]);
    let mut extra = card("extra-field");
    extra["run_command"] = "anything".into();
    let content = format!(
        "```kronn-question\n{}\n```\n\n```kronn-question\n{}\n```\n\n```kronn-question\n{}\n```",
        broken,
        card("fine"),
        extra
    );
    crate::db::discussions::insert_message(&conn, "d", &message("m", content)).unwrap();

    let cards = list(&conn, "d").unwrap().questions;
    assert_eq!(cards.len(), 1, "only the valid fence becomes a card");
    assert_eq!(cards[0].key, "fine");
    let report = stored_lint_report(&conn).expect("the author is told through the lint report");
    assert_eq!(report.invalid_cards.len(), 2, "{:?}", report.invalid_cards);
    assert_eq!(report.invalid_cards[0].key.as_deref(), Some("quota-policy"));
    assert!(
        report.invalid_cards[0]
            .reason
            .contains("recommended_option_ids"),
        "{:?}",
        report.invalid_cards[0]
    );
    assert_eq!(report.invalid_cards[1].key.as_deref(), Some("extra-field"));
    assert!(
        report.invalid_cards[1].reason.contains("run_command"),
        "{:?}",
        report.invalid_cards[1]
    );
    assert!(report.has_signal());
}

#[test]
fn an_invalid_fence_keeps_the_existing_lint_report_of_the_message() {
    let conn = database();
    let mut msg = message("m", "```kronn-question\n{\"version\":2}\n```".into());
    let mut lint = crate::core::anti_halluc::LintReport::empty();
    lint.unsourced_count = 3;
    msg.lint_report = Some(lint);
    crate::db::discussions::insert_message(&conn, "d", &msg).unwrap();
    let report = stored_lint_report(&conn).unwrap();
    assert_eq!(report.unsourced_count, 3);
    assert_eq!(report.invalid_cards.len(), 1);
}

/// KT-2043 — a question's stored resume holds run variables, one of which may
/// be the manual override of a project env credential.
#[test]
fn a_resume_spec_does_not_print_its_variables_debug_without_secrets() {
    let spec = QuestionResumeSpec {
        workflow_id: "wf-1".into(),
        variables: [("token".to_string(), "Fake2043VariableValue".to_string())].into(),
    };
    for printed in [format!("{spec:?}"), format!("{spec:#?}")] {
        assert!(!printed.contains("Fake2043"), "{printed}");
        assert!(printed.contains("wf-1"), "{printed}");
        assert!(printed.contains("\"token\""), "{printed}");
    }
}

// ── KT-2085: a key asked again ──────────────────────────────────────────────

fn lint_report_of(
    conn: &Connection,
    message_id: &str,
) -> Option<crate::core::anti_halluc::LintReport> {
    let json: Option<String> = conn
        .query_row(
            "SELECT lint_report FROM messages WHERE id=?1",
            [message_id],
            |row| row.get(0),
        )
        .unwrap();
    json.map(|json| serde_json::from_str(&json).unwrap())
}

fn stored_keys(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT question_key FROM discussion_questions ORDER BY created_at, id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The real case: the human answered with a question back, the agent answered
/// it and asked again under the same key, in a new message.
#[test]
fn a_key_asked_again_after_its_answer_opens_a_new_round() {
    let conn = database();
    insert(&conn, "m1", payload());
    let question_back = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec![],
        item_answers: vec![],
        text: Some("Que coûte chaque option ?".into()),
        idempotency_key: "answer-1".into(),
    };
    answer(&conn, "d", "question:m1:0", &question_back, "Romu", None).unwrap();

    insert(&conn, "m2", payload());
    assert!(
        lint_report_of(&conn, "m2").is_none(),
        "a new round is not an invalid card"
    );
    let listed = list(&conn, "d").unwrap();
    assert_eq!(listed.pending_count, 1);
    let rounds: Vec<_> = listed
        .questions
        .iter()
        .map(|q| (q.source_message_id.as_str(), q.key.as_str(), q.state))
        .collect();
    assert_eq!(
        rounds,
        [
            ("m1", "quota-policy", DiscussionQuestionState::Answered),
            ("m2", "quota-policy", DiscussionQuestionState::Pending),
        ]
    );

    // The new round is answered like any card, and the next one follows it.
    let second = AnswerDiscussionQuestionRequest {
        idempotency_key: "answer-2".into(),
        ..request()
    };
    let answered = answer(&conn, "d", "question:m2:0", &second, "Romu", None).unwrap();
    assert_eq!(answered.state, DiscussionQuestionState::Answered);
    assert_eq!(answered.answer.unwrap().selected_option_ids, ["retry"]);
    insert(&conn, "m3", payload());
    assert_eq!(
        stored_keys(&conn),
        ["quota-policy", "quota-policy~2", "quota-policy~3"]
    );
    assert_eq!(list(&conn, "d").unwrap().pending_count, 1);
}

#[test]
fn a_key_asked_again_after_a_refusal_opens_a_new_round() {
    let conn = database();
    insert(&conn, "m1", payload());
    decline(
        &conn,
        "d",
        "question:m1:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();

    insert(&conn, "m2", payload());
    assert!(lint_report_of(&conn, "m2").is_none());
    let listed = list(&conn, "d").unwrap();
    assert_eq!(listed.questions.len(), 2);
    assert_eq!(listed.questions[1].source_message_id, "m2");
    assert_eq!(listed.questions[1].state, DiscussionQuestionState::Pending);
    let again = DeclineDiscussionQuestionRequest {
        idempotency_key: "decline-2".into(),
        reason: None,
    };
    let declined = decline(&conn, "d", "question:m2:0", &again, "Romu", None).unwrap();
    assert_eq!(declined.state, DiscussionQuestionState::Declined);
}

#[test]
fn a_key_still_waiting_is_refused_in_another_message_with_its_reason() {
    let conn = database();
    insert(&conn, "m1", payload());
    insert(&conn, "m2", payload());

    let listed = list(&conn, "d").unwrap();
    assert_eq!(listed.questions.len(), 1);
    assert_eq!(listed.questions[0].source_message_id, "m1");
    let report = lint_report_of(&conn, "m2").expect("the refusal rides the message's lint report");
    assert_eq!(report.invalid_cards.len(), 1, "{:?}", report.invalid_cards);
    assert_eq!(report.invalid_cards[0].key.as_deref(), Some("quota-policy"));
    let reason = &report.invalid_cards[0].reason;
    assert!(
        reason.contains("still waiting for an answer")
            && reason.contains("m1")
            && reason.contains("answer, decline or cancel it")
            && reason.contains("new key"),
        "{reason}"
    );
    assert!(lint_report_of(&conn, "m1").is_none());
}

#[test]
fn replaying_a_message_records_nothing_twice_and_reports_nothing() {
    let conn = database();
    insert(&conn, "m1", payload());
    let content = format!("```kronn-question\n{}\n```", payload());
    assert!(ingest_message_questions(&conn, "d", "m1", &content)
        .unwrap()
        .is_empty());
    answer(&conn, "d", "question:m1:0", &request(), "Romu", None).unwrap();
    // Once answered, the replay is still the same card, not a second round.
    assert!(ingest_message_questions(&conn, "d", "m1", &content)
        .unwrap()
        .is_empty());
    assert_eq!(stored_keys(&conn), ["quota-policy"]);
    assert!(lint_report_of(&conn, "m1").is_none());
}

#[test]
fn a_card_never_recorded_is_recorded_when_republished_with_the_same_key() {
    let conn = database();
    let mut invalid = payload();
    invalid["version"] = "1".into();
    insert(&conn, "m1", invalid);
    assert!(lint_report_of(&conn, "m1").is_some());

    insert(&conn, "m2", payload());
    assert!(lint_report_of(&conn, "m2").is_none());
    let listed = list(&conn, "d").unwrap();
    assert_eq!(listed.questions.len(), 1);
    assert_eq!(listed.questions[0].source_message_id, "m2");
    assert_eq!(stored_keys(&conn), ["quota-policy"]);
}

#[test]
fn a_key_carrying_the_round_separator_is_refused_with_a_reason() {
    let conn = database();
    let mut value = payload();
    value["key"] = "quota-policy~2".into();
    insert(&conn, "m1", value);

    assert!(list(&conn, "d").unwrap().questions.is_empty());
    let report = lint_report_of(&conn, "m1").unwrap();
    assert_eq!(
        report.invalid_cards[0].key.as_deref(),
        Some("quota-policy~2")
    );
    assert!(
        report.invalid_cards[0].reason.contains("`key`"),
        "{:?}",
        report.invalid_cards[0]
    );
}

// ── KT-2085 review round: what may open a new round ─────────────────────────

/// A ceiling question Kronn recorded and its card, as Kronn posts them.
fn ceiling_card(conn: &Connection) -> (String, serde_json::Value) {
    use crate::db::discussion_ceiling_requests as ceilings;
    let key = format!("{}abc", ceilings::QUESTION_KEY_PREFIX);
    ceilings::record(
        conn,
        "d",
        &key,
        "Codex",
        &[ceilings::Ceiling::Rounds {
            limit: 150,
            step: 50,
        }],
    )
    .unwrap();
    let card = serde_json::json!({"version":1,"key":key,"question":"Plafond atteint ?",
        "options":[{"id":ceilings::OPTION_GRANT,"label":"Accorder"},{"id":ceilings::OPTION_STOP,"label":"Arrêter"}]});
    (key, card)
}

fn ceiling_decision(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT decision FROM discussion_ceiling_requests WHERE question_key=?1",
        [key],
        |row| row.get(0),
    )
    .unwrap()
}

/// The one reason `message_id` was refused for.
fn refusal_of(conn: &Connection, message_id: &str) -> crate::core::anti_halluc::InvalidCard {
    let report = lint_report_of(conn, message_id).expect("the refusal rides the lint report");
    assert_eq!(report.invalid_cards.len(), 1, "{:?}", report.invalid_cards);
    report.invalid_cards.into_iter().next().unwrap()
}

/// A later round of `key` stored directly, as no ingest records one for a
/// Kronn card any more: only the effects that would read it are under test.
fn force_round(
    conn: &Connection,
    message_id: &str,
    key: &str,
    round: u32,
    card: &serde_json::Value,
) {
    crate::db::discussions::insert_message(conn, "d", &message(message_id, "Encore ?".into()))
        .unwrap();
    conn.execute(
        "INSERT INTO discussion_questions (id, discussion_id, source_message_id, fence_index,
             question_key, payload_json, created_at, updated_at)
         VALUES (?1, 'd', ?2, 0, ?3, ?4, ?5, ?5)",
        params![
            format!("question:{message_id}:0"),
            message_id,
            format!("{key}~{round}"),
            card.to_string(),
            Utc::now().to_rfc3339()
        ],
    )
    .unwrap();
}

/// KT-2085 — ceiling keys are Kronn's: a look-alike card an agent posts is refused at once,
/// while the card Kronn recorded before posting it is recorded.
#[test]
fn a_ceiling_key_kronn_did_not_record_is_refused() {
    use crate::db::discussion_ceiling_requests::QUESTION_KEY_PREFIX;
    let conn = database();
    let forged = format!("{QUESTION_KEY_PREFIX}forged");
    let mut look_alike = payload();
    look_alike["key"] = forged.clone().into();
    insert(&conn, "m1", look_alike);
    let (key, card) = ceiling_card(&conn);
    insert(&conn, "m2", card);

    assert_eq!(stored_keys(&conn), [key]);
    let refusal = refusal_of(&conn, "m1");
    assert_eq!(refusal.key.as_deref(), Some(forged.as_str()));
    assert_eq!(
        refusal.reason,
        format!(
            "keys starting with `{QUESTION_KEY_PREFIX}` are Kronn's ceiling cards; use another key"
        )
    );
    assert!(lint_report_of(&conn, "m2").is_none());
}

/// A ceiling card is Kronn's: asked again under its key, pending or refused,
/// it is turned back with the reason instead of opening a round the human
/// could answer for nothing.
#[test]
fn a_ceiling_card_asked_again_is_refused_with_the_reason() {
    let conn = database();
    let (key, card) = ceiling_card(&conn);
    insert(&conn, "m1", card.clone());
    insert(&conn, "m2", card.clone());
    decline(
        &conn,
        "d",
        "question:m1:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();
    assert_eq!(ceiling_decision(&conn, &key).as_deref(), Some("stop"));
    insert(&conn, "m3", card);

    assert_eq!(stored_keys(&conn), std::slice::from_ref(&key));
    // Never "use a new key": a ceiling card under another key is an answer
    // nothing reads.
    for message_id in ["m2", "m3"] {
        let refusal = refusal_of(&conn, message_id);
        assert_eq!(refusal.key.as_deref(), Some(key.as_str()));
        assert!(
            refusal.reason.contains("ceiling card Kronn posted")
                && refusal.reason.contains("do not repost it under any key")
                && !refusal.reason.contains("new key"),
            "{message_id}: {}",
            refusal.reason
        );
    }
    // While Kronn's card waits, the agent is told so first.
    let waiting = refusal_of(&conn, "m2").reason;
    assert!(
        waiting.starts_with(&format!(
            "key `{key}` is still waiting for an answer in message m1"
        )),
        "{waiting}"
    );
    assert!(!refusal_of(&conn, "m3").reason.contains("still waiting"));
    assert_eq!(ceiling_decision(&conn, &key).as_deref(), Some("stop"));
}

/// Only Kronn's card, round 1, decides a ceiling: a later round under its key,
/// answered or declined, moves no budget.
#[test]
fn a_later_round_of_a_ceiling_key_never_moves_the_budget() {
    use crate::db::discussion_ceiling_requests as ceilings;
    let conn = database();
    let (key, card) = ceiling_card(&conn);
    insert(&conn, "m1", card.clone());
    let prose = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec![],
        item_answers: vec![],
        text: Some("Pourquoi autant d'appels ?".into()),
        idempotency_key: "answer-1".into(),
    };
    answer(&conn, "d", "question:m1:0", &prose, "Romu", None).unwrap();
    assert_eq!(ceiling_decision(&conn, &key), None, "prose decides nothing");

    force_round(&conn, "m2", &key, 2, &card);
    let grant = AnswerDiscussionQuestionRequest {
        selected_option_ids: vec![ceilings::OPTION_GRANT.into()],
        text: None,
        idempotency_key: "answer-2".into(),
        ..prose
    };
    let answered = answer(&conn, "d", "question:m2:0", &grant, "Romu", None).unwrap();
    force_round(&conn, "m3", &key, 3, &card);
    decline(
        &conn,
        "d",
        "question:m3:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();

    assert_eq!(ceiling_decision(&conn, &key), None);
    let notice: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&answered.answer.unwrap().message_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!notice.contains("relevé le plafond"), "{notice}");
}

/// A briefing set is read once, first card per field: a later round would be
/// an answer nobody reads. A new briefing request gets a new set instead.
#[test]
fn a_briefing_card_asked_again_is_refused_with_the_reason() {
    let conn = database();
    let briefing = serde_json::json!({"version":1,"key":"briefing-purpose.s1",
        "question":"What is this project for?","options":[{"id":"not_provided","label":"Not provided"}],
        "task_ref":"audit-briefing:p1:d:s1"});
    insert(&conn, "m1", briefing.clone());
    decline(
        &conn,
        "d",
        "question:m1:0",
        &decline_request(),
        "Romu",
        None,
    )
    .unwrap();
    insert(&conn, "m2", briefing.clone());
    // Nor does an answered card of another key come back as a briefing card.
    insert(&conn, "m3", payload());
    answer(&conn, "d", "question:m3:0", &request(), "Romu", None).unwrap();
    let mut disguised = briefing;
    disguised["key"] = "quota-policy".into();
    insert(&conn, "m4", disguised);

    assert_eq!(stored_keys(&conn), ["briefing-purpose.s1", "quota-policy"]);
    let declined = refusal_of(&conn, "m2");
    assert_eq!(declined.key.as_deref(), Some("briefing-purpose.s1"));
    assert!(
        declined.reason.contains(
            "a declined briefing card is not asked again: the human fills in the briefing \
             form, or a new audit launch gives a new set; do not repost it under any key"
        ) && !declined.reason.contains("new key"),
        "{}",
        declined.reason
    );
    let answered = refusal_of(&conn, "m4");
    assert_eq!(answered.key.as_deref(), Some("quota-policy"));
    assert_eq!(
        answered.reason,
        "key `quota-policy` was answered in message m3; a briefing card is not asked again; \
         do not repost it under any key"
    );
}

/// KT-2084 — an answered briefing card is not asked again: the reason says it was answered,
/// never that the human still answers through the set.
#[test]
fn an_answered_briefing_card_refusal_says_it_was_answered() {
    let conn = database();
    let key = "briefing-purpose.s1";
    let briefing = serde_json::json!({"version":1,"key":key,
        "question":"What is this project for?","options":[{"id":"not_provided","label":"Not provided"}],
        "task_ref":"audit-briefing:p1:d:s1"});
    insert(&conn, "m1", briefing.clone());
    force_state(&conn, "question:m1:0", "answered");
    insert(&conn, "m2", briefing);

    assert_eq!(
        refusal_of(&conn, "m2").reason,
        format!(
            "key `{key}` was answered in message m1; a briefing card is not asked again; do not \
             repost it under any key"
        )
    );
}

/// KT-2085 — a briefing card's refusal follows its round: still waiting, the
/// agent is told so first; cancelled, it is not asked again either.
#[test]
fn a_briefing_card_refusal_follows_the_state_of_its_round() {
    let conn = database();
    let key = "briefing-purpose.s1";
    let briefing = serde_json::json!({"version":1,"key":key,
        "question":"What is this project for?","options":[{"id":"not_provided","label":"Not provided"}],
        "task_ref":"audit-briefing:p1:d:s1"});
    insert(&conn, "m1", briefing.clone());
    insert(&conn, "m2", briefing.clone());
    force_state(&conn, "question:m1:0", "cancelled");
    insert(&conn, "m3", briefing);

    assert_eq!(stored_keys(&conn), [key]);
    let waiting = refusal_of(&conn, "m2").reason;
    assert!(
        waiting.starts_with(&format!(
            "key `{key}` is still waiting for an answer in message m1"
        )) && waiting.contains("answers through the set they already have")
            && waiting.contains("do not repost it under any key")
            && !waiting.contains("new key"),
        "{waiting}"
    );
    let cancelled = refusal_of(&conn, "m3").reason;
    assert!(
        cancelled.contains(
            "a cancelled briefing card is not asked again: the human fills in the briefing \
             form, or a new audit launch gives a new set; do not repost it under any key"
        ) && !cancelled.contains("new key"),
        "{cancelled}"
    );
}

fn force_state(conn: &Connection, question_id: &str, state: &str) {
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute(
        "UPDATE discussion_questions SET state=?2 WHERE id=?1",
        params![question_id, state],
    )
    .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();
}

/// Answered, declined and cancelled (the state the 0.15.4 UI adds) are the
/// finished states: only they open a new round; any other is refused.
#[test]
fn only_a_finished_round_opens_a_new_one() {
    let conn = database();
    insert(&conn, "m1", payload());
    force_state(&conn, "question:m1:0", "cancelled");
    insert(&conn, "m2", payload());
    assert!(lint_report_of(&conn, "m2").is_none());
    assert_eq!(stored_keys(&conn), ["quota-policy", "quota-policy~2"]);

    force_state(&conn, "question:m2:0", "archived");
    insert(&conn, "m3", payload());
    assert_eq!(stored_keys(&conn), ["quota-policy", "quota-policy~2"]);
    let refusal = refusal_of(&conn, "m3");
    assert!(
        refusal.reason.contains("`archived`") && refusal.reason.contains("new key"),
        "{}",
        refusal.reason
    );
}

/// One card per key and message: after an answered round the first fence
/// opens the next round and the second is refused with its reason, the same
/// as for a key asked for the first time.
#[test]
fn the_same_key_twice_in_one_message_records_the_first_fence_only() {
    let conn = database();
    let twice = |value: serde_json::Value| {
        format!("```kronn-question\n{value}\n```\n\n```kronn-question\n{value}\n```")
    };
    insert(&conn, "m1", payload());
    answer(&conn, "d", "question:m1:0", &request(), "Romu", None).unwrap();
    crate::db::discussions::insert_message(&conn, "d", &message("m2", twice(payload()))).unwrap();
    let mut fresh = payload();
    fresh["key"] = "fresh".into();
    crate::db::discussions::insert_message(&conn, "d", &message("m3", twice(fresh))).unwrap();

    assert_eq!(
        stored_keys(&conn),
        ["quota-policy", "quota-policy~2", "fresh"]
    );
    let recorded: Vec<_> = list(&conn, "d")
        .unwrap()
        .questions
        .into_iter()
        .map(|q| (q.source_message_id, q.fence_index, q.state))
        .collect();
    assert_eq!(
        recorded,
        [
            ("m1".to_string(), 0, DiscussionQuestionState::Answered),
            ("m2".to_string(), 0, DiscussionQuestionState::Pending),
            ("m3".to_string(), 0, DiscussionQuestionState::Pending),
        ]
    );
    for (message_id, key) in [("m2", "quota-policy"), ("m3", "fresh")] {
        let refusal = refusal_of(&conn, message_id);
        assert_eq!(refusal.key.as_deref(), Some(key));
        assert!(
            refusal.reason.contains("twice in this message"),
            "{message_id}: {}",
            refusal.reason
        );
    }
}
