//! KT-811 — an Agent step refused for a provider quota or session limit waits
//! for the announced reset instead of failing the run.
//!
//! The step and the run go to `WaitingQuota`; the workflow engine tick wakes
//! the run once its `wake_at` is due and continues it at that same step through
//! the ordinary resume path. Without a trustworthy reset the run is parked
//! until a human resumes it. The state lives on the run row, so it survives a
//! backend restart.
//!
//! A sub-workflow child (foreach item or single, at any depth) waits the same
//! way, but its parent mirrors the wait and owns the wake-up: resuming the
//! parent reclaims the child at its step. A batch whose children were all
//! refused makes its own step wait, like an Agent step.

use chrono::{DateTime, Duration, Utc};

use crate::models::{QuotaParkReason, QuotaWait, RunStatus, Workflow, WorkflowRun};
use crate::AppState;

/// Consecutive quota refusals of one step, persisted across wake-ups.
pub const QUOTA_ATTEMPTS_STATE_KEY: &str = "__kronn.quota_attempts";

/// A reset announced to the minute can still be refused for a few seconds.
const RESET_MARGIN: Duration = Duration::minutes(2);
/// Wake-ups the engine attempts on its own before handing the run to a human.
pub const MAX_AUTOMATIC_WAKES: u32 = 4;
/// Longest reset Kronn trusts; anything further is a misparse.
const MAX_RESET_HORIZON: Duration = Duration::days(45);
/// Set by a parent on a child it can resume after a quota wait.
pub const PARENT_RESUMES_KEY: &str = "__subwf_quota_resumable__";

/// Whether a quota refusal in `run` may wait instead of failing: a top-level
/// linear run, or a child whose parent marked it as resumable.
pub fn can_wait(run: &WorkflowRun) -> bool {
    match run.parent_run_id {
        None => run.run_type == "linear",
        Some(_) => {
            run.run_type == "subworkflow"
                && run
                    .trigger_context
                    .as_ref()
                    .and_then(|ctx| ctx.get(PARENT_RESUMES_KEY))
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        }
    }
}

/// The parent's copy of a child's planned wait, under its own identity: the
/// child decided when to wake, the parent's claim names the parent's wait.
pub fn mirror_child_wait(child: QuotaWait) -> QuotaWait {
    QuotaWait {
        id: Some(uuid::Uuid::new_v4().to_string()),
        ..child
    }
}

fn mirrored_child_key(step_name: &str) -> String {
    format!("__kronn.quota_mirrored_child.{step_name}")
}

/// Records which child, and which of its waits, a foreach step mirrored, so
/// its resume tells that wait apart from a child some other pass left waiting.
pub fn remember_mirrored_child(
    run_state: &mut std::collections::HashMap<String, String>,
    step_name: &str,
    child_run_id: Option<&str>,
    child_wait_id: Option<&str>,
) {
    run_state.insert(
        mirrored_child_key(step_name),
        serde_json::json!({ "v": 1, "child_run_id": child_run_id, "wait_id": child_wait_id })
            .to_string(),
    );
}

pub fn forget_mirrored_child(
    run_state: &mut std::collections::HashMap<String, String>,
    step_name: &str,
) {
    run_state.remove(&mirrored_child_key(step_name));
}

/// The child run and wait id a foreach step mirrored, if it is resuming one.
pub fn mirrored_child(
    run_state: &std::collections::HashMap<String, String>,
    step_name: &str,
) -> Option<(String, Option<String>)> {
    let doc: serde_json::Value =
        serde_json::from_str(run_state.get(&mirrored_child_key(step_name))?).ok()?;
    let child = doc.get("child_run_id")?.as_str()?.to_string();
    let wait = doc
        .get("wait_id")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    Some((child, wait))
}

/// A quota or session-limit refusal, as opposed to an agent failure. Shared
/// capacity saturation (529, NVIDIA worker limits) is transient, not quota.
pub fn is_quota_refusal(error: &str) -> bool {
    use crate::api::discussions::orchestration::{
        is_hard_quota_exhausted, is_transient_provider_overload,
    };
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
    crate::api::discussions::orchestration::announced_quota_reset(error, now)
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

/// Floor between two wake-ups of the same step: a provider that refuses again
/// right after its announced reset must not drive an immediate retry loop.
fn backoff_floor(attempt: u32) -> Duration {
    Duration::minutes(5 * (1_i64 << attempt.saturating_sub(1).min(6)))
}

/// Decides how the quota-refused step waits and records the attempt on the run.
/// The provider wait is paused time, so it does not consume the workflow's
/// active-execution timeout.
pub fn plan_wait(
    run_state: &mut std::collections::HashMap<String, String>,
    step_name: &str,
    classified: QuotaWait,
    now: DateTime<Utc>,
) -> QuotaWait {
    let attempt = previous_attempts(run_state, step_name) + 1;
    run_state.insert(
        QUOTA_ATTEMPTS_STATE_KEY.to_string(),
        serde_json::json!({ "v": 1, "step": step_name, "attempts": attempt }).to_string(),
    );
    let mut wait = QuotaWait {
        id: Some(uuid::Uuid::new_v4().to_string()),
        attempt,
        ..classified
    };
    let Some(reset_at) = wait.reset_at else {
        wait.parked = Some(QuotaParkReason::NoResetTime);
        return wait;
    };
    if attempt > MAX_AUTOMATIC_WAKES {
        wait.parked = Some(QuotaParkReason::TooManyAttempts);
        return wait;
    }
    let wake_at = (reset_at + RESET_MARGIN).max(now + backoff_floor(attempt));
    wait.wake_at = Some(wake_at);
    wait
}

fn previous_attempts(run_state: &std::collections::HashMap<String, String>, step: &str) -> u32 {
    run_state
        .get(QUOTA_ATTEMPTS_STATE_KEY)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .filter(|doc| doc.get("step").and_then(serde_json::Value::as_str) == Some(step))
        .and_then(|doc| doc.get("attempts").and_then(serde_json::Value::as_u64))
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// The quota wait a `WaitingQuota` run is parked on: its trailing step.
pub fn pending_wait(run: &WorkflowRun) -> Option<&QuotaWait> {
    (run.status == RunStatus::WaitingQuota)
        .then(|| run.step_results.last())
        .flatten()
        .filter(|step| step.status == RunStatus::WaitingQuota)
        .and_then(|step| step.quota_wait.as_ref())
}

/// Parks the wait `run` was read with, so it stops being polled and shows why.
/// Compare-and-set on that same wait: a run resumed, cancelled or waiting
/// again since the read is left untouched. Returns whether it parked.
pub(crate) async fn park(
    state: &AppState,
    run: &WorkflowRun,
    reason: QuotaParkReason,
    detail: String,
) -> bool {
    let Some(step_index) = run.step_results.len().checked_sub(1) else {
        return false;
    };
    let wait_id = pending_wait(run).and_then(|wait| wait.id.clone());
    let run_id = run.id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::park_quota_wait(
                conn,
                &run_id,
                step_index,
                wait_id.as_deref(),
                reason,
                &detail,
            )
        })
        .await
    {
        Ok(parked) => parked,
        Err(error) => {
            tracing::warn!(run_id = %run.id, "could not park a quota-waiting run: {error}");
            false
        }
    }
}

/// One engine pass: resume every `WaitingQuota` run whose wake-up is due.
/// Returns the ids it resumed.
pub async fn wake_due_runs(state: &AppState, now: DateTime<Utc>) -> Vec<String> {
    let runs = match state
        .db
        .with_conn(crate::db::workflows::list_quota_waiting_runs)
        .await
    {
        Ok(runs) => runs,
        Err(error) => {
            tracing::warn!("quota-waiting run scan failed: {error}");
            return Vec::new();
        }
    };
    let mut resumed = Vec::new();
    for mut run in runs {
        // A child is resumed by its parent; alone, it can only be orphaned.
        if run.parent_run_id.is_some() {
            fail_if_orphaned(state, &run).await;
            continue;
        }
        reschedule_after_deadline(state, &mut run, now).await;
        let due = pending_wait(&run)
            .and_then(|wait| wait.wake_at)
            .is_some_and(|wake_at| wake_at <= now);
        if !due {
            continue;
        }
        let workflow_id = run.workflow_id.clone();
        let workflow = match state
            .db
            .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &workflow_id))
            .await
        {
            Ok(Some(workflow)) => workflow,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(run_id = %run.id, "quota wake-up skipped: {error}");
                continue;
            }
        };
        if let Some(id) = wake_one(state, workflow, run).await {
            resumed.push(id);
        }
    }
    resumed
}

/// How old a legacy `AfterDeadline` park's reset may be and still wake on its own.
const LEGACY_PARK_MAX_AGE: chrono::Duration = chrono::Duration::hours(24);

/// A wait parked `AfterDeadline` by an engine that counted the pause against
/// the timeout has no wake; its reset now schedules one like any other.
/// A park whose reset is older than [`LEGACY_PARK_MAX_AGE`] stays for a human:
/// the run was left alone long enough that its repository may have moved on.
async fn reschedule_after_deadline(state: &AppState, run: &mut WorkflowRun, now: DateTime<Utc>) {
    let Some(wait) = pending_wait(run) else {
        return;
    };
    if wait.parked != Some(QuotaParkReason::AfterDeadline) || wait.wake_at.is_some() {
        return;
    }
    let Some(reset_at) = wait.reset_at else {
        return;
    };
    if now - reset_at > LEGACY_PARK_MAX_AGE {
        return;
    }
    let wake_at = reset_at + RESET_MARGIN;
    let wait_id = wait.id.clone();
    let run_id = run.id.clone();
    let step_index = run.step_results.len() - 1;
    let rescheduled = state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::reschedule_after_deadline_wait(
                conn,
                &run_id,
                step_index,
                wait_id.as_deref(),
                wake_at,
            )
        })
        .await;
    match rescheduled {
        Ok(true) => {
            if let Some(wait) = run
                .step_results
                .last_mut()
                .and_then(|step| step.quota_wait.as_mut())
            {
                wait.wake_at = Some(wake_at);
                wait.parked = None;
                wait.detail = None;
            }
        }
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(run_id = %run.id, "could not reschedule a parked quota wait: {error}");
        }
    }
}

/// Fails a waiting child whose parent can no longer resume it, so the child
/// does not hold its workflow's concurrency slot forever.
async fn fail_if_orphaned(state: &AppState, run: &WorkflowRun) {
    let Some(step_index) = run.step_results.len().checked_sub(1) else {
        return;
    };
    let wait_id = pending_wait(run).and_then(|wait| wait.id.clone());
    let run_id = run.id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::fail_orphaned_quota_wait(
                conn,
                &run_id,
                step_index,
                wait_id.as_deref(),
            )
        })
        .await
    {
        Ok(true) => {
            tracing::info!(run_id = %run.id, "quota-waiting child failed: its parent run ended")
        }
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(run_id = %run.id, "orphan check of a quota-waiting child failed: {error}")
        }
    }
}

async fn wake_one(state: &AppState, workflow: Workflow, mut run: WorkflowRun) -> Option<String> {
    // A disabled workflow resumes only at a human's request (KT-1037).
    if !workflow.enabled {
        park(
            state,
            &run,
            QuotaParkReason::NotResumable,
            "the workflow is disabled".into(),
        )
        .await;
        return None;
    }
    if let Err(error) = super::runner::check_resume_preconditions(&mut run, false) {
        park(
            state,
            &run,
            QuotaParkReason::NotResumable,
            error.to_string(),
        )
        .await;
        return None;
    }
    match super::runner::try_claim_interrupted_run_row(state, &mut run, None).await {
        Ok(Ok(())) => {}
        // The concurrency limit refused it: the next tick tries again.
        Ok(Err(reason)) => {
            tracing::info!(run_id = %run.id, "quota wake-up deferred: {reason}");
            return None;
        }
        // Cancelled or claimed by a manual resume meanwhile.
        Err(error) => {
            tracing::info!(run_id = %run.id, "quota wake-up skipped: {error}");
            return None;
        }
    }
    tracing::info!(run_id = %run.id, "resuming a run after its provider quota reset");
    let id = run.id.clone();
    super::runner::spawn_claimed_resume(state.clone(), workflow, run);
    Some(id)
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "quota_wait_tests.rs"]
mod tests;
