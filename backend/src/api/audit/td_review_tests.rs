use super::*;
use std::sync::Arc;
use tokio::sync::RwLock;

fn test_state() -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = Arc::new(RwLock::new(crate::core::config::default_config()));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn sheet(id: &str, severity: &str, status: &str) -> String {
    format!(
        "---\nname: {id}\ndescription: Problème {id}\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-10-01\n      status: Verified in source\n      reviewer: Full audit\n---\n\n# {id}\n\n\
- **Area**: Backend\n- **Severity**: {severity}\n- **Status**: {status}\n- **Effort**: S\n- **Blast radius**: Module\n\n\
## Problem (fact)\nIt breaks.\n\n## Where (pointers)\n- `backend/src/lib.rs:10`\n\n## Verification\nIt no longer breaks.\n"
    )
}

const A: &str = "TD-20261001-a";
const B: &str = "TD-20261001-b";
const C: &str = "TD-20261001-c";

/// A project with three sheets (Critical, High, Low), a completed run and its
/// validation discussion.
async fn fixture() -> (AppState, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
    std::fs::write(docs.join("AGENTS.md"), "# Projet\n").unwrap();
    for (id, severity) in [(A, "Critical"), (B, "High"), (C, "Low")] {
        std::fs::write(
            docs.join(format!("tech-debt/{id}.md")),
            sheet(id, severity, "Verified in source"),
        )
        .unwrap();
    }
    let state = test_state();
    let path = tmp.path().to_str().unwrap().to_string();
    state
        .db
        .with_conn(move |conn| {
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            crate::db::audit_runs::insert_running(
                conn,
                "r1",
                "p1",
                "Full",
                "ClaudeCode",
                Utc::now(),
            )?;
            crate::db::audit_runs::complete(
                conn,
                "r1",
                Utc::now(),
                "Completed",
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                100,
                None,
                None,
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('dv', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                [],
            )?;
            for (i, content) in [
                "Liste : TD-20261001-a et TD-20261001-b.",
                "Sur TD-20261001-a, c'est voulu ?",
                "Rien à voir, XTD-20261001-c.",
            ]
            .iter()
            .enumerate()
            {
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used, sort_order)
                     VALUES (?1, 'dv', 'Agent', ?2, '2026-10-01T10:00:00Z', 0, ?3)",
                    rusqlite::params![format!("mv{i}"), content, i as i64],
                )?;
            }
            crate::db::audit_runs::set_validation_discussion(conn, "r1", "dv")?;
            let documents = crate::core::audit_documents::capture_json(std::path::Path::new(&path))
                .map_err(anyhow::Error::msg)?;
            crate::db::audit_documents::record(conn, "r1", &documents)?;
            Ok(())
        })
        .await
        .unwrap();
    (state, tmp)
}

async fn list(state: &AppState) -> TdReviewList {
    let response = list_tds(State(state.clone()), Path("p1".into())).await.0;
    assert!(response.success, "{:?}", response.error);
    response.data.unwrap()
}

async fn decide(state: &AppState, items: Vec<TdDecisionItem>) -> ApiResponse<TdDecideOutcome> {
    decide_tds(
        State(state.clone()),
        Path("p1".into()),
        Json(TdDecideRequest { items }),
    )
    .await
    .0
}

fn decision(td_id: &str, decision: TdReviewDecision, reason: Option<&str>) -> TdDecisionItem {
    TdDecisionItem {
        td_id: td_id.into(),
        decision,
        reason: reason.map(str::to_string),
    }
}

fn read(tmp: &tempfile::TempDir, id: &str) -> String {
    std::fs::read_to_string(tmp.path().join(format!("docs/tech-debt/{id}.md"))).unwrap()
}

async fn task_count(state: &AppState) -> i64 {
    state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM planning_tasks", [], |r| r.get(0))?)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn the_list_carries_fields_counts_gate_tasks_and_talk_without_writing() {
    let (state, _tmp) = fixture().await;
    let created = create_td_task(State(state.clone()), Path(("p1".into(), A.into())))
        .await
        .0;
    assert!(created.success, "{:?}", created.error);
    let before = task_count(&state).await;

    let listed = list(&state).await;
    assert_eq!(task_count(&state).await, before, "listing creates no task");
    let ids: Vec<&str> = listed.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, [A, B, C]);
    let a = &listed.items[0];
    assert_eq!(
        (a.severity.as_str(), a.title.as_str()),
        ("Critical", "Problème TD-20261001-a")
    );
    assert_eq!(a.state, TdReviewState::Pending);
    assert_eq!(a.effort.as_deref(), Some("S"));
    assert_eq!(a.pointers[0].path, "backend/src/lib.rs");
    assert!(a
        .task
        .as_ref()
        .is_some_and(|t| t.reference.starts_with("KT-")));
    assert!(listed.items[1].task.is_none());
    assert_eq!(
        (
            a.talk_count,
            listed.items[1].talk_count,
            listed.items[2].talk_count
        ),
        (2, 1, 0)
    );
    assert_eq!(listed.counts.pending, 3);
    assert_eq!(
        (
            listed.gate.blocking_total,
            listed.gate.blocking_settled,
            listed.gate.met
        ),
        (2, 0, false)
    );
    assert_eq!(listed.validation_discussion_id.as_deref(), Some("dv"));
    assert!(listed.audit_date.is_some());
    assert!(!listed.validated);
}

#[tokio::test]
async fn one_unknown_id_refuses_the_whole_batch_before_any_write() {
    let (state, tmp) = fixture().await;
    let before: Vec<String> = [A, B, C].iter().map(|id| read(&tmp, id)).collect();
    for bad in [
        vec![
            decision(A, TdReviewDecision::Confirm, None),
            decision("TD-20261001-zz", TdReviewDecision::Reject, None),
        ],
        vec![
            decision(A, TdReviewDecision::Confirm, None),
            decision(A, TdReviewDecision::Reject, None),
        ],
        vec![decision("../decisions", TdReviewDecision::Reject, None)],
        vec![],
    ] {
        let response = decide(&state, bad).await;
        assert!(!response.success);
    }
    let after: Vec<String> = [A, B, C].iter().map(|id| read(&tmp, id)).collect();
    assert_eq!(before, after, "nothing written");
    assert!(!tmp.path().join("docs/decisions.md").exists());
}

#[tokio::test]
async fn a_bulk_decision_is_persisted_on_every_sheet_and_moves_the_gate() {
    let (state, tmp) = fixture().await;
    let response = decide(
        &state,
        vec![
            decision(A, TdReviewDecision::Confirm, None),
            decision(B, TdReviewDecision::Defer, None),
            decision(C, TdReviewDecision::Reject, None),
        ],
    )
    .await;
    assert!(response.success, "{:?}", response.error);
    let outcome = response.data.unwrap();
    let states: Vec<TdReviewState> = outcome.results.iter().map(|r| r.state).collect();
    assert_eq!(
        states,
        [
            TdReviewState::Confirmed,
            TdReviewState::Deferred,
            TdReviewState::Rejected
        ]
    );
    assert!(!outcome.gate.met, "a deferred High does not count");
    assert!(read(&tmp, A).contains("- **Status**: Confirmed by user"));
    assert!(read(&tmp, A).contains("reviewer: Kronn tech-debt review"));
    assert!(read(&tmp, B).contains("- **Status**: Deferred"));
    assert!(read(&tmp, C).contains("- **Status**: Rejected"));

    // Kept on purpose: the reason lands in the decision log, trusted.
    let response = decide(
        &state,
        vec![decision(
            B,
            TdReviewDecision::AcceptDecision,
            Some("Choix voulu : le cache est volontairement simple."),
        )],
    )
    .await;
    let outcome = response.data.expect("accepted");
    assert!(outcome.gate.met);
    assert_eq!(outcome.results[0].state, TdReviewState::AcceptedDecision);
    let decisions = std::fs::read_to_string(tmp.path().join("docs/decisions.md")).unwrap();
    assert!(decisions.contains("le cache est volontairement simple"));
    assert!(decisions.contains("- **Source**: user (tech-debt review)"));
    let adrs = crate::core::adr::parse(&decisions);
    assert_eq!(crate::core::adr::in_force(&adrs).len(), 1);
    assert!(read(&tmp, B).contains("- **Status**: Accepted decision"));

    let listed = list(&state).await;
    assert_eq!(
        (
            listed.counts.confirmed,
            listed.counts.accepted_decision,
            listed.counts.rejected,
            listed.counts.pending
        ),
        (1, 1, 1, 0)
    );
}

#[tokio::test]
async fn decisions_wait_for_a_running_audit() {
    let (state, tmp) = fixture().await;
    assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p1"));
    let response = decide(&state, vec![decision(A, TdReviewDecision::Confirm, None)]).await;
    assert!(!response.success);
    assert!(read(&tmp, A).contains("Verified in source"));
}

#[tokio::test]
async fn decide_tds_refuses_with_docs_busy_and_writes_nothing_while_the_lease_is_held() {
    let (state, tmp) = fixture().await;
    let sheets_before: Vec<String> = [A, B, C].iter().map(|id| read(&tmp, id)).collect();
    let documents_before = state
        .db
        .with_read_conn(|conn| crate::db::audit_documents::latest(conn, "p1"))
        .await
        .unwrap();
    let request = || {
        vec![
            decision(A, TdReviewDecision::Confirm, None),
            decision(
                B,
                TdReviewDecision::AcceptDecision,
                Some("Choix voulu : le cache est volontairement simple."),
            ),
            decision(C, TdReviewDecision::Reject, None),
        ]
    };
    let lease = crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p1").unwrap();
    let response = decide(&state, request()).await;
    assert!(!response.success, "{response:?}");
    assert_eq!(
        response.error.as_deref(),
        Some(crate::api::audit::docs_write::DOCS_BUSY)
    );
    let sheets_after: Vec<String> = [A, B, C].iter().map(|id| read(&tmp, id)).collect();
    assert_eq!(
        sheets_after, sheets_before,
        "a refused decision wrote a sheet"
    );
    assert!(
        !tmp.path().join("docs/decisions.md").exists(),
        "a refused decision wrote the decision log"
    );
    let documents_after = state
        .db
        .with_read_conn(|conn| crate::db::audit_documents::latest(conn, "p1"))
        .await
        .unwrap();
    assert_eq!(documents_after, documents_before);
    assert!(state.audit_tracker.lock().unwrap().leased.contains("p1"));
    // Released, the very same request goes through: the lease alone refused it.
    drop(lease);
    let response = decide(&state, request()).await;
    assert!(response.success, "{:?}", response.error);
    assert_ne!(read(&tmp, A), sheets_before[0]);
    assert!(tmp.path().join("docs/decisions.md").exists());
    assert!(!state.audit_tracker.lock().unwrap().leased.contains("p1"));
}

#[tokio::test]
async fn a_td_decision_updates_its_proof_without_accepting_pulled_documents() {
    for external_doc in [
        None,
        Some("docs/AGENTS.md"),
        Some("docs/tech-debt/TD-20261001-a.md"),
    ] {
        let (state, tmp) = fixture().await;
        if let Some(doc) = external_doc {
            let file = tmp.path().join(doc);
            let before = std::fs::read_to_string(&file).unwrap();
            std::fs::write(file, before + "\nUnreviewed content pulled from Git.\n").unwrap();
        }
        let decision_result = decide(
            &state,
            vec![
                decision(A, TdReviewDecision::Confirm, None),
                decision(B, TdReviewDecision::Confirm, None),
            ],
        )
        .await;
        assert!(decision_result.success, "{:?}", decision_result.error);
        assert!(decision_result.data.unwrap().gate.met);
        let validated = crate::api::audit::validate_audit(State(state), Path("p1".into()))
            .await
            .0;
        match external_doc {
            None => assert!(validated.success, "{:?}", validated.error),
            Some(doc) => {
                assert!(!validated.success);
                let error = validated.error.unwrap();
                assert!(
                    error.contains("changed since") && error.contains(doc),
                    "{error}"
                );
            }
        }
    }
}

#[tokio::test]
async fn one_sheet_becomes_a_task_once() {
    let (state, _tmp) = fixture().await;
    let first = create_td_task(State(state.clone()), Path(("p1".into(), B.into())))
        .await
        .0
        .data
        .unwrap();
    assert_eq!((first.created, first.items.len()), (1, 1));
    assert_eq!(
        task_count(&state).await,
        2,
        "the sheet and its theme parent"
    );
    let again = create_td_task(State(state.clone()), Path(("p1".into(), B.into())))
        .await
        .0
        .data
        .unwrap();
    assert_eq!((again.created, again.already_linked), (0, 1));
    let unknown = create_td_task(
        State(state.clone()),
        Path(("p1".into(), "TD-20261001-zz".into())),
    )
    .await
    .0;
    assert!(!unknown.success);
    assert_eq!(task_count(&state).await, 2);
}

#[tokio::test]
async fn the_talk_shows_validation_mentions_then_the_sheets_discussion() {
    let (state, _tmp) = fixture().await;
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('dt', 'p1', 'TD · TD-20261001-a — Problème', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used, sort_order)
                 VALUES ('mt', 'dt', 'User', 'Est-ce voulu ?', '2026-10-02 09:00:00', 0, 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let talk = td_talk(State(state.clone()), Path(("p1".into(), A.into())))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(talk.discussion_id.as_deref(), Some("dt"));
    let rows: Vec<(&str, TdTalkSource)> = talk
        .messages
        .iter()
        .map(|m| (m.id.as_str(), m.source))
        .collect();
    assert_eq!(
        rows,
        [
            ("mv0", TdTalkSource::Validation),
            ("mv1", TdTalkSource::Validation),
            ("mt", TdTalkSource::Td)
        ]
    );
    assert_eq!(
        talk.messages[2].timestamp.to_rfc3339(),
        "2026-10-02T09:00:00+00:00"
    );
    let listed = list(&state).await;
    assert_eq!(listed.items[0].talk_count, 3);
    assert_eq!(listed.items[0].talk_discussion_id.as_deref(), Some("dt"));

    // The sheet's discussion exists: a new message goes to it.
    let started = start_td_talk(
        State(state.clone()),
        Path(("p1".into(), A.into())),
        Json(TdTalkRequest {
            message: "Et maintenant ?".into(),
            targets: vec![MessageTarget::agent(AgentType::Codex)],
        }),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(
        started,
        TdTalkStart {
            discussion_id: "dt".into(),
            created: false
        }
    );
}

#[test]
fn the_first_target_leads_the_sheets_discussion() {
    let item = crate::core::td_review::item_from(
        A,
        &sheet(A, "High", "Verified in source"),
        "docs/tech-debt/TD-20261001-a.md".into(),
    );
    let mut custom = MessageTarget::agent(AgentType::Custom);
    custom.connection_id = Some("conn-1".into());
    custom.tier = Some(ModelTier::Reasoning);
    let request = talk_discussion_request(
        "p1",
        &item,
        "Vrai problème ?",
        &[custom, MessageTarget::agent(AgentType::Codex)],
        "fr".into(),
    )
    .unwrap();
    assert_eq!(request.agent, AgentType::Custom);
    assert_eq!(request.connection_id.as_deref(), Some("conn-1"));
    assert_eq!(request.tier, ModelTier::Reasoning);
    assert_eq!(request.title, "TD · TD-20261001-a — Problème TD-20261001-a");
    let kinds: Vec<MessageTargetKind> = request.initial_targets.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        [MessageTargetKind::DiscussionAgent, MessageTargetKind::Agent]
    );
    assert!(request.initial_prompt.ends_with("Vrai problème ?"));
    assert!(talk_discussion_request("p1", &item, "x", &[], "fr".into()).is_none());
}

/// Detail files a dimension audit wrote without front matter (KT-1309): the
/// project card counts them, so the review lists and settles them too.
fn write_dimension_files(tmp: &tempfile::TempDir) {
    let docs = tmp.path().join("docs");
    // Severity only in the index row.
    std::fs::write(
        docs.join("tech-debt/CQ-001.md"),
        "# CQ-001: Oversized controller\n\n## Problem\n\nThe controller grew.\n",
    )
    .unwrap();
    // Severity in the file itself, and a Status line.
    std::fs::write(
        docs.join("tech-debt/CQ-002.md"),
        "# CQ-002: Dead branch\n\n- **Severity**: Low\n- **Status**: Open\n",
    )
    .unwrap();
    // Nothing anywhere: unknown.
    std::fs::write(
        docs.join("tech-debt/PERF-20261016-deep-clone.md"),
        "# Deep clone on the hot path\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("inconsistencies-tech-debt.md"),
        "# Index\n\n## Current list\n\n| ID | Problem | Area | Severity | Status |\n|----|---------|------|----------|--------|\n| CQ-001 | Oversized controller | Backend | High | Open |\n",
    )
    .unwrap();
}

#[tokio::test]
async fn the_review_lists_every_sheet_the_card_counts() {
    let (state, tmp) = fixture().await;
    write_dimension_files(&tmp);
    let listed = list(&state).await;
    let ids: Vec<&str> = listed.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        ["CQ-001", "CQ-002", "PERF-20261016-deep-clone", A, B, C]
    );
    let card = crate::core::scanner::count_tech_debt(tmp.path().to_str().unwrap());
    assert_eq!(
        listed.items.len() as u32,
        card,
        "card and review count the same debts"
    );
    let by_id = |id: &str| listed.items.iter().find(|i| i.id == id).unwrap();
    // The index row says what the file does not.
    assert_eq!(by_id("CQ-001").severity, "High");
    assert_eq!(by_id("CQ-001").status, "Open");
    assert_eq!(by_id("CQ-001").area, "Backend");
    assert_eq!(by_id("CQ-002").severity, "Low");
    assert_eq!(by_id("PERF-20261016-deep-clone").severity, "Unknown");
    assert_eq!(listed.counts.pending, 6);
    assert!(
        listed
            .by_severity
            .iter()
            .any(|row| row.severity == "Unknown" && row.total == 1),
        "an unknown severity is shown, not dropped"
    );
    // A High file without front matter blocks the gate like any other.
    assert_eq!(
        (listed.gate.blocking_total, listed.gate.blocking_settled),
        (3, 0)
    );
}

#[tokio::test]
async fn a_high_detail_file_without_front_matter_gates_the_validation_until_it_is_settled() {
    let (state, tmp) = fixture().await;
    write_dimension_files(&tmp);
    let response = decide(
        &state,
        vec![
            decision(A, TdReviewDecision::Confirm, None),
            decision(B, TdReviewDecision::Reject, None),
        ],
    )
    .await;
    let outcome = response.data.expect("decided");
    assert!(!outcome.gate.met, "CQ-001 is High and still pending");
    assert_eq!(
        (outcome.gate.blocking_total, outcome.gate.blocking_settled),
        (3, 2)
    );

    let response = decide(
        &state,
        vec![decision(
            "CQ-001",
            TdReviewDecision::Confirm,
            Some("Yes, split it before the next feature."),
        )],
    )
    .await;
    let outcome = response.data.expect("a detail file can be decided");
    assert!(outcome.gate.met);
    assert_eq!(outcome.results[0].state, TdReviewState::Confirmed);
    let sheet = read(&tmp, "CQ-001");
    assert!(sheet.starts_with("---\n"), "{sheet}");
    assert!(sheet.contains("- **Status**: Confirmed by user"), "{sheet}");
    assert!(
        sheet.contains("## Problem\n\nThe controller grew."),
        "{sheet}"
    );
    let front = sheet.strip_prefix("---\n").unwrap();
    let front: serde_yaml::Value =
        serde_yaml::from_str(&front[..front.find("\n---").unwrap()]).expect("strict YAML");
    assert_eq!(
        front["metadata"]["audit_history"][0]["note"].as_str(),
        Some("Yes, split it before the next feature.")
    );
    let listed = list(&state).await;
    assert_eq!(listed.counts.confirmed, 2);
}

#[tokio::test]
async fn the_reason_typed_for_a_decision_is_what_the_sheet_keeps() {
    let (state, tmp) = fixture().await;
    for (id, kind, reason) in [
        (A, TdReviewDecision::Confirm, "Real: seen in prod."),
        (B, TdReviewDecision::Defer, "After the migration."),
        (C, TdReviewDecision::Reject, "Not reachable."),
    ] {
        let response = decide(&state, vec![decision(id, kind, Some(reason))]).await;
        assert!(response.success, "{:?}", response.error);
        let sheet = read(&tmp, id);
        assert!(sheet.contains(&format!("note: \"{reason}\"")), "{sheet}");
        assert!(!sheet.contains("from the tech-debt review screen"));
    }
    // Without a reason the fixed note stays.
    let (state, tmp) = fixture().await;
    decide(&state, vec![decision(A, TdReviewDecision::Confirm, None)]).await;
    assert!(read(&tmp, A).contains("Confirmed from the tech-debt review screen."));
}

/// The card's active count and what the review shows as active: same set.
fn review_active(listed: &TdReviewList) -> u32 {
    listed.counts.pending + listed.counts.confirmed + listed.counts.deferred
}

fn index_with_rows(rows: &[&str]) -> String {
    let mut out = String::from(
        "# Index\n\n## Current list\n\n| ID | Problem | Area | Severity | Status |\n|----|---------|------|----------|--------|\n",
    );
    for row in rows {
        out.push_str(row);
        out.push('\n');
    }
    out
}

#[tokio::test]
async fn card_and_review_count_the_same_debts_with_a_decided_sheet_and_a_sheetless_row() {
    let tmp = tempfile::TempDir::new().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
    std::fs::write(docs.join("AGENTS.md"), "# Projet\n").unwrap();
    let path = tmp.path().to_str().unwrap();
    // TD-a Open/High, TD-b Rejected, TD-c only in the index.
    std::fs::write(
        docs.join(format!("tech-debt/{A}.md")),
        sheet(A, "High", "Open"),
    )
    .unwrap();
    std::fs::write(
        docs.join(format!("tech-debt/{B}.md")),
        sheet(B, "Low", "Rejected"),
    )
    .unwrap();
    let with_row = index_with_rows(&[&format!(
        "| {C} | Only in the index | API | Critical | Open |"
    )]);
    std::fs::write(docs.join("inconsistencies-tech-debt.md"), &with_row).unwrap();

    let items = crate::core::td_review::read_items(&docs);
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, [A, B, C], "the sheetless row is listed, not hidden");
    let card = crate::core::scanner::count_tech_debt(path);
    let (counts, _) = crate::core::td_review::counts(&items);
    assert_eq!(card, 2, "TD-a and TD-c are the active debts");
    assert_eq!(
        counts.pending + counts.confirmed + counts.deferred,
        card,
        "the review's active debts are the card's"
    );
    let row = items.iter().find(|i| i.id == C).unwrap();
    assert_eq!(
        (row.severity.as_str(), row.area.as_str()),
        ("Critical", "API")
    );
    assert_eq!(row.state, TdReviewState::Pending);
    assert!(
        crate::core::td_review::gate(&items).blocking_total == 2,
        "a Critical sheetless row gates the validation"
    );

    // Drop the row: 1 vs 1 (the rejected sheet is listed but is not debt).
    std::fs::write(
        docs.join("inconsistencies-tech-debt.md"),
        index_with_rows(&[]),
    )
    .unwrap();
    let items = crate::core::td_review::read_items(&docs);
    let (counts, _) = crate::core::td_review::counts(&items);
    assert_eq!(crate::core::scanner::count_tech_debt(path), 1);
    assert_eq!(counts.pending + counts.confirmed + counts.deferred, 1);
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn a_debt_only_the_index_lists_can_be_decided_and_gets_its_sheet() {
    let (state, tmp) = fixture().await;
    let docs = tmp.path().join("docs");
    std::fs::write(
        docs.join("inconsistencies-tech-debt.md"),
        index_with_rows(&["| TD-20261016-rowonly | Row only | API | High | Open |"]),
    )
    .unwrap();
    let before = list(&state).await;
    assert_eq!(before.items.len(), 4);
    assert_eq!(review_active(&before), 4);
    assert!(before.items.iter().any(|i| i.id == "TD-20261016-rowonly"));

    let response = decide(
        &state,
        vec![decision(
            "TD-20261016-rowonly",
            TdReviewDecision::Reject,
            Some("Not a real problem."),
        )],
    )
    .await;
    let outcome = response.data.expect("a sheetless row can be decided");
    assert_eq!(outcome.results[0].state, TdReviewState::Rejected);
    let sheet = read(&tmp, "TD-20261016-rowonly");
    assert!(sheet.contains("- **Severity**: High"), "{sheet}");
    assert!(sheet.contains("- **Status**: Rejected"), "{sheet}");
    let front = sheet.strip_prefix("---\n").unwrap();
    let front: serde_yaml::Value =
        serde_yaml::from_str(&front[..front.find("\n---").unwrap()]).expect("strict YAML");
    assert_eq!(
        front["metadata"]["audit_history"][0]["note"].as_str(),
        Some("Not a real problem.")
    );
    let after = list(&state).await;
    assert_eq!(after.counts.rejected, 1);
    assert_eq!(
        review_active(&after),
        crate::core::scanner::count_tech_debt(tmp.path().to_str().unwrap())
    );
}

#[tokio::test]
async fn an_index_status_stands_in_for_a_sheet_that_states_none() {
    let tmp = tempfile::TempDir::new().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
    std::fs::write(
        docs.join("tech-debt/CQ-001.md"),
        "# CQ-001: Oversized controller\n\nNo status line.\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("tech-debt/CQ-002.md"),
        "# CQ-002: Dead branch\n\n- **Status**: Open\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("inconsistencies-tech-debt.md"),
        index_with_rows(&[
            "| CQ-001 | Oversized controller | Backend | High | Resolved |",
            "| CQ-002 | Dead branch | Backend | Low | Resolved |",
        ]),
    )
    .unwrap();
    let items = crate::core::td_review::read_items(&docs);
    let state_of = |id: &str| items.iter().find(|i| i.id == id).unwrap().state;
    // The file says nothing: its index row's Resolved settles it.
    assert_eq!(state_of("CQ-001"), TdReviewState::Rejected);
    // The file says Open: the file decides.
    assert_eq!(state_of("CQ-002"), TdReviewState::Pending);
    assert_eq!(
        crate::core::scanner::count_tech_debt(tmp.path().to_str().unwrap()),
        1
    );
}
