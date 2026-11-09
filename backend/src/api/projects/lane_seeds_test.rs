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
        confirm_secret_paths: vec![],
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

fn confirmed(paths: &[&str], confirm: &[&str]) -> Json<SetProjectLaneSeeds> {
    Json(SetProjectLaneSeeds {
        seeds: paths.iter().map(|path| path.to_string()).collect(),
        confirm_secret_paths: confirm.iter().map(|path| path.to_string()).collect(),
    })
}

/// A path that looks like a secret file is not saved until the human confirmed it.
#[tokio::test]
async fn a_secret_looking_path_is_saved_only_once_confirmed() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p1", dir.path()).await;
    let _ = set(State(state.clone()), Path("p1".into()), declared(&["a/b"])).await;

    let held = set(
        State(state.clone()),
        Path("p1".into()),
        confirmed(&["a/b", ".env", "config/server.key"], &[]),
    )
    .await;
    assert!(held.0.success, "{:?}", held.0.error);
    let held = held.0.data.unwrap();
    assert_eq!(
        held.needs_confirmation,
        vec![".env".to_string(), "config/server.key".to_string()]
    );
    assert_eq!(held.seeds, vec!["a/b".to_string()], "nothing was saved");
    assert_eq!(current(&state, "p1").await, vec!["a/b".to_string()]);

    // Confirming only one of them still holds the other.
    let partly = set(
        State(state.clone()),
        Path("p1".into()),
        confirmed(&["a/b", ".env", "config/server.key"], &[".env"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(
        partly.needs_confirmation,
        vec!["config/server.key".to_string()]
    );
    assert_eq!(current(&state, "p1").await, vec!["a/b".to_string()]);

    let saved = set(
        State(state.clone()),
        Path("p1".into()),
        confirmed(&["a/b", ".env"], &[".env"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(saved.needs_confirmation.is_empty());
    assert_eq!(
        current(&state, "p1").await,
        vec!["a/b".to_string(), ".env".to_string()]
    );
}

fn committed_repo_with_profile(profile: &str) -> (tempfile::TempDir, std::path::PathBuf) {
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
    std::fs::create_dir_all(repo.join("kronn")).unwrap();
    std::fs::write(repo.join("kronn/project.toml"), profile).unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "profile"]);
    (dir, repo)
}

/// A project nobody declared seeds for in Kronn falls back to the table its
/// default branch carries; a human's declaration (even an empty one) wins, and a
/// save keeps the working copy of the file in step with it.
#[tokio::test]
async fn the_declaration_and_the_projects_profile_stay_consistent() {
    let state = test_state();
    let (_dir, repo) = committed_repo_with_profile(
        "schema_version = 1\n\n[lane_seeds]\npaths = [\"public/images\", \".env\"]\n\n[forge]\nbase_branch = \"main\"\n",
    );
    seed_project(&state, "p1", &repo).await;

    let read = get(State(state.clone()), Path("p1".into()))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(read.source, LaneSeedsSource::Profile);
    assert_eq!(
        read.seeds,
        vec!["public/images".to_string()],
        "a secret-looking path out of the file is never applied unconfirmed"
    );
    assert_eq!(
        read.profile_seeds,
        Some(vec!["public/images".to_string(), ".env".to_string()])
    );

    let saved = set(
        State(state.clone()),
        Path("p1".into()),
        declared(&["public/build"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(saved.source, LaneSeedsSource::Project);
    assert_eq!(saved.seeds, vec!["public/build".to_string()]);
    assert!(saved.profile_file_written, "{:?}", saved.profile_file_error);
    let file = std::fs::read_to_string(repo.join("kronn/project.toml")).unwrap();
    assert!(file.contains("public/build"), "{file}");
    assert!(!file.contains("public/images"), "{file}");
    assert!(file.contains("base_branch = \"main\""), "{file}");
    assert_eq!(
        saved.profile_seeds,
        Some(vec!["public/images".to_string(), ".env".to_string()]),
        "the default branch still says what it said until the edit is committed"
    );

    let cleared = set(State(state.clone()), Path("p1".into()), declared(&[]))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(cleared.source, LaneSeedsSource::Project);
    assert!(cleared.seeds.is_empty(), "cleared stays cleared");
    assert!(!std::fs::read_to_string(repo.join("kronn/project.toml"))
        .unwrap()
        .contains("lane_seeds"));
}

/// A project without a `kronn/project.toml` is not given one by a save.
#[tokio::test]
async fn a_project_without_a_profile_file_is_not_given_one() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    seed_project(&state, "p1", dir.path()).await;
    let saved = set(
        State(state.clone()),
        Path("p1".into()),
        declared(&["public/images"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(!saved.profile_file_written);
    assert!(saved.profile_file_error.is_none());
    assert!(!dir.path().join("kronn/project.toml").exists());
}

/// The settings show a skipped seed with its reason, and what a seeded one left out.
#[test]
fn the_last_run_names_why_a_seed_was_skipped() {
    use crate::core::task_worker_lane_seeds::{LaneSeedOutcome, SeedMethod};
    let record = LaneSeedsRecord {
        seeds: vec![
            LaneSeedOutcome {
                path: "a".into(),
                status: SeedStatus::Seeded {
                    method: SeedMethod::Clone,
                    refreshed: false,
                    ms: 1,
                    dropped_links: vec!["x".into(), "y/z".into()],
                },
            },
            LaneSeedOutcome {
                path: "b".into(),
                status: SeedStatus::Skipped {
                    reason: "the main checkout does not have it".into(),
                },
            },
            LaneSeedOutcome {
                path: "c".into(),
                status: SeedStatus::UpToDate,
            },
        ],
    };
    let shown = reports(&record);
    assert_eq!(shown[0].outcome, "seeded");
    assert!(shown[0].detail.as_deref().unwrap().contains("x, y/z"));
    assert_eq!(shown[1].outcome, "skipped");
    assert_eq!(
        shown[1].detail.as_deref(),
        Some("the main checkout does not have it")
    );
    assert_eq!(shown[2].outcome, "up_to_date");
    assert!(shown[2].detail.is_none());
}
