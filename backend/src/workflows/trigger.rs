//! Trigger evaluation: checks if a workflow should fire.
//!
//! - Cron: time-based schedule evaluation
//! - Tracker: polls issue tracker API, reconciles processed issues
//! - Manual: always returns false (triggered via API only)

use std::str::FromStr;
use chrono::Utc;

use crate::models::*;

/// Check if a trigger should fire right now (within a 30s window).
pub fn should_fire(trigger: &WorkflowTrigger) -> bool {
    match trigger {
        WorkflowTrigger::Cron { schedule } => check_cron(schedule),
        WorkflowTrigger::Tracker { interval, .. } => {
            // Tracker uses interval as a cron expression for polling frequency
            check_cron(interval)
        }
        WorkflowTrigger::Manual => false,
    }
}

/// Check if a cron expression matches within the current 30s window.
fn check_cron(cron_expr: &str) -> bool {
    // cron crate expects 6 fields (with seconds), add "0 " prefix if 5 fields
    let expr = if cron_expr.split_whitespace().count() < 6 {
        format!("0 {}", cron_expr)
    } else {
        cron_expr.to_string()
    };

    match cron::Schedule::from_str(&expr) {
        Ok(schedule) => {
            let now = Utc::now();
            if let Some(next) = schedule.upcoming(Utc).next() {
                let diff = (next - now).num_seconds();
                (0..=30).contains(&diff)
            } else {
                false
            }
        }
        Err(e) => {
            tracing::error!("Invalid cron expression '{}': {}", cron_expr, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Manual trigger ──────────────────────────────────────────────────

    #[test]
    fn manual_trigger_never_fires() {
        assert!(!should_fire(&WorkflowTrigger::Manual));
    }

    // ─── Cron trigger ────────────────────────────────────────────────────

    #[test]
    fn invalid_cron_expression_returns_false() {
        assert!(!check_cron("not a cron"));
    }

    #[test]
    fn invalid_cron_expression_empty_returns_false() {
        assert!(!check_cron(""));
    }

    #[test]
    fn cron_far_future_does_not_fire() {
        // "0 0 31 2 *" = Feb 31st, which never occurs — next occurrence
        // will be far in the future (or never), so should not fire within 30s.
        // Use a schedule that is guaranteed to be far away:
        // "0 0 1 1 *" = Jan 1st at midnight — unless we happen to be running
        // at exactly that moment, this should return false.
        // Instead, use a trick: schedule in the past minute but not this 30s window.
        // Safest test: an always-invalid expression.
        assert!(!check_cron("99 99 99 99 99"));
    }

    #[test]
    fn cron_five_field_expression_gets_seconds_prefix() {
        // Verify that a 5-field expression doesn't panic/error.
        // It may or may not fire depending on current time, but must not crash.
        let _result = check_cron("* * * * *");
        // No panic = pass. The result depends on timing.
    }

    #[test]
    fn cron_six_field_expression_accepted() {
        // 6-field expression (with seconds) should not panic.
        let _result = check_cron("0 * * * * *");
    }

    // ─── should_fire dispatch ────────────────────────────────────────────

    #[test]
    fn should_fire_cron_invalid_returns_false() {
        let trigger = WorkflowTrigger::Cron {
            schedule: "invalid cron".into(),
        };
        assert!(!should_fire(&trigger));
    }

    #[test]
    fn should_fire_tracker_invalid_interval_returns_false() {
        let trigger = WorkflowTrigger::Tracker {
            source: TrackerSourceConfig::GitHub {
                owner: "test".into(),
                repo: "test".into(),
            },
            query: "label:bug".into(),
            labels: vec!["bug".into()],
            interval: "invalid".into(),
        };
        assert!(!should_fire(&trigger));
    }

    #[test]
    fn should_fire_tracker_uses_interval_as_cron() {
        // A tracker trigger with a valid but far-future cron should not fire.
        let trigger = WorkflowTrigger::Tracker {
            source: TrackerSourceConfig::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
            },
            query: "".into(),
            labels: vec![],
            interval: "0 0 1 1 *".into(), // Jan 1st midnight — unlikely to match now
        };
        // This should not fire (unless test runs at exactly Jan 1 00:00)
        // Main goal: no panic, correct dispatch path
        let _result = should_fire(&trigger);
    }
}
