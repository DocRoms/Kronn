//! Tests for CLI session budgets — KT-193.
//!
//! The failure that matters is a budget that stays green while a session runs
//! away — which is exactly what happened before it existed: 4 143 787 451 tokens
//! over 9 days, unwatched. So the tests centre on the cases where a permissive
//! answer would be invisible: an unmeasured axis, one axis breaching while
//! others are fine, and the boundary values themselves.

use super::*;

fn budget() -> SessionBudget {
    SessionBudget {
        max_traffic_tokens: 1_000,
        max_active_hours: 10,
        max_inactive_gap_minutes: 30,
        max_turns: 100,
        soft_ratio: 0.75,
    }
}

#[test]
fn a_quiet_session_is_ok() {
    let out = assess(&budget(), Some(10), Some(1.0), Some(2));
    assert_eq!(out.verdict, BudgetVerdict::Ok);
    assert_eq!(out.axes.len(), 3);
}

#[test]
fn the_soft_ratio_warns_before_the_ceiling() {
    // The whole reason there are two thresholds: a warning leaves a turn to
    // write the resume bundle. A hard cap alone cuts the session off mid-thought
    // with nothing prepared.
    let out = assess(&budget(), Some(800), Some(1.0), Some(2));
    assert_eq!(out.verdict, BudgetVerdict::Warn);
    assert!(out.reason.contains("resume bundle"), "{}", out.reason);
}

#[test]
fn exactly_at_the_soft_ratio_warns() {
    // 750/1000 with soft_ratio 0.75 — a boundary that must not fall through.
    assert_eq!(
        assess(&budget(), Some(750), Some(1.0), Some(2)).verdict,
        BudgetVerdict::Warn
    );
}

#[test]
fn just_below_the_soft_ratio_is_still_ok() {
    assert_eq!(
        assess(&budget(), Some(749), Some(1.0), Some(2)).verdict,
        BudgetVerdict::Ok
    );
}

#[test]
fn exactly_at_the_ceiling_rotates() {
    assert_eq!(
        assess(&budget(), Some(1_000), Some(1.0), Some(2)).verdict,
        BudgetVerdict::Rotate
    );
}

#[test]
fn the_worst_axis_decides() {
    // Being inside two ceilings does not offset breaking the third. A verdict
    // that averaged the axes would let a runaway traffic figure hide behind a
    // young session.
    let out = assess(&budget(), Some(5_000), Some(1.0), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("traffic_tokens"), "{}", out.reason);
}

#[test]
fn active_time_alone_can_trigger_a_rotation() {
    let out = assess(&budget(), Some(1), Some(99.0), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("active_hours"), "{}", out.reason);
}

#[test]
fn turns_alone_can_trigger_a_rotation() {
    let out = assess(&budget(), Some(1), Some(1.0), Some(500));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("turns"), "{}", out.reason);
}

#[test]
fn an_unmeasured_axis_is_unknown_not_ok() {
    // THE test. A vendor with no collector must not be exempt from the budget:
    // an unmeasured session is not known to be cheap, only unwatched.
    let out = assess(&budget(), None, Some(1.0), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Unknown);
    assert!(out.reason.contains("not measured"), "{}", out.reason);
    assert!(out.reason.contains("unwatched"), "{}", out.reason);
    // The axis still appears, with no fabricated value.
    let traffic = out
        .axes
        .iter()
        .find(|axis| axis.name == "traffic_tokens")
        .unwrap();
    assert_eq!(traffic.current, None);
    assert_eq!(traffic.ratio, None);
}

#[test]
fn a_real_breach_outranks_an_unmeasured_axis() {
    // Unknown must be visible, but it must not drown out a ceiling that is
    // genuinely broken — that would be the worst of both.
    let out = assess(&budget(), None, Some(99.0), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
}

#[test]
fn a_warning_outranks_an_unmeasured_axis() {
    let out = assess(&budget(), None, Some(8.0), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Warn);
}

#[test]
fn every_axis_is_reported_even_when_ok() {
    // A report that only listed the offending axis would give no sense of how
    // close the others are.
    let out = assess(&budget(), Some(10), Some(1.0), Some(2));
    let names: Vec<&str> = out.axes.iter().map(|axis| axis.name.as_str()).collect();
    assert_eq!(names, vec!["traffic_tokens", "active_hours", "turns"]);
    for axis in &out.axes {
        assert!(axis.ratio.is_some());
    }
}

#[test]
fn a_zero_ceiling_does_not_divide_by_zero() {
    let out = assess(
        &SessionBudget {
            max_traffic_tokens: 0,
            ..budget()
        },
        Some(5),
        Some(1.0),
        Some(1),
    );
    // No ratio is derivable, so the axis is unknown rather than infinitely bad.
    let traffic = out
        .axes
        .iter()
        .find(|axis| axis.name == "traffic_tokens")
        .unwrap();
    assert_eq!(traffic.ratio, None);
}

#[test]
fn the_defaults_would_have_flagged_the_measured_session() {
    // Sanity against reality: this very session reached 4 143 787 451 tokens
    // over 9 days. If the shipped defaults called that healthy, they would be
    // decoration.
    let out = assess(
        &SessionBudget::default(),
        Some(4_143_787_451),
        Some((9 * 24) as f64),
        Some(300),
    );
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
}

#[test]
fn the_defaults_leave_ordinary_work_alone() {
    // A cap that fires on a normal afternoon trains people to ignore it, which
    // is worse than no cap.
    let out = assess(
        &SessionBudget::default(),
        Some(50_000_000),
        Some(4.0),
        Some(30),
    );
    assert_eq!(out.verdict, BudgetVerdict::Ok);
}

// ── the cache told apart from the real input — KT-894 ───────────────────

/// The counters of the measured session KT-193 was calibrated on.
fn measured_traffic() -> TrafficBreakdown {
    TrafficBreakdown {
        input_tokens: Some(16_826),
        cache_write_tokens: Some(61_095_483),
        cache_read_tokens: Some(4_077_307_836),
        output_tokens: Some(5_367_306),
    }
}

#[test]
fn the_traffic_axis_still_counts_the_cache_but_reports_it_apart() {
    let out = assess_traffic(
        &SessionBudget::default(),
        Some(measured_traffic()),
        Some(1.0),
        Some(2),
    );
    // The ceiling was calibrated on the total, cache reads included: unchanged.
    let axis = out
        .axes
        .iter()
        .find(|axis| axis.name == "traffic_tokens")
        .unwrap();
    assert_eq!(axis.current, Some(4_143_787_451.0));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    // What changed: the split rides along, so 98% cache is not read as 4 billion
    // fresh tokens.
    let traffic = out.traffic.expect("the counters are reported apart");
    assert_eq!(traffic.input_tokens, Some(16_826));
    assert_eq!(traffic.cache_read_tokens, Some(4_077_307_836));
    assert!(traffic.cache_read_share().unwrap() > 0.98);
    assert!(
        out.reason.contains("98% of that traffic is cache reads"),
        "{}",
        out.reason
    );
    assert!(
        out.reason.contains("16826 tokens fresh input"),
        "{}",
        out.reason
    );
}

#[test]
fn an_unreported_cache_read_gives_no_share_and_no_invented_clause() {
    // A vendor that publishes no cache breakdown (Vibe): the share is unknown,
    // and the reason must not pretend otherwise.
    let no_cache = TrafficBreakdown {
        input_tokens: Some(900_000_000),
        cache_write_tokens: None,
        cache_read_tokens: None,
        output_tokens: Some(200_000_000),
    };
    assert_eq!(no_cache.cache_read_share(), None);
    let out = assess_traffic(
        &SessionBudget::default(),
        Some(no_cache),
        Some(1.0),
        Some(2),
    );
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(!out.reason.contains("cache"), "{}", out.reason);
    assert_eq!(out.traffic, Some(no_cache));
}

#[test]
fn a_quiet_session_carries_the_split_without_it_changing_the_verdict() {
    let quiet = TrafficBreakdown {
        input_tokens: Some(10),
        cache_write_tokens: Some(0),
        cache_read_tokens: Some(90),
        output_tokens: Some(5),
    };
    let out = assess_traffic(&budget(), Some(quiet), Some(1.0), Some(2));
    assert_eq!(out.verdict, BudgetVerdict::Ok);
    assert_eq!(out.reason, "within every ceiling");
    assert_eq!(out.traffic, Some(quiet));
}

#[test]
fn nothing_measured_is_unknown_and_reports_no_share() {
    let nothing = TrafficBreakdown {
        input_tokens: None,
        cache_write_tokens: None,
        cache_read_tokens: None,
        output_tokens: None,
    };
    assert_eq!(nothing.total(), None);
    assert_eq!(nothing.cache_read_share(), None);
    let out = assess_traffic(&budget(), Some(nothing), Some(1.0), Some(2));
    assert_eq!(out.verdict, BudgetVerdict::Unknown);
    // No counters at all is the same as no telemetry row.
    assert_eq!(
        assess_traffic(&budget(), None, Some(1.0), Some(2)).verdict,
        BudgetVerdict::Unknown
    );
}

#[test]
fn a_non_traffic_axis_never_borrows_the_cache_clause() {
    // Turns fire, traffic is fine: the reason names turns, not cache reads.
    let small = TrafficBreakdown {
        input_tokens: Some(1),
        cache_write_tokens: Some(0),
        cache_read_tokens: Some(8),
        output_tokens: Some(1),
    };
    let out = assess_traffic(&budget(), Some(small), Some(1.0), Some(100));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("turns"), "{}", out.reason);
    assert!(!out.reason.contains("cache reads"), "{}", out.reason);
}

#[test]
fn the_plain_assessment_reports_no_split() {
    // `assess` has only a total to work with: it must not invent a breakdown.
    let out = assess(&budget(), Some(10), Some(1.0), Some(2));
    assert_eq!(out.traffic, None);
}
