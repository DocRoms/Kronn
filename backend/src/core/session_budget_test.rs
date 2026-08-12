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
        max_age_hours: 10,
        max_turns: 100,
        soft_ratio: 0.75,
    }
}

#[test]
fn a_quiet_session_is_ok() {
    let out = assess(&budget(), Some(10), Some(1), Some(2));
    assert_eq!(out.verdict, BudgetVerdict::Ok);
    assert_eq!(out.axes.len(), 3);
}

#[test]
fn the_soft_ratio_warns_before_the_ceiling() {
    // The whole reason there are two thresholds: a warning leaves a turn to
    // write the resume bundle. A hard cap alone cuts the session off mid-thought
    // with nothing prepared.
    let out = assess(&budget(), Some(800), Some(1), Some(2));
    assert_eq!(out.verdict, BudgetVerdict::Warn);
    assert!(out.reason.contains("resume bundle"), "{}", out.reason);
}

#[test]
fn exactly_at_the_soft_ratio_warns() {
    // 750/1000 with soft_ratio 0.75 — a boundary that must not fall through.
    assert_eq!(
        assess(&budget(), Some(750), Some(1), Some(2)).verdict,
        BudgetVerdict::Warn
    );
}

#[test]
fn just_below_the_soft_ratio_is_still_ok() {
    assert_eq!(
        assess(&budget(), Some(749), Some(1), Some(2)).verdict,
        BudgetVerdict::Ok
    );
}

#[test]
fn exactly_at_the_ceiling_rotates() {
    assert_eq!(
        assess(&budget(), Some(1_000), Some(1), Some(2)).verdict,
        BudgetVerdict::Rotate
    );
}

#[test]
fn the_worst_axis_decides() {
    // Being inside two ceilings does not offset breaking the third. A verdict
    // that averaged the axes would let a runaway traffic figure hide behind a
    // young session.
    let out = assess(&budget(), Some(5_000), Some(1), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("traffic_tokens"), "{}", out.reason);
}

#[test]
fn age_alone_can_trigger_a_rotation() {
    // A session can be cheap and still stale: it carries context it will never
    // use again, and every future read pays for it.
    let out = assess(&budget(), Some(1), Some(99), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("age_hours"), "{}", out.reason);
}

#[test]
fn turns_alone_can_trigger_a_rotation() {
    let out = assess(&budget(), Some(1), Some(1), Some(500));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
    assert!(out.reason.contains("turns"), "{}", out.reason);
}

#[test]
fn an_unmeasured_axis_is_unknown_not_ok() {
    // THE test. A vendor with no collector must not be exempt from the budget:
    // an unmeasured session is not known to be cheap, only unwatched.
    let out = assess(&budget(), None, Some(1), Some(1));
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
    let out = assess(&budget(), None, Some(99), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Rotate);
}

#[test]
fn a_warning_outranks_an_unmeasured_axis() {
    let out = assess(&budget(), None, Some(8), Some(1));
    assert_eq!(out.verdict, BudgetVerdict::Warn);
}

#[test]
fn every_axis_is_reported_even_when_ok() {
    // A report that only listed the offending axis would give no sense of how
    // close the others are.
    let out = assess(&budget(), Some(10), Some(1), Some(2));
    let names: Vec<&str> = out.axes.iter().map(|axis| axis.name.as_str()).collect();
    assert_eq!(names, vec!["traffic_tokens", "age_hours", "turns"]);
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
        Some(1),
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
        Some(9 * 24),
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
        Some(4),
        Some(30),
    );
    assert_eq!(out.verdict, BudgetVerdict::Ok);
}
