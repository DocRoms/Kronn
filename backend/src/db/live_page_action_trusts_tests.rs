use std::collections::HashMap;

use chrono::Utc;
use rusqlite::{params, Connection};

use super::*;
use crate::db::kronn_action_engine::{self, ActionCompletion};
use crate::db::live_page_actions::{
    claim_launch, claim_trusted_launch, complete, ingest_page_actions, LivePageActionClaimOutcome,
};
use crate::models::{CollectApiDataConfig, CollectApiDataSource, PromptVariable};

pub(crate) const PAGE: &str = "page-todo";
pub(crate) const ACTION: &str = "page-action:page-todo:todo-move";
const ROWS: &str = r#"[{"id":"T-1"},{"id":"T-2"},{"id":"T-3"}]"#;

fn connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn
}

pub(crate) fn block(action_ref: &str, values: &str) -> String {
    format!(
        r#"<button data-kronn-action="{action_ref}">Go</button>
<script type="application/kronn-action" data-action-id="{action_ref}">{{"kind":"workflow","target_id":"wf-move","values":{values}}}</script>"#
    )
}

pub(crate) const BOUND: &str = r#"[{"name":"ticket","provenance":"dynamic_binding","source_ref":"<page.dataset.todo.find(id).id>"}]"#;

pub(crate) fn workflow() -> Workflow {
    let mut workflow = crate::db::tests::sample_workflow("wf-move");
    workflow.enabled = true;
    workflow.steps[0].step_type = StepType::JsonData;
    workflow.variables = vec![PromptVariable {
        name: "ticket".into(),
        label: "Ticket".into(),
        placeholder: String::new(),
        description: None,
        required: true,
        pattern: None,
        source: None,
        source_ref: None,
        allow_manual_override: false,
        control: None,
    }];
    workflow
}

fn insert_project(conn: &Connection, id: &str) {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?1, ?1, ?2, ?3, ?3)",
        params![id, format!("/tmp/{id}"), now],
    )
    .unwrap();
}

/// A Todo page whose `todo-move` block targets `wf-move`, ingested as Kronn does.
pub(crate) fn setup(
    conn: &Connection,
    workflow: &Workflow,
    page_project: Option<&str>,
    html: &str,
) {
    crate::db::workflows::insert_workflow(conn, workflow).unwrap();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO live_pages (id, project_id, title, slug, current_revision_id, data_revision,
             created_at, updated_at, last_published_at, pinned, archived)
         VALUES (?1, ?2, 'Todo', ?1, 'rev-1', 0, ?3, ?3, NULL, 0, 0)",
        params![PAGE, page_project, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO live_page_revisions (id, page_id, revision, html, created_by_agent, created_at)
         VALUES ('rev-1', ?1, 1, ?2, NULL, ?3)",
        params![PAGE, html, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO live_page_datasets (id, page_id, name, kind, current_json, schema_json,
             max_points, max_age_days, updated_at)
         VALUES ('ds-todo', ?1, 'todo', 'collection', ?2, NULL, 50000, NULL, ?3)",
        params![PAGE, ROWS, now],
    )
    .unwrap();
    ingest_page_actions(conn, PAGE, "rev-1", html).unwrap();
}

fn republish(conn: &Connection, revision: u32, html: &str) {
    let id = format!("rev-{revision}");
    conn.execute(
        "INSERT INTO live_page_revisions (id, page_id, revision, html, created_by_agent, created_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![id, PAGE, revision, html, Utc::now().to_rfc3339()],
    )
    .unwrap();
    conn.execute(
        "UPDATE live_pages SET current_revision_id = ?2 WHERE id = ?1",
        params![PAGE, id],
    )
    .unwrap();
    ingest_page_actions(conn, PAGE, &id, html).unwrap();
}

fn state(conn: &Connection, action_id: &str) -> LivePageActionTrustState {
    list_for_page(conn, PAGE)
        .unwrap()
        .into_iter()
        .find(|state| state.action_id == action_id)
        .expect("the offer is listed")
}

fn approve_current(conn: &Connection, action_id: &str) -> LivePageActionTrust {
    let fingerprint = state(conn, action_id)
        .fingerprint
        .expect("the action is eligible");
    approve(conn, action_id, &fingerprint).unwrap()
}

fn row(ticket: &str) -> HashMap<String, String> {
    HashMap::from([("ticket".to_string(), ticket.to_string())])
}

fn refusal(result: Result<Option<LivePageActionClaimOutcome>>) -> LivePageActionTrustRefusal {
    match result {
        Err(error) => *error
            .downcast_ref::<LivePageActionTrustRefusal>()
            .unwrap_or_else(|| panic!("not a trust refusal: {error}")),
        Ok(_) => panic!("the trusted launch was admitted"),
    }
}

fn finish(conn: &Connection, launch_id: &str) {
    complete(
        conn,
        launch_id,
        ActionCompletion {
            state: DiscussionActionState::Succeeded,
            shared_run_id: None,
            result_discussion_id: None,
            deep_link: None,
            diagnostic: None,
        },
    )
    .unwrap();
}

fn claimed(result: Result<Option<LivePageActionClaimOutcome>>) -> LivePageAction {
    match result.unwrap().unwrap() {
        LivePageActionClaimOutcome::Claimed { action, .. } => action,
        LivePageActionClaimOutcome::Existing(action) => {
            panic!("expected a fresh launch, got {:?}", action.state)
        }
    }
}

#[test]
fn an_approved_action_launches_without_a_card_and_is_marked_trusted() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    let launch = claimed(claim_trusted_launch(&conn, ACTION, &row("T-1")));
    assert_eq!(launch.trusted, Some(true));
    assert_eq!(launch.binding_key.as_deref(), Some("ticket=T-1"));
    // The button's history carries it like any other launch.
    let launches = crate::db::live_page_actions::latest_launches_for_live_page(
        kronn_action_engine::Reconcile::Persisted,
        &conn,
        PAGE,
    )
    .unwrap();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].trusted, Some(true));
    // A card launch is never marked.
    let card = claimed(claim_launch(&conn, ACTION, &HashMap::new(), &row("T-2")));
    assert_eq!(card.trusted, None);
}

#[test]
fn without_an_approval_the_card_is_required() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    assert_eq!(
        refusal(claim_trusted_launch(&conn, ACTION, &row("T-1"))),
        LivePageActionTrustRefusal::NotTrusted
    );
}

#[test]
fn an_approval_covers_only_its_own_action() {
    let conn = connection();
    let html = format!(
        "{}{}",
        block("todo-move", BOUND),
        block("todo-other", BOUND)
    );
    setup(&conn, &workflow(), None, &html);
    approve_current(&conn, ACTION);
    assert_eq!(
        refusal(claim_trusted_launch(
            &conn,
            "page-action:page-todo:todo-other",
            &row("T-1")
        )),
        LivePageActionTrustRefusal::NotTrusted
    );
}

#[test]
fn an_approval_of_another_fingerprint_than_the_current_one_is_refused() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    let error = approve(&conn, ACTION, "what-the-human-saw-before").unwrap_err();
    assert_eq!(
        error.downcast_ref::<LivePageActionTrustRefusal>(),
        Some(&LivePageActionTrustRefusal::Changed)
    );
    assert!(get(&conn, ACTION).unwrap().is_none());
}

#[test]
fn a_changed_block_parameter_invalidates_the_approval_for_good() {
    let conn = connection();
    let html = block("todo-move", BOUND);
    setup(&conn, &workflow(), None, &html);
    approve_current(&conn, ACTION);

    // Same block, another revision: still approved.
    republish(&conn, 2, &format!("<h1>Todo</h1>{html}"));
    assert!(state(&conn, ACTION).active);

    let changed = r#"[{"name":"ticket","provenance":"dynamic_binding","source_ref":"<page.dataset.todo.find(id).owner>"}]"#;
    republish(&conn, 3, &block("todo-move", changed));
    let listed = state(&conn, ACTION);
    assert!(!listed.active);
    assert_eq!(
        listed.trust.unwrap().invalidated_reason,
        Some(LivePageActionTrustRefusal::Changed)
    );
    assert_eq!(
        refusal(claim_trusted_launch(&conn, ACTION, &row("T-1"))),
        LivePageActionTrustRefusal::Changed
    );

    // Putting the approved block back does not revive it: a human approves again.
    republish(&conn, 4, &html);
    assert!(!state(&conn, ACTION).active);
    approve_current(&conn, ACTION);
    assert!(state(&conn, ACTION).active);
}

#[test]
fn a_block_removed_from_the_page_invalidates_its_approval() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    republish(&conn, 2, "<p>no actions</p>");
    let trust = get(&conn, ACTION).unwrap().unwrap();
    assert_eq!(
        trust.invalidated_reason,
        Some(LivePageActionTrustRefusal::StaleSource)
    );
}

#[test]
fn an_edited_workflow_invalidates_the_approval() {
    let conn = connection();
    let mut workflow = workflow();
    setup(&conn, &workflow, None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    workflow.steps[0].json_data_payload = Some(serde_json::json!({"column":"done"}));
    crate::db::workflows::update_workflow(&conn, &workflow).unwrap();
    assert_eq!(
        refusal(claim_trusted_launch(&conn, ACTION, &row("T-1"))),
        LivePageActionTrustRefusal::Changed
    );
}

#[test]
fn a_quick_exec_data_source_is_refused_until_runs_freeze_it() {
    let conn = connection();
    let now = Utc::now();
    let mut exec = crate::models::QuickExec {
        id: "qe-move".into(),
        name: "Move".into(),
        icon: "⌨".into(),
        description: String::new(),
        project_id: None,
        command: "printf".into(),
        args: vec!["doing".into()],
        timeout_secs: 30,
        output_format: crate::models::CollectQuickExecOutputFormat::Json,
        variables: vec![],
        pinned: false,
        created_at: now,
        updated_at: now,
        unmodelled_args_approved: None,
        agent_written: None,
    };
    crate::db::quick_execs::insert_quick_exec(&conn, &exec).unwrap();
    let mut workflow = workflow();
    workflow.steps[0].step_type = StepType::CollectApiData;
    workflow.steps[0].collect_api_data = Some(CollectApiDataConfig {
        sources: vec![CollectApiDataSource {
            alias: "move".into(),
            quick_api_id: String::new(),
            quick_exec_id: "qe-move".into(),
            quick_exec: None,
            required: true,
            variables: HashMap::new(),
        }],
        concurrent_limit: None,
    });
    setup(&conn, &workflow, None, &block("todo-move", BOUND));
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::UnpinnedDependency)
    );
    exec.args = vec![];
    let _ = exec;
}

#[test]
fn revocation_takes_effect_on_the_next_click() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    assert!(revoke(&conn, ACTION).unwrap());
    assert_eq!(
        refusal(claim_trusted_launch(&conn, ACTION, &row("T-1"))),
        LivePageActionTrustRefusal::NotTrusted
    );
}

fn stored_workflow(conn: &Connection, id: &str) -> Workflow {
    crate::db::workflows::get_workflow(conn, id)
        .unwrap()
        .unwrap()
}

#[test]
fn a_revocation_or_edit_after_the_claim_stops_the_run() {
    let conn = connection();
    let mut workflow = workflow();
    setup(&conn, &workflow, None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);

    let first = claimed(claim_trusted_launch(&conn, ACTION, &row("T-1")));
    let snapshot = stored_workflow(&conn, "wf-move");
    assert_eq!(
        admit_run(&conn, &first.id, &snapshot, &Default::default()).unwrap(),
        Ok(())
    );
    revoke(&conn, ACTION).unwrap();
    assert_eq!(
        admit_run(&conn, &first.id, &snapshot, &Default::default()).unwrap(),
        Err(LivePageActionTrustRefusal::NotTrusted)
    );

    approve_current(&conn, ACTION);
    let second = claimed(claim_trusted_launch(&conn, ACTION, &row("T-2")));
    workflow.steps[0].json_data_payload = Some(serde_json::json!({"column":"done"}));
    crate::db::workflows::update_workflow(&conn, &workflow).unwrap();
    assert_eq!(
        admit_run(
            &conn,
            &second.id,
            &stored_workflow(&conn, "wf-move"),
            &Default::default()
        )
        .unwrap(),
        Err(LivePageActionTrustRefusal::Changed)
    );
    // A card launch never passes for a trusted one.
    let card = claimed(claim_launch(&conn, ACTION, &HashMap::new(), &row("T-3")));
    assert_eq!(
        admit_run(&conn, &card.id, &snapshot, &Default::default()).unwrap(),
        Err(LivePageActionTrustRefusal::NotTrusted)
    );
}

#[test]
fn a_snapshot_other_than_the_approved_definition_is_never_admitted() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    let launch = claimed(claim_trusted_launch(&conn, ACTION, &row("T-1")));
    let mut unapproved = stored_workflow(&conn, "wf-move");
    unapproved.steps[0].json_data_payload = Some(serde_json::json!({"column":"done"}));
    assert_eq!(
        admit_run(&conn, &launch.id, &unapproved, &Default::default()).unwrap(),
        Err(LivePageActionTrustRefusal::Changed)
    );
    let mut agent = stored_workflow(&conn, "wf-move");
    agent.steps[0].step_type = StepType::Agent;
    assert!(admit_run(&conn, &launch.id, &agent, &Default::default())
        .unwrap()
        .is_err());
}

/// Codex r1 P1-b: one action_ref, two harmless targets A then B.
#[test]
fn a_claim_under_an_older_approval_never_runs_under_a_newer_one() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    let mut other = workflow();
    other.id = "wf-other".into();
    crate::db::workflows::insert_workflow(&conn, &other).unwrap();
    approve_current(&conn, ACTION);
    let claim_a = claimed(claim_trusted_launch(&conn, ACTION, &row("T-1")));
    assert_eq!(claim_a.target_id, "wf-move");

    republish(
        &conn,
        2,
        &block("todo-move", BOUND).replace("wf-move", "wf-other"),
    );
    approve_current(&conn, ACTION);
    for snapshot in [
        stored_workflow(&conn, "wf-move"),
        stored_workflow(&conn, "wf-other"),
    ] {
        assert!(
            admit_run(&conn, &claim_a.id, &snapshot, &Default::default())
                .unwrap()
                .is_err()
        );
    }

    // Revoke then re-approve the same content: the old claim stays dead.
    let claim_b = claimed(claim_trusted_launch(&conn, ACTION, &row("T-2")));
    revoke(&conn, ACTION).unwrap();
    approve_current(&conn, ACTION);
    assert_eq!(
        admit_run(
            &conn,
            &claim_b.id,
            &stored_workflow(&conn, "wf-other"),
            &Default::default()
        )
        .unwrap(),
        Err(LivePageActionTrustRefusal::NotTrusted)
    );
    // A new click under the new approval does run.
    let claim_c = claimed(claim_trusted_launch(&conn, ACTION, &row("T-3")));
    assert_eq!(
        admit_run(
            &conn,
            &claim_c.id,
            &stored_workflow(&conn, "wf-other"),
            &Default::default()
        )
        .unwrap(),
        Ok(())
    );
}

/// Codex r1 P2-a: no read and no claim between the change and its undoing.
#[test]
fn a_write_to_the_workflow_invalidates_even_when_undone_unobserved() {
    let conn = connection();
    let original = workflow();
    setup(&conn, &original, None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    let mut changed = original.clone();
    changed.steps[0].json_data_payload = Some(serde_json::json!({"column":"done"}));
    crate::db::workflows::update_workflow(&conn, &changed).unwrap();
    crate::db::workflows::update_workflow(&conn, &original).unwrap();
    let trust = get(&conn, ACTION).unwrap().unwrap();
    assert_eq!(
        trust.invalidated_reason,
        Some(LivePageActionTrustRefusal::Changed)
    );

    // Disable then re-enable, identical content: still invalidated.
    approve_current(&conn, ACTION);
    conn.execute("UPDATE workflows SET enabled = 0 WHERE id = 'wf-move'", [])
        .unwrap();
    conn.execute("UPDATE workflows SET enabled = 1 WHERE id = 'wf-move'", [])
        .unwrap();
    let trust = get(&conn, ACTION).unwrap().unwrap();
    assert_eq!(
        trust.invalidated_reason,
        Some(LivePageActionTrustRefusal::WorkflowDisabled)
    );
    // A pin toggle is not a change to what runs.
    approve_current(&conn, ACTION);
    conn.execute("UPDATE workflows SET pinned = 1 WHERE id = 'wf-move'", [])
        .unwrap();
    assert!(get(&conn, ACTION)
        .unwrap()
        .unwrap()
        .invalidated_at
        .is_none());
}

#[test]
fn concurrent_trusted_clicks_on_one_row_start_one_launch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kronn.db");
    {
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        setup(&conn, &workflow(), None, &block("todo-move", BOUND));
        approve_current(&conn, ACTION);
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(&path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                barrier.wait();
                // Busy, in flight or rate-limited are all fine; a second run is not.
                let _ = claim_trusted_launch(&conn, ACTION, &row("T-1"));
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    let launches: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM live_page_action_launches WHERE action_id = ?1",
            [ACTION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(launches, 1);
}

#[test]
fn approving_and_revoking_the_same_trust_concurrently_leaves_one_coherent_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kronn.db");
    let fingerprint = {
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        setup(&conn, &workflow(), None, &block("todo-move", BOUND));
        state(&conn, ACTION).fingerprint.unwrap()
    };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let spawn = |approving: bool| {
        let path = path.clone();
        let barrier = barrier.clone();
        let fingerprint = fingerprint.clone();
        std::thread::spawn(move || {
            let conn = Connection::open(&path).unwrap();
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            barrier.wait();
            for _ in 0..20 {
                if approving {
                    approve(&conn, ACTION, &fingerprint).unwrap();
                } else {
                    revoke(&conn, ACTION).unwrap();
                }
            }
        })
    };
    let approver = spawn(true);
    let revoker = spawn(false);
    approver.join().unwrap();
    revoker.join().unwrap();
    let conn = Connection::open(&path).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM live_page_action_trusts WHERE action_id = ?1",
            [ACTION],
            |row| row.get(0),
        )
        .unwrap();
    assert!(rows <= 1);
    // Whatever won, what remains is either nothing or the exact approval.
    if let Some(trust) = get(&conn, ACTION).unwrap() {
        assert_eq!(trust.fingerprint, fingerprint);
        assert!(trust.invalidated_at.is_none());
    }
}

#[test]
fn a_row_cannot_be_relaunched_faster_than_the_row_interval() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    let first = claimed(claim_trusted_launch(&conn, ACTION, &row("T-1")));
    // A row in flight is never relaunched: the click answers with its run.
    assert!(matches!(
        claim_trusted_launch(&conn, ACTION, &row("T-1")).unwrap().unwrap(),
        LivePageActionClaimOutcome::Existing(ref running) if running.id == first.id
    ));
    finish(&conn, &first.id);
    assert_eq!(
        refusal(claim_trusted_launch(&conn, ACTION, &row("T-1"))),
        LivePageActionTrustRefusal::RateLimited
    );
    // Another row is not held back by this one.
    claimed(claim_trusted_launch(&conn, ACTION, &row("T-2")));
}

#[test]
fn an_action_cannot_exceed_its_window_of_trusted_launches() {
    let conn = connection();
    let rows: Vec<serde_json::Value> = (0..=ACTION_MAX_PER_WINDOW)
        .map(|index| serde_json::json!({"id": format!("T-{index}")}))
        .collect();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    conn.execute(
        "UPDATE live_page_datasets SET current_json = ?1 WHERE id = 'ds-todo'",
        [serde_json::to_string(&rows).unwrap()],
    )
    .unwrap();
    approve_current(&conn, ACTION);
    for index in 0..ACTION_MAX_PER_WINDOW {
        claimed(claim_trusted_launch(
            &conn,
            ACTION,
            &row(&format!("T-{index}")),
        ));
    }
    assert_eq!(
        refusal(claim_trusted_launch(
            &conn,
            ACTION,
            &row(&format!("T-{ACTION_MAX_PER_WINDOW}"))
        )),
        LivePageActionTrustRefusal::RateLimited
    );
}

fn eligibility(conn: &Connection) -> Option<LivePageActionTrustRefusal> {
    state(conn, ACTION).refusal
}

#[test]
fn only_agentless_workflows_without_typed_values_are_eligible() {
    let conn = connection();
    let mut agent = workflow();
    agent.steps[0].step_type = StepType::Agent;
    setup(&conn, &agent, None, &block("todo-move", BOUND));
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::AgentStep)
    );
    let fingerprint_free = approve(&conn, ACTION, "anything").unwrap_err();
    assert_eq!(
        fingerprint_free.downcast_ref::<LivePageActionTrustRefusal>(),
        Some(&LivePageActionTrustRefusal::AgentStep)
    );

    for (step_type, indirect) in [
        (StepType::SubWorkflow, None),
        (StepType::TriggerWorkflow, None),
        (StepType::BatchQuickPrompt, None),
        (StepType::JsonData, Some("qp-hidden")),
    ] {
        let mut workflow = workflow();
        workflow.steps[0].step_type = step_type.clone();
        workflow.steps[0].quick_prompt_id = indirect.map(str::to_string);
        crate::db::workflows::update_workflow(&conn, &workflow).unwrap();
        assert_eq!(
            eligibility(&conn),
            Some(LivePageActionTrustRefusal::AgentStep),
            "{step_type:?}"
        );
    }
    let mut rollback = workflow();
    rollback.on_failure = vec![agent.steps[0].clone()];
    crate::db::workflows::update_workflow(&conn, &rollback).unwrap();
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::AgentStep)
    );

    let mut disabled = workflow();
    disabled.enabled = false;
    crate::db::workflows::update_workflow(&conn, &disabled).unwrap();
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::WorkflowDisabled)
    );

    crate::db::workflows::update_workflow(&conn, &workflow()).unwrap();
    assert_eq!(eligibility(&conn), None);
    let typed = r#"[{"name":"ticket","provenance":"user_input"}]"#;
    republish(&conn, 2, &block("todo-move", typed));
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::UserInput)
    );
    let mut with_env = workflow();
    with_env.variables.push(PromptVariable {
        name: "token".into(),
        label: "Token".into(),
        placeholder: String::new(),
        description: None,
        required: true,
        pattern: None,
        source: Some(crate::models::PromptVariableSource::ProjectEnv),
        source_ref: Some("<env.TOKEN>".into()),
        allow_manual_override: false,
        control: None,
    });
    crate::db::workflows::update_workflow(&conn, &with_env).unwrap();
    republish(&conn, 3, &block("todo-move", BOUND));
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::SecretValue)
    );
}

#[test]
fn a_target_in_another_project_than_the_page_is_refused() {
    let conn = connection();
    insert_project(&conn, "proj-a");
    insert_project(&conn, "proj-b");
    let mut foreign = workflow();
    foreign.project_id = Some("proj-b".into());
    setup(&conn, &foreign, Some("proj-a"), &block("todo-move", BOUND));
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::CrossProject)
    );
    assert!(approve(&conn, ACTION, "anything").is_err());
    assert!(get(&conn, ACTION).unwrap().is_none());
}

#[test]
fn moving_the_page_to_another_project_invalidates_the_approval() {
    let conn = connection();
    insert_project(&conn, "proj-a");
    insert_project(&conn, "proj-b");
    setup(
        &conn,
        &workflow(),
        Some("proj-a"),
        &block("todo-move", BOUND),
    );
    approve_current(&conn, ACTION);
    conn.execute(
        "UPDATE live_pages SET project_id = 'proj-b' WHERE id = ?1",
        [PAGE],
    )
    .unwrap();
    assert!(claim_trusted_launch(&conn, ACTION, &row("T-1")).is_err());
    assert!(!state(&conn, ACTION).active);
}

#[test]
fn a_replaced_quick_api_invalidates_the_approval_through_the_shared_revision() {
    let conn = connection();
    let now = Utc::now();
    let mut api = crate::models::QuickApi {
        id: "qa-move".into(),
        pinned: false,
        name: "Move".into(),
        description: String::new(),
        icon: "🔌".into(),
        project_id: None,
        api_plugin_slug: "tracker".into(),
        api_config_id: "config".into(),
        api_endpoint_path: "/move".into(),
        api_method: Some("POST".into()),
        api_query: None,
        api_path_params: None,
        api_headers: None,
        api_body: None,
        api_extract: None,
        api_pagination: None,
        api_timeout_ms: None,
        api_max_retries: None,
        variables: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_apis::insert_quick_api(&conn, &api).unwrap();
    let mut workflow = workflow();
    workflow.steps[0].step_type = StepType::ApiCall;
    workflow.steps[0].quick_api_id = Some("qa-move".into());
    setup(&conn, &workflow, None, &block("todo-move", BOUND));
    approve_current(&conn, ACTION);
    api.api_endpoint_path = "/delete".into();
    crate::db::quick_apis::update_quick_api(&conn, &api).unwrap();
    // Invalidated at the write itself, before anything reads it.
    assert!(get(&conn, ACTION)
        .unwrap()
        .unwrap()
        .invalidated_at
        .is_some());
    api.api_endpoint_path = "/move".into();
    crate::db::quick_apis::update_quick_api(&conn, &api).unwrap();
    assert_eq!(
        refusal(claim_trusted_launch(&conn, ACTION, &row("T-1"))),
        LivePageActionTrustRefusal::Changed
    );
    // A deleted dependency is refused, not silently skipped.
    conn.execute("DELETE FROM quick_apis WHERE id = 'qa-move'", [])
        .unwrap();
    assert_eq!(
        eligibility(&conn),
        Some(LivePageActionTrustRefusal::TargetMissing)
    );
}

/// Codex r2 P2: a pin change in the same write as a content change still counts.
#[test]
fn a_pin_change_never_hides_a_content_or_enabled_change() {
    let conn = connection();
    let original = workflow();
    setup(&conn, &original, None, &block("todo-move", BOUND));

    approve_current(&conn, ACTION);
    let mut changed = original.clone();
    changed.steps[0].json_data_payload = Some(serde_json::json!({"column": "done"}));
    changed.pinned = true;
    crate::db::workflows::update_workflow(&conn, &changed).unwrap();
    crate::db::workflows::update_workflow(&conn, &original).unwrap();
    assert_eq!(
        get(&conn, ACTION).unwrap().unwrap().invalidated_reason,
        Some(LivePageActionTrustRefusal::Changed)
    );

    approve_current(&conn, ACTION);
    let mut disabled = original.clone();
    disabled.enabled = false;
    disabled.pinned = true;
    crate::db::workflows::update_workflow(&conn, &disabled).unwrap();
    crate::db::workflows::update_workflow(&conn, &original).unwrap();
    assert_eq!(
        get(&conn, ACTION).unwrap().unwrap().invalidated_reason,
        Some(LivePageActionTrustRefusal::WorkflowDisabled)
    );

    // A pure pin toggle, through the same full write, does not.
    approve_current(&conn, ACTION);
    let mut pinned = original.clone();
    pinned.pinned = true;
    crate::db::workflows::update_workflow(&conn, &pinned).unwrap();
    crate::db::workflows::update_workflow(&conn, &original).unwrap();
    assert!(get(&conn, ACTION)
        .unwrap()
        .unwrap()
        .invalidated_at
        .is_none());
}

/// Every column but `pinned` and `updated_at` is compared by the write triggers,
/// so a column added later cannot slip past them unnoticed.
#[test]
fn the_write_triggers_compare_every_content_column() {
    let conn = connection();
    for (table, trigger) in [
        ("workflows", "trg_trust_workflow_update"),
        ("quick_apis", "trg_trust_quick_api_update"),
    ] {
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        let mut statement = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap();
        let columns: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for column in columns {
            let exempt = column == "pinned" || column == "updated_at";
            let compared = sql.contains(&format!("OLD.{column} IS NEW.{column}"));
            assert_eq!(compared, !exempt, "{table}.{column} in {trigger}");
        }
    }
}

/// Codex r2 P2: a literal suggestion is scrubbed from the stored claim; the
/// admission compares it the same way and admits an unchanged action.
#[test]
fn an_unchanged_literal_suggestion_is_admitted() {
    let conn = connection();
    let literal = r#"[{"name":"ticket","provenance":"agent_suggestion","value":"T-1","suggested_by":"@claude-cli"}]"#;
    setup(&conn, &workflow(), None, &block("todo-move", literal));
    approve_current(&conn, ACTION);
    let launch = claimed(claim_trusted_launch(&conn, ACTION, &HashMap::new()));
    assert!(
        launch.values[0].value.is_none(),
        "the claim stores no runtime value"
    );
    assert_eq!(
        admit_run(
            &conn,
            &launch.id,
            &stored_workflow(&conn, "wf-move"),
            &Default::default()
        )
        .unwrap(),
        Ok(())
    );
}

/// Codex r2 contract: only step types known to run no agent are eligible.
#[test]
fn step_types_are_refused_unless_known_to_run_no_agent() {
    for unknown in ["DelegateSubtasks", "SomeFutureStep", ""] {
        assert!(!agentless_step_type(unknown), "{unknown}");
    }
    let refused = [
        StepType::Agent,
        StepType::BatchQuickPrompt,
        StepType::SubWorkflow,
        StepType::TriggerWorkflow,
    ];
    for step_type in refused {
        let mut step = workflow().steps[0].clone();
        step.step_type = step_type.clone();
        assert!(step_needs_agent(&step), "{step_type:?}");
    }
    for name in AGENTLESS_STEP_TYPES {
        let step_type: StepType = serde_json::from_value(serde_json::json!({ "type": name }))
            .unwrap_or_else(|_| panic!("{name} is a real step type"));
        let mut step = workflow().steps[0].clone();
        step.step_type = step_type;
        step.quick_prompt_id = None;
        assert!(!step_needs_agent(&step), "{name}");
    }
}

// ─── KT-920 — the repository profile is part of what was approved ────────

fn lint_profile(command: &str, commit: &str) -> ProfileSnapshots {
    BTreeMap::from([(
        String::new(),
        crate::core::project_profile::ProfileSnapshot {
            git_ref: Some("refs/heads/main".into()),
            commit: Some(commit.into()),
            state: crate::core::project_profile::SnapshotState::Loaded,
            error: None,
            values: BTreeMap::from([(
                "project.validation.targets.lint.command".to_string(),
                command.to_string(),
            )]),
        },
    )])
}

fn state_with(conn: &Connection, profiles: &ProfileSnapshots) -> LivePageActionTrustState {
    list_for_page_in(conn, PAGE, ProfileView::Fresh(profiles))
        .unwrap()
        .into_iter()
        .find(|state| state.action_id == ACTION)
        .expect("the offer is listed")
}

fn approve_with(conn: &Connection, profiles: &ProfileSnapshots) -> LivePageActionTrust {
    let fingerprint = state_with(conn, profiles).fingerprint.expect("eligible");
    approve_with_profiles(conn, ACTION, &fingerprint, profiles).unwrap()
}

#[test]
fn a_changed_profile_command_invalidates_the_approval_and_a_new_commit_does_not() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_with(&conn, &lint_profile("make lint", "c1"));

    // A revalidation inside a write transaction compares the approved snapshot.
    revalidate_page(&conn, PAGE).unwrap();
    assert!(get(&conn, ACTION)
        .unwrap()
        .unwrap()
        .invalidated_at
        .is_none());
    assert!(state_with(&conn, &lint_profile("make lint", "c2")).active);

    let changed = state_with(&conn, &lint_profile("make lint && curl evil", "c3"));
    assert!(!changed.active);
    assert_eq!(
        changed.trust.unwrap().invalidated_reason,
        Some(LivePageActionTrustRefusal::Changed)
    );
    assert!(
        !state_with(&conn, &lint_profile("make lint", "c4")).active,
        "invalidated for good"
    );
}

#[test]
fn a_run_is_admitted_only_with_the_profile_its_approval_saw() {
    let conn = connection();
    setup(&conn, &workflow(), None, &block("todo-move", BOUND));
    approve_with(&conn, &lint_profile("make lint", "c1"));
    let snapshot = stored_workflow(&conn, "wf-move");

    let launch = claimed(claim_trusted_launch(&conn, ACTION, &row("T-1")));
    assert_eq!(
        admit_run(
            &conn,
            &launch.id,
            &snapshot,
            &lint_profile("make lint", "c2")
        )
        .unwrap(),
        Ok(()),
        "same values on a new commit"
    );
    let launch = claimed(claim_trusted_launch(&conn, ACTION, &row("T-2")));
    assert_eq!(
        admit_run(
            &conn,
            &launch.id,
            &snapshot,
            &lint_profile("make lint && curl evil", "c3")
        )
        .unwrap(),
        Err(LivePageActionTrustRefusal::Changed)
    );
}
