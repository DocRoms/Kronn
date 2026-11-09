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
        held.needs_confirmation
            .iter()
            .map(|finding| finding.path.as_str())
            .collect::<Vec<_>>(),
        vec![".env", "config/server.key"]
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
        partly
            .needs_confirmation
            .iter()
            .map(|finding| finding.path.as_str())
            .collect::<Vec<_>>(),
        vec!["config/server.key"]
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

/// The table the default branch carries is only proposed: nothing is seeded from
/// it until a human adopts it. A human's declaration (even an empty one) is what
/// applies, and a save keeps the working copy of the file in step with it.
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
    assert_eq!(read.source, LaneSeedsSource::None);
    assert!(
        read.seeds.is_empty(),
        "a repo-controlled table seeds nothing by itself: {:?}",
        read.seeds
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

/// Adopting the proposal is a declaration like any other: a secret-looking path
/// in it is held, listed, until the human confirms it.
#[tokio::test]
async fn adopting_the_profile_proposal_holds_a_secret_until_confirmed() {
    let state = test_state();
    let (_dir, repo) = committed_repo_with_profile(
        "schema_version = 1\n\n[lane_seeds]\npaths = [\"public/images\", \".env\"]\n",
    );
    seed_project(&state, "p1", &repo).await;
    let proposal = get(State(state.clone()), Path("p1".into()))
        .await
        .0
        .data
        .unwrap()
        .profile_seeds
        .unwrap();

    let held = set(
        State(state.clone()),
        Path("p1".into()),
        Json(SetProjectLaneSeeds {
            seeds: proposal.clone(),
            confirm_secret_paths: vec![],
        }),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(held.needs_confirmation.len(), 1);
    assert_eq!(held.needs_confirmation[0].path, ".env");
    assert!(current(&state, "p1").await.is_empty(), "nothing is saved");

    let adopted = set(
        State(state.clone()),
        Path("p1".into()),
        Json(SetProjectLaneSeeds {
            seeds: proposal.clone(),
            confirm_secret_paths: vec![".env".into()],
        }),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(adopted.needs_confirmation.is_empty());
    assert_eq!(adopted.seeds, proposal);
}

/// A declared directory that holds a `.env` or a key is held with the files
/// listed, although its own path is harmless; a seed already accepted is not
/// asked about again when another one is added.
#[tokio::test]
async fn a_secret_inside_a_declared_directory_is_listed_and_held() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join("generated/deep")).unwrap();
    std::fs::write(repo.join("generated/ok.txt"), "fine").unwrap();
    std::fs::write(repo.join("generated/deep/.env.local"), "TOKEN=1").unwrap();
    std::fs::create_dir_all(repo.join("clean")).unwrap();
    std::fs::write(repo.join("clean/a.txt"), "a").unwrap();
    seed_project(&state, "p1", repo).await;

    let held = set(
        State(state.clone()),
        Path("p1".into()),
        declared(&["clean", "generated"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(
        held.needs_confirmation.len(),
        1,
        "{:?}",
        held.needs_confirmation
    );
    assert_eq!(held.needs_confirmation[0].path, "generated");
    assert_eq!(
        held.needs_confirmation[0].files,
        vec!["generated/deep/.env.local".to_string()]
    );
    assert!(current(&state, "p1").await.is_empty());

    let saved = set(
        State(state.clone()),
        Path("p1".into()),
        confirmed(&["clean", "generated"], &["generated"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(saved.needs_confirmation.is_empty());
    assert_eq!(saved.secret_findings.len(), 1, "still shown once saved");
    assert_eq!(saved.secret_findings[0].path, "generated");

    std::fs::create_dir_all(repo.join("more")).unwrap();
    let next = set(
        State(state.clone()),
        Path("p1".into()),
        declared(&["clean", "generated", "more"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(
        next.needs_confirmation.is_empty(),
        "{:?}",
        next.needs_confirmation
    );
    assert_eq!(next.seeds.len(), 3);
}

/// A `.env` that appears inside an accepted directory is not covered by the
/// confirmation: the settings show the seed as held, saving the same list keeps
/// it held, and confirming it releases it.
#[tokio::test]
async fn a_secret_added_after_acceptance_is_shown_held_until_confirmed() {
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join("generated")).unwrap();
    std::fs::write(repo.join("generated/.env.local"), "TOKEN=1").unwrap();
    seed_project(&state, "p1", repo).await;

    let accepted = set(
        State(state.clone()),
        Path("p1".into()),
        confirmed(&["generated"], &["generated"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(accepted.seeds, vec!["generated".to_string()]);
    assert!(!accepted.secret_findings[0].held, "{accepted:?}");

    std::fs::write(repo.join("generated/id_rsa"), "key").unwrap();
    let seen = get(State(state.clone()), Path("p1".into()))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(seen.secret_findings.len(), 1);
    assert!(seen.secret_findings[0].held, "{seen:?}");
    assert_eq!(
        seen.secret_findings[0].files,
        vec![
            "generated/.env.local".to_string(),
            "generated/id_rsa".to_string()
        ]
    );

    // Saving the list again changes nothing about what was confirmed.
    let resaved = set(
        State(state.clone()),
        Path("p1".into()),
        declared(&["generated"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(resaved.seeds, vec!["generated".to_string()]);
    assert!(resaved.needs_confirmation.is_empty());
    assert!(resaved.secret_findings[0].held, "{resaved:?}");
    let stored = state
        .db
        .with_read_conn(|conn| crate::db::projects::lane_seeds_confirmed(conn, "p1"))
        .await
        .unwrap();
    assert_eq!(
        stored["generated"],
        vec!["generated/.env.local".to_string()],
        "the new file is not recorded as confirmed"
    );

    let released = set(
        State(state.clone()),
        Path("p1".into()),
        confirmed(&["generated"], &["generated"]),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(!released.secret_findings[0].held, "{released:?}");
}

/// A tree that cannot be checked refuses the save instead of saving unchecked.
#[cfg(unix)]
#[tokio::test]
async fn a_tree_that_cannot_be_checked_refuses_the_save() {
    use std::os::unix::fs::PermissionsExt;
    let state = test_state();
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("gen/inner");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("a.txt"), "a").unwrap();
    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A superuser reads it anyway: then there is nothing to fail on.
    let unreadable = std::fs::read_dir(&inner).is_err();
    seed_project(&state, "p1", dir.path()).await;

    let refused = set(State(state.clone()), Path("p1".into()), declared(&["gen"])).await;

    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o755)).unwrap();
    if unreadable {
        assert!(!refused.0.success, "an unchecked tree must not be saved");
        assert!(current(&state, "p1").await.is_empty());
    }
}

/// A table with a path that is no valid seed is an error of the proposal, never
/// handed back raw.
#[tokio::test]
async fn a_malformed_profile_table_is_an_error_not_raw_paths() {
    let state = test_state();
    let (_dir, repo) = committed_repo_with_profile(
        "schema_version = 1\n\n[lane_seeds]\npaths = [\"../escape\"]\n",
    );
    seed_project(&state, "p1", &repo).await;

    let read = get(State(state.clone()), Path("p1".into()))
        .await
        .0
        .data
        .unwrap();

    assert!(read.profile_seeds.is_none(), "{:?}", read.profile_seeds);
    assert!(read.profile_error.is_some());
    assert!(read.seeds.is_empty());
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
