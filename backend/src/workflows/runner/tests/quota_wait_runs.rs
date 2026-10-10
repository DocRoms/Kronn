//! KT-811 — a provider quota refusal parks the run at its step and the engine
//! resumes it there after the reset, through the ordinary resume path.

use super::*;
use crate::models::{QuotaParkReason, WorkflowGuards};
use std::sync::{Arc, Mutex};

const SESSION_LIMIT: &str = "[Agent provider error] You've hit your session limit · resets 9:40pm (Europe/Paris) (HTTP 429; terminal_reason=api_error)";

/// Claude over ACP: refuses prompts naming `refused_marker` while `refusals`
/// remain, answers everything else, and records every prompt it is handed.
struct QuotaThenAnswer {
    refusal: String,
    refused_marker: &'static str,
    refusals: Mutex<usize>,
    prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl crate::acp::AcpTransport for QuotaThenAnswer {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, crate::acp::AcpError> {
        Ok(crate::acp::AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: std::collections::BTreeSet::from([
                crate::acp::AcpCapability::Sessions,
                crate::acp::AcpCapability::Streaming,
                crate::acp::AcpCapability::Cancellation,
                crate::acp::AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, crate::acp::AcpError> {
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "quota-turn")
    }
    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn resume_session(
        &self,
        _: &crate::acp::AcpSessionTarget,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        prompt: &str,
        events: tokio::sync::mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        use crate::acp::AcpSessionEvent;
        self.prompts.lock().unwrap().push(prompt.to_string());
        if prompt.contains(self.refused_marker) {
            let mut left = self.refusals.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                return Err(crate::acp::AcpError::Transport(self.refusal.clone()));
            }
        }
        let _ = events.send(AcpSessionEvent::TextDelta("done".into())).await;
        let _ = events.send(AcpSessionEvent::Completed).await;
        Ok(())
    }
    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
}

struct Fixture {
    state: crate::AppState,
    workflow: Workflow,
    prompts: Arc<Mutex<Vec<String>>>,
    _repo: tempfile::TempDir,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
}

impl Fixture {
    /// Agent prompts the provider received, by step marker.
    fn prompt_markers(&self) -> Vec<&'static str> {
        self.prompts
            .lock()
            .unwrap()
            .iter()
            .map(|p| {
                if p.contains("PREPARE-MARKER") {
                    "prepare"
                } else if p.contains("ANALYSE-MARKER") {
                    "analyse"
                } else {
                    "other"
                }
            })
            .collect()
    }
}

/// `prepare` then `analyse` (the step the provider refuses), on a git project
/// whose agent is served by [`QuotaThenAnswer`].
async fn fixture(id: &str, refusal: &str, refusals: usize, timeout_seconds: u64) -> Fixture {
    let (state, _tokens, _agents) = test_state_and_configs();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let repo_path = repo.path().to_string_lossy().into_owned();
    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id": format!("proj-{id}"), "name": id, "path": repo_path,
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": Utc::now().to_rfc3339(), "updated_at": Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        Arc::new(QuotaThenAnswer {
            refusal: refusal.to_string(),
            refused_marker: "ANALYSE-MARKER",
            refusals: Mutex::new(refusals),
            prompts: prompts.clone(),
        }),
    );
    let mut workflow = make_workflow_with_artifacts(Default::default());
    workflow.id = format!("wf-{id}");
    workflow.project_id = Some(format!("proj-{id}"));
    workflow.guards = Some(WorkflowGuards {
        timeout_seconds: Some(timeout_seconds),
        ..Default::default()
    });
    let mut prepare = fake_step("prepare");
    prepare.prompt_template = "PREPARE-MARKER".into();
    let mut analyse = fake_step("analyse");
    analyse.prompt_template = "ANALYSE-MARKER".into();
    workflow.steps = vec![prepare, analyse];
    Fixture {
        state,
        workflow,
        prompts,
        _repo: repo,
        _route: route,
    }
}

async fn run_until_paused(fx: &Fixture, run_id: &str) -> WorkflowRun {
    let (_, tokens, agents) = test_state_and_configs();
    let mut run = pending_run(run_id, &fx.workflow.id);
    insert_wf_and_run(&fx.state, &fx.workflow, &run).await;
    execute_run(
        fx.state.clone(),
        &fx.workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .expect("the run pauses, it does not error");
    stored(&fx.state, run_id).await
}

async fn stored(state: &crate::AppState, run_id: &str) -> WorkflowRun {
    let id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &id))
        .await
        .unwrap()
        .expect("run row")
}

/// The backend restarting: same database, nothing else carried over.
fn restarted(state: &crate::AppState) -> crate::AppState {
    let cfg = crate::core::config::default_config();
    crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg)),
        state.db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

#[tokio::test]
async fn a_session_limit_waits_then_resumes_at_its_step_after_a_restart() {
    let fx = fixture("quota-wait", SESSION_LIMIT, 1, 3 * 24 * 3600).await;
    let paused = run_until_paused(&fx, "run-quota-wait").await;

    assert_eq!(paused.status, RunStatus::WaitingQuota);
    assert_eq!(paused.finished_at, None);
    assert_eq!(paused.step_results.len(), 2);
    assert_eq!(paused.step_results[0].status, RunStatus::Success);
    let refused = &paused.step_results[1];
    assert_eq!(refused.step_name, "analyse");
    assert_eq!(refused.status, RunStatus::WaitingQuota);
    assert!(
        refused.output.contains("session limit"),
        "{}",
        refused.output
    );
    let wait = refused.quota_wait.clone().expect("classified as quota");
    let reset = wait.reset_at.expect("9:40pm Europe/Paris is a reset");
    let wake_at = wait.wake_at.expect("a wake-up is scheduled");
    assert_eq!(wake_at, reset + chrono::Duration::minutes(2));
    assert_eq!(wait.parked, None);
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse"]);

    // Before the wake-up nothing happens; after a restart the row alone drives it.
    let state = restarted(&fx.state);
    let early =
        crate::workflows::quota_wait::wake_due_runs(&state, wake_at - chrono::Duration::seconds(1))
            .await;
    assert!(early.is_empty());
    let woken = crate::workflows::quota_wait::wake_due_runs(&state, wake_at).await;
    assert_eq!(woken, vec!["run-quota-wait".to_string()]);
    // A second engine tick cannot claim it again.
    assert!(crate::workflows::quota_wait::wake_due_runs(&state, wake_at)
        .await
        .is_empty());

    let done = wait_until_settled(&state, "run-quota-wait").await;
    assert_eq!(done.status, RunStatus::Success);
    assert_eq!(done.step_results.len(), 2);
    assert_eq!(done.step_results[1].status, RunStatus::Success);
    assert_eq!(done.step_results[1].quota_wait, None);
    // `prepare` ran once: only the refused step was replayed.
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse", "analyse"]);
    assert!(!done
        .state
        .contains_key(crate::workflows::quota_wait::QUOTA_ATTEMPTS_STATE_KEY));
    let history = done
        .state
        .get(RUN_RESUME_HISTORY_KEY)
        .expect("resume trail");
    assert!(history.contains("WaitingQuota"), "{history}");
}

#[tokio::test]
async fn a_new_refusal_right_after_the_wake_up_backs_off_instead_of_looping() {
    let refusal = "HTTP 429 Too Many Requests: rate_limit_exceeded, retry-after: 1";
    let fx = fixture("quota-again", refusal, 2, 3 * 24 * 3600).await;
    let first = run_until_paused(&fx, "run-quota-again").await;
    let first_wait = first.step_results[1].quota_wait.clone().unwrap();
    assert_eq!(first_wait.attempt, 1);
    let first_wake = first_wait.wake_at.unwrap();
    // The reset is a second away: the floor, not the reset, sets the wake-up.
    assert!(first_wake - first.step_results[1].started_at.unwrap() >= chrono::Duration::minutes(4));

    let woken = crate::workflows::quota_wait::wake_due_runs(&fx.state, first_wake).await;
    assert_eq!(woken.len(), 1);
    let second = wait_until_settled(&fx.state, "run-quota-again").await;
    assert_eq!(second.status, RunStatus::WaitingQuota);
    let second_wait = second.step_results[1].quota_wait.clone().unwrap();
    assert_eq!(second_wait.attempt, 2);
    assert!(second_wait.wake_at.unwrap() - Utc::now() >= chrono::Duration::minutes(9));
    // Not due yet, even though the announced reset has long passed.
    assert!(
        crate::workflows::quota_wait::wake_due_runs(&fx.state, Utc::now())
            .await
            .is_empty()
    );
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse", "analyse"]);
}

#[tokio::test]
async fn a_run_cancelled_while_waiting_is_never_woken_even_after_a_restart() {
    let fx = fixture("quota-cancel", SESSION_LIMIT, 1, 3 * 24 * 3600).await;
    let paused = run_until_paused(&fx, "run-quota-cancel").await;
    let wake_at = paused.step_results[1]
        .quota_wait
        .as_ref()
        .and_then(|w| w.wake_at)
        .unwrap();
    let outcome = crate::workflows::cancellation::cancel_run_tree(
        &fx.state,
        "run-quota-cancel",
        crate::workflows::cancellation::CancellationScope::RunTree,
        "cancelled_by_operator",
    )
    .await
    .unwrap();
    assert!(outcome.run_cancelled);
    assert_eq!(
        stored(&fx.state, "run-quota-cancel").await.status,
        RunStatus::Cancelled
    );

    let state = restarted(&fx.state);
    let woken =
        crate::workflows::quota_wait::wake_due_runs(&state, wake_at + chrono::Duration::days(1))
            .await;
    assert!(woken.is_empty());
    let after = stored(&state, "run-quota-cancel").await;
    assert_eq!(after.status, RunStatus::Cancelled);
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse"]);
    // A manual resume is refused too: Cancelled stays sticky.
    let mut run = after;
    assert!(claim_interrupted_run(&state, &mut run, false)
        .await
        .is_err());
}

#[tokio::test]
async fn without_a_reset_time_the_run_is_parked_and_resumes_by_hand() {
    let refusal = "[Agent provider error] You've hit your usage limit. Upgrade to Pro or try again at 3:05 PM. (HTTP 429)";
    let fx = fixture("quota-parked", refusal, 1, 3 * 24 * 3600).await;
    let parked = run_until_paused(&fx, "run-quota-parked").await;
    assert_eq!(parked.status, RunStatus::WaitingQuota);
    let wait = parked.step_results[1].quota_wait.clone().unwrap();
    assert_eq!(wait.reset_at, None);
    assert_eq!(wait.wake_at, None);
    assert_eq!(wait.parked, Some(QuotaParkReason::NoResetTime));

    // The engine never wakes a parked run.
    assert!(crate::workflows::quota_wait::wake_due_runs(
        &fx.state,
        Utc::now() + chrono::Duration::days(30)
    )
    .await
    .is_empty());

    // The human "Resume" goes through the same endpoint as an interrupted run.
    let axum::Json(response) = crate::api::workflows::resume_interrupted(
        axum::extract::State(fx.state.clone()),
        axum::extract::Path("run-quota-parked".to_string()),
        None,
        axum::body::Bytes::new(),
    )
    .await;
    assert!(response.success, "{:?}", response.error);
    let done = wait_until_settled(&fx.state, "run-quota-parked").await;
    assert_eq!(done.status, RunStatus::Success);
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse", "analyse"]);
}

#[tokio::test]
async fn a_reset_after_the_workflow_deadline_parks_the_run() {
    // Three hours away against the default two-hour wall clock: waking would
    // only meet the Timeout guard.
    let refusal = "HTTP 429 Too Many Requests, retry-after: 10800";
    let fx = fixture("quota-deadline", refusal, 1, 2 * 3600).await;
    let parked = run_until_paused(&fx, "run-quota-deadline").await;
    assert_eq!(parked.status, RunStatus::WaitingQuota);
    let wait = parked.step_results[1].quota_wait.clone().unwrap();
    assert!(wait.reset_at.is_some());
    assert_eq!(wait.wake_at, None);
    assert_eq!(wait.parked, Some(QuotaParkReason::AfterDeadline));
}

#[tokio::test]
async fn a_wake_up_reapplies_the_resume_preconditions_and_parks_when_refused() {
    let fx = fixture("quota-uncertain", SESSION_LIMIT, 1, 3 * 24 * 3600).await;
    let paused = run_until_paused(&fx, "run-quota-uncertain").await;
    let wake_at = paused.step_results[1]
        .quota_wait
        .as_ref()
        .and_then(|w| w.wake_at)
        .unwrap();
    // An external effect whose outcome is unknown must never be replayed
    // automatically (KT-150).
    let intent = serde_json::json!({
        "version": 1, "step_name": "notify", "step_index": 0,
        "step_type": "Notify", "started_at": Utc::now(),
    })
    .to_string();
    fx.state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::set_run_state_key(
                conn,
                "run-quota-uncertain",
                UNCERTAIN_SIDE_EFFECT_STATE_KEY,
                &intent,
                &[RunStatus::WaitingQuota],
            )
        })
        .await
        .unwrap();

    let woken = crate::workflows::quota_wait::wake_due_runs(&fx.state, wake_at).await;
    assert!(woken.is_empty());
    let after = stored(&fx.state, "run-quota-uncertain").await;
    assert_eq!(after.status, RunStatus::WaitingQuota);
    let wait = after.step_results[1].quota_wait.clone().unwrap();
    assert_eq!(wait.parked, Some(QuotaParkReason::NotResumable));
    assert_eq!(wait.wake_at, None);
    assert!(wait.detail.unwrap().contains("external effect"));
    assert!(after.state.contains_key(UNCERTAIN_SIDE_EFFECT_STATE_KEY));
    assert_eq!(fx.prompt_markers(), vec!["prepare", "analyse"]);
}

#[tokio::test]
async fn a_sub_workflow_child_refused_for_quota_fails_but_reads_as_quota() {
    let fx = fixture("quota-child", SESSION_LIMIT, 1, 3 * 24 * 3600).await;
    let (_, tokens, agents) = test_state_and_configs();
    let mut run = pending_run("run-quota-child", &fx.workflow.id);
    run.run_type = "subworkflow".into();
    insert_wf_and_run(&fx.state, &fx.workflow, &run).await;
    execute_run(
        fx.state.clone(),
        &fx.workflow,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let child = stored(&fx.state, "run-quota-child").await;
    assert_eq!(child.status, RunStatus::Failed);
    let step = &child.step_results[1];
    assert_eq!(step.status, RunStatus::Failed);
    let wait = step.quota_wait.clone().expect("still classified as quota");
    assert!(wait.reset_at.is_some());
    assert_eq!(wait.wake_at, None);
}

#[tokio::test]
async fn a_park_from_a_stale_read_never_undoes_a_manual_resume() {
    let fx = fixture("quota-park-race", SESSION_LIMIT, 1, 3 * 24 * 3600).await;
    let stale = run_until_paused(&fx, "run-quota-park-race").await;

    // A human resumes between the tick's read and its park.
    let mut fresh = stored(&fx.state, "run-quota-park-race").await;
    claim_interrupted_run(&fx.state, &mut fresh, false)
        .await
        .expect("the manual resume wins");
    let claimed = stored(&fx.state, "run-quota-park-race").await;
    assert_eq!(claimed.status, RunStatus::Running);

    let parked = crate::workflows::quota_wait::park(
        &fx.state,
        &stale,
        QuotaParkReason::NotResumable,
        "the workflow is disabled".into(),
    )
    .await;
    assert!(!parked);
    let after = stored(&fx.state, "run-quota-park-race").await;
    assert_eq!(after.status, RunStatus::Running);
    assert_eq!(after.state, claimed.state, "the claimed state stays");
    assert!(after.state.contains_key(RUN_RESUME_HISTORY_KEY));
}

#[tokio::test]
async fn an_old_wait_can_neither_park_nor_claim_the_new_one() {
    let refusal = "HTTP 429 Too Many Requests: rate_limit_exceeded, retry-after: 1";
    let fx = fixture("quota-new-wait", refusal, 2, 3 * 24 * 3600).await;
    let stale = run_until_paused(&fx, "run-quota-new-wait").await;
    let first_wake = stale.step_results[1]
        .quota_wait
        .as_ref()
        .unwrap()
        .wake_at
        .unwrap();

    assert_eq!(
        crate::workflows::quota_wait::wake_due_runs(&fx.state, first_wake)
            .await
            .len(),
        1
    );
    let second = wait_until_settled(&fx.state, "run-quota-new-wait").await;
    assert_eq!(second.status, RunStatus::WaitingQuota);
    let new_wait = second.step_results[1].quota_wait.clone().unwrap();
    assert_ne!(
        new_wait.id,
        stale.step_results[1].quota_wait.as_ref().unwrap().id
    );

    assert!(
        !crate::workflows::quota_wait::park(
            &fx.state,
            &stale,
            QuotaParkReason::NotResumable,
            "stale".into(),
        )
        .await
    );
    let mut stale_copy = stale.clone();
    assert!(
        try_claim_interrupted_run_row(&fx.state, &mut stale_copy, None)
            .await
            .is_err()
    );
    let after = stored(&fx.state, "run-quota-new-wait").await;
    assert_eq!(after.status, RunStatus::WaitingQuota);
    assert_eq!(after.step_results[1].quota_wait, Some(new_wait.clone()));
    assert_eq!(after.state, second.state);

    // The current wait parks normally, and only its wait fields change.
    assert!(
        crate::workflows::quota_wait::park(
            &fx.state,
            &after,
            QuotaParkReason::NotResumable,
            "the workflow is disabled".into(),
        )
        .await
    );
    let parked = stored(&fx.state, "run-quota-new-wait").await;
    let wait = parked.step_results[1].quota_wait.clone().unwrap();
    assert_eq!(wait.parked, Some(QuotaParkReason::NotResumable));
    assert_eq!(wait.wake_at, None);
    assert_eq!(wait.id, new_wait.id);
    assert_eq!(wait.reset_at, new_wait.reset_at);
    assert_eq!(parked.state, second.state);
    assert_eq!(parked.step_results[1].output, second.step_results[1].output);
}
