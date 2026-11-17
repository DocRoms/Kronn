//! KT-1819 — an open Page reads what changed, not every dataset.
use axum::extract::{Path, Query, State};
use axum::Json;

use super::*;
use crate::models::{
    LivePageDataDelta, LivePageDetail, LivePageWrite, LivePageWriteOperation,
    PublishLivePageResult, UpdateLivePageDatasetRequest,
};

const HEAVY: &str = "heavy-value-that-must-not-travel";

/// A Page `board` with snapshots `summary` and `heavy` and a time series
/// `traffic` of three points.
async fn seeded() -> (AppState, LivePageDetail) {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let request: CreateLivePageRequest = serde_json::from_value(serde_json::json!({
        "title": "Board", "slug": "board", "html": "<p>board-html</p>",
        "datasets": [
            {"name": "summary", "kind": "snapshot", "initial": {"count": 0}},
            {"name": "heavy", "kind": "snapshot", "initial": {"blob": HEAVY}},
            {"name": "traffic", "kind": "time_series", "initial": [1, 2, 3]}
        ]
    }))
    .unwrap();
    let Json(created) = create(State(state.clone()), None, Json(request)).await;
    assert!(created.success, "{:?}", created.error);
    (state, created.data.unwrap())
}

fn write(
    dataset: &str,
    operation: LivePageWriteOperation,
    value: serde_json::Value,
) -> LivePageWrite {
    LivePageWrite {
        dataset: dataset.into(),
        operation,
        value,
        observed_at: None,
        dedupe_key: None,
        key_field: None,
    }
}

/// One point appended under a dedupe key: sent twice, it lands once.
fn keyed_point(key: &str, value: serde_json::Value) -> LivePageWrite {
    LivePageWrite {
        dedupe_key: Some(key.into()),
        ..write("traffic", LivePageWriteOperation::Append, value)
    }
}

async fn publish_writes(state: &AppState, writes: Vec<LivePageWrite>) -> PublishLivePageResult {
    let Json(published) = publish(
        State(state.clone()),
        Path("board".into()),
        Json(PublishLivePageRequest {
            workflow_id: None,
            workflow_run_id: None,
            writes,
        }),
    )
    .await;
    assert!(published.success, "{:?}", published.error);
    published.data.unwrap()
}

async fn replace(state: &AppState, writes: &[(&str, serde_json::Value)]) -> PublishLivePageResult {
    publish_writes(
        state,
        writes
            .iter()
            .map(|(dataset, value)| write(dataset, LivePageWriteOperation::Replace, value.clone()))
            .collect(),
    )
    .await
}

async fn declare(state: &AppState, dataset: serde_json::Value) {
    let Json(added) = add_dataset(
        State(state.clone()),
        Path("board".into()),
        Json(serde_json::from_value(dataset).unwrap()),
    )
    .await;
    assert!(added.success, "{:?}", added.error);
}

/// `updated_at` of every dataset, by name: a reader that skips a dataset
/// must still hold this field of it.
fn dataset_dates(detail: &LivePageDetail) -> Vec<(String, chrono::DateTime<chrono::Utc>)> {
    detail
        .datasets
        .iter()
        .map(|view| (view.dataset.name.clone(), view.dataset.updated_at))
        .collect()
}

async fn read(state: &AppState, since: Option<&str>, revision: Option<&str>) -> LivePageDataDelta {
    let Json(response) = data(
        State(state.clone()),
        Path("board".into()),
        Query(LivePageDataQuery {
            since: since.map(str::to_owned),
            revision: revision.map(str::to_owned),
        }),
    )
    .await;
    assert!(response.success, "{:?}", response.error);
    response.data.unwrap()
}

async fn detail(state: &AppState) -> LivePageDetail {
    let Json(response) = get(State(state.clone()), Path("board".into())).await;
    response.data.unwrap()
}

fn names(delta: &LivePageDataDelta) -> Vec<&str> {
    delta
        .datasets
        .iter()
        .map(|view| view.dataset.name.as_str())
        .collect()
}

/// The full state: the whole Page, HTML and every dataset.
fn assert_full(delta: &LivePageDataDelta, why: &str) {
    let full = delta.full.as_ref().unwrap_or_else(|| panic!("{why}"));
    assert_eq!(full.page.data_revision, delta.page.data_revision, "{why}");
    assert!(delta.datasets.is_empty(), "{why}");
}

#[tokio::test]
async fn a_publication_that_changes_one_dataset_is_read_alone() {
    let (state, created) = seeded().await;
    let published = replace(
        &state,
        &[
            ("summary", serde_json::json!({"count": 1})),
            ("heavy", serde_json::json!({"blob": HEAVY})),
        ],
    )
    .await;
    assert_eq!(published.data_revision, 1);
    assert_eq!(published.changed_datasets, vec!["summary"]);

    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;

    assert!(delta.full.is_none(), "the journal covers revision 1");
    assert_eq!(delta.page.data_revision, 1);
    assert_eq!(names(&delta), vec!["summary"]);
    assert_eq!(
        delta.datasets[0].dataset.current,
        Some(serde_json::json!({"count": 1}))
    );
    let wire = serde_json::to_string(&delta).unwrap();
    assert!(!wire.contains(HEAVY), "an unchanged dataset stays home");
    assert!(!wire.contains("board-html"), "the HTML stays home");
}

#[tokio::test]
async fn several_missed_publications_add_up_and_points_come_whole() {
    let (state, created) = seeded().await;
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;
    let Json(appended) = publish(
        State(state.clone()),
        Path("board".into()),
        Json(PublishLivePageRequest {
            workflow_id: None,
            workflow_run_id: None,
            writes: vec![LivePageWrite {
                dataset: "traffic".into(),
                operation: LivePageWriteOperation::Append,
                value: serde_json::json!(4),
                observed_at: None,
                dedupe_key: None,
                key_field: None,
            }],
        }),
    )
    .await;
    assert_eq!(appended.data.unwrap().data_revision, 2);

    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;
    assert!(delta.full.is_none());
    assert_eq!(names(&delta), vec!["summary", "traffic"]);
    assert_eq!(delta.datasets[1].points.len(), 4, "the whole series");

    let last = read(&state, Some("1"), Some(&created.revision.id)).await;
    assert_eq!(names(&last), vec!["traffic"]);
}

#[tokio::test]
async fn a_publication_that_changes_nothing_keeps_the_revision_and_stays_silent() {
    let (state, _) = seeded().await;
    declare(
        &state,
        serde_json::json!({"name": "items", "kind": "collection"}),
    )
    .await;
    let upsert = || LivePageWrite {
        key_field: Some("id".into()),
        ..write(
            "items",
            LivePageWriteOperation::Upsert,
            serde_json::json!([{"id": 1, "label": "one"}]),
        )
    };
    let first = publish_writes(
        &state,
        vec![
            write(
                "summary",
                LivePageWriteOperation::Replace,
                serde_json::json!({"count": 1}),
            ),
            upsert(),
            keyed_point("run-1", serde_json::json!(4)),
        ],
    )
    .await;
    assert_eq!(first.data_revision, 2, "the declaration, then the data");
    let before = detail(&state).await;
    let mut events = state.ws_broadcast.subscribe();

    let unchanged = publish_writes(
        &state,
        vec![
            write(
                "summary",
                LivePageWriteOperation::Replace,
                serde_json::json!({"count": 1}),
            ),
            write(
                "heavy",
                LivePageWriteOperation::Replace,
                serde_json::json!({"blob": HEAVY}),
            ),
            upsert(),
            keyed_point("run-1", serde_json::json!(4)),
        ],
    )
    .await;

    assert!(!unchanged.content_changed);
    assert_eq!(
        unchanged.unchanged_datasets,
        vec!["summary", "heavy", "items", "traffic"]
    );
    assert_eq!(unchanged.data_revision, 2, "the caller reads the revision");
    assert!(events.try_recv().is_err(), "no page-data event");
    let after = detail(&state).await;
    assert_eq!(after.page.data_revision, 2);
    assert!(
        after.page.last_published_at > before.page.last_published_at,
        "the refresh is still dated"
    );
    assert_eq!(before.datasets.len(), 4);
    assert_eq!(dataset_dates(&before), dataset_dates(&after));
    let Json(journal) = publications(State(state.clone()), Path("board".into())).await;
    let journal = journal.data.unwrap();
    assert_eq!(journal.len(), 1, "only the publication that changed data");
    assert_eq!(journal[0].data_revision, 2);
}

#[tokio::test]
async fn a_clear_or_a_deduplicated_append_that_changes_nothing_reads_nothing() {
    let (state, created) = seeded().await;
    let clear = || {
        write(
            "summary",
            LivePageWriteOperation::Clear,
            serde_json::Value::Null,
        )
    };
    let emptied = publish_writes(
        &state,
        vec![clear(), keyed_point("run-1", serde_json::json!(4))],
    )
    .await;
    assert_eq!(emptied.changed_datasets, vec!["summary", "traffic"]);
    let before = detail(&state).await;
    let mut events = state.ws_broadcast.subscribe();

    let again = publish_writes(
        &state,
        vec![clear(), keyed_point("run-1", serde_json::json!(4))],
    )
    .await;

    assert!(!again.content_changed);
    assert_eq!(again.unchanged_datasets, vec!["summary", "traffic"]);
    assert_eq!((again.points_added, again.points_removed), (0, 0));
    assert_eq!(again.data_revision, emptied.data_revision);
    assert!(events.try_recv().is_err(), "no page-data event");
    assert_eq!(dataset_dates(&before), dataset_dates(&detail(&state).await));
    let delta = read(&state, Some("1"), Some(&created.revision.id)).await;
    assert!(delta.full.is_none());
    assert!(delta.datasets.is_empty(), "nothing to read");
}

/// Points age out while nobody publishes: the append that finds them too old
/// removes them, and that alone is a change to read.
#[tokio::test]
async fn an_append_that_only_prunes_is_read_as_a_changed_dataset() {
    let (state, created) = seeded().await;
    let Json(limited) = update_dataset(
        State(state.clone()),
        None,
        None,
        Path(("board".into(), "traffic".into())),
        Json(UpdateLivePageDatasetRequest {
            max_points: None,
            max_age_days: Some(Some(7)),
        }),
    )
    .await;
    assert_eq!(limited.data.unwrap().points_removed, 0);
    let kept = publish_writes(&state, vec![keyed_point("run-1", serde_json::json!(4))]).await;
    let since = kept.data_revision.to_string();
    state
        .db
        .with_conn(|conn| {
            let aged = conn.execute(
                "UPDATE live_page_dataset_points SET observed_at = ?1
                  WHERE dedupe_key IS NOT 'run-1'",
                [(chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339()],
            )?;
            assert_eq!(aged, 3, "the three seeded points");
            Ok(())
        })
        .await
        .unwrap();

    let pruned = publish_writes(&state, vec![keyed_point("run-1", serde_json::json!(4))]).await;

    assert_eq!((pruned.points_added, pruned.points_removed), (0, 3));
    assert_eq!(pruned.changed_datasets, vec!["traffic"]);
    assert_eq!(pruned.data_revision, kept.data_revision + 1);
    let delta = read(&state, Some(&since), Some(&created.revision.id)).await;
    assert!(delta.full.is_none());
    assert_eq!(names(&delta), vec!["traffic"]);
    assert_eq!(delta.datasets[0].points.len(), 1, "the point that is left");
}

#[tokio::test]
async fn two_writes_to_one_dataset_in_one_publication_are_read_once() {
    let (state, created) = seeded().await;
    let count = |value: u32| {
        write(
            "summary",
            LivePageWriteOperation::Replace,
            serde_json::json!({"count": value}),
        )
    };

    // The second write repeats the first: the dataset changed all the same.
    let repeated = publish_writes(&state, vec![count(1), count(1)]).await;

    assert_eq!(repeated.datasets_updated, vec!["summary"]);
    assert_eq!(repeated.changed_datasets, vec!["summary"]);
    assert!(repeated.unchanged_datasets.is_empty());
    assert_eq!(repeated.data_revision, 1);
    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;
    assert_eq!(names(&delta), vec!["summary"]);
    assert_eq!(
        delta.datasets[0].dataset.current,
        Some(serde_json::json!({"count": 1}))
    );

    // Each write builds on the one before it: the last value is the one read.
    let stacked = publish_writes(&state, vec![count(2), count(3)]).await;

    assert_eq!(stacked.data_revision, 2, "one publication, one revision");
    let delta = read(&state, Some("1"), Some(&created.revision.id)).await;
    assert_eq!(names(&delta), vec!["summary"]);
    assert_eq!(
        delta.datasets[0].dataset.current,
        Some(serde_json::json!({"count": 3}))
    );
}

/// The name is back but the dataset is another one: its old value must not
/// survive in a reader that only takes what the journal names.
#[tokio::test]
async fn a_dataset_deleted_then_declared_again_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;
    let Json(deleted) = delete_dataset(
        State(state.clone()),
        None,
        None,
        Path(("board".into(), "heavy".into())),
        Query(DeleteLivePageDatasetQuery { force: false }),
    )
    .await;
    assert!(deleted.success, "{:?}", deleted.error);
    declare(
        &state,
        serde_json::json!({"name": "heavy", "kind": "snapshot"}),
    )
    .await;
    replace(&state, &[("summary", serde_json::json!({"count": 2}))]).await;

    let delta = read(&state, Some("1"), Some(&created.revision.id)).await;

    assert_full(&delta, "heavy was replaced by an empty dataset");
    let full = delta.full.unwrap();
    let heavy = full
        .datasets
        .iter()
        .find(|view| view.dataset.name == "heavy")
        .unwrap();
    assert_eq!(heavy.dataset.current, None);
}

#[tokio::test]
async fn declaring_a_dataset_tells_open_pages_once() {
    let (state, _) = seeded().await;
    let mut events = state.ws_broadcast.subscribe();
    let extra = || serde_json::json!({"name": "extra", "kind": "snapshot"});

    declare(&state, extra()).await;

    match events.try_recv() {
        Ok(crate::models::WsMessage::LivePageDataChanged {
            page_id,
            data_revision,
        }) => {
            let page = detail(&state).await.page;
            assert_eq!((page_id, data_revision), (page.id, page.data_revision));
            assert_eq!(data_revision, 1);
        }
        other => panic!("expected a page-data event, got {other:?}"),
    }

    // Declaring it again changes nothing: no event.
    declare(&state, extra()).await;
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn an_unchanged_revision_reads_no_dataset_value() {
    let (state, created) = seeded().await;
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;

    let delta = read(&state, Some("1"), Some(&created.revision.id)).await;

    assert!(delta.full.is_none());
    assert!(delta.datasets.is_empty());
    assert_eq!(delta.page.data_revision, 1);
    let wire = serde_json::to_string(&delta).unwrap();
    assert!(!wire.contains(HEAVY) && !wire.contains("count"));
}

#[tokio::test]
async fn an_added_dataset_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    let Json(added) = add_dataset(
        State(state.clone()),
        Path("board".into()),
        Json(
            serde_json::from_value(serde_json::json!({"name": "extra", "kind": "snapshot"}))
                .unwrap(),
        ),
    )
    .await;
    assert!(added.success);
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;

    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;

    assert_full(&delta, "a dataset was added since revision 0");
    let full = delta.full.unwrap();
    assert!(full
        .datasets
        .iter()
        .any(|view| view.dataset.name == "extra"));

    // Declaring it again changes nothing, so the Page does not move.
    let revision = detail(&state).await.page.data_revision;
    let Json(again) = add_dataset(
        State(state.clone()),
        Path("board".into()),
        Json(
            serde_json::from_value(serde_json::json!({"name": "extra", "kind": "snapshot"}))
                .unwrap(),
        ),
    )
    .await;
    assert!(again.success);
    assert_eq!(detail(&state).await.page.data_revision, revision);
}

#[tokio::test]
async fn a_deleted_dataset_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    let Json(deleted) = delete_dataset(
        State(state.clone()),
        None,
        None,
        Path(("board".into(), "heavy".into())),
        Query(DeleteLivePageDatasetQuery { force: false }),
    )
    .await;
    assert!(deleted.success, "{:?}", deleted.error);
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;

    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;

    assert_full(&delta, "a dataset was deleted since revision 0");
    assert_eq!(delta.full.unwrap().datasets.len(), 2);
}

#[tokio::test]
async fn changed_retention_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    let Json(limited) = update_dataset(
        State(state.clone()),
        None,
        None,
        Path(("board".into(), "traffic".into())),
        Json(UpdateLivePageDatasetRequest {
            max_points: Some(1),
            max_age_days: None,
        }),
    )
    .await;
    assert_eq!(limited.data.unwrap().points_removed, 2);

    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;

    assert_full(&delta, "retention pruned points since revision 0");
    let full = delta.full.unwrap();
    let traffic = full
        .datasets
        .iter()
        .find(|view| view.dataset.name == "traffic")
        .unwrap();
    assert_eq!(traffic.points.len(), 1);
}

#[tokio::test]
async fn updated_html_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    let Json(updated) = update_html(
        State(state.clone()),
        Path("board".into()),
        Json(
            serde_json::from_value(
                serde_json::json!({"html": "<p>second</p>", "created_by_agent": null}),
            )
            .unwrap(),
        ),
    )
    .await;
    let new_revision = updated.data.unwrap().id;

    let stale = read(&state, Some("0"), Some(&created.revision.id)).await;
    assert_full(&stale, "the reader shows an older HTML revision");
    assert_eq!(stale.full.unwrap().revision.html, "<p>second</p>");

    let current = read(&state, Some("0"), Some(&new_revision)).await;
    assert!(current.full.is_none(), "the reader shows the current HTML");
}

/// The status and the JSON body of `PUT /api/pages/board/html`.
async fn save_html(
    state: &AppState,
    html: &str,
    base: Option<&str>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::response::IntoResponse;
    let response = update_html_route(
        State(state.clone()),
        Path("board".into()),
        Json(UpdateLivePageHtmlRequest {
            html: html.into(),
            created_by_agent: None,
            base_revision_id: base.map(str::to_owned),
        }),
    )
    .await
    .into_response();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn an_html_save_from_a_stale_base_is_refused_and_writes_nothing() {
    let (state, created) = seeded().await;
    let first = created.revision.id.as_str();

    let (status, saved) = save_html(&state, "<p>second</p>", Some(first)).await;
    assert_eq!(status, axum::http::StatusCode::OK, "the base is current");
    let second = saved["data"]["id"].as_str().expect("saved").to_owned();

    // Another editor still holds the first revision.
    let (status, refused) = save_html(&state, "<p>stale</p>", Some(first)).await;

    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(refused["success"], false);
    assert_eq!(refused["error_code"], "conflict");
    assert_eq!(
        refused["current_revision_id"], second,
        "the refusal names the current revision: {refused}"
    );
    assert!(refused["error"].as_str().unwrap().contains(&second));
    let page = detail(&state).await;
    assert_eq!(page.revision.id, second);
    assert_eq!(page.revision.html, "<p>second</p>");
    let Json(history) = revisions(State(state.clone()), Path("board".into())).await;
    assert_eq!(history.data.unwrap().len(), 2, "no revision was written");

    // Saving over it names the revision the user was shown.
    let (status, over) = save_html(&state, "<p>stale</p>", Some(&second)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(detail(&state).await.revision.id, over["data"]["id"]);
}

#[tokio::test]
async fn an_html_save_from_a_base_is_refused_when_the_page_has_no_revision() {
    let (state, created) = seeded().await;
    state
        .db
        .with_conn(|conn| {
            conn.execute("UPDATE live_pages SET current_revision_id = NULL", [])?;
            Ok(())
        })
        .await
        .unwrap();

    let (status, refused) = save_html(&state, "<p>stale</p>", Some(&created.revision.id)).await;

    assert_eq!(status, axum::http::StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error_code"], "conflict");
    assert!(refused["current_revision_id"].is_null(), "{refused}");

    // A save that names no base gives the Page a revision again.
    let (status, saved) = save_html(&state, "<p>back</p>", None).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{saved}");
    assert_eq!(detail(&state).await.revision.html, "<p>back</p>");
}

#[tokio::test]
async fn an_html_save_without_a_base_is_never_refused() {
    let (state, _) = seeded().await;
    save_html(&state, "<p>second</p>", None).await;

    let (status, saved) = save_html(&state, "<p>third</p>", None).await;

    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(saved["success"], true, "{saved}");
    assert_eq!(detail(&state).await.revision.html, "<p>third</p>");
}

#[tokio::test]
async fn a_pruned_journal_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;
    replace(&state, &[("summary", serde_json::json!({"count": 2}))]).await;
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM live_page_publications WHERE data_revision = 1",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let gap = read(&state, Some("0"), Some(&created.revision.id)).await;
    assert_full(&gap, "revision 1 has no journal row");

    let covered = read(&state, Some("1"), Some(&created.revision.id)).await;
    assert!(covered.full.is_none(), "revision 2 is journaled");
    assert_eq!(names(&covered), vec!["summary"]);
}

#[tokio::test]
async fn a_journaled_dataset_that_is_gone_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;
    state
        .db
        .with_conn(|conn| {
            conn.execute("DELETE FROM live_page_datasets WHERE name = 'summary'", [])?;
            Ok(())
        })
        .await
        .unwrap();

    let delta = read(&state, Some("0"), Some(&created.revision.id)).await;

    assert_full(&delta, "the journal names a dataset the Page no longer has");
}

#[tokio::test]
async fn an_unusable_since_falls_back_to_the_full_state() {
    let (state, created) = seeded().await;
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;
    let revision = Some(created.revision.id.as_str());

    assert_full(
        &read(&state, Some("2"), revision).await,
        "ahead of the Page",
    );
    assert_full(&read(&state, None, revision).await, "no revision given");
    assert_full(&read(&state, Some("-1"), revision).await, "not a revision");
    assert_full(&read(&state, Some("abc"), revision).await, "not a number");
}

#[tokio::test]
async fn an_unknown_page_is_not_found() {
    let (state, _) = seeded().await;
    let Json(response) = data(
        State(state),
        Path("missing".into()),
        Query(LivePageDataQuery::default()),
    )
    .await;
    assert_eq!(response.error_code.as_deref(), Some("not_found"));
}
