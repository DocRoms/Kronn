//! KT-1751 fourth review — an import or a reset replaces the project table, and
//! the lane snapshot follows it: no list of a project that is gone stays in
//! force, and a project that came in owns its tree.
//!
//! The reset is driven through `reset_data`, the step of the handler that
//! clears the table and rebuilds the snapshot: the rest of the handler rewrites
//! `config.toml` and the stored credentials of the data directory.

use super::*;
use crate::core::task_worker_lane_settings::registry_tests::project;
use crate::core::task_worker_lane_settings::{set_project, test_guard, validation_executables_for};
use crate::models::LaneWarmConfig;

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn listing(entries: &[&str]) -> LaneWarmConfig {
    LaneWarmConfig {
        validation_executables: Some(entries.iter().map(|e| e.to_string()).collect()),
        ..LaneWarmConfig::default()
    }
}

async fn insert(state: &AppState, rows: Vec<crate::models::Project>) {
    state
        .db
        .with_conn(move |conn| {
            for row in &rows {
                crate::db::projects::insert_project(conn, row)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .expect("insert projects");
}

#[tokio::test]
async fn an_import_rebuilds_the_lane_snapshot_from_the_imported_projects() {
    let _owner = test_guard::exclusive_async().await;
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let mono = base.join("mono");
    let sub = mono.join("sub");
    std::fs::create_dir_all(sub.join("src")).unwrap();

    // The backup: `mono` lists `new-outer`, `mono/sub` (a plain sub-folder) none.
    let source = state();
    insert(
        &source,
        vec![
            project("mono", &mono, Some(listing(&["new-outer"]))),
            project("sub", &sub, None),
        ],
    )
    .await;
    let export = build_export(&source).await.expect("export");

    // This instance had `mono` with another list, and no `mono/sub`.
    let target = state();
    insert(
        &target,
        vec![project("mono", &mono, Some(listing(&["stale-outer"])))],
    )
    .await;
    set_project(&mono, Some(&listing(&["stale-outer"])));

    do_import_db(&target, &export, ContextBlobs::default())
        .await
        .expect("import");
    let in_sub = validation_executables_for(&sub.join("src"));
    let outer = validation_executables_for(&mono);

    assert!(in_sub.is_empty(), "the imported sub-project got {in_sub:?}");
    assert_eq!(outer, vec!["new-outer".to_string()]);
}

#[tokio::test]
async fn a_reset_empties_the_lane_snapshot() {
    let _owner = test_guard::exclusive_async().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().canonicalize().unwrap().join("p");
    std::fs::create_dir_all(&dir).unwrap();
    let state = state();
    insert(
        &state,
        vec![project("p", &dir, Some(listing(&["before-reset"])))],
    )
    .await;
    crate::arm_lane_settings(&state.db, &LaneWarmConfig::default())
        .await
        .expect("armed");
    set_project(&dir, Some(&listing(&["before-reset"])));

    reset_data(&state).await.expect("reset");
    let after = validation_executables_for(&dir);
    // A project re-created at the same path starts with no list.
    crate::core::task_worker_lane_settings::register_project(&dir);
    let recreated = validation_executables_for(&dir);

    assert!(after.is_empty(), "a deleted project's list: {after:?}");
    assert!(
        recreated.is_empty(),
        "the re-created project got {recreated:?}"
    );
}
