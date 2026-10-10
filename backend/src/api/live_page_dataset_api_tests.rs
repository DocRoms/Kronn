//! KT-1104 — who may delete, empty and re-limit a dataset.
use axum::extract::{Path, Query, State};
use axum::Json;

use super::*;
use crate::core::bridge_token::{route_for, BridgeCaller, Rule};
use crate::models::{LivePageWrite, LivePageWriteOperation, UpdateLivePageDatasetRequest};

type Caller = Option<axum::Extension<BridgeCaller>>;

fn agent() -> Caller {
    Some(axum::Extension(BridgeCaller {
        token_id: "t".into(),
        project: None,
        own_discussions: vec![],
        own_run: None,
        agent: None,
    }))
}

/// A Page with `free` (unused) and `fed` (written by a workflow).
async fn seeded() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let request: CreateLivePageRequest = serde_json::from_value(serde_json::json!({
        "title": "Stats", "slug": "stats", "html": "<p>Stats</p>",
        "datasets": [
            {"name": "free", "kind": "time_series", "initial": [1, 2, 3]},
            {"name": "fed", "kind": "snapshot"},
            {"name": "spare", "kind": "snapshot"}
        ]
    }))
    .unwrap();
    let Json(created) = create(State(state.clone()), None, Json(request)).await;
    assert!(created.success, "{:?}", created.error);
    state
        .db
        .with_conn(|conn| {
            crate::db::live_pages::datasets::tests::writer(conn, "wf-feed", "stats", "fed");
            Ok(())
        })
        .await
        .unwrap();
    state
}

async fn remove(
    state: &AppState,
    caller: Caller,
    name: &str,
    force: bool,
) -> ApiResponse<crate::models::DeleteLivePageDatasetResult> {
    let Json(response) = delete_dataset(
        State(state.clone()),
        caller,
        Path(("stats".into(), name.into())),
        Query(DeleteLivePageDatasetQuery { force }),
    )
    .await;
    response
}

#[tokio::test]
async fn a_human_deletes_an_unused_dataset_and_forces_past_a_writer() {
    let state = seeded().await;
    let mut events = state.ws_broadcast.subscribe();
    assert!(remove(&state, None, "free", false).await.success);
    assert!(matches!(
        events.try_recv(),
        Ok(crate::models::WsMessage::LivePageDataChanged { .. })
    ));

    let refused = remove(&state, None, "fed", false).await;
    assert_eq!(refused.error_code.as_deref(), Some("conflict"));
    assert!(
        refused.error.unwrap().contains("wf-feed"),
        "names the writer"
    );

    let forced = remove(&state, None, "fed", true).await;
    assert_eq!(
        forced.data.unwrap().overridden.writers[0].workflow_id,
        "wf-feed"
    );
    let missing = remove(&state, None, "fed", false).await;
    assert_eq!(missing.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn an_agent_deletes_only_what_nothing_uses_and_never_forces() {
    let state = seeded().await;
    let forced = remove(&state, agent(), "fed", true).await;
    assert_eq!(forced.error_code.as_deref(), Some("validation"));
    assert!(forced.error.unwrap().contains("Only a human"));
    let refused = remove(&state, agent(), "fed", false).await;
    assert_eq!(refused.error_code.as_deref(), Some("conflict"));
    assert!(remove(&state, agent(), "free", false).await.success);
}

#[tokio::test]
async fn only_a_human_changes_limits_or_reads_the_usage() {
    let state = seeded().await;
    let limits = || UpdateLivePageDatasetRequest {
        max_points: Some(1),
        max_age_days: None,
    };
    let Json(agent_limits) = update_dataset(
        State(state.clone()),
        agent(),
        Path(("stats".into(), "free".into())),
        Json(limits()),
    )
    .await;
    assert_eq!(agent_limits.error_code.as_deref(), Some("validation"));
    let Json(human_limits) = update_dataset(
        State(state.clone()),
        None,
        Path(("stats".into(), "free".into())),
        Json(limits()),
    )
    .await;
    assert_eq!(human_limits.data.unwrap().points_removed, 2);
    let Json(zero) = update_dataset(
        State(state.clone()),
        None,
        Path(("stats".into(), "free".into())),
        Json(UpdateLivePageDatasetRequest {
            max_points: Some(0),
            max_age_days: None,
        }),
    )
    .await;
    assert_eq!(zero.error_code.as_deref(), Some("validation"));

    let Json(agent_usage) =
        dataset_usage(State(state.clone()), agent(), Path("stats".into())).await;
    assert_eq!(agent_usage.error_code.as_deref(), Some("validation"));
    let Json(usage) = dataset_usage(State(state.clone()), None, Path("stats".into())).await;
    let usage = usage.data.unwrap();
    assert_eq!(usage.len(), 3);
    assert!(usage
        .iter()
        .any(|item| item.name == "fed" && !item.writers.is_empty()));
}

#[tokio::test]
async fn clear_goes_through_the_publish_route_for_every_kind() {
    let state = seeded().await;
    let Json(cleared) = publish(
        State(state.clone()),
        Path("stats".into()),
        Json(PublishLivePageRequest {
            workflow_id: None,
            workflow_run_id: None,
            writes: ["free", "spare"]
                .into_iter()
                .map(|dataset| LivePageWrite {
                    dataset: dataset.into(),
                    operation: LivePageWriteOperation::Clear,
                    value: serde_json::Value::Null,
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                })
                .collect(),
        }),
    )
    .await;
    assert_eq!(cleared.data.unwrap().points_removed, 3);
}

/// A token reaches deletion through the page's write scope; limits, usage and
/// publishing (hence `clear`) stay off its route list.
#[test]
fn the_bridge_route_list_matches_the_human_only_rules() {
    let delete = route_for("DELETE", "/api/pages/{id}/datasets/{name}").unwrap();
    assert_eq!(delete.rule, Rule::Write);
    for (method, pattern) in [
        ("PATCH", "/api/pages/{id}/datasets/{name}"),
        ("GET", "/api/pages/{id}/dataset-usage"),
        ("POST", "/api/pages/{id}/publish"),
    ] {
        assert!(route_for(method, pattern).is_none(), "{method} {pattern}");
    }
}
