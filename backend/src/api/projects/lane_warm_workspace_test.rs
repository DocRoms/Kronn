//! KT-1742 — a project's lane warm-up settings travel with its workspace
//! settings: validated, stored beside the hooks, published to the lane code.

use super::*;
use crate::models::{LaneWarmConfig, ProjectWorkspace, WorkspaceHooks};
use chrono::Utc;

fn test_state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

async fn seed_project(state: &AppState, id: &str, path: &std::path::Path) {
    let now = Utc::now();
    let project = crate::models::Project {
        id: id.into(),
        name: "test".into(),
        path: path.display().to_string(),
        repo_url: None,
        token_override: None,
        ai_config: crate::models::AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: Default::default(),
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        write_access: None,
        mcp_sync_report: None,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &project)?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .expect("insert project");
}

fn lane_warm(minutes: Option<u32>, command: Option<&str>) -> LaneWarmConfig {
    LaneWarmConfig {
        first_build_timeout_min: minutes,
        first_build_command: command.map(str::to_string),
        ..LaneWarmConfig::default()
    }
}

async fn stored(state: &AppState, id: &str) -> Option<ProjectWorkspace> {
    let response = get_project_workspace(State(state.clone()), Path(id.into())).await;
    assert!(response.0.success, "{:?}", response.0.error);
    response.0.data.unwrap()
}

#[tokio::test]
async fn a_projects_lane_warm_settings_are_stored_published_and_cleared() {
    let _shared = crate::core::task_worker_lane_settings::test_guard::shared_async().await;
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p1", dir.path()).await;
    let worktree = dir
        .path()
        .join(".kronn")
        .join("worktrees")
        .join("task-kt-1742");
    let bound = |minutes: u64| std::time::Duration::from_secs(minutes * 60);
    let effective =
        || crate::core::task_worker_lane_settings::effective_for(&worktree, Some(dir.path()));
    assert_eq!(effective().first_build_timeout, bound(60), "the default");

    let saved = set_project_workspace(
        State(state.clone()),
        Path("p1".into()),
        Json(Some(ProjectWorkspace {
            hooks: WorkspaceHooks {
                after_create: Some("make setup".into()),
                ..WorkspaceHooks::default()
            },
            lane_warm: lane_warm(Some(5), Some("  composer install  ")),
            ..ProjectWorkspace::default()
        })),
    )
    .await;
    assert!(saved.0.success, "{:?}", saved.0.error);

    let workspace = stored(&state, "p1").await.expect("stored");
    assert_eq!(workspace.hooks.after_create.as_deref(), Some("make setup"));
    assert_eq!(
        workspace.lane_warm.first_build_command.as_deref(),
        Some("composer install"),
        "stored without stray blanks"
    );
    let now = effective();
    assert_eq!(now.first_build_timeout, bound(5), "published at once");
    assert_eq!(now.first_build_command.as_deref(), Some("composer install"));

    // A refused entry leaves the stored one, and the published one, alone.
    let refused = set_project_workspace(
        State(state.clone()),
        Path("p1".into()),
        Json(Some(ProjectWorkspace {
            hooks: WorkspaceHooks::default(),
            lane_warm: lane_warm(Some(0), None),
            ..ProjectWorkspace::default()
        })),
    )
    .await;
    assert!(!refused.0.success);
    assert!(
        refused
            .0
            .error
            .unwrap_or_default()
            .contains("first_build_timeout_min"),
        "the refusal names the field"
    );
    assert_eq!(
        stored(&state, "p1")
            .await
            .unwrap()
            .lane_warm
            .first_build_timeout_min,
        Some(5)
    );
    assert_eq!(effective().first_build_timeout, bound(5));

    // Hooks alone keep a workspace; nothing at all clears it and the override.
    let cleared = set_project_workspace(State(state.clone()), Path("p1".into()), Json(None)).await;
    assert!(cleared.0.success, "{:?}", cleared.0.error);
    assert!(stored(&state, "p1").await.is_none());
    assert_eq!(
        effective().first_build_timeout,
        bound(60),
        "back to the default"
    );
}

#[tokio::test]
async fn a_workspace_with_neither_hooks_nor_lane_settings_is_not_stored() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p2", dir.path()).await;
    let saved = set_project_workspace(
        State(state.clone()),
        Path("p2".into()),
        Json(Some(ProjectWorkspace::default())),
    )
    .await;
    assert!(saved.0.success);
    assert!(stored(&state, "p2").await.is_none());
}

/// KT-1751 review — a single-segment repository path (`./gradlew`) keeps its
/// `./` through the real save: stored as typed, it admits `./gradlew test` and
/// never a bare `gradlew` from the PATH; a bare name stays a PATH program and
/// never admits a repository file `./composer`.
#[tokio::test]
async fn a_saved_repository_wrapper_keeps_its_dot_slash_and_a_bare_name_stays_a_name() {
    let _shared = crate::core::task_worker_lane_settings::test_guard::shared_async().await;
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p3", dir.path()).await;
    let saved = set_project_workspace(
        State(state.clone()),
        Path("p3".into()),
        Json(Some(ProjectWorkspace {
            lane_warm: LaneWarmConfig {
                validation_executables: Some(vec![
                    " ./gradlew ".into(),
                    "composer".into(),
                    "./vendor/bin/phpunit".into(),
                ]),
                ..LaneWarmConfig::default()
            },
            ..ProjectWorkspace::default()
        })),
    )
    .await;
    assert!(saved.0.success, "{:?}", saved.0.error);
    let listed = stored(&state, "p3")
        .await
        .expect("stored")
        .lane_warm
        .validation_executables
        .expect("listed");
    assert!(listed.contains(&"./gradlew".to_string()), "{listed:?}");
    let worktree = dir.path().join(".kronn/worktrees/task-kt-1751-dot");
    std::fs::create_dir_all(&worktree).unwrap();
    let published = crate::core::task_worker_lane_settings::validation_executables_for(&worktree);
    crate::core::task_worker_lane_settings::set_project(dir.path(), None);

    let check = |line: &str| crate::core::quick_exec::check_command_line_with(line, &published);
    assert!(
        check("./gradlew test").is_ok(),
        "{:?}",
        check("./gradlew test").err()
    );
    assert!(
        check("gradlew test").is_err(),
        "a bare gradlew is a PATH program"
    );
    assert!(check("composer install").is_ok());
    assert!(
        check("./composer install").is_err(),
        "a repository file `./composer`"
    );
    assert!(check("vendor/bin/phpunit").is_ok());
    assert!(check("./vendor/bin/phpunit").is_ok());
}

/// KT-1751 third review — the path branch of a validation executable refuses,
/// through the real save, every spelling that leaves the repository or hides a
/// `.`/`..`/empty segment, and nothing of the refused entry is stored.
#[tokio::test]
async fn a_saved_validation_path_that_escapes_or_hides_a_segment_is_refused() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p4", dir.path()).await;
    for entry in ["./..", "./../x", ".//x", "./a/../b", "././x", "/abs", "./"] {
        let saved = set_project_workspace(
            State(state.clone()),
            Path("p4".into()),
            Json(Some(ProjectWorkspace {
                lane_warm: LaneWarmConfig {
                    validation_executables: Some(vec![entry.into()]),
                    ..LaneWarmConfig::default()
                },
                ..ProjectWorkspace::default()
            })),
        )
        .await;
        assert!(!saved.0.success, "`{entry}` was accepted");
        assert!(
            saved
                .0
                .error
                .unwrap_or_default()
                .contains("validation_executables"),
            "the refusal of `{entry}` names the field"
        );
        assert!(stored(&state, "p4").await.is_none(), "`{entry}` was stored");
    }
}
