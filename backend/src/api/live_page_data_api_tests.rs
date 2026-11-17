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

async fn replace(state: &AppState, writes: &[(&str, serde_json::Value)]) -> PublishLivePageResult {
    let Json(published) = publish(
        State(state.clone()),
        Path("board".into()),
        Json(PublishLivePageRequest {
            workflow_id: None,
            workflow_run_id: None,
            writes: writes
                .iter()
                .map(|(dataset, value)| LivePageWrite {
                    dataset: (*dataset).into(),
                    operation: LivePageWriteOperation::Replace,
                    value: value.clone(),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                })
                .collect(),
        }),
    )
    .await;
    assert!(published.success, "{:?}", published.error);
    published.data.unwrap()
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
    replace(&state, &[("summary", serde_json::json!({"count": 1}))]).await;
    let before = detail(&state).await;
    let mut events = state.ws_broadcast.subscribe();

    let unchanged = replace(
        &state,
        &[
            ("summary", serde_json::json!({"count": 1})),
            ("heavy", serde_json::json!({"blob": HEAVY})),
        ],
    )
    .await;

    assert!(!unchanged.content_changed);
    assert_eq!(unchanged.unchanged_datasets, vec!["summary", "heavy"]);
    assert_eq!(unchanged.data_revision, 1, "the caller reads the revision");
    assert!(events.try_recv().is_err(), "no page-data event");
    let after = detail(&state).await;
    assert_eq!(after.page.data_revision, 1);
    assert!(
        after.page.last_published_at > before.page.last_published_at,
        "the refresh is still dated"
    );
    for (old, new) in before.datasets.iter().zip(&after.datasets) {
        assert_eq!(old.dataset.updated_at, new.dataset.updated_at);
    }
    let Json(journal) = publications(State(state.clone()), Path("board".into())).await;
    let journal = journal.data.unwrap();
    assert_eq!(journal.len(), 1, "only the publication that changed data");
    assert_eq!(journal[0].data_revision, 1);
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
