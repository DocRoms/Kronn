// 0.8.6 phase 4 — Smart polling hints for MCP remote-control tools.
//
// Shared between `workflow_trigger`, `workflow_run_status`, `qp_run` to
// answer one question: *when should the agent ping us again?*
//
// Without this, an agent polling a 2-min workflow burns ~13 round-trips
// (~13k tokens) instead of 2-3 (~2k). The `NextCheck` we return is a
// **suggestion** — the agent can ignore it, but every Claude / Codex /
// Gemini that respects the hint cuts its mobile-control cost by 80%.
//
// The math is intentionally boring: pick the smallest of (sanity-check
// floor, time-to-expected-completion, backoff after overshoot). All the
// surface area is in the explanation string so an LLM reading the
// response understands *why* it should wait that long.

use serde::Serialize;

/// Wait floor in seconds for the very first check after a trigger.
/// Below this we can't even tell whether the runner actually started.
pub const SANITY_CHECK_S: u64 = 30;

/// Minimum buffer added on top of expected duration so a healthy run
/// that's within the average doesn't get woken up "just in case" at
/// the exact same millisecond it should finish.
pub const COMPLETION_BUFFER_S: u64 = 15;

/// Wait we suggest *after* the run has overshot its expected duration —
/// no useful prior to extrapolate from, so just step at a fixed rate.
pub const OVERSHOOT_BACKOFF_S: u64 = 30;

/// Wait we suggest when no historical baseline exists (< 3 runs).
pub const NO_BASELINE_WAIT_S: u64 = 60;

/// Minimum sample size required to trust the average. Below this we
/// emit `confidence: NoBaseline` and fall back to `NO_BASELINE_WAIT_S`.
/// Matches `qp_versions` metrics' 3-launch floor (cf.
/// `project_qp_versions_metrics`) — same threshold project-wide.
pub const MIN_SAMPLES: u32 = 3;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// `< MIN_SAMPLES` runs in history — we don't trust the average yet.
    NoBaseline,
    /// Enough samples; the suggested wait is anchored to the average.
    Baseline,
    /// We've overshot the average — backoff fixed, no projection.
    Overshoot,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NextCheck {
    /// Seconds the agent should wait before calling status again.
    pub wait_seconds: u64,
    /// Human-readable rationale (logged + surfaced verbatim in MCP).
    pub reason: String,
    pub confidence: Confidence,
}

/// First-poll hint right after a trigger. We always wait at least
/// `SANITY_CHECK_S` to confirm the runner actually picked up the run —
/// it's the cheapest signal we have that nothing broke at boot.
pub fn next_check_initial(expected_ms: Option<u64>, samples: u32) -> NextCheck {
    if let (true, Some(expected_ms)) = (samples >= MIN_SAMPLES, expected_ms) {
        NextCheck {
            wait_seconds: SANITY_CHECK_S,
            reason: format!(
                "sanity check — confirm the run actually started (expected total ~{}s based on {} prior runs)",
                expected_ms / 1000,
                samples,
            ),
            confidence: Confidence::Baseline,
        }
    } else {
        NextCheck {
            wait_seconds: SANITY_CHECK_S,
            reason: "sanity check — confirm the run actually started (no historical baseline yet)"
                .to_string(),
            confidence: Confidence::NoBaseline,
        }
    }
}

/// In-flight hint after at least one status poll. Three branches:
///
/// 1. **No baseline** (`samples < MIN_SAMPLES`): suggest a fixed
///    `NO_BASELINE_WAIT_S` — we can't project anything sensible.
/// 2. **Within baseline** (`elapsed < expected`): suggest waiting until
///    completion + a small buffer, so the agent calls back exactly when
///    the run *should* be done.
/// 3. **Overshoot** (`elapsed >= expected`): fixed `OVERSHOOT_BACKOFF_S`
///    backoff — the average can't help us anymore.
pub fn next_check_polling(expected_ms: Option<u64>, elapsed_ms: u64, samples: u32) -> NextCheck {
    match expected_ms {
        Some(expected) if samples >= MIN_SAMPLES => {
            if elapsed_ms < expected {
                let remaining_ms = expected - elapsed_ms;
                let remaining_s = (remaining_ms / 1000).max(1);
                let wait = remaining_s + COMPLETION_BUFFER_S;
                NextCheck {
                    wait_seconds: wait,
                    reason: format!(
                        "average of {} prior runs is ~{}s ; you've waited {}s already, expect ~{}s left (+{}s buffer)",
                        samples,
                        expected / 1000,
                        elapsed_ms / 1000,
                        remaining_s,
                        COMPLETION_BUFFER_S,
                    ),
                    confidence: Confidence::Baseline,
                }
            } else {
                NextCheck {
                    wait_seconds: OVERSHOOT_BACKOFF_S,
                    reason: format!(
                        "already past the {}s average ({} prior runs) — the run is taking longer than usual, backing off {}s",
                        expected / 1000,
                        samples,
                        OVERSHOOT_BACKOFF_S,
                    ),
                    confidence: Confidence::Overshoot,
                }
            }
        }
        _ => NextCheck {
            wait_seconds: NO_BASELINE_WAIT_S,
            reason: format!(
                "no historical baseline yet (< {} prior runs) — checking every {}s until completion",
                MIN_SAMPLES, NO_BASELINE_WAIT_S,
            ),
            confidence: Confidence::NoBaseline,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_with_baseline_anchors_on_sanity_floor() {
        let n = next_check_initial(Some(120_000), 5);
        assert_eq!(n.wait_seconds, SANITY_CHECK_S);
        assert_eq!(n.confidence, Confidence::Baseline);
        assert!(n.reason.contains("120s"));
        assert!(n.reason.contains("5 prior runs"));
    }

    #[test]
    fn initial_without_baseline_still_returns_sanity_floor_but_no_baseline_confidence() {
        let n = next_check_initial(None, 0);
        assert_eq!(n.wait_seconds, SANITY_CHECK_S);
        assert_eq!(n.confidence, Confidence::NoBaseline);
    }

    #[test]
    fn initial_with_expected_but_too_few_samples_falls_back_to_no_baseline() {
        // Only 2 samples — we have an "average" but it's not trustworthy.
        let n = next_check_initial(Some(60_000), 2);
        assert_eq!(n.confidence, Confidence::NoBaseline);
    }

    #[test]
    fn polling_within_baseline_projects_remaining_time_plus_buffer() {
        // 120s expected, 30s elapsed, 5 samples → wait = (120-30) + 15 = 105
        let n = next_check_polling(Some(120_000), 30_000, 5);
        assert_eq!(n.wait_seconds, 90 + COMPLETION_BUFFER_S);
        assert_eq!(n.confidence, Confidence::Baseline);
        assert!(n.reason.contains("5 prior runs"));
        assert!(n.reason.contains("90s left"));
    }

    #[test]
    fn polling_after_overshoot_backs_off_fixed() {
        // 60s expected, 75s elapsed → overshoot
        let n = next_check_polling(Some(60_000), 75_000, 4);
        assert_eq!(n.wait_seconds, OVERSHOOT_BACKOFF_S);
        assert_eq!(n.confidence, Confidence::Overshoot);
        assert!(n.reason.contains("past"));
    }

    #[test]
    fn polling_with_no_samples_uses_fixed_backoff() {
        let n = next_check_polling(None, 5_000, 0);
        assert_eq!(n.wait_seconds, NO_BASELINE_WAIT_S);
        assert_eq!(n.confidence, Confidence::NoBaseline);
    }

    #[test]
    fn polling_at_exact_expected_treats_it_as_overshoot() {
        // Boundary: elapsed == expected → overshoot branch (the run
        // *should* have finished, so further waiting is backoff territory).
        let n = next_check_polling(Some(60_000), 60_000, 5);
        assert_eq!(n.confidence, Confidence::Overshoot);
    }

    #[test]
    fn polling_minimum_remaining_clamped_to_one_second() {
        // 60s expected, 59.999s elapsed → 1ms left → clamped to 1s + buffer
        let n = next_check_polling(Some(60_000), 59_999, 5);
        assert_eq!(n.wait_seconds, 1 + COMPLETION_BUFFER_S);
        assert_eq!(n.confidence, Confidence::Baseline);
    }

    #[test]
    fn min_samples_threshold_matches_qp_versions_metrics_floor() {
        // 0.8.4 set the qp_versions floor at 3 — keep this assertion so a
        // refactor that touches one knows to look at the other.
        assert_eq!(MIN_SAMPLES, 3);
    }
}
