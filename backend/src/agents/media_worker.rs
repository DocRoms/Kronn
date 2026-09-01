//! Progression logic for media jobs.
//!
//! The decision of what to do next is kept pure and separate from the I/O so
//! it can be tested without a provider: the expensive mistakes here are
//! ordering ones — calling a provider after the deadline, resubmitting a job
//! that already has a handle (and is therefore already billed), or polling so
//! often that a 100 s generation costs ten round-trips.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::db::media_jobs::MediaJob;

/// What a claimed job needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    /// A billable POST may have been sent without its handle being stored, so
    /// resubmitting could pay for a second generation. Refused explicitly: an
    /// honest failure the user can retry deliberately beats a silent double
    /// charge, and no provider response can tell us afterwards which happened.
    RefuseUnsafeResume,
    /// Past its deadline. Terminal, and checked FIRST: an overdue job must not
    /// reach the provider one more time.
    Expire,
    /// No provider handle yet, so nothing has been billed: submit.
    Submit,
    /// A handle exists — the provider is already working and already charging.
    /// Never resubmit; poll.
    Poll,
}

pub fn next_action(job: &MediaJob, now: DateTime<Utc>) -> MediaAction {
    if now >= job.deadline_at {
        return MediaAction::Expire;
    }
    match job.provider_job_id.as_deref() {
        Some(handle) if !handle.is_empty() => MediaAction::Poll,
        // No handle. Whether submitting is safe depends on whether one was
        // already attempted.
        _ if job.submit_attempted_at.is_some() => MediaAction::RefuseUnsafeResume,
        _ => MediaAction::Submit,
    }
}

/// Delay before the next poll, growing with the attempt count.
///
/// A 5 s / 480p clip — the cheapest case — took ~100 s end to end. A fixed 10 s
/// interval spends ten round-trips getting there; this schedule reaches the
/// same point in six, and keeps a ceiling so a long 1080p job settles into a
/// steady, cheap rhythm instead of hammering.
pub fn backoff_delay(attempts: u32) -> Duration {
    const LADDER: [u64; 6] = [5, 10, 15, 20, 30, 45];
    const CEILING: u64 = 60;
    let index = attempts.saturating_sub(1) as usize;
    Duration::from_secs(LADDER.get(index).copied().unwrap_or(CEILING))
}

/// Default budget for one generation. Well beyond the ~100 s measured on the
/// lightest case, because resolution and duration both stretch it.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(20 * 60);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MediaModality, MediaParams, MediaRendered};
    use chrono::Duration as ChronoDuration;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn job(provider_job_id: Option<&str>, deadline: DateTime<Utc>) -> MediaJob {
        MediaJob {
            id: "j1".into(),
            modality: MediaModality::Video,
            status: crate::models::MediaJobStatus::Running,
            connection_id: "c1".into(),
            model: "m".into(),
            prompt: "p".into(),
            params: MediaParams::default(),
            discussion_id: None,
            message_id: None,
            project_id: None,
            submit_attempted_at: None,
            provider_job_id: provider_job_id.map(str::to_string),
            provider_generation_id: None,
            context_file_id: None,
            rendered: MediaRendered::default(),
            cost: None,
            last_error: None,
            attempts: 1,
            scheduled_at: now(),
            deadline_at: deadline,
        }
    }

    #[test]
    fn a_job_whose_submission_may_already_be_billed_is_never_resubmitted() {
        // The crash window: the POST left, the handle never landed. Retrying
        // would pay for a second generation, and nothing can tell us whether
        // the first one was accepted.
        let mut j = job(None, now() + ChronoDuration::minutes(10));
        j.submit_attempted_at = Some(now());
        assert_eq!(next_action(&j, now()), MediaAction::RefuseUnsafeResume);

        // With a handle it polls, as before: the submission is confirmed.
        let mut confirmed = job(Some("provider-abc"), now() + ChronoDuration::minutes(10));
        confirmed.submit_attempted_at = Some(now());
        assert_eq!(next_action(&confirmed, now()), MediaAction::Poll);

        // And the deadline still outranks everything.
        let mut overdue = job(None, now());
        overdue.submit_attempted_at = Some(now());
        assert_eq!(next_action(&overdue, now()), MediaAction::Expire);
    }

    #[test]
    fn a_job_without_a_handle_is_submitted() {
        let j = job(None, now() + ChronoDuration::minutes(10));
        assert_eq!(next_action(&j, now()), MediaAction::Submit);
        // An empty string is not a handle either.
        let blank = job(Some(""), now() + ChronoDuration::minutes(10));
        assert_eq!(next_action(&blank, now()), MediaAction::Submit);
    }

    #[test]
    fn a_job_with_a_handle_is_polled_never_resubmitted() {
        let j = job(Some("provider-abc"), now() + ChronoDuration::minutes(10));
        // Resubmitting would pay for a second generation.
        assert_eq!(next_action(&j, now()), MediaAction::Poll);
    }

    #[test]
    fn the_deadline_wins_over_everything_else() {
        // Even with a live handle, an overdue job must not touch the provider.
        let overdue = job(Some("provider-abc"), now() - ChronoDuration::seconds(1));
        assert_eq!(next_action(&overdue, now()), MediaAction::Expire);

        let overdue_unsubmitted = job(None, now());
        assert_eq!(
            next_action(&overdue_unsubmitted, now()),
            MediaAction::Expire
        );
    }

    #[test]
    fn the_backoff_grows_then_settles_on_a_ceiling() {
        let secs = |n| backoff_delay(n).as_secs();
        assert_eq!(secs(1), 5);
        assert_eq!(secs(2), 10);
        assert_eq!(secs(6), 45);
        assert_eq!(secs(7), 60, "settles instead of growing forever");
        assert_eq!(secs(50), 60);
        // Monotonic: a later attempt is never polled sooner than an earlier one.
        for n in 1..30 {
            assert!(secs(n + 1) >= secs(n), "attempt {n} regressed");
        }
        // Zero must not panic nor produce an instant re-poll loop.
        assert_eq!(secs(0), 5);
    }

    #[test]
    fn the_backoff_reaches_the_measured_latency_in_fewer_polls_than_a_fixed_interval() {
        // The lightest real generation took ~100 s.
        let target = 100u64;
        let mut elapsed = 0;
        let mut polls = 0;
        while elapsed < target {
            polls += 1;
            elapsed += backoff_delay(polls).as_secs();
        }
        let fixed_interval_polls = (target / 10) as u32;
        assert!(
            polls < fixed_interval_polls,
            "{polls} polls should beat {fixed_interval_polls} at a fixed 10 s"
        );
    }

    #[test]
    fn the_default_deadline_leaves_ample_room_over_the_measured_latency() {
        assert!(DEFAULT_DEADLINE.as_secs() > 100 * 10);
    }
}
