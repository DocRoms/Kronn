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
        .unwrap()
}

async fn admitted_run(
    state: &AppState,
    launch_id: String,
    before_check: impl FnOnce(&rusqlite::Connection) + Send + 'static,
) -> Result<crate::models::WorkflowRun, String> {
    let admission: crate::api::workflows::RunAdmission = Box::new(move |conn, snapshot| {
        before_check(conn);
        Ok(
            crate::db::live_page_action_trusts::admit_run(conn, &launch_id, snapshot)?
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
