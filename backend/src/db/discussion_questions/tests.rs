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
        session_continuity: None,
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

fn finished_step_question(value: serde_json::Value) -> Connection {
    finished_step_cards(&[value])
}

/// One message of a workflow step carrying several cards, the step then ended.
fn finished_step_cards(values: &[serde_json::Value]) -> Connection {
    let conn = database();
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
        &conn,
        "source-run",
        "orchestrate",
        "Orchestrate",
        "d",
        "Codex",
    )
    .unwrap();
    let session = crate::db::workflow_step_rooms::join(
        &conn,
        "source-run",
        "orchestrate",
        "d",
        "Codex",
        "step-session",
    )
    .unwrap();
    crate::db::discussions::insert_cli_message_with_targets_and_dispatches(
        &conn,
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
    crate::db::workflow_step_rooms::finish_activity(&conn, "source-run", "orchestrate").unwrap();
    crate::db::workflow_step_rooms::revoke(&conn, &[session.session_pk]).unwrap();
    conn.execute(
        "UPDATE workflow_runs SET status='Success', finished_at='now' WHERE id='source-run'",
        [],
    )
    .unwrap();
    conn
}

fn request() -> AnswerDiscussionQuestionRequest {
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

fn dispatch_count(conn: &Connection) -> i64 {
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
    assert!(
        content.contains("Ne bloque pas sur cette question"),
        "{content}"
    );
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

fn card(key: &str) -> serde_json::Value {
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
