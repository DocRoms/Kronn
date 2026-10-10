use super::*;
use std::collections::HashMap;

fn utc(stamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(stamp)
        .unwrap()
        .with_timezone(&Utc)
}

/// The refusal of run 02b6e1f7 (KT-811), verbatim.
const CLAUDE_SESSION_LIMIT: &str = "ACP prompt failed: ACP transport failed: [Agent provider error] You've hit your session limit · resets 9:40pm (Europe/Paris) (HTTP 429; terminal_reason=api_error)";

#[test]
fn the_claude_session_limit_is_quota_with_its_paris_reset() {
    // Paris is UTC+2 on 25/09.
    let now = utc("2026-09-25T17:00:00Z");
    let quota = classify(CLAUDE_SESSION_LIMIT, now).expect("a session limit is quota");
    assert_eq!(quota.reset_at, Some(utc("2026-09-25T19:40:00Z")));
    assert_eq!(quota.wake_at, None);
    assert_eq!(quota.parked, None);
}

#[test]
fn codex_and_openai_compatible_refusals_are_quota_with_their_reset() {
    let now = utc("2026-10-09T10:00:00Z");
    let codex = "You've hit your usage limit. Upgrade to Pro (https://openai.com/chatgpt/pricing) or try again in 2 hours 13 minutes.";
    assert_eq!(
        classify(codex, now).unwrap().reset_at,
        Some(now + Duration::minutes(133))
    );
    let codex_json = r#"stream error: {"type":"usage_limit_reached","message":"The usage limit has been reached","resets_in_seconds":5400}"#;
    assert_eq!(
        classify(codex_json, now).unwrap().reset_at,
        Some(now + Duration::seconds(5400))
    );
    let openai = "HTTP provider returned 429 Too Many Requests: {\"error\":{\"type\":\"rate_limit_exceeded\"}} retry-after: 30";
    assert_eq!(
        classify(openai, now).unwrap().reset_at,
        Some(now + Duration::seconds(30))
    );
    let http_date = "status 429; Retry-After: Fri, 09 Oct 2026 11:30:00 GMT";
    assert_eq!(
        classify(http_date, now).unwrap().reset_at,
        Some(utc("2026-10-09T11:30:00Z"))
    );
    let iso = r#"{"error":{"type":"rate_limit_error"},"anthropic-ratelimit-tokens-reset":"2026-10-09T12:15:00Z"}"#;
    assert_eq!(
        classify(iso, now).unwrap().reset_at,
        Some(utc("2026-10-09T12:15:00Z"))
    );
}

#[test]
fn a_refusal_without_a_trustworthy_reset_is_still_quota() {
    let now = utc("2026-10-09T10:00:00Z");
    // Codex names a wall clock without a zone: Kronn cannot place it.
    let codex = "You've hit your usage limit. Upgrade to Pro or try again at 3:05 PM.";
    let quota = classify(codex, now).expect("quota");
    assert_eq!(quota.reset_at, None);
    assert_eq!(
        classify("HTTP 429 Too Many Requests", now)
            .unwrap()
            .reset_at,
        None
    );
    // A reset already past is not a reset to wait for.
    assert_eq!(
        classify("rate_limit_exceeded; reset_at: 2026-10-09T09:00:00Z", now)
            .unwrap()
            .reset_at,
        None
    );
}

#[test]
fn agent_failures_and_transient_overloads_are_not_quota() {
    let now = utc("2026-10-09T10:00:00Z");
    for error in [
        "Agent exited with code 1: panic in tool",
        "[Agent provider error]\n\nOverloaded\n\n(HTTP 529; terminal_reason=api_error)",
        "Worker local total request limit reached (16/16)",
        "Agent stalled: no output for 600s",
        "processed 429 files",
    ] {
        assert!(classify(error, now).is_none(), "{error}");
    }
}

fn quota(reset_at: Option<DateTime<Utc>>) -> QuotaWait {
    QuotaWait {
        id: None,
        reset_at,
        wake_at: None,
        attempt: 0,
        parked: None,
        detail: None,
    }
}

#[test]
fn the_step_wakes_after_the_reset_plus_a_margin() {
    let now = utc("2026-09-25T17:00:00Z");
    let deadline = now + Duration::hours(6);
    let mut state = HashMap::new();
    let reset = utc("2026-09-25T19:40:00Z");
    let wait = plan_wait(&mut state, "opus", quota(Some(reset)), now, deadline);
    assert_eq!(wait.wake_at, Some(reset + Duration::minutes(2)));
    assert_eq!(wait.attempt, 1);
    assert_eq!(wait.parked, None);
}

#[test]
fn no_reset_parks_for_a_manual_resume() {
    let now = utc("2026-09-25T17:00:00Z");
    let mut state = HashMap::new();
    let wait = plan_wait(
        &mut state,
        "opus",
        quota(None),
        now,
        now + Duration::hours(6),
    );
    assert_eq!(wait.wake_at, None);
    assert_eq!(wait.parked, Some(QuotaParkReason::NoResetTime));
}

#[test]
fn a_reset_after_the_workflow_deadline_parks_instead_of_waking_into_the_guard() {
    let now = utc("2026-09-25T17:00:00Z");
    let mut state = HashMap::new();
    let wait = plan_wait(
        &mut state,
        "opus",
        quota(Some(now + Duration::hours(3))),
        now,
        now + Duration::hours(2),
    );
    assert_eq!(wait.wake_at, None);
    assert_eq!(wait.parked, Some(QuotaParkReason::AfterDeadline));
}

#[test]
fn repeated_refusals_back_off_then_park() {
    let now = utc("2026-09-25T17:00:00Z");
    let deadline = now + Duration::days(2);
    let mut state = HashMap::new();
    // The provider keeps announcing a reset seconds away: the floor grows.
    let imminent = Some(now + Duration::seconds(10));
    let floors: Vec<_> = (0..MAX_AUTOMATIC_WAKES)
        .map(|_| {
            plan_wait(&mut state, "opus", quota(imminent), now, deadline)
                .wake_at
                .unwrap()
                - now
        })
        .collect();
    assert_eq!(
        floors,
        vec![
            Duration::minutes(5),
            Duration::minutes(10),
            Duration::minutes(20),
            Duration::minutes(40)
        ]
    );
    let parked = plan_wait(&mut state, "opus", quota(imminent), now, deadline);
    assert_eq!(parked.attempt, MAX_AUTOMATIC_WAKES + 1);
    assert_eq!(parked.parked, Some(QuotaParkReason::TooManyAttempts));
    assert_eq!(parked.wake_at, None);
    // Another step starts its own count.
    let other = plan_wait(&mut state, "review", quota(imminent), now, deadline);
    assert_eq!(other.attempt, 1);
}

#[test]
fn absurd_relative_resets_yield_no_reset_instead_of_panicking() {
    let now = utc("2026-10-09T10:00:00Z");
    for error in [
        "HTTP 429; try again in 9223372036854775807 days",
        "HTTP 429; try again in 1000000000 days",
        "HTTP 429; try again in 44 days 23 hours 120 minutes",
        "HTTP 429; try again in 46 days",
        "HTTP 429; retry-after: 9999999",
        r#"usage_limit_reached "resets_in_seconds": 99999999"#,
        "HTTP 429; resets_at: 9999999999",
        "HTTP 429; reset at 9999-12-31T23:59:59Z",
    ] {
        let quota = classify(error, now).expect("still quota");
        assert_eq!(quota.reset_at, None, "{error}");
    }
    assert_eq!(
        classify("HTTP 429; try again in 1 day 2 hours 3 minutes", now)
            .unwrap()
            .reset_at,
        Some(now + Duration::days(1) + Duration::hours(2) + Duration::minutes(3))
    );
    assert_eq!(
        classify("HTTP 429; try again in 2 hours 13 minutes", now)
            .unwrap()
            .reset_at,
        Some(now + Duration::minutes(133))
    );
}

#[test]
fn a_parked_wait_keeps_a_valid_reset_to_explain_itself() {
    let now = utc("2026-09-25T17:00:00Z");
    let mut state = HashMap::new();
    let reset = now + Duration::hours(3);
    let wait = plan_wait(
        &mut state,
        "opus",
        quota(Some(reset)),
        now,
        now + Duration::hours(2),
    );
    assert_eq!(wait.parked, Some(QuotaParkReason::AfterDeadline));
    assert_eq!(wait.reset_at, Some(reset));
    assert!(wait.id.is_some());
}
