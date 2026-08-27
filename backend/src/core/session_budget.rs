//! Caps that make a CLI session a disposable compute unit — KT-193.
//!
//! A joined CLI session is not supposed to be durable memory; the plan and its
//! tasks are. But nothing bounded a session, so a thread that stayed open for
//! days kept paying to re-read its own history. Measured on this very session:
//! 4 143 787 451 tokens of traffic over 9 days, 98.4% of it cache reads of a
//! transcript that was growing by the hour.
//!
//! Two thresholds, not one. A hard cap alone would end a session mid-thought
//! with nothing prepared; a soft cap gives the agent a turn to write its resume
//! bundle first, which is the difference between rotating and being cut off.
//!
//! Nothing here terminates anything. It reports a verdict, and the caller
//! decides — a budget that silently killed a session would be worse than the
//! cost it saves.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Configurable ceilings for one CLI session.
///
/// Defaults come from the real measurement above, deliberately generous: a cap
/// that fires on ordinary work would train people to ignore it, which is worse
/// than no cap at all.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct SessionBudget {
    /// Cumulative traffic — everything the vendor reported, cache reads
    /// included. It is the right axis because cache reads ARE the cost of a long
    /// thread: they were 98.4% of that 4.1 billion.
    pub max_traffic_tokens: i64,
    /// Observed active time, in hours. Long gaps between this session's turns
    /// are treated as inactivity rather than work.
    #[serde(alias = "max_age_hours")]
    pub max_active_hours: i64,
    /// Maximum gap counted as continuous work between two turns. The default
    /// is 30 minutes: enough to cover a normal edit/test cycle, while a longer
    /// gap is more plausibly a pause than active context accumulation.
    #[serde(default = "default_inactivity_threshold_minutes")]
    pub max_inactive_gap_minutes: i64,
    /// Messages this session posted to rooms.
    pub max_turns: i64,
    /// Fraction of each ceiling at which a WARNING fires, leaving room to
    /// prepare a handover instead of being cut off mid-task.
    pub soft_ratio: f64,
}

impl Default for SessionBudget {
    fn default() -> Self {
        Self {
            // ~1 billion: a quarter of what this session reached, so the warning
            // lands while the thread is still workable rather than after the
            // damage.
            max_traffic_tokens: 1_000_000_000,
            // Two days of observed active work. Long enough for a real piece of
            // work, short enough that a forgotten session does not run all week.
            max_active_hours: 48,
            // A half-hour is a pragmatic compromise: it preserves short pauses
            // inside a work block without charging nights or other long stops.
            max_inactive_gap_minutes: default_inactivity_threshold_minutes(),
            max_turns: 400,
            soft_ratio: 0.75,
        }
    }
}

fn default_inactivity_threshold_minutes() -> i64 {
    30
}

/// Which ceiling a session is closest to, and how close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BudgetVerdict {
    /// Comfortably inside every ceiling.
    Ok,
    /// Past the soft ratio on at least one axis. Time to write the resume bundle
    /// — while there is still budget to write it with.
    Warn,
    /// A ceiling is exceeded. The session should rotate.
    Rotate,
    /// At least one axis could not be measured, so no verdict is possible.
    /// Reported rather than optimistically treated as `Ok`: a session whose
    /// cost is unknown is exactly the one that has been running unwatched.
    Unknown,
}

/// One axis of the assessment, kept separate so a report can name WHICH ceiling
/// is close instead of a bare colour.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BudgetAxis {
    pub name: String,
    /// `None` when unmeasured — never 0, which would read as "cost nothing".
    pub current: Option<f64>,
    pub ceiling: i64,
    /// Share of the ceiling used. `None` when unmeasured.
    pub ratio: Option<f64>,
}

impl BudgetAxis {
    fn new(name: &str, current: Option<f64>, ceiling: i64) -> Self {
        Self {
            name: name.to_string(),
            current,
            ceiling,
            ratio: current.and_then(|value| (ceiling > 0).then(|| value / ceiling as f64)),
        }
    }

    fn verdict(&self, soft_ratio: f64) -> BudgetVerdict {
        match self.ratio {
            None => BudgetVerdict::Unknown,
            Some(ratio) if ratio >= 1.0 => BudgetVerdict::Rotate,
            Some(ratio) if ratio >= soft_ratio => BudgetVerdict::Warn,
            Some(_) => BudgetVerdict::Ok,
        }
    }
}

/// The whole assessment: a verdict, plus every axis so the reason is visible.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BudgetAssessment {
    pub verdict: BudgetVerdict,
    pub axes: Vec<BudgetAxis>,
    /// Why, in one sentence, for whoever reads it in a log or a tooltip.
    pub reason: String,
}

/// Assess a session against its budget.
///
/// `traffic_tokens` is `None` for a vendor with no collector — and that case
/// yields `Unknown`, not `Ok`. Treating unmeasured as fine would exempt from the
/// budget precisely the sessions nobody is watching. `active_hours` is derived
/// from this session's posted turns, with each inter-turn gap already capped by
/// the configured inactivity threshold.
pub fn assess(
    budget: &SessionBudget,
    traffic_tokens: Option<i64>,
    active_hours: Option<f64>,
    turns: Option<i64>,
) -> BudgetAssessment {
    let axes = vec![
        BudgetAxis::new(
            "traffic_tokens",
            traffic_tokens.map(|value| value as f64),
            budget.max_traffic_tokens,
        ),
        BudgetAxis::new("active_hours", active_hours, budget.max_active_hours),
        BudgetAxis::new("turns", turns.map(|value| value as f64), budget.max_turns),
    ];

    // The WORST axis decides: being inside two ceilings does not offset breaking
    // the third.
    let mut verdict = BudgetVerdict::Ok;
    let mut culprit: Option<&BudgetAxis> = None;
    for axis in &axes {
        let axis_verdict = axis.verdict(budget.soft_ratio);
        let rank = |v: BudgetVerdict| match v {
            BudgetVerdict::Ok => 0,
            // Unknown outranks Ok but not Warn: a missing axis must be visible
            // without drowning out a ceiling that is genuinely close.
            BudgetVerdict::Unknown => 1,
            BudgetVerdict::Warn => 2,
            BudgetVerdict::Rotate => 3,
        };
        if rank(axis_verdict) > rank(verdict) {
            verdict = axis_verdict;
            culprit = Some(axis);
        }
    }

    let reason = match (verdict, culprit) {
        (BudgetVerdict::Ok, _) => "within every ceiling".to_string(),
        (BudgetVerdict::Unknown, Some(axis)) => format!(
            "{} is not measured, so this session cannot be assessed — it is not \
             known to be cheap, only unwatched",
            axis.name
        ),
        (BudgetVerdict::Warn, Some(axis)) => format!(
            "{} at {:.0}% of its ceiling — write the resume bundle now, while \
             there is still budget to write it with",
            axis.name,
            axis.ratio.unwrap_or(0.0) * 100.0
        ),
        (BudgetVerdict::Rotate, Some(axis)) => format!(
            "{} past its ceiling ({} of {}) — rotate this session",
            axis.name,
            axis.current.unwrap_or(0.0),
            axis.ceiling
        ),
        (verdict, None) => format!("{verdict:?} with no axis to explain it"),
    };

    BudgetAssessment {
        verdict,
        axes,
        reason,
    }
}

#[cfg(test)]
#[path = "session_budget_test.rs"]
mod session_budget_test;
