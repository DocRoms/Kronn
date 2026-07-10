//! B6 (0.8.11) — failure notifications for scheduled / auto-triggered workflow
//! runs. An autonomous cron that dies mid-run used to be discovered only by
//! opening the UI; when `server.failure_notify_url` (or the
//! `KRONN_FAILURE_NOTIFY_URL` env) is set, a run ending in a non-success
//! terminal state POSTs a Slack/Teams/generic-JSON message instead.
//!
//! Best-effort by design: a dead or slow webhook must NEVER affect the run
//! itself — every error is logged and swallowed.
use crate::models::{RunStatus, Workflow, WorkflowRun};
use crate::AppState;

/// True for terminal states that warrant an alert — a real failure, a
/// guard-stop, or an interruption (backend died mid-run). Success / Cancelled
/// (user-initiated) / non-terminal states never notify.
pub fn should_notify(status: &RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Failed | RunStatus::Interrupted | RunStatus::StoppedByGuard
    )
}

/// Slack/Teams/generic-webhook compatible JSON body (`{"text": …}`). Takes the
/// primitive fields (not the whole run) so it is trivially unit-testable.
pub fn build_payload(workflow_name: &str, status: &RunStatus, run_id: &str, started_at_rfc3339: &str) -> String {
    let label = match status {
        RunStatus::Failed => "FAILED",
        RunStatus::Interrupted => "INTERRUPTED (backend restarted mid-run)",
        RunStatus::StoppedByGuard => "STOPPED BY GUARD",
        _ => "ended",
    };
    let text = format!(
        "⚠️ Kronn — workflow “{}” run {}\n• run_id: {}\n• started: {}",
        workflow_name, label, run_id, started_at_rfc3339
    );
    serde_json::json!({ "text": text }).to_string()
}

/// Resolve the effective notify URL (config first, then env), trimmed &
/// non-empty. The config value is trimmed/filtered BEFORE the env fallback —
/// an empty/whitespace config entry must not mask a valid env URL.
fn resolve_url(cfg_url: Option<String>) -> Option<String> {
    let non_empty = |u: String| {
        let u = u.trim().to_string();
        (!u.is_empty()).then_some(u)
    };
    cfg_url
        .and_then(non_empty)
        .or_else(|| std::env::var("KRONN_FAILURE_NOTIFY_URL").ok().and_then(non_empty))
}

/// Fire the failure webhook if the run failed and a URL is configured.
pub async fn notify_if_failed(state: &AppState, workflow: &Workflow, run: &WorkflowRun) {
    if !should_notify(&run.status) {
        return;
    }
    notify_terminal(state, &workflow.name, &run.status, &run.id, &run.started_at.to_rfc3339()).await;
}

/// Webhook the boot-reconciled Interrupted runs. The engine-spawn tail can
/// never notify these — the process that owned them died mid-run; this is
/// the only origin for an `Interrupted` alert ("cron died at 6am" case).
pub async fn notify_boot_interrupted(state: &AppState) {
    for r in state.db.take_boot_interrupted() {
        notify_terminal(state, &r.workflow_name, &RunStatus::Interrupted, &r.run_id, &r.started_at).await;
    }
}

/// Primitive-field variant shared by all notify origins.
pub async fn notify_terminal(
    state: &AppState,
    workflow_name: &str,
    status: &RunStatus,
    run_id: &str,
    started_rfc3339: &str,
) {
    let cfg_url = { state.config.read().await.server.failure_notify_url.clone() };
    let Some(url) = resolve_url(cfg_url) else {
        return;
    };
    let body = build_payload(workflow_name, status, run_id, started_rfc3339);
    // Bounded client: reqwest's default has NO request timeout, so a hanging
    // webhook endpoint would pin this task forever and accumulate across
    // failed runs — the opposite of "best-effort" (Copilot review, PR #114).
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
    else {
        tracing::warn!("Failure notification skipped: could not build HTTP client");
        return;
    };
    let send = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;
    match send {
        Ok(r) if r.status().is_success() => {
            tracing::info!("Failure notification sent for run {} ({:?})", run_id, status)
        }
        Ok(r) => tracing::warn!(
            "Failure notification returned {} for run {}",
            r.status(),
            run_id
        ),
        Err(e) => tracing::warn!("Failure notification POST failed for run {}: {}", run_id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn should_notify_only_on_non_success_terminal() {
        assert!(should_notify(&RunStatus::Failed));
        assert!(should_notify(&RunStatus::Interrupted));
        assert!(should_notify(&RunStatus::StoppedByGuard));
        assert!(!should_notify(&RunStatus::Success));
        assert!(!should_notify(&RunStatus::Cancelled));
        assert!(!should_notify(&RunStatus::Running));
        assert!(!should_notify(&RunStatus::WaitingApproval));
    }

    #[test]
    fn payload_is_slack_shaped_and_names_the_workflow_and_status() {
        let body = build_payload("PR Review cron", &RunStatus::Failed, "run-1", "2026-07-07T06:00:00Z");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let text = v["text"].as_str().unwrap();
        assert!(text.contains("PR Review cron"));
        assert!(text.contains("FAILED"));
        assert!(text.contains("run-1"));
    }

    #[test]
    #[serial]
    fn resolve_url_prefers_config_trims_and_rejects_empty() {
        // Env-free assertions first (this test is the only env manipulator).
        std::env::remove_var("KRONN_FAILURE_NOTIFY_URL");
        assert_eq!(resolve_url(Some("  https://hook  ".into())).as_deref(), Some("https://hook"));
        assert_eq!(resolve_url(Some("   ".into())), None);
        // No config, no env → None.
        assert_eq!(resolve_url(None), None);

        // An empty/whitespace config value must NOT mask a valid env URL.
        std::env::set_var("KRONN_FAILURE_NOTIFY_URL", "  https://env-hook  ");
        assert_eq!(resolve_url(Some("   ".into())).as_deref(), Some("https://env-hook"));
        assert_eq!(resolve_url(None).as_deref(), Some("https://env-hook"));
        // A real config value still wins over env.
        assert_eq!(resolve_url(Some("https://cfg-hook".into())).as_deref(), Some("https://cfg-hook"));
        std::env::remove_var("KRONN_FAILURE_NOTIFY_URL");
    }
}
