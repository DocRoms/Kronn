//! Trigger evaluation: checks if a workflow should fire.
//!
//! - Cron: time-based schedule evaluation
//! - Tracker: polls issue tracker API, reconciles processed issues
//! - Manual: always returns false (triggered via API only)
//! - Watch: polls a source on its interval (see `watch_trigger`)

use chrono::{DateTime, Utc};
use std::str::FromStr;

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
        WorkflowTrigger::Cron { schedule, timezone } => {
            cron_fires_between_in(schedule, timezone.as_deref(), since, now)
        }
        WorkflowTrigger::Tracker { interval, .. } => {
            // Tracker uses interval as a cron expression for polling frequency
            cron_fires_between(interval, since, now)
        }
        WorkflowTrigger::Manual => false,
        WorkflowTrigger::Watch(watch) => {
            cron_fires_between_in(&watch.interval, watch.timezone.as_deref(), since, now)
        }
    }
}

/// Refuses a trigger the scheduler could not evaluate as written: an unknown
/// timezone, or a Watch without a source or with an invalid interval.
pub fn validate_trigger(trigger: &WorkflowTrigger) -> Result<(), String> {
    match trigger {
        WorkflowTrigger::Cron { timezone, .. } => validate_timezone(timezone.as_deref()),
        WorkflowTrigger::Tracker { .. } | WorkflowTrigger::Manual => Ok(()),
        WorkflowTrigger::Watch(watch) => {
            validate_timezone(watch.timezone.as_deref())?;
            // Five fields only: a poll more often than once a minute is refused.
            if watch.interval.split_whitespace().count() != 5
                || parse_schedule(&watch.interval).is_err()
            {
                return Err(format!(
                    "Watch trigger: `{}` is not a five-field cron expression \
                     (minute hour day month weekday; at most once a minute).",
                    watch.interval
                ));
            }
            let filled =
                |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
            let api = filled(&watch.api_plugin_slug)
                && filled(&watch.api_config_id)
                && filled(&watch.api_endpoint_path);
            if !api && !filled(&watch.quick_api_id) {
                return Err(
                    "Watch trigger: choose a Quick API, or an API (api_plugin_slug, \
                     api_config_id) and an endpoint path (api_endpoint_path)."
                        .into(),
                );
            }
            if let WatchDetection::JsonPath { path } = &watch.detection {
                serde_json_path::JsonPath::parse(path)
                    .map_err(|e| format!("Watch trigger: invalid JSONPath `{path}`: {e}"))?;
            }
            Ok(())
        }
    }
}

fn validate_timezone(timezone: Option<&str>) -> Result<(), String> {
    match timezone {
        None => Ok(()),
        Some(name) => name.parse::<chrono_tz::Tz>().map(|_| ()).map_err(|_| {
            format!("Unknown timezone `{name}`: use an IANA name such as Europe/Paris.")
        }),
    }
}

/// True when the cron expression, read in `timezone` (UTC when absent), has
/// an occurrence in `(since, now]`.
fn cron_fires_between_in(
    cron_expr: &str,
    timezone: Option<&str>,
    since: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    let Some(name) = timezone else {
        return cron_fires_between(cron_expr, since, now);
    };
    let Ok(tz) = name.parse::<chrono_tz::Tz>() else {
        tracing::error!("Unknown cron timezone '{}'", name);
        return false;
    };
    match parse_schedule(cron_expr) {
        // An ambiguous local time yields its earlier instant first, even when
        // it precedes `since`, then the later one: skip both cases.
        Ok(schedule) => schedule
            .after(&since.with_timezone(&tz))
            .take(8)
            .find(|occ| occ.with_timezone(&Utc) > since && !is_repeated_local_time(&tz, occ))
            .map(|occ| occ.with_timezone(&Utc) <= now)
            .unwrap_or(false),
        Err(e) => {
            tracing::error!("Invalid cron expression '{}': {}", cron_expr, e);
            false
        }
    }
}

/// The second pass of a local time the autumn DST change repeats: a schedule
/// fires once, at the first pass. (A local time the spring change skips has
/// no instant, so it does not fire that day.)
fn is_repeated_local_time(tz: &chrono_tz::Tz, occ: &DateTime<chrono_tz::Tz>) -> bool {
    use chrono::TimeZone;
    matches!(
        tz.from_local_datetime(&occ.naive_local()),
        chrono::LocalResult::Ambiguous(_, later) if later == *occ
    )
}

/// Kronn's five-field cron (or the crate's own six/seven-field form).
fn parse_schedule(cron_expr: &str) -> Result<cron::Schedule, cron::error::Error> {
    let fields: Vec<&str> = cron_expr.split_whitespace().collect();
    // Kronn's editor exposes standard five-field cron where Sunday is 0/7
    // and Monday is 1. The `cron` crate uses Sunday=1 through Saturday=7,
    // so numeric weekdays must be shifted while adding its seconds field.
    let expr = if fields.len() == 5 {
        let day_of_week = normalize_five_field_weekdays(fields[4]);
        format!(
            "0 {} {} {} {} {}",
            fields[0], fields[1], fields[2], fields[3], day_of_week
        )
    } else {
        cron_expr.to_string()
    };
    cron::Schedule::from_str(&expr)
}

/// True when the cron expression has an occurrence in `(since, now]`.
fn cron_fires_between(cron_expr: &str, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    match parse_schedule(cron_expr) {
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

fn normalize_five_field_weekdays(field: &str) -> String {
    fn crate_weekday(standard: u32) -> Option<u32> {
        match standard {
            0 | 7 => Some(1),
            1..=6 => Some(standard + 1),
            _ => None,
        }
    }

    let mut normalized = Vec::new();
    for segment in field.split(',') {
        let (base, step_suffix) = match segment.split_once('/') {
            Some((base, step)) => (base, Some(step)),
            None => (segment, None),
        };

        if base == "*" {
            normalized.push(segment.to_string());
            continue;
        }

        if let Some((start, end)) = base.split_once('-') {
            let numeric_range = start.parse::<u32>().ok().zip(end.parse::<u32>().ok()).zip(
                step_suffix
                    .map(|step| step.parse::<usize>().ok())
                    .unwrap_or(Some(1)),
            );
            if let Some(((start, end), step)) = numeric_range {
                if start <= end && step > 0 {
                    let mapped: Option<Vec<String>> = (start..=end)
                        .step_by(step)
                        .map(|day| crate_weekday(day).map(|d| d.to_string()))
                        .collect();
                    if let Some(mapped) = mapped {
                        normalized.extend(mapped);
                        continue;
                    }
                }
            }
        } else if step_suffix.is_none() {
            if let Ok(day) = base.parse::<u32>() {
                if let Some(day) = crate_weekday(day) {
                    normalized.push(day.to_string());
                    continue;
                }
            }
        }

        // Named weekdays already have the same meaning in both syntaxes.
        // Leave unsupported/invalid shapes untouched so the cron parser emits
        // its normal validation error rather than silently changing intent.
        normalized.push(segment.to_string());
    }
    normalized.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // ─── Manual trigger ──────────────────────────────────────────────────

    #[test]
    fn manual_trigger_never_fires() {
        let now = Utc::now();
        assert!(!should_fire(
            &WorkflowTrigger::Manual,
            now - Duration::seconds(30),
            now
        ));
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
        assert!(
            fires("0 0 7 * * *", occ - Duration::seconds(30), occ),
            "occurrence exactly at `now` fires (window is right-inclusive)"
        );
        assert!(
            fires(
                "0 0 7 * * *",
                occ - Duration::seconds(10),
                occ + Duration::seconds(20)
            ),
            "occurrence strictly inside the window fires"
        );
    }

    #[test]
    fn occurrence_fires_exactly_once_across_adjacent_windows() {
        // THE double-fire regression (concurrency review, 0.8.11): with the
        // old "next within [0,31)s of now" logic an occurrence at the seam
        // fired on both surrounding 30s ticks. Windows are half-open
        // (since, now] so adjacent windows partition time.
        let occ = "2026-07-09T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let tick1_start = occ - Duration::milliseconds(30_400); // occ − 30.4s
        let tick1_end = tick1_start + Duration::seconds(30); // occ − 0.4s
        let tick2_end = tick1_end + Duration::seconds(30); // occ + 29.6s
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
        assert!(fires(
            "0 0 7 * * *",
            occ - Duration::seconds(30),
            occ + Duration::seconds(90)
        ));
    }

    #[test]
    fn no_occurrence_in_window_does_not_fire() {
        let occ = "2026-07-09T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(!fires(
            "0 0 7 * * *",
            occ + Duration::seconds(1),
            occ + Duration::seconds(31)
        ));
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

    #[test]
    fn five_field_numeric_weekdays_follow_standard_cron_semantics() {
        let weekdays = "0 7,10,13,16,19 * * 1-5";

        for day in 20..=24 {
            let occurrence = format!("2026-07-{day}T10:00:00Z")
                .parse::<DateTime<Utc>>()
                .unwrap();
            assert!(
                fires(weekdays, occurrence - Duration::seconds(30), occurrence),
                "Monday through Friday must fire (day {day})"
            );
        }
        for day in 25..=26 {
            let occurrence = format!("2026-07-{day}T10:00:00Z")
                .parse::<DateTime<Utc>>()
                .unwrap();
            assert!(
                !fires(weekdays, occurrence - Duration::seconds(30), occurrence),
                "Saturday and Sunday must not fire (day {day})"
            );
        }
        assert_eq!(normalize_five_field_weekdays("1-5"), "2,3,4,5,6");
    }

    #[test]
    fn five_field_zero_and_seven_both_mean_sunday() {
        let sunday = "2026-07-26T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let monday = "2026-07-27T10:00:00Z".parse::<DateTime<Utc>>().unwrap();

        for expr in ["0 10 * * 0", "0 10 * * 7"] {
            assert!(fires(expr, sunday - Duration::seconds(30), sunday));
            assert!(!fires(expr, monday - Duration::seconds(30), monday));
        }
    }

    // ─── should_fire dispatch ────────────────────────────────────────────

    #[test]
    fn should_fire_cron_invalid_returns_false() {
        let now = Utc::now();
        let trigger = WorkflowTrigger::Cron {
            schedule: "invalid cron".into(),
            timezone: None,
        };
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
            source: TrackerSourceConfig::GitHub {
                owner: "o".into(),
                repo: "r".into(),
            },
            query: "".into(),
            labels: vec![],
            interval: "invalid".into(),
        };
        assert!(!should_fire(&invalid, now - Duration::seconds(61), now));
    }

    // ─── KT-1099 — timezone and Watch validation ─────────────────────────

    fn paris_cron(schedule: &str) -> WorkflowTrigger {
        WorkflowTrigger::Cron {
            schedule: schedule.into(),
            timezone: Some("Europe/Paris".into()),
        }
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        rfc3339.parse().unwrap()
    }

    #[test]
    fn a_cron_with_a_timezone_follows_its_local_hours_across_dst() {
        let seven_paris = paris_cron("0 7 * * *");
        // Summer (UTC+2): 07:00 Paris is 05:00Z, not 07:00Z.
        let summer = at("2026-07-09T05:00:00Z");
        assert!(should_fire(
            &seven_paris,
            summer - Duration::seconds(30),
            summer
        ));
        let utc_seven = at("2026-07-09T07:00:00Z");
        assert!(!should_fire(
            &seven_paris,
            utc_seven - Duration::seconds(30),
            utc_seven
        ));
        // Winter (UTC+1): the same expression follows the clock change.
        let winter = at("2026-12-09T06:00:00Z");
        assert!(should_fire(
            &seven_paris,
            winter - Duration::seconds(30),
            winter
        ));
    }

    /// Fires of `trigger` over 30-second ticks from `from` to `to`.
    fn fires_over_ticks(trigger: &WorkflowTrigger, from: &str, to: &str) -> Vec<DateTime<Utc>> {
        let (mut since, end) = (at(from), at(to));
        let mut fired = vec![];
        while since < end {
            let now = since + Duration::seconds(30);
            if should_fire(trigger, since, now) {
                fired.push(now);
            }
            since = now;
        }
        fired
    }

    #[test]
    fn a_local_time_the_spring_change_skips_does_not_fire_that_day() {
        // 2026-03-29: Paris jumps from 02:00 to 03:00; 02:30 does not exist.
        let half_past_two = paris_cron("30 2 * * *");
        assert!(fires_over_ticks(
            &half_past_two,
            "2026-03-28T23:00:00Z",
            "2026-03-29T23:00:00Z"
        )
        .is_empty());
        // The next day it fires again, at 02:30 CEST.
        assert_eq!(
            fires_over_ticks(
                &half_past_two,
                "2026-03-29T23:00:00Z",
                "2026-03-30T23:00:00Z"
            ),
            vec![at("2026-03-30T00:30:00Z")]
        );
    }

    #[test]
    fn a_local_time_the_autumn_change_repeats_fires_once_at_its_first_pass() {
        // 2026-10-25: Paris goes from 03:00 CEST back to 02:00 CET; 02:30
        // happens at 00:30Z and again at 01:30Z.
        let half_past_two = paris_cron("30 2 * * *");
        assert_eq!(
            fires_over_ticks(
                &half_past_two,
                "2026-10-24T22:00:00Z",
                "2026-10-25T04:00:00Z"
            ),
            vec![at("2026-10-25T00:30:00Z")]
        );
        // A frequent schedule fires once per local time: the repeated hour
        // (01:00Z–02:00Z) adds no fire, and nothing is skipped either side.
        let hourly = paris_cron("0 * * * *");
        assert_eq!(
            fires_over_ticks(&hourly, "2026-10-24T22:30:00Z", "2026-10-25T03:30:00Z"),
            vec![
                at("2026-10-24T23:00:00Z"),
                at("2026-10-25T00:00:00Z"),
                at("2026-10-25T02:00:00Z"),
                at("2026-10-25T03:00:00Z"),
            ]
        );
    }

    #[test]
    fn a_cron_without_a_timezone_stays_in_utc() {
        let utc = WorkflowTrigger::Cron {
            schedule: "0 7 * * *".into(),
            timezone: None,
        };
        let seven = at("2026-07-09T07:00:00Z");
        assert!(should_fire(&utc, seven - Duration::seconds(30), seven));
        let five = at("2026-07-09T05:00:00Z");
        assert!(!should_fire(&utc, five - Duration::seconds(30), five));
        // Saved before the field existed: no `timezone` key at all.
        let stored: WorkflowTrigger =
            serde_json::from_str(r#"{"type":"Cron","schedule":"0 7 * * *"}"#).unwrap();
        assert!(should_fire(&stored, seven - Duration::seconds(30), seven));
        assert_eq!(
            serde_json::to_string(&stored).unwrap(),
            r#"{"type":"Cron","schedule":"0 7 * * *"}"#,
            "an unset timezone is not written back"
        );
    }

    fn watch(interval: &str) -> WatchTrigger {
        WatchTrigger {
            api_plugin_slug: Some("github".into()),
            api_config_id: Some("cfg".into()),
            api_endpoint_path: Some("/repos/o/r/commits".into()),
            interval: interval.into(),
            ..WatchTrigger::default()
        }
    }

    #[test]
    fn a_watch_interval_is_read_in_its_timezone() {
        let mut trigger = watch("0 7 * * *");
        trigger.timezone = Some("Europe/Paris".into());
        let trigger = WorkflowTrigger::Watch(trigger);
        let summer = at("2026-07-09T05:00:00Z");
        assert!(should_fire(
            &trigger,
            summer - Duration::seconds(30),
            summer
        ));
    }

    #[test]
    fn validation_refuses_what_the_scheduler_could_not_run() {
        assert!(validate_trigger(&paris_cron("0 7 * * *")).is_ok());
        let unknown = WorkflowTrigger::Cron {
            schedule: "0 7 * * *".into(),
            timezone: Some("Europe/Atlantis".into()),
        };
        assert!(validate_trigger(&unknown)
            .unwrap_err()
            .contains("Europe/Atlantis"));

        assert!(validate_trigger(&WorkflowTrigger::Watch(watch("*/5 * * * *"))).is_ok());
        let every_second = WorkflowTrigger::Watch(watch("* * * * * *"));
        assert!(validate_trigger(&every_second)
            .unwrap_err()
            .contains("five-field"));
        let invalid = WorkflowTrigger::Watch(watch("not a cron at all"));
        assert!(validate_trigger(&invalid).is_err());

        let no_source = WorkflowTrigger::Watch(WatchTrigger {
            interval: "*/5 * * * *".into(),
            ..WatchTrigger::default()
        });
        assert!(validate_trigger(&no_source)
            .unwrap_err()
            .contains("Quick API"));
        let quick_api = WorkflowTrigger::Watch(WatchTrigger {
            quick_api_id: Some("qa-1".into()),
            interval: "*/5 * * * *".into(),
            ..WatchTrigger::default()
        });
        assert!(validate_trigger(&quick_api).is_ok());

        let mut bad_path = watch("*/5 * * * *");
        bad_path.detection = WatchDetection::JsonPath { path: "$[".into() };
        assert!(validate_trigger(&WorkflowTrigger::Watch(bad_path))
            .unwrap_err()
            .contains("JSONPath"));
    }
}
