use axum::extract::{Path, State};
use axum::Json;

use super::*;
use crate::db::live_page_action_trusts::tests::{block, setup, workflow, ACTION, BOUND, PAGE};

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let mut config = crate::core::config::default_config();
    config.encryption_secret = Some(crate::core::crypto::generate_secret());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(config));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn agent() -> Option<axum::Extension<crate::core::bridge_token::BridgeCaller>> {
    Some(axum::Extension(crate::core::bridge_token::BridgeCaller {
        token_id: "t".into(),
        project: None,
        own_discussions: vec![],
        own_run: None,
        agent: None,
    }))
}

async fn seeded() -> (AppState, String) {
    let state = state();
    state
        .db
        .with_conn(|conn| {
            setup(conn, &workflow(), None, &block("todo-move", BOUND));
            Ok(())
        })
        .await
        .unwrap();
    let Json(listed) = trusts_for_live_page(State(state.clone()), Path(PAGE.into())).await;
    let fingerprint = listed.data.unwrap()[0].fingerprint.clone().unwrap();
    (state, fingerprint)
}

#[test]
fn no_trust_route_is_open_to_a_bridge_token() {
    for (method, pattern) in [
        ("POST", "/api/live-page-actions/{id}/trust"),
        ("DELETE", "/api/live-page-actions/{id}/trust"),
        ("GET", "/api/pages/{id}/action-trusts"),
    ] {
        assert!(
            crate::core::bridge_token::route_for(method, pattern).is_none(),
            "{method} {pattern}"
        );
    }
}

#[tokio::test]
async fn an_agent_identity_can_neither_approve_nor_revoke() {
    let (state, fingerprint) = seeded().await;
    let Json(refused) = trust(
        State(state.clone()),
        agent(),
        Path(ACTION.into()),
        Json(TrustLivePageActionRequest {
            fingerprint: fingerprint.clone(),
        }),
    )
    .await;
    assert!(!refused.success);
    let Json(listed) = trusts_for_live_page(State(state.clone()), Path(PAGE.into())).await;
    assert!(listed.data.unwrap()[0].trust.is_none(), "nothing recorded");

    let Json(approved) = trust(
        State(state.clone()),
        None,
        Path(ACTION.into()),
        Json(TrustLivePageActionRequest { fingerprint }),
    )
    .await;
    assert!(approved.success, "{:?}", approved.error);
    let Json(refused) = revoke_trust(State(state.clone()), agent(), Path(ACTION.into())).await;
    assert!(!refused.success);
    let Json(listed) = trusts_for_live_page(State(state.clone()), Path(PAGE.into())).await;
    assert!(
        listed.data.unwrap()[0].active,
        "an agent cannot withdraw it either"
    );

    let Json(revoked) = revoke_trust(State(state.clone()), None, Path(ACTION.into())).await;
    assert_eq!(revoked.data, Some(true));
}

#[tokio::test]
async fn a_trusted_launch_takes_no_typed_value_nor_agent_choice() {
    let (state, fingerprint) = seeded().await;
    let Json(approved) = trust(
        State(state.clone()),
        None,
        Path(ACTION.into()),
        Json(TrustLivePageActionRequest { fingerprint }),
    )
    .await;
    assert!(approved.success);
    let Json(refused) = launch(
        State(state.clone()),
        Path(ACTION.into()),
        Json(LaunchLivePageActionRequest {
            variables: HashMap::from([("ticket".into(), "T-9".into())]),
            bindings: HashMap::from([("ticket".into(), "T-1".into())]),
            step_agents: None,
            trusted: Some(true),
        }),
    )
    .await;
    assert!(!refused.success);
    let launches: i64 = state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM live_page_action_launches",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(launches, 0);
}

/// Claims one trusted launch of `todo-move` for row T-1, as the click would.
async fn trusted_claim(state: &AppState, fingerprint: String) -> String {
    let Json(approved) = trust(
        State(state.clone()),
        None,
        Path(ACTION.into()),
        Json(TrustLivePageActionRequest { fingerprint }),
    )
    .await;
    assert!(approved.success, "{:?}", approved.error);
    claim_without_approving(state)
        .await
        .expect("the trusted claim was refused")
}

/// Claims one trusted launch of `todo-move` for row T-1 under the approval
/// already stored, or why it was refused.
async fn claim_without_approving(state: &AppState) -> Result<String, String> {
    state
        .db
        .with_conn(|conn| {
            // Each case starts from a fresh row: no launch in flight, no rate limit.
            conn.execute("DELETE FROM live_page_action_launches", [])?;
            match crate::db::live_page_actions::claim_trusted_launch(
                conn,
                ACTION,
                &HashMap::from([("ticket".to_string(), "T-1".to_string())]),
            )?
            .ok_or_else(|| anyhow::anyhow!("the trusted claim was refused"))?
            {
                LivePageActionClaimOutcome::Claimed { action, .. } => Ok(action.id),
                LivePageActionClaimOutcome::Existing(_) => anyhow::bail!("not a fresh claim"),
            }
        })
        .await
        .map_err(|error| error.to_string())
}

async fn admitted_run(
    state: &AppState,
    launch_id: String,
    before_check: impl FnOnce(&rusqlite::Connection) + Send + 'static,
) -> Result<crate::models::WorkflowRun, String> {
    let admission: crate::api::workflows::RunAdmission =
        Box::new(move |conn, snapshot, profiles| {
            before_check(conn);
            Ok(
                crate::db::live_page_action_trusts::admit_run(
                    conn, &launch_id, snapshot, profiles,
                )?
                .map_err(|refusal| refusal.to_string()),
            )
        });
    crate::api::workflows::create_manual_run_admitted(
        state,
        "wf-move",
        HashMap::from([("ticket".to_string(), "T-1".to_string())]),
        Default::default(),
        crate::core::launch_context::LaunchContext::from_live_page(None),
        uuid::Uuid::new_v4().to_string(),
        Some(admission),
    )
    .await
    .map(|(_, run)| run)
}

async fn run_count(state: &AppState) -> i64 {
    state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM workflow_runs", [], |row| row.get(0))?)
        })
        .await
        .unwrap()
}

fn edit_payload(conn: &rusqlite::Connection) {
    let mut workflow = crate::db::workflows::get_workflow(conn, "wf-move")
        .unwrap()
        .unwrap();
    workflow.steps[0].json_data_payload = Some(serde_json::json!({"column": "done"}));
    crate::db::workflows::update_workflow(conn, &workflow).unwrap();
}

/// Codex r1 P1-a: an edit or a revocation landing in the gap is never executed;
/// once admitted, the run is pinned to the approved definition whatever follows.
#[tokio::test]
async fn a_trusted_run_executes_only_the_definition_admitted_with_its_approval() {
    let (state, fingerprint) = seeded().await;
    let launch = trusted_claim(&state, fingerprint.clone()).await;
    let refused = admitted_run(&state, launch, edit_payload).await;
    assert!(refused.is_err(), "an edit in the gap must not run");
    assert_eq!(run_count(&state).await, 0);

    // Restore and re-approve; a revocation in the gap is refused the same way.
    let restored = state
        .db
        .with_conn(|conn| {
            let mut workflow = crate::db::workflows::get_workflow(conn, "wf-move")?
                .ok_or_else(|| anyhow::anyhow!("wf-move is missing"))?;
            workflow.steps[0].json_data_payload = workflow_payload();
            crate::db::workflows::update_workflow(conn, &workflow)?;
            crate::db::live_page_action_trusts::list_for_page(conn, PAGE)?[0]
                .fingerprint
                .clone()
                .ok_or_else(|| anyhow::anyhow!("the approval has no fingerprint"))
        })
        .await
        .unwrap();
    let launch = trusted_claim(&state, restored.clone()).await;
    let refused = admitted_run(&state, launch, |conn| {
        crate::db::live_page_action_trusts::revoke(conn, ACTION).unwrap();
    })
    .await;
    assert!(refused.is_err(), "a revocation in the gap must not run");
    assert_eq!(run_count(&state).await, 0);

    // Admitted: the pin holds the approved definition, an edit after it does not.
    let launch = trusted_claim(&state, restored).await;
    let run = admitted_run(&state, launch, |_| {}).await.unwrap();
    let approved = state
        .db
        .with_conn(|conn| {
            crate::db::workflows::get_workflow(conn, "wf-move")?
                .ok_or_else(|| anyhow::anyhow!("wf-move is missing"))
        })
        .await
        .unwrap();
    state
        .db
        .with_conn(|conn| {
            edit_payload(conn);
            Ok(())
        })
        .await
        .unwrap();
    let run_id = run.id.clone();
    let pinned = state
        .db
        .with_conn(move |conn| crate::workflows::run_pins::pinned_workflow(conn, &run_id))
        .await
        .unwrap()
        .expect("pinned with its admission");
    assert_eq!(
        serde_json::to_value(&pinned.steps).unwrap(),
        serde_json::to_value(&approved.steps).unwrap()
    );
}

fn workflow_payload() -> Option<serde_json::Value> {
    workflow().steps[0].json_data_payload.clone()
}

// ─── KT-920 — the approval, the admission and the pin share one profile ───

const PROJECT: &str = "proj-trust-profile";
const PROFILE: &str = "schema_version = 1\n[validation.targets.lint]\ncommand = \"make lint\"\n";

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
}

fn commit(dir: &std::path::Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", path]);
}

async fn seeded_with_profile() -> (AppState, tempfile::TempDir) {
    let state = state();
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    git(repo.path(), &["config", "user.email", "t@kronn.local"]);
    git(repo.path(), &["config", "user.name", "t"]);
    git(repo.path(), &["config", "commit.gpgsign", "false"]);
    commit(repo.path(), "kronn/project.toml", PROFILE);
    let path = repo.path().to_string_lossy().to_string();
    state
        .db
        .with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            let project: crate::models::Project = serde_json::from_value(serde_json::json!({
                "id": PROJECT, "name": PROJECT, "path": path,
                "repo_url": null, "token_override": null,
                "ai_config": {"detected": false, "configs": []},
                "created_at": now, "updated_at": now,
            }))?;
            crate::db::projects::insert_project(conn, &project)?;
            let mut targeted = workflow();
            targeted.project_id = Some(PROJECT.into());
            setup(conn, &targeted, Some(PROJECT), &block("todo-move", BOUND));
            Ok(())
        })
        .await
        .unwrap();
    (state, repo)
}

async fn listed_state(
    state: &AppState,
) -> crate::db::live_page_action_trusts::LivePageActionTrustState {
    let Json(listed) = trusts_for_live_page(State(state.clone()), Path(PAGE.into())).await;
    listed
        .data
        .expect("listed")
        .into_iter()
        .find(|row| row.action_id == ACTION)
        .unwrap()
}

async fn approve_listed(state: &AppState) {
    let fingerprint = listed_state(state).await.fingerprint.expect("eligible");
    let Json(approved) = trust(
        State(state.clone()),
        None,
        Path(ACTION.into()),
        Json(TrustLivePageActionRequest { fingerprint }),
    )
    .await;
    assert!(approved.success, "{:?}", approved.error);
}

#[tokio::test]
async fn a_profile_command_change_on_main_invalidates_the_approval() {
    let (state, repo) = seeded_with_profile().await;
    approve_listed(&state).await;
    commit(repo.path(), "README.md", "unrelated");
    assert!(
        listed_state(&state).await.active,
        "a new commit, same values"
    );

    commit(
        repo.path(),
        "kronn/project.toml",
        &PROFILE.replace("make lint", "make lint && curl evil"),
    );
    let changed = listed_state(&state).await;
    assert!(!changed.active);
    assert_eq!(
        changed.trust.unwrap().invalidated_reason,
        Some(LivePageActionTrustRefusal::Changed)
    );
}

#[tokio::test]
async fn the_admitted_run_pins_the_profile_snapshot_it_was_checked_against() {
    let (state, repo) = seeded_with_profile().await;
    approve_listed(&state).await;
    let fingerprint = listed_state(&state).await.fingerprint.unwrap();
    let launch = trusted_claim(&state, fingerprint).await;
    let expected = crate::core::project_profile::snapshot(Some(repo.path()));
    let run = admitted_run(&state, launch, |_| {})
        .await
        .expect("admitted");
    commit(
        repo.path(),
        "kronn/project.toml",
        &PROFILE.replace("make lint", "make other"),
    );
    let run_id = run.id.clone();
    let pinned = state
        .db
        .with_conn(move |conn| {
            crate::workflows::run_pins::pinned_project_profile(conn, &run_id, Some(PROJECT))
        })
        .await
        .unwrap()
        .expect("the admission pinned the profile");
    assert_eq!(pinned, expected);
    assert_eq!(
        pinned.values["project.validation.targets.lint.command"],
        "make lint"
    );
}

#[tokio::test]
async fn a_profile_change_between_the_claim_and_the_admission_is_refused() {
    let (state, repo) = seeded_with_profile().await;
    approve_listed(&state).await;
    let fingerprint = listed_state(&state).await.fingerprint.unwrap();
    let launch = trusted_claim(&state, fingerprint).await;
    commit(
        repo.path(),
        "kronn/project.toml",
        &PROFILE.replace("make lint", "make lint && curl evil"),
    );
    assert!(admitted_run(&state, launch, |_| {}).await.is_err());
    assert_eq!(run_count(&state).await, 0);
}

// ─── KT-1100 x KT-1029 — retention is not a revision, a step is ───

/// A human edit of `wf-move` through the workflow API.
async fn human_edit(state: &AppState, body: serde_json::Value) {
    let Json(saved) = crate::api::workflows::update_as_labeled(
        state.clone(),
        "wf-move".into(),
        serde_json::from_value(body).unwrap(),
        crate::api::workflows::WorkflowWriter::Human,
        None,
    )
    .await;
    assert!(saved.success, "{:?}", saved.error);
}

/// `wf-move`'s steps with the first one's payload set to `column`.
async fn steps_with(state: &AppState, column: &str) -> serde_json::Value {
    let column = column.to_string();
    let workflow = state
        .db
        .with_conn(|conn| {
            crate::db::workflows::get_workflow(conn, "wf-move")?
                .ok_or_else(|| anyhow::anyhow!("wf-move is missing"))
        })
        .await
        .unwrap();
    let mut steps = workflow.steps;
    steps[0].json_data_payload = Some(serde_json::json!({ "column": column }));
    serde_json::to_value(steps).unwrap()
}

/// The seeded action approved once, its workflow saved by a human first.
async fn approved_by_a_human() -> AppState {
    let (state, _) = seeded().await;
    let steps = steps_with(&state, "todo").await;
    human_edit(&state, serde_json::json!({ "steps": steps })).await;
    approve_listed(&state).await;
    state
}

#[tokio::test]
async fn a_human_retention_edit_keeps_the_approval_and_the_run_is_admitted() {
    let state = approved_by_a_human().await;
    let approval = listed_state(&state).await.trust.unwrap().approval_id;
    for retention in [
        serde_json::json!({"success_days": 7, "failure_days": 0}),
        serde_json::Value::Null,
    ] {
        human_edit(&state, serde_json::json!({ "retention": retention })).await;
        let listed = listed_state(&state).await;
        assert!(listed.active, "retention {retention} kept the approval");
        let trust = listed.trust.unwrap();
        assert_eq!(trust.invalidated_at, None);
        assert_eq!(trust.approval_id, approval, "never re-approved");
        let launch = claim_without_approving(&state).await.unwrap();
        let run = admitted_run(&state, launch, |_| {}).await;
        assert!(run.is_ok(), "retention {retention}: {run:?}");
    }
    assert_eq!(run_count(&state).await, 2);
}

#[tokio::test]
async fn a_human_step_edit_invalidates_the_approval_even_once_reverted() {
    let state = approved_by_a_human().await;
    let done = steps_with(&state, "done").await;
    human_edit(&state, serde_json::json!({ "steps": done })).await;
    let todo = steps_with(&state, "todo").await;
    human_edit(&state, serde_json::json!({ "steps": todo })).await;

    let listed = listed_state(&state).await;
    assert!(!listed.active, "the reverted step does not restore it");
    assert_eq!(
        listed.trust.unwrap().invalidated_reason,
        Some(LivePageActionTrustRefusal::Changed)
    );
    assert!(claim_without_approving(&state).await.is_err());
    assert_eq!(run_count(&state).await, 0);
}
