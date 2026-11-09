use super::*;
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

fn declared(paths: &[&str]) -> Json<SetProjectLaneSeeds> {
    Json(SetProjectLaneSeeds {
        seeds: paths.iter().map(|path| path.to_string()).collect(),
    })
}

async fn current(state: &AppState, id: &str) -> Vec<String> {
    let response = get(State(state.clone()), Path(id.into())).await;
    assert!(response.0.success, "{:?}", response.0.error);
    response.0.data.unwrap().seeds
}

#[tokio::test]
async fn a_project_declares_edits_and_clears_its_lane_seeds() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p1", dir.path()).await;
    assert!(current(&state, "p1").await.is_empty());

    let saved = set(
        State(state.clone()),
        Path("p1".into()),
        declared(&["./application/public/images/", "application/public/build"]),
    )
    .await;
    assert!(saved.0.success, "{:?}", saved.0.error);
    assert_eq!(
        current(&state, "p1").await,
        vec![
            "application/public/images".to_string(),
            "application/public/build".to_string()
        ],
        "stored normalized, in the human's order"
    );

    let cleared = set(State(state.clone()), Path("p1".into()), declared(&[])).await;
    assert!(cleared.0.success);
    assert!(current(&state, "p1").await.is_empty());
}

#[tokio::test]
async fn an_invalid_declaration_is_refused_and_changes_nothing() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p1", dir.path()).await;
    let _ = set(State(state.clone()), Path("p1".into()), declared(&["a/b"])).await;

    for bad in [
        &["../escape"][..],
        &["/etc"],
        &[".git/hooks"],
        &["a", "a/b"],
    ] {
        let refused = set(State(state.clone()), Path("p1".into()), declared(bad)).await;
        assert!(!refused.0.success, "{bad:?} must be refused");
    }
    assert_eq!(current(&state, "p1").await, vec!["a/b".to_string()]);
}

#[tokio::test]
async fn an_unknown_project_is_not_found() {
    let state = test_state();

    let read = get(State(state.clone()), Path("nope".into())).await;
    let write = set(State(state.clone()), Path("nope".into()), declared(&["a"])).await;
    let scan = candidates(State(state), Path("nope".into())).await;

    assert!(!read.0.success && !write.0.success && !scan.0.success);
}

#[tokio::test]
async fn candidates_are_the_ignored_directories_the_code_mentions() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(["-c", "user.email=t@t.com", "-c", "user.name=T"])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
    };
    git(&["init", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), "public/images/\n").unwrap();
    std::fs::create_dir_all(repo.join("templates")).unwrap();
    std::fs::write(repo.join("templates/menu.twig"), "@web/images/icon.svg").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "init"]);
    std::fs::create_dir_all(repo.join("public/images")).unwrap();
    std::fs::write(repo.join("public/images/icon.svg"), "<svg/>").unwrap();
    seed_project(&state, "p1", &repo).await;

    let response = candidates(State(state), Path("p1".into())).await;

    assert!(response.0.success, "{:?}", response.0.error);
    let found = response.0.data.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, "public/images");
}
