//! Trigger evaluation: checks if a workflow should fire.
//!
//! - Cron: time-based schedule evaluation
//! - Tracker: polls issue tracker API, reconciles processed issues
//! - Manual: always returns false (triggered via API only)

use std::str::FromStr;
use chrono::{DateTime, Utc};

use crate::models::*;

/// Did the trigger have an occurrence in the window `(since, now]`?
///
/// The engine passes the previous tick's timestamp as `since`, so each cron
/// occurrence fires EXACTLY ONCE regardless of tick jitter. The old stateless
/// version fired on "next occurrence within 30s of now": with 30s ticks that
/// window ([0s, 31s) after truncation) overlapped itself — an occurrence
/// landing on the seam fired on BOTH surrounding ticks (two concurrent runs
/// of the same cron, ~1 occurrence in 31), and a tick delayed past the window
/// (slow tracker poll starving the loop) silently skipped the occurrence.
pub fn should_fire(trigger: &WorkflowTrigger, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    match trigger {
        WorkflowTrigger::Cron { schedule } => cron_fires_between(schedule, since, now),
        WorkflowTrigger::Tracker { interval, .. } => {
            // Tracker uses interval as a cron expression for polling frequency
            cron_fires_between(interval, since, now)
        }
        WorkflowTrigger::Manual => false,
    }
}

/// True when the cron expression has an occurrence in `(since, now]`.
fn cron_fires_between(cron_expr: &str, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    // cron crate expects 6 fields (with seconds), add "0 " prefix if 5 fields
    let expr = if cron_expr.split_whitespace().count() < 6 {
        format!("0 {}", cron_expr)
    } else {
        cron_expr.to_string()
    };

    match cron::Schedule::from_str(&expr) {
        Ok(schedule) => schedule
            .after(&since)
            .next()
            .map(|occ| occ <= now)
            .unwrap_or(false),
        Err(e) => {
            tracing::error!("Invalid cron expression '{}': {}", cron_expr, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // ─── Manual trigger ──────────────────────────────────────────────────

    #[test]
    fn manual_trigger_never_fires() {
        let now = Utc::now();
        assert!(!should_fire(&WorkflowTrigger::Manual, now - Duration::seconds(30), now));
    }

    // ─── Cron trigger ────────────────────────────────────────────────────

    fn fires(expr: &str, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        cron_fires_between(expr, since, now)
    }

    #[test]
    fn invalid_cron_expression_returns_false() {
        let now = Utc::now();
        assert!(!fires("not a cron", now - Duration::seconds(30), now));
        assert!(!fires("", now - Duration::seconds(30), now));
        assert!(!fires("99 99 99 99 99", now - Duration::seconds(30), now));
    }

    #[test]
    fn occurrence_inside_window_fires() {
        // Deterministic: pick a fixed occurrence and build windows around it.
        // "0 0 7 * * *" = every day at 07:00:00.
        let occ = "2026-07-09T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(fires("0 0 7 * * *", occ - Duration::seconds(30), occ),
            "occurrence exactly at `now` fires (window is right-inclusive)");
        assert!(fires("0 0 7 * * *", occ - Duration::seconds(10), occ + Duration::seconds(20)),
            "occurrence strictly inside the window fires");
    }

    #[test]
    fn occurrence_fires_exactly_once_across_adjacent_windows() {
        // THE double-fire regression (concurrency review, 0.8.11): with the
        // old "next within [0,31)s of now" logic an occurrence at the seam
        // fired on both surrounding 30s ticks. Windows are half-open
        // (since, now] so adjacent windows partition time.
        let occ = "2026-07-09T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let tick1_start = occ - Duration::milliseconds(30_400); // occ − 30.4s
        let tick1_end = tick1_start + Duration::seconds(30);    // occ − 0.4s
        let tick2_end = tick1_end + Duration::seconds(30);      // occ + 29.6s
        let in_first = fires("0 0 7 * * *", tick1_start, tick1_end);
        let in_second = fires("0 0 7 * * *", tick1_end, tick2_end);
        assert!(!in_first, "occurrence is after the first window's end");
        assert!(in_second, "…and fires in the second window");
    }

    #[test]
    fn delayed_tick_still_catches_the_occurrence() {
        // Tick starvation (slow tracker poll): the next evaluation happens
        // 90s late — the occurrence must STILL fire (old logic skipped it).
        let occ = "2026-07-09T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(fires("0 0 7 * * *", occ - Duration::seconds(30), occ + Duration::seconds(90)));
    }

    #[test]
    fn no_occurrence_in_window_does_not_fire() {
        let occ = "2026-07-09T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(!fires("0 0 7 * * *", occ + Duration::seconds(1), occ + Duration::seconds(31)));
    }

    #[test]
    fn cron_five_field_expression_gets_seconds_prefix() {
        // "* * * * *" = every minute; a 61s window always contains one.
        let now = Utc::now();
        assert!(fires("* * * * *", now - Duration::seconds(61), now));
    }

    #[test]
    fn cron_six_field_expression_accepted() {
        let now = Utc::now();
        assert!(fires("0 * * * * *", now - Duration::seconds(61), now));
    }

    // ─── should_fire dispatch ────────────────────────────────────────────

    #[test]
    fn should_fire_cron_invalid_returns_false() {
        let now = Utc::now();
        let trigger = WorkflowTrigger::Cron { schedule: "invalid cron".into() };
        assert!(!should_fire(&trigger, now - Duration::seconds(30), now));
    }

    #[test]
    fn should_fire_tracker_uses_interval_as_cron() {
        let now = Utc::now();
        let trigger = WorkflowTrigger::Tracker {
            source: TrackerSourceConfig::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
            },
            query: "".into(),
            labels: vec![],
            interval: "* * * * *".into(),
        };
        assert!(should_fire(&trigger, now - Duration::seconds(61), now));
        let invalid = WorkflowTrigger::Tracker {
            source: TrackerSourceConfig::GitHub { owner: "o".into(), repo: "r".into() },
            query: "".into(),
            labels: vec![],
            interval: "invalid".into(),
        };
        assert!(!should_fire(&invalid, now - Duration::seconds(61), now));
    }
}
