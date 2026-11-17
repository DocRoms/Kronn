//! KT-1751 fourth review — the project handlers keep the lane snapshot's
//! registry in step with the project table. A missing call is a widening (a
//! nested project would run the outer project's list), so each wiring is
//! pinned through the real handler.

use super::*;
use crate::core::task_worker_lane_settings::registry_tests::project;
use crate::core::task_worker_lane_settings::{
    forget_project, test_guard, validation_executables_for,
};
use crate::models::{LaneWarmConfig, ProjectWorkspace};

fn test_state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

async fn seed(state: &AppState, id: &str, path: &std::path::Path) {
    let row = project(id, path, None);
    state
        .db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &row)?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .expect("insert project");
}

/// The real workspace save publishes the project's list.
async fn list_through_save(state: &AppState, id: &str, entries: &[&str]) {
    let saved = set_project_workspace(
        State(state.clone()),
        Path(id.into()),
        Json(Some(ProjectWorkspace {
            lane_warm: LaneWarmConfig {
                validation_executables: Some(entries.iter().map(|e| e.to_string()).collect()),
                ..LaneWarmConfig::default()
            },
            ..ProjectWorkspace::default()
        })),
    )
    .await;
    assert!(saved.0.success, "{:?}", saved.0.error);
}

#[tokio::test]
async fn an_added_sub_folder_owns_its_tree_until_it_is_deleted() {
    let _owner = test_guard::exclusive_async().await;
    let root = tempfile::tempdir().unwrap();
    let mono = root.path().canonicalize().unwrap().join("mono");
    let sub = mono.join("sub");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    let state = test_state();
    seed(&state, "mono", &mono).await;
    list_through_save(&state, "mono", &["outer-tool"]).await;

    let added = add_folder(
        State(state.clone()),
        Json(AddFolderRequest {
            path: sub.display().to_string(),
            name: None,
        }),
    )
    .await;
    assert!(added.0.success, "{:?}", added.0.error);
    let sub_id = added.0.data.expect("the added project").id;
    let while_registered = validation_executables_for(&sub.join("src"));

    let deleted = delete(
        State(state.clone()),
        Path(sub_id),
        Query(DeleteProjectQuery { hard: false }),
    )
    .await;
    assert!(deleted.0.success, "{:?}", deleted.0.error);
    let after_delete = validation_executables_for(&sub.join("src"));
    forget_project(&mono);

    assert!(
        while_registered.is_empty(),
        "the added sub-folder got {while_registered:?}"
    );
    assert_eq!(
        after_delete,
        vec!["outer-tool".to_string()],
        "once deleted, the sub-folder is the outer project's again"
    );
}

/// Two rows can hold one folder (a trailing `/` on add-folder): deleting one
/// leaves the other's list in force, as its stored workspace still says.
#[tokio::test]
async fn deleting_a_duplicate_leaves_the_live_projects_list_in_force() {
    let _owner = test_guard::exclusive_async().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().canonicalize().unwrap().join("twice");
    std::fs::create_dir_all(&dir).unwrap();
    let state = test_state();
    seed(&state, "live", &dir).await;
    // The same folder under another spelling: the table takes both rows.
    let spelled = std::path::PathBuf::from(format!("{}/", dir.display()));
    seed(&state, "duplicate", &spelled).await;
    list_through_save(&state, "live", &["live-tool"]).await;

    let deleted = delete(
        State(state.clone()),
        Path("duplicate".into()),
        Query(DeleteProjectQuery { hard: false }),
    )
    .await;
    assert!(deleted.0.success, "{:?}", deleted.0.error);
    let after_delete = validation_executables_for(&dir);
    forget_project(&dir);

    assert_eq!(after_delete, vec!["live-tool".to_string()]);
}

#[tokio::test]
async fn a_remap_moves_the_projects_list_and_registration_to_the_new_path() {
    let _shared = test_guard::shared_async().await;
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let old = base.join("old");
    let new = base.join("new");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let state = test_state();
    seed(&state, "p", &old).await;
    list_through_save(&state, "p", &["remap-tool"]).await;

    let remapped = remap_path(
        State(state.clone()),
        Path("p".into()),
        Json(serde_json::json!({ "path": new.display().to_string() })),
    )
    .await;
    assert!(remapped.0.success, "{:?}", remapped.0.error);
    let at_new = validation_executables_for(&new);
    let at_old = validation_executables_for(&old);
    forget_project(&new);
    forget_project(&old);

    assert_eq!(at_new, vec!["remap-tool".to_string()]);
    assert!(at_old.is_empty(), "the old path kept {at_old:?}");
}
