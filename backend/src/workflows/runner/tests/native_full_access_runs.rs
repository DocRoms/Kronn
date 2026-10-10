//! KT-1086 — an Agent step on a native ACP agent whose full access is off
//! fails at once with the runtime's typed refusal: not a quota wait, not
//! retried, and the rollback chain runs as for any failed step.

use super::quota_wait_runs::{fixture, stored, SESSION_LIMIT};
use super::*;

#[tokio::test]
async fn a_native_step_without_full_access_fails_typed_without_retry_or_quota() {
    let _off = crate::core::config::test_saved_access::set(&AgentType::CopilotCli, false);
    // Claude answers every prompt: only the Copilot step can fail.
    let mut fx = fixture("native-access", SESSION_LIMIT, 0, 3600).await;
    let analyse = &mut fx.workflow.steps[1];
    analyse.agent = AgentType::CopilotCli;
    analyse.retry = Some(RetryConfig {
        max_retries: 2,
        backoff: "exponential".into(),
    });
    let mut undo = fake_step("undo");
    undo.prompt_template = "UNDO-MARKER".into();
    fx.workflow.on_failure = vec![undo];

    let (_, tokens, agents) = test_state_and_configs();
    let mut run = pending_run("run-native-access", &fx.workflow.id);
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
    .expect("the run ends, it does not error");
    let ended = stored(&fx.state, "run-native-access").await;

    assert_eq!(ended.status, RunStatus::Failed, "{:?}", ended.step_results);
    let names: Vec<&str> = ended
        .step_results
        .iter()
        .map(|r| r.step_name.as_str())
        .collect();
    assert_eq!(names, vec!["prepare", "analyse", "undo"]);
    let refused = &ended.step_results[1];
    assert_eq!(refused.status, RunStatus::Failed);
    assert_eq!(
        refused.output,
        "native_full_access_required: GitHub Copilot runs only with full access. Enable it in \
         Config › Agents › GitHub Copilot › Full access, or choose another agent."
    );
    assert_eq!(refused.quota_wait, None, "not a quota refusal");
    assert_eq!(refused.terminal_stop, None, "an ordinary failure");
    assert_eq!(refused.tokens_used, Some(0));
    let attempts = &refused.agent_provenance.as_ref().unwrap().attempts;
    assert!(
        attempts.is_empty(),
        "never launched, never retried: {attempts:?}"
    );
    assert!(!ended
        .state
        .contains_key(crate::workflows::quota_wait::QUOTA_ATTEMPTS_STATE_KEY));
    // `on_failure` runs on a Failed step that is not a terminal stop.
    let rollback = &ended.step_results[2];
    assert!(rollback.is_rollback);
    assert_eq!(rollback.status, RunStatus::Success);
    assert_eq!(fx.prompt_markers(), vec!["prepare", "other"]);
}
