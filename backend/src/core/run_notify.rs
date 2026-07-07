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

/// Resolve the effective notify URL (config first, then env), trimmed & non-empty.
fn resolve_url(cfg_url: Option<String>) -> Option<String> {
    cfg_url
        .or_else(|| std::env::var("KRONN_FAILURE_NOTIFY_URL").ok())
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
}

/// Fire the failure webhook if the run failed and a URL is configured.
pub async fn notify_if_failed(state: &AppState, workflow: &Workflow, run: &WorkflowRun) {
    if !should_notify(&run.status) {
        return;
    }
    let cfg_url = { state.config.read().await.server.failure_notify_url.clone() };
    let Some(url) = resolve_url(cfg_url) else {
        return;
    };
    let body = build_payload(&workflow.name, &run.status, &run.id, &run.started_at.to_rfc3339());
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
            tracing::info!("Failure notification sent for run {} ({:?})", run.id, run.status)
        }
        Ok(r) => tracing::warn!(
            "Failure notification returned {} for run {}",
            r.status(),
            run.id
        ),
        Err(e) => tracing::warn!("Failure notification POST failed for run {}: {}", run.id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolve_url_prefers_config_trims_and_rejects_empty() {
        assert_eq!(resolve_url(Some("  https://hook  ".into())).as_deref(), Some("https://hook"));
        assert_eq!(resolve_url(Some("   ".into())), None);
        // No config, no env → None.
        std::env::remove_var("KRONN_FAILURE_NOTIFY_URL");
        assert_eq!(resolve_url(None), None);
    }
}
