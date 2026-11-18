//! Text classifiers for provider refusals (quota, overload, edge auth errors) and
//! the reset time a refusal announces. Pure string logic, shared by the workflow
//! engine and the discussion runtime.

use chrono::{DateTime, Duration, Utc};

use crate::models::QuotaWait;

/// A provider saturation that will likely clear on its own, as opposed to a
/// plan or credit limit that will not.
///
/// Issue 202: two turns ended on `API Error: 529 Overloaded` and simply stopped.
/// Nothing retried, nothing surfaced, and the user waited 18 minutes once and
/// 1 h 27 the other time before relaunching by hand. A 529 is the one failure
/// where waiting a little and trying again is the correct answer.
///
/// A hard quota must never reach this: callers test
/// [`is_hard_quota_exhausted`] first, and the shared-capacity wordings it
/// deliberately excludes are transient, so they belong here.
pub fn is_transient_provider_overload(output: &str) -> bool {
    let lower = output.to_lowercase();
    if is_hard_quota_exhausted(output) {
        return false;
    }
    // `529` is never matched on its own: the digits turn up in ids, token
    // counts and durations, and a false positive spends a real API call.
    [
        "overloaded",
        "http 529",
        "error: 529",
        "status 529",
        "529 overloaded",
        "resourceexhausted",
        "worker local total request limit",
        "request limit reached",
        "service unavailable",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

/// KT-1743 — the longest reply that can still be "just the provider's error".
/// A turn that did real work ends on a report far longer than this; one that
/// only relays a Cloudflare challenge page does not.
const TRANSIENT_UPSTREAM_REPLY_MAX_CHARS: usize = 1_500;

/// A provider auth/edge error that is the network's doing, not the account's:
/// `Failed to authenticate. API Error: 403 Attention Required! | Cloudflare`
/// (a challenge page served to the host during a network blip), a 403 from the
/// API edge, or a Cloudflare 52x. It clears on its own, so the caller backs off
/// and redispatches instead of ending the worker.
///
/// A real credential loss keeps its own path: a 401, `authentication_error`,
/// `please run /login`, an expired token — all refused here, as is a hard
/// quota. The reply must also be short, i.e. the error and nothing else: a
/// report that merely quotes "403" is the agent's own work.
pub fn is_transient_upstream_auth_error(output: &str) -> bool {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.chars().count() > TRANSIENT_UPSTREAM_REPLY_MAX_CHARS {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if is_hard_quota_exhausted(trimmed) {
        return false;
    }
    const REAL_AUTH_LOSS: &[&str] = &[
        "api error: 401",
        "http 401",
        "status 401",
        "401 unauthorized",
        "authentication_error",
        "invalid authentication",
        "invalid x-api-key",
        "invalid api key",
        "not authenticated",
        "not logged in",
        "please run /login",
        "please log in",
        "oauth token has expired",
        "token has been revoked",
    ];
    if REAL_AUTH_LOSS.iter().any(|needle| lower.contains(needle)) {
        return false;
    }
    [
        "attention required",
        "cloudflare",
        "api error: 403",
        "http 403",
        "api error: 520",
        "api error: 521",
        "api error: 522",
        "api error: 523",
        "api error: 524",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn is_hard_quota_exhausted(output: &str) -> bool {
    let lower = output.to_lowercase();
    // NVIDIA's hosted NIM uses ResourceExhausted for shared worker saturation;
    // that is transient capacity, not the user's plan. Check it first.
    if lower.contains("resourceexhausted")
        || lower.contains("worker local total request limit")
        || lower.contains("request limit reached")
    {
        return false;
    }
    [
        "insufficient_quota",
        "payment required",
        "usage limit",
        "session limit",
        "hit your limit",
        "purchase more credits",
        "upgrade to pro",
        "out of credits",
        "quota exhausted",
        "quota exceeded",
        "monthly spend limit",
        "monthly credit limit",
        "organization spend limit",
        "org spend limit",
        // KT-1517 — the other wordings a native agent ends a turn on when the
        // provider account is out of credits. Codex over an API key reports the
        // OpenAI quota error verbatim; Claude Code reports Anthropic's prepaid
        // balance and its extra-usage allowance; both CLIs name the rolling
        // windows. None of them clears before a reset or a top-up.
        "exceeded your current quota",
        "credit balance is too low",
        "credit balance too low",
        "out of extra usage",
        "billing_hard_limit_reached",
        "insufficient credits",
        "insufficient funds",
        "no credits remaining",
        "hour limit reached",
        "weekly limit reached",
        "monthly limit reached",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
        || lower.contains("api error: 402")
        || lower.contains("http 402")
}

/// The instant a provider's own quota refusal says the quota comes back
/// (KT-838). Claude Code words it `resets 4:20pm (Europe/Paris)`; a weekly
/// limit adds the day: `resets Oct 3, 4pm (Europe/Paris)`.
///
/// Only a wall-clock time that names its IANA zone is resolved: the backend may
/// run in a container whose local zone says nothing about the user's, and a
/// guessed instant would tell a human to re-arm at the wrong hour. Anything
/// else, or an instant already past or more than 45 days out, is `None` — the
/// caller then simply has no time to show.
pub fn announced_quota_reset(
    output: &str,
    now: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone};
    static RESET_RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"(?i)\bresets?\s+(?:at\s+)?(?:(?P<mon>[a-z]{3,9})\s+(?P<day>\d{1,2})(?:st|nd|rd|th)?,?\s+(?:at\s+)?)?(?P<hour>\d{1,2})(?::(?P<min>\d{2}))?\s*(?P<ampm>am|pm)\s*\((?P<tz>[A-Za-z_]+(?:/[A-Za-z0-9_+-]+)+|UTC)\)",
        )
        .expect("static quota reset regex")
    });
    let caps = RESET_RE.captures(output)?;
    let tz: chrono_tz::Tz = caps["tz"].parse().ok()?;
    let hour12: u32 = caps["hour"].parse().ok()?;
    let minute: u32 = caps
        .name("min")
        .map_or(Some(0), |m| m.as_str().parse().ok())?;
    if !(1..=12).contains(&hour12) || minute > 59 {
        return None;
    }
    let pm = caps["ampm"].eq_ignore_ascii_case("pm");
    let time = NaiveTime::from_hms_opt(hour12 % 12 + if pm { 12 } else { 0 }, minute, 0)?;
    let at = |date: NaiveDate| {
        tz.from_local_datetime(&date.and_time(time))
            .earliest()
            .map(|local| local.with_timezone(&Utc))
    };
    let today = now.with_timezone(&tz).date_naive();
    let reset = match (caps.name("mon"), caps.name("day")) {
        (Some(mon), Some(day)) => {
            let month = [
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            ]
            .iter()
            .position(|name| mon.as_str().to_lowercase().starts_with(name))?
                as u32
                + 1;
            let day: u32 = day.as_str().parse().ok()?;
            // A date already gone this year is next year's (a `Jan 2` seen in December).
            [today.year(), today.year() + 1]
                .into_iter()
                .find_map(|year| {
                    at(NaiveDate::from_ymd_opt(year, month, day)?).filter(|when| *when > now)
                })?
        }
        _ => {
            let same_day = at(today)?;
            if same_day > now {
                same_day
            } else {
                at(today.succ_opt()?)?
            }
        }
    };
    (reset > now && reset <= now + chrono::Duration::days(45)).then_some(reset)
}

/// Longest reset Kronn trusts; anything further is a misparse.
const MAX_RESET_HORIZON: Duration = Duration::days(45);

/// A quota or session-limit refusal, as opposed to an agent failure. Shared
/// capacity saturation (529, NVIDIA worker limits) is transient, not quota.
pub fn is_quota_refusal(error: &str) -> bool {
    if is_hard_quota_exhausted(error) {
        return true;
    }
    if is_transient_provider_overload(error) {
        return false;
    }
    // Claude's short form names no plan: "5-hour limit · resets 5:30pm (…)".
    // Only its dot separator and a reset time, maybe dated ("resets Nov 7 at 8pm"):
    // "limit: reset the cache" is no quota.
    static LIMIT_RESETS: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"(?i)\blimit(?:\s+reached)?\s*[·∙•]\s*resets\s+(?:[a-z]{3,9}\s+)?\d",
        )
        .expect("static limit-resets regex")
    });
    if LIMIT_RESETS.is_match(error) {
        return true;
    }
    let lower = error.to_lowercase();
    [
        "http 429",
        "status 429",
        "status: 429",
        "error: 429",
        "429 too many requests",
        "too many requests",
        "rate_limit_exceeded",
        "rate limit exceeded",
        "rate_limit_error",
        "usage_limit_reached",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The step's quota classification, with the reset the refusal announced.
pub fn classify(error: &str, now: DateTime<Utc>) -> Option<QuotaWait> {
    is_quota_refusal(error).then(|| QuotaWait {
        id: None,
        reset_at: announced_reset(error, now),
        wake_at: None,
        attempt: 0,
        parked: None,
        detail: None,
    })
}

/// The reset instant a refusal names: Claude's wall clock with its zone, a
/// Retry-After (seconds or HTTP date), a relative "try again in", a Codex
/// `resets_in_seconds`/`resets_at`, or an ISO timestamp next to "reset".
pub fn announced_reset(error: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    // Every parser stays within the horizon with checked arithmetic: an
    // absurd provider number yields no reset, never a panic.
    announced_quota_reset(error, now)
        .or_else(|| retry_after(error, now))
        .or_else(|| relative_reset(error, now))
        .or_else(|| codex_reset(error, now))
        .or_else(|| iso_reset(error))
        .filter(|at| *at > now && *at <= now + MAX_RESET_HORIZON)
}

/// `now + secs`, or `None` when the delay is not positive or beyond the horizon.
fn after_seconds(now: DateTime<Utc>, secs: i64) -> Option<DateTime<Utc>> {
    if secs <= 0 || secs > MAX_RESET_HORIZON.num_seconds() {
        return None;
    }
    now.checked_add_signed(Duration::try_seconds(secs)?)
}

fn retry_after(error: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    static SECONDS: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r#"(?i)retry[-_ ]after(?:[-_]seconds)?["']?\s*[:=]?\s*"?(\d{1,7})(?:\.\d+)?\s*(?:s\b|sec|seconds?\b|"|,|;|\)|\s|$)"#)
            .expect("static retry-after regex")
    });
    static HTTP_DATE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"(?i)retry-after\s*:\s*([A-Za-z]{3},\s*\d{1,2}\s+[A-Za-z]{3}\s+\d{4}\s+\d{2}:\d{2}:\d{2}\s+GMT)",
        )
        .expect("static retry-after date regex")
    });
    if let Some(caps) = SECONDS.captures(error) {
        return after_seconds(now, caps[1].parse().ok()?);
    }
    let caps = HTTP_DATE.captures(error)?;
    DateTime::parse_from_rfc2822(&caps[1])
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn relative_reset(error: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    static RELATIVE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"(?i)(?:try again|resets?|retry)\s+in\s+((?:\d+\s*(?:days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b[\s,]*(?:and\s+)?)+)")
            .expect("static relative reset regex")
    });
    static PART: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"(?i)(\d+)\s*(days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b",
        )
        .expect("static duration part regex")
    });
    let span = RELATIVE.captures(error)?;
    let cap = MAX_RESET_HORIZON.num_seconds();
    let mut total: i64 = 0;
    for part in PART.captures_iter(&span[1]) {
        // Capped per component and per running sum, before any Duration exists.
        let n: i64 = part[1].parse().ok().filter(|n| *n <= cap)?;
        let unit: i64 = match part[2].to_lowercase().chars().next()? {
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            _ => 1,
        };
        total = total
            .checked_add(n.checked_mul(unit)?)
            .filter(|t| *t <= cap)?;
    }
    after_seconds(now, total)
}

fn codex_reset(error: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    static IN_SECONDS: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r#"(?i)"?resets_in_seconds"?\s*[:=]\s*(\d{1,8})"#)
            .expect("static resets_in_seconds regex")
    });
    static AT_EPOCH: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r#"(?i)"?resets_at"?\s*[:=]\s*(\d{10})\b"#)
            .expect("static resets_at regex")
    });
    if let Some(caps) = IN_SECONDS.captures(error) {
        return after_seconds(now, caps[1].parse().ok()?);
    }
    let caps = AT_EPOCH.captures(error)?;
    DateTime::from_timestamp(caps[1].parse().ok()?, 0)
}

fn iso_reset(error: &str) -> Option<DateTime<Utc>> {
    static ISO: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r#"(?i)(?:reset|retry)[a-z_-]*["']?\s*(?:at)?\s*[:=]?\s*"?(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2}))"#)
            .expect("static iso reset regex")
    });
    let caps = ISO.captures(error)?;
    DateTime::parse_from_rfc3339(&caps[1])
        .ok()
        .map(|at| at.with_timezone(&Utc))
}
