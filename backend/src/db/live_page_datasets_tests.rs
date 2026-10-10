use super::*;
use crate::models::{
    LivePageWrite, LivePageWriteOperation, PublishPageDataConfig, PublishPageDataWrite, StepType,
};

fn connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn
}

const HTML: &str = "<script>draw(data.datasets.summary)</script><p>summary_v2</p>";

/// A Page `stats` (renamed from `old-stats`) with a snapshot `summary`, a
/// time series `traffic` of three points and a collection `alerts`.
fn page(conn: &Connection) {
    let now = Utc::now();
    let page = LivePage {
        id: "page-1".into(),
        project_id: None,
        title: "Stats".into(),
        slug: "old-stats".into(),
        current_revision_id: "rev-1".into(),
        data_revision: 0,
        created_at: now,
        updated_at: now,
        last_published_at: None,
        pinned: false,
        archived: false,
    };
    let revision = LivePageRevision {
        id: "rev-1".into(),
        page_id: page.id.clone(),
        revision: 1,
        html: HTML.into(),
        created_by_agent: None,
        created_at: now,
    };
    let dataset = |name: &str, kind| CreateLivePageDataset {
        name: name.into(),
        kind,
        initial: None,
        schema: None,
        max_points: None,
        max_age_days: None,
    };
    create_live_page(
        conn,
        &page,
        &revision,
        &[
            dataset("summary", LivePageDatasetKind::Snapshot),
            dataset("traffic", LivePageDatasetKind::TimeSeries),
            dataset("alerts", LivePageDatasetKind::Collection),
        ],
        None,
    )
    .unwrap();
    update_live_page(
        conn,
        "page-1",
        &crate::models::UpdateLivePageRequest {
            title: None,
            slug: Some("stats".into()),
            pinned: None,
            archived: None,
        },
    )
    .unwrap();
    for (dataset, value) in [
        ("summary", serde_json::json!({"count": 3})),
        ("alerts", serde_json::json!([{"id": 1}])),
    ] {
        publish(conn, dataset, LivePageWriteOperation::Replace, value);
    }
    for days_ago in [10, 5, 0] {
        let write = LivePageWrite {
            dataset: "traffic".into(),
            operation: LivePageWriteOperation::Append,
            value: serde_json::json!({"hits": days_ago}),
            observed_at: Some(now - chrono::Duration::days(days_ago)),
            dedupe_key: Some(format!("d{days_ago}")),
            key_field: None,
        };
        publish_live_page(conn, "page-1", &request(write)).unwrap();
    }
}

fn request(write: LivePageWrite) -> PublishLivePageRequest {
    PublishLivePageRequest {
        workflow_id: None,
        workflow_run_id: None,
        writes: vec![write],
    }
}

fn publish(
    conn: &Connection,
    dataset: &str,
    operation: LivePageWriteOperation,
    value: serde_json::Value,
) -> PublishLivePageResult {
    let write = LivePageWrite {
        dataset: dataset.into(),
        operation,
        value,
        observed_at: None,
        dedupe_key: None,
        key_field: None,
    };
    publish_live_page(conn, "page-1", &request(write)).unwrap()
}

/// A workflow whose PublishPageData step writes `dataset` into `target`.
pub(crate) fn writer(conn: &Connection, id: &str, target: &str, dataset: &str) {
    let mut workflow = crate::db::tests::sample_workflow(id);
    workflow.name = format!("Feed {id}");
    workflow.steps[0].step_type = StepType::PublishPageData;
    workflow.steps[0].page_publish = Some(PublishPageDataConfig {
        page_id: target.into(),
        writes: vec![PublishPageDataWrite {
            dataset: dataset.into(),
            operation: LivePageWriteOperation::Replace,
            value_from: "steps.data.data".into(),
            observed_at: None,
            dedupe_key: None,
            key_field: None,
        }],
    });
    crate::db::workflows::insert_workflow(conn, &workflow).unwrap();
}

fn dataset(conn: &Connection, name: &str) -> Option<LivePageDatasetView> {
    get_live_page(conn, "page-1")
        .unwrap()
        .unwrap()
        .datasets
        .into_iter()
        .find(|view| view.dataset.name == name)
}

fn point_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM live_page_dataset_points", [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn an_unused_dataset_is_deleted_with_its_points_and_the_page_moves_on() {
    let conn = connection();
    page(&conn);
    let before = get_live_page(&conn, "page-1")
        .unwrap()
        .unwrap()
        .page
        .data_revision;
    assert_eq!(point_count(&conn), 3);

    let deleted = delete_live_page_dataset(&conn, "stats", "traffic", false).unwrap();

    assert_eq!(deleted.page_id, "page-1");
    assert!(!deleted.overridden.is_referenced());
    assert_eq!(deleted.data_revision, before + 1);
    assert!(dataset(&conn, "traffic").is_none());
    assert_eq!(point_count(&conn), 0);
    let missing = delete_live_page_dataset(&conn, "page-1", "traffic", false).unwrap_err();
    assert!(missing.is::<DatasetNotFound>());
    let unknown_page = delete_live_page_dataset(&conn, "nope", "alerts", false).unwrap_err();
    assert_eq!(unknown_page.to_string(), "Page not found");
}

#[test]
fn a_dataset_a_workflow_writes_is_refused_with_the_workflows_unless_forced() {
    let conn = connection();
    page(&conn);
    // Targets by id, live slug and retired slug all count.
    writer(&conn, "wf-id", "page-1", "alerts");
    writer(&conn, "wf-alias", "old-stats", "alerts");
    writer(&conn, "wf-other", "another-page", "alerts");

    let refused = delete_live_page_dataset(&conn, "page-1", "alerts", false).unwrap_err();
    let usage = &refused.downcast_ref::<DatasetReferenced>().unwrap().0;
    let ids = usage
        .writers
        .iter()
        .map(|writer| writer.workflow_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["wf-alias", "wf-id"]);
    assert!(refused.to_string().contains("'Feed wf-id' (wf-id)"));
    assert!(
        dataset(&conn, "alerts").is_some(),
        "a refusal deletes nothing"
    );

    let forced = delete_live_page_dataset(&conn, "page-1", "alerts", true).unwrap();
    assert_eq!(forced.overridden.writers.len(), 2);
    assert!(dataset(&conn, "alerts").is_none());
}

#[test]
fn a_dataset_only_a_rollback_step_writes_is_refused() {
    let conn = connection();
    page(&conn);
    let mut workflow = crate::db::tests::sample_workflow("wf-rollback");
    workflow.name = "Rollback".into();
    let mut rollback = workflow.steps[0].clone();
    rollback.name = "restore".into();
    rollback.step_type = StepType::PublishPageData;
    rollback.page_publish = Some(PublishPageDataConfig {
        page_id: "page-1".into(),
        writes: vec![PublishPageDataWrite {
            dataset: "alerts".into(),
            operation: LivePageWriteOperation::Clear,
            value_from: String::new(),
            observed_at: None,
            dedupe_key: None,
            key_field: None,
        }],
    });
    workflow.on_failure = vec![rollback];
    assert!(workflow
        .steps
        .iter()
        .all(|step| step.page_publish.is_none()));
    crate::db::workflows::insert_workflow(&conn, &workflow).unwrap();

    let usage = list_live_page_dataset_usage(&conn, "page-1")
        .unwrap()
        .unwrap();
    let alerts = usage.iter().find(|usage| usage.name == "alerts").unwrap();
    assert_eq!(alerts.writers[0].workflow_id, "wf-rollback");
    let refused = delete_live_page_dataset(&conn, "page-1", "alerts", false).unwrap_err();
    assert!(refused.is::<DatasetReferenced>());
    assert!(dataset(&conn, "alerts").is_some(), "the dataset is kept");
}

#[test]
fn a_dataset_the_html_names_or_a_button_binds_is_refused() {
    let conn = connection();
    page(&conn);
    let refused = delete_live_page_dataset(&conn, "page-1", "summary", false).unwrap_err();
    let usage = &refused.downcast_ref::<DatasetReferenced>().unwrap().0;
    assert!(usage.html_referenced && usage.writers.is_empty());

    conn.execute(
        "INSERT INTO live_page_actions (id, live_page_id, live_page_revision_id, action_ref,
             kind, target_id, target_name, values_json, created_at, updated_at)
         VALUES ('a-1', 'page-1', 'rev-1', 'move', 'workflow', 'wf', 'wf', ?1, 'now', 'now')",
        [r#"[{"name":"t","provenance":"dynamic_binding","source_ref":"<page.dataset.alerts.find(id).id>"}]"#],
    )
    .unwrap();
    let refused = delete_live_page_dataset(&conn, "page-1", "alerts", false).unwrap_err();
    let usage = &refused.downcast_ref::<DatasetReferenced>().unwrap().0;
    assert_eq!(usage.action_refs, ["move"]);
    assert!(!usage.html_referenced);
    assert!(refused.to_string().contains("bound by Page button move"));
}

#[test]
fn the_html_reference_is_a_whole_name() {
    assert!(html_names("data.datasets.summary)", "summary"));
    assert!(html_names("datasets['summary']", "summary"));
    assert!(!html_names("summary_v2 summary-old my_summary", "summary"));
    assert!(html_names("é summary é", "summary"));
}

#[test]
fn the_usage_lists_every_dataset_with_its_references() {
    let conn = connection();
    page(&conn);
    writer(&conn, "wf-1", "stats", "traffic");
    let usage = list_live_page_dataset_usage(&conn, "page-1")
        .unwrap()
        .unwrap();
    let by_name = |name: &str| usage.iter().find(|usage| usage.name == name).unwrap();
    assert_eq!(usage.len(), 3);
    assert!(!by_name("alerts").is_referenced());
    assert!(by_name("summary").html_referenced);
    assert_eq!(by_name("traffic").writers[0].workflow_name, "Feed wf-1");
    assert!(list_live_page_dataset_usage(&conn, "nope")
        .unwrap()
        .is_none());
}

#[test]
fn clear_empties_every_kind() {
    let conn = connection();
    page(&conn);

    let cleared = publish(
        &conn,
        "traffic",
        LivePageWriteOperation::Clear,
        serde_json::Value::Null,
    );
    assert_eq!(cleared.points_removed, 3);
    assert!(cleared.content_changed);
    assert!(dataset(&conn, "traffic").unwrap().points.is_empty());

    publish(
        &conn,
        "summary",
        LivePageWriteOperation::Clear,
        serde_json::Value::Null,
    );
    assert_eq!(dataset(&conn, "summary").unwrap().dataset.current, None);

    publish(
        &conn,
        "alerts",
        LivePageWriteOperation::Clear,
        serde_json::Value::Null,
    );
    let alerts = dataset(&conn, "alerts").unwrap();
    assert_eq!(alerts.dataset.current, Some(serde_json::json!([])));
    assert_eq!(alerts.data_size_bytes, 2);

    // Clearing what is already empty changes nothing.
    let again = publish(
        &conn,
        "traffic",
        LivePageWriteOperation::Clear,
        serde_json::Value::Null,
    );
    assert!(!again.content_changed);
    assert_eq!(again.points_removed, 0);
}

#[test]
fn only_clear_may_omit_the_value_on_the_wire() {
    let parse = |operation: &str, value: Option<serde_json::Value>| {
        let mut write = serde_json::json!({"dataset": "traffic", "operation": operation});
        if let Some(value) = value {
            write["value"] = value;
        }
        serde_json::from_value::<LivePageWrite>(write)
    };
    let clear = parse("clear", None).unwrap();
    assert_eq!(clear.operation, LivePageWriteOperation::Clear);
    assert!(clear.value.is_null());
    for operation in ["replace", "append", "upsert"] {
        let error = parse(operation, None).unwrap_err();
        assert!(
            error.to_string().contains("missing field `value`"),
            "{operation}: {error}"
        );
        // An explicit null stays a value, as before.
        assert!(parse(operation, Some(serde_json::Value::Null))
            .unwrap()
            .value
            .is_null());
        assert_eq!(
            parse(operation, Some(serde_json::json!([1])))
                .unwrap()
                .value,
            serde_json::json!([1])
        );
    }
    let request: Result<PublishLivePageRequest, _> = serde_json::from_value(serde_json::json!({
        "workflow_id": null, "workflow_run_id": null,
        "writes": [{"dataset": "summary", "operation": "replace"}]
    }));
    assert!(
        request.is_err(),
        "an omitted value never becomes a null snapshot"
    );
}

#[test]
fn new_limits_prune_the_existing_points_at_once() {
    let conn = connection();
    page(&conn);
    let limits = |max_points, max_age_days| UpdateLivePageDatasetRequest {
        max_points,
        max_age_days,
    };

    let by_age =
        update_live_page_dataset_limits(&conn, "page-1", "traffic", &limits(None, Some(Some(7))))
            .unwrap();
    assert_eq!(by_age.points_removed, 1);
    assert_eq!(by_age.dataset.max_age_days, Some(7));
    assert_eq!(by_age.dataset.max_points, 50_000);

    let by_count =
        update_live_page_dataset_limits(&conn, "stats", "traffic", &limits(Some(1), None)).unwrap();
    assert_eq!(by_count.points_removed, 1);
    assert_eq!(by_count.data_revision, by_age.data_revision + 1);
    let traffic = dataset(&conn, "traffic").unwrap();
    assert_eq!(traffic.points.len(), 1);
    assert_eq!(traffic.points[0].payload, serde_json::json!({"hits": 0}));
    assert_eq!(
        (traffic.dataset.max_points, traffic.dataset.max_age_days),
        (1, Some(7))
    );

    let lifted =
        update_live_page_dataset_limits(&conn, "page-1", "traffic", &limits(None, Some(None)))
            .unwrap();
    assert_eq!(lifted.points_removed, 0);
    assert_eq!(
        lifted.data_revision, by_count.data_revision,
        "nothing pruned, no new revision"
    );
    assert_eq!(
        dataset(&conn, "traffic").unwrap().dataset.max_age_days,
        None
    );

    for invalid in [limits(Some(0), None), limits(None, Some(Some(0)))] {
        let error =
            update_live_page_dataset_limits(&conn, "page-1", "traffic", &invalid).unwrap_err();
        assert!(error.to_string().contains("greater than zero"));
    }
    let missing = update_live_page_dataset_limits(&conn, "page-1", "nope", &limits(Some(5), None))
        .unwrap_err();
    assert!(missing.is::<DatasetNotFound>());
}

#[test]
fn the_limits_request_tells_absent_from_null() {
    let absent: UpdateLivePageDatasetRequest = serde_json::from_str("{}").unwrap();
    assert_eq!(absent.max_age_days, None);
    let null: UpdateLivePageDatasetRequest =
        serde_json::from_str(r#"{"max_age_days": null}"#).unwrap();
    assert_eq!(null.max_age_days, Some(None));
}

#[test]
fn deleting_clearing_or_pruning_keeps_an_approved_button_trusted() {
    use crate::db::live_page_action_trusts::{self as trusts, tests as fixture};
    let conn = connection();
    fixture::setup(
        &conn,
        &fixture::workflow(),
        None,
        &fixture::block("todo-move", fixture::BOUND),
    );
    conn.execute(
        "INSERT INTO live_page_datasets (id, page_id, name, kind, current_json, schema_json,
             max_points, max_age_days, updated_at)
         VALUES ('ds-dead', ?1, 'dead', 'time_series', NULL, NULL, 50000, NULL, ?2)",
        [fixture::PAGE, &Utc::now().to_rfc3339()],
    )
    .unwrap();
    let state = || {
        trusts::list_for_page(&conn, fixture::PAGE)
            .unwrap()
            .remove(0)
    };
    let fingerprint = state().fingerprint.unwrap();
    trusts::approve(&conn, &state().action_id, &fingerprint).unwrap();
    assert!(state().active);

    // The bound dataset is a reference: deleting it needs force.
    let refused = delete_live_page_dataset(&conn, fixture::PAGE, "todo", false).unwrap_err();
    assert_eq!(
        refused
            .downcast_ref::<DatasetReferenced>()
            .unwrap()
            .0
            .action_refs,
        ["todo-move"]
    );
    delete_live_page_dataset(&conn, fixture::PAGE, "dead", false).unwrap();
    let write = LivePageWrite {
        dataset: "todo".into(),
        operation: LivePageWriteOperation::Clear,
        value: serde_json::Value::Null,
        observed_at: None,
        dedupe_key: None,
        key_field: None,
    };
    publish_live_page(&conn, fixture::PAGE, &request(write)).unwrap();
    update_live_page_dataset_limits(
        &conn,
        fixture::PAGE,
        "todo",
        &UpdateLivePageDatasetRequest {
            max_points: Some(10),
            max_age_days: None,
        },
    )
    .unwrap();

    let after = state();
    assert!(after.active, "the approval survives: {after:?}");
    assert_eq!(after.fingerprint.as_deref(), Some(fingerprint.as_str()));
}
