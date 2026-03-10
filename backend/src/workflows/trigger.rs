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
