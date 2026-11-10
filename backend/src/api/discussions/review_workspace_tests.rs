// KT-1645 — a review never runs in the owner's main checkout.
//
// A reviewer's subagent once ran `git checkout --detach` in the owner's main
// checkout. A discussion that names the commit it reviews now gets its own
// detached worktree, and a refusal to make it fails the creation instead of
// falling back to the project's checkout.
use super::crud::{create, delete};
use crate::models::*;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use std::sync::Arc;
use tokio::sync::RwLock;

fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with two commits and a file the owner has not committed.
/// Returns (repo, first commit, second commit).
fn owner_checkout() -> (tempfile::TempDir, String, String) {
    let repo = tempfile::tempdir().unwrap();
    run_git(repo.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(repo.path().join("a.txt"), "one").unwrap();
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-q", "-m", "one"]);
    let first = run_git(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::write(repo.path().join("a.txt"), "two").unwrap();
    run_git(repo.path(), &["commit", "-q", "-am", "two"]);
    let second = run_git(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::write(repo.path().join("wip.txt"), "uncommitted").unwrap();
    (repo, first, second)
}

async fn state_with_project(repo: &std::path::Path) -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let path = repo.to_string_lossy().into_owned();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, ai_config_json, created_at, updated_at)
             VALUES ('p-review', 'Review project', ?1, '{}', 'now', 'now')",
            [&path],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let config = Arc::new(RwLock::new(crate::core::config::default_config()));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn request(
    project: Option<&str>,
    head: Option<&str>,
    base: Option<&str>,
) -> CreateDiscussionRequest {
    serde_json::from_value(serde_json::json!({
        "project_id": project,
        "title": "Review of the range",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Review this range",
        "workspace_mode": "Direct",
        "review_head": head,
        "review_base": base,
    }))
    .expect("valid create payload")
}

fn same_dir(left: &str, right: &std::path::Path) -> bool {
    std::path::Path::new(left).canonicalize().ok() == right.canonicalize().ok()
}

async fn discussion_count(state: &AppState) -> i64 {
    state
        .db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM discussions", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_review_discussion_runs_in_a_detached_worktree_at_the_reviewed_head() {
    let (repo, first, second) = owner_checkout();
    let state = state_with_project(repo.path()).await;

    let response = create(
        State(state.clone()),
        Json(request(Some("p-review"), Some(&first), Some(&second))),
    )
    .await;

    let discussion = response.0.data.expect("the review is created");
    assert_eq!(
        discussion.workspace_mode, "Review",
        "even though Direct was asked"
    );
    let workspace = discussion
        .workspace_path
        .clone()
        .expect("it has a worktree");
    assert!(
        !same_dir(&workspace, repo.path()),
        "the review must not run in the main checkout"
    );
    assert!(workspace.contains(".kronn/worktrees"), "{workspace}");
    let checkout = std::path::Path::new(&workspace);
    assert_eq!(
        run_git(checkout, &["rev-parse", "HEAD"]),
        first,
        "at the reviewed head"
    );
    assert_eq!(
        run_git(checkout, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD",
        "detached: no branch to move"
    );
    assert_eq!(discussion.worktree_branch, None);

    // The owner's checkout is exactly as it was.
    assert_eq!(run_git(repo.path(), &["rev-parse", "HEAD"]), second);
    assert_eq!(
        run_git(repo.path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("wip.txt")).unwrap(),
        "uncommitted"
    );

    // What was stored is what the agent launch reads its working directory from.
    let stored = state
        .db
        .with_conn({
            let id = discussion.id.clone();
            move |conn| crate::db::discussions::get_discussion(conn, &id)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.workspace_mode, "Review");
    assert_eq!(stored.workspace_path.as_deref(), Some(workspace.as_str()));
}

#[tokio::test]
async fn a_review_that_cannot_be_pinned_is_refused_not_run_in_the_main_checkout() {
    let (repo, first, _) = owner_checkout();
    let state = state_with_project(repo.path()).await;

    for refused in [
        request(Some("p-review"), Some("no-such-revision"), None),
        request(Some("p-review"), Some(&first), Some("no-such-base")),
        request(Some("p-review"), Some("--detach"), None),
        request(None, Some(&first), None),
        request(Some("p-review"), None, Some(&first)),
    ] {
        let response = create(State(state.clone()), Json(refused)).await;
        assert!(!response.0.success, "the creation must fail");
    }

    assert_eq!(discussion_count(&state).await, 0, "nothing was stored");
    let worktrees = repo.path().join(".kronn/worktrees");
    assert!(
        !worktrees.exists() || std::fs::read_dir(&worktrees).unwrap().next().is_none(),
        "no worktree was left behind"
    );
    assert_eq!(
        run_git(repo.path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
}

#[tokio::test]
async fn deleting_a_review_discussion_removes_its_worktree_and_spares_the_main_checkout() {
    let (repo, first, second) = owner_checkout();
    let state = state_with_project(repo.path()).await;
    let discussion = create(
        State(state.clone()),
        Json(request(Some("p-review"), Some(&first), None)),
    )
    .await
    .0
    .data
    .expect("created");
    let workspace = discussion.workspace_path.clone().unwrap();
    assert!(std::path::Path::new(&workspace).exists());

    let response = delete(State(state), Path(discussion.id)).await;

    assert!(response.0.success);
    assert!(!std::path::Path::new(&workspace).exists());
    assert_eq!(run_git(repo.path(), &["rev-parse", "HEAD"]), second);
}

#[tokio::test]
async fn a_review_worktree_cannot_be_unlocked() {
    let (repo, first, _) = owner_checkout();
    let state = state_with_project(repo.path()).await;
    let discussion = create(
        State(state.clone()),
        Json(request(Some("p-review"), Some(&first), None)),
    )
    .await
    .0
    .data
    .expect("created");
    let workspace = discussion.workspace_path.clone().unwrap();

    let response = crate::api::disc_git::worktree_unlock(State(state), Path(discussion.id)).await;

    assert!(!response.0.success);
    assert!(std::path::Path::new(&workspace).exists());
}

#[tokio::test]
async fn a_plain_discussion_keeps_the_workspace_mode_it_asked_for() {
    let (repo, _, _) = owner_checkout();
    let state = state_with_project(repo.path()).await;

    let discussion = create(State(state), Json(request(Some("p-review"), None, None)))
        .await
        .0
        .data
        .expect("created");

    assert_eq!(discussion.workspace_mode, "Direct");
    assert_eq!(discussion.workspace_path, None);
}

async fn review_of(state: &AppState, head: &str) -> (Discussion, String) {
    let discussion = create(
        State(state.clone()),
        Json(request(Some("p-review"), Some(head), None)),
    )
    .await
    .0
    .data
    .expect("created");
    let workspace = discussion.workspace_path.clone().unwrap();
    (discussion, workspace)
}

async fn archive(state: &AppState, id: &str) {
    let patch: UpdateDiscussionRequest =
        serde_json::from_value(serde_json::json!({ "archived": true })).unwrap();
    let response =
        super::crud::update(State(state.clone()), Path(id.to_string()), Json(patch)).await;
    assert!(response.0.success, "{:?}", response.0.error);
}

async fn stored_workspace(state: &AppState, id: &str) -> Option<String> {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &id))
        .await
        .unwrap()
        .unwrap()
        .workspace_path
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

#[tokio::test]
async fn archiving_a_review_removes_its_clean_worktree_and_clears_the_pointer() {
    let (repo, first, second) = owner_checkout();
    let state = state_with_project(repo.path()).await;
    let (discussion, workspace) = review_of(&state, &first).await;
    assert!(exists(&workspace));

    archive(&state, &discussion.id).await;

    assert!(
        !exists(&workspace),
        "a clean detached review worktree is disposable"
    );
    assert_eq!(stored_workspace(&state, &discussion.id).await, None);
    assert!(
        !run_git(repo.path(), &["worktree", "list", "--porcelain"]).contains("--review-"),
        "its admin entry is gone too"
    );
    assert_eq!(run_git(repo.path(), &["rev-parse", "HEAD"]), second);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("wip.txt")).unwrap(),
        "uncommitted"
    );
}

#[tokio::test]
async fn archiving_a_review_keeps_a_worktree_that_holds_work() {
    let (repo, first, _) = owner_checkout();
    let state = state_with_project(repo.path()).await;

    // A modified file, a new untracked file, and a commit no ref reaches.
    let (dirty, dirty_ws) = review_of(&state, &first).await;
    std::fs::write(std::path::Path::new(&dirty_ws).join("a.txt"), "edited").unwrap();
    let (untracked, untracked_ws) = review_of(&state, &first).await;
    std::fs::write(std::path::Path::new(&untracked_ws).join("notes.md"), "n").unwrap();
    let (committed, committed_ws) = review_of(&state, &first).await;
    let checkout = std::path::Path::new(&committed_ws);
    std::fs::write(checkout.join("b.txt"), "b").unwrap();
    run_git(checkout, &["add", "b.txt"]);
    run_git(checkout, &["commit", "-q", "-m", "reviewer commit"]);

    for (discussion, workspace) in [
        (&dirty, &dirty_ws),
        (&untracked, &untracked_ws),
        (&committed, &committed_ws),
    ] {
        archive(&state, &discussion.id).await;
        assert!(exists(workspace), "{workspace} holds work and must stay");
        assert_eq!(
            stored_workspace(&state, &discussion.id).await.as_deref(),
            Some(workspace.as_str())
        );
    }
}

#[tokio::test]
async fn copied_agent_configs_do_not_make_a_review_worktree_dirty() {
    let (repo, first, _) = owner_checkout();
    std::fs::write(repo.path().join(".mcp.json"), "{}").unwrap();
    let state = state_with_project(repo.path()).await;
    let (discussion, workspace) = review_of(&state, &first).await;
    assert!(
        std::path::Path::new(&workspace).join(".mcp.json").is_file(),
        "the config was copied in"
    );

    archive(&state, &discussion.id).await;

    assert!(!exists(&workspace));
}

#[tokio::test]
async fn the_sweep_reclaims_archived_reviews_only() {
    let (repo, first, _) = owner_checkout();
    let state = state_with_project(repo.path()).await;
    let (open, open_ws) = review_of(&state, &first).await;
    let (done, done_ws) = review_of(&state, &first).await;
    let (dirty, dirty_ws) = review_of(&state, &first).await;
    std::fs::write(std::path::Path::new(&dirty_ws).join("a.txt"), "edited").unwrap();
    // Archived straight in the database, as if before the archive hook existed.
    state
        .db
        .with_conn({
            let (done, dirty) = (done.id.clone(), dirty.id.clone());
            move |conn| {
                conn.execute(
                    "UPDATE discussions SET archived = 1 WHERE id IN (?1, ?2)",
                    [&done, &dirty],
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();

    let removed = super::crud::sweep_archived_review_worktrees(&state.db).await;

    assert_eq!(removed, 1);
    assert!(exists(&open_ws), "still open");
    assert!(!exists(&done_ws), "archived and clean");
    assert!(exists(&dirty_ws), "archived but dirty");
    assert!(stored_workspace(&state, &open.id).await.is_some());
    assert_eq!(stored_workspace(&state, &done.id).await, None);
}

#[tokio::test]
async fn archiving_a_non_review_discussion_leaves_its_worktree_alone() {
    let (repo, first, _) = owner_checkout();
    let state = state_with_project(repo.path()).await;
    let (other, other_ws) = review_of(&state, &first).await;
    // Same folder, but not a review discussion.
    state
        .db
        .with_conn({
            let id = other.id.clone();
            move |conn| {
                conn.execute(
                    "UPDATE discussions SET workspace_mode = 'Isolated' WHERE id = ?1",
                    [&id],
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();

    archive(&state, &other.id).await;

    assert!(exists(&other_ws));
}

#[test]
fn only_a_review_worktree_can_be_released() {
    let (repo, first, _) = owner_checkout();
    let review = crate::core::worktree::create_review_worktree(
        repo.path(),
        "Review project",
        "11111111-aaaa",
        &first,
        None,
    )
    .unwrap();

    for foreign in [
        repo.path().to_string_lossy().into_owned(),
        repo.path().join("wip.txt").to_string_lossy().into_owned(),
        repo.path()
            .join(".kronn/worktrees/task-kt-1")
            .to_string_lossy()
            .into_owned(),
        repo.path()
            .join("elsewhere--review-1")
            .to_string_lossy()
            .into_owned(),
    ] {
        assert!(
            crate::core::worktree::release_review_worktree(repo.path(), &foreign).is_err(),
            "{foreign}"
        );
    }
    assert!(repo.path().join("wip.txt").exists());

    assert_eq!(
        crate::core::worktree::release_review_worktree(repo.path(), &review.path).unwrap(),
        crate::core::worktree::ReviewRelease::Removed
    );
    // Releasing it again is harmless.
    assert_eq!(
        crate::core::worktree::release_review_worktree(repo.path(), &review.path).unwrap(),
        crate::core::worktree::ReviewRelease::Removed
    );
}
