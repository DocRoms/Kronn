use super::*;

use crate::models::{
    AgentType, CollectQuickExecOutputFormat, ModelTier, PromptVariable, PromptVariableSource,
    QuickApi, QuickExec, QuickPrompt,
};

pub(super) fn connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn
}

pub(super) fn insert_page(conn: &Connection, page_id: &str, revision_id: &str, html: &str) {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO live_pages (
                 id, project_id, title, slug, current_revision_id, data_revision,
                 created_at, updated_at, last_published_at, pinned, archived
             ) VALUES (?1, NULL, 'Test Page', ?1, ?2, 0, ?3, ?3, NULL, 0, 0)",
        params![page_id, revision_id, now],
    )
    .unwrap();
    conn.execute(
            "INSERT INTO live_page_revisions (id, page_id, revision, html, created_by_agent, created_at)
             VALUES (?1, ?2, 1, ?3, NULL, ?4)",
            params![revision_id, page_id, html, now],
        )
        .unwrap();
}

fn insert_dataset(conn: &Connection, page_id: &str, name: &str, kind: &str, current_json: &str) {
    let now = Utc::now().to_rfc3339();
    conn.execute(
            "INSERT INTO live_page_datasets (
                 id, page_id, name, kind, current_json, schema_json, max_points, max_age_days, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 50000, NULL, ?6)",
            params![format!("ds-{page_id}-{name}"), page_id, name, kind, current_json, now],
        )
        .unwrap();
}

pub(super) fn action_block(action_ref: &str, json: &str) -> String {
    format!(
        r#"<div><button data-kronn-action="{action_ref}">Go</button>
<script type="application/kronn-action" data-action-id="{action_ref}">{json}</script></div>"#
    )
}

pub(super) fn insert_target(conn: &Connection) {
    let now = Utc::now();
    crate::db::quick_execs::insert_quick_exec(
        conn,
        &QuickExec {
            approved_by_trust: None,
            id: "qe-1".into(),
            name: "Collect logs".into(),
            icon: "⌨".into(),
            description: String::new(),
            project_id: None,
            command: "printf".into(),
            args: vec![],
            timeout_secs: 30,
            output_format: CollectQuickExecOutputFormat::Json,
            variables: vec![PromptVariable {
                name: "service".into(),
                label: "Service".into(),
                placeholder: String::new(),
                description: None,
                required: true,
                pattern: None,
                source: None,
                source_ref: None,
                allow_manual_override: false,
                control: None,
            }],
            pinned: false,
            created_at: now,
            updated_at: now,
            unmodelled_args_approved: None,
            agent_written: None,
        },
    )
    .unwrap();
}

#[test]
fn a_launch_whose_run_was_interrupted_at_boot_frees_its_row() {
    let conn = connection();
    insert_target(&conn);
    let html = action_block(
        "implem",
        r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).key>"}]}"#,
    );
    insert_page(&conn, "page-autocode", "rev-1", &html);
    insert_dataset(
        &conn,
        "page-autocode",
        "tickets",
        "collection",
        r#"[{"key":"EW-7633"}]"#,
    );
    ingest_page_actions(&conn, "page-autocode", "rev-1", &html).unwrap();
    crate::db::workflows::insert_workflow(&conn, &crate::db::tests::sample_workflow("w-implem"))
        .unwrap();
    let mut run = crate::db::tests::sample_run("run-implem", "w-implem");
    run.status = crate::models::RunStatus::Running;
    crate::db::workflows::insert_run(&conn, &run).unwrap();
    let click = || {
        claim_launch(
            &conn,
            "page-action:page-autocode:implem",
            &HashMap::new(),
            &HashMap::from([("service".into(), "EW-7633".into())]),
        )
        .unwrap()
        .unwrap()
    };
    let LivePageActionClaimOutcome::Claimed { action: first, .. } = click() else {
        panic!("the first click launches");
    };
    complete(
        &conn,
        &first.id,
        kronn_action_engine::ActionCompletion {
            state: DiscussionActionState::Running,
            shared_run_id: Some("run-implem".into()),
            result_discussion_id: None,
            deep_link: None,
            diagnostic: None,
        },
    )
    .unwrap();
    assert!(
        matches!(click(), LivePageActionClaimOutcome::Existing(ref running) if running.id == first.id)
    );

    // A backend restart interrupts the run.
    crate::db::workflows::reconcile_stale_runs(&conn, 0).unwrap();

    let LivePageActionClaimOutcome::Claimed { action: second, .. } = click() else {
        panic!("an interrupted run must not block its row");
    };
    assert_ne!(second.id, first.id);
    let settled = get(kronn_action_engine::Reconcile::Persisted, &conn, &first.id)
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, DiscussionActionState::Failed);
}
