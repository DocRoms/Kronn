//! KT-1046 — a run's anti-loop guard counters, kept in `run.state` so every
//! resume (Gate, quota wake-up, crash) continues them instead of restarting.

use std::collections::HashMap;

use crate::models::{RunStatus, StepResult, StepType, WorkflowStep};

/// `run.state` key holding the serialized [`GuardCounters`].
pub const GUARD_COUNTERS_STATE_KEY: &str = "__kronn.guard_counters";

/// Step visits, Goto-edge fires, total iterations and LLM calls of a run.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GuardCounters {
    pub iterations: usize,
    /// The root run's count covers its whole sub-workflow tree.
    pub llm_calls: u32,
    pub visits: HashMap<String, usize>,
    pub goto_fires: HashMap<(String, String), u32>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    v: u32,
    iterations: usize,
    llm_calls: u32,
    visits: std::collections::BTreeMap<String, usize>,
    goto_fires: Vec<(String, String, u32)>,
}

impl GuardCounters {
    /// The counters a (re)started run continues from. `retried_step` is a step
    /// whose unfinished attempt is replayed: its replay is not a new visit.
    pub fn resume(
        state: &HashMap<String, String>,
        steps: &[WorkflowStep],
        history: &[StepResult],
        retried_step: Option<&str>,
    ) -> Self {
        match state
            .get(GUARD_COUNTERS_STATE_KEY)
            .and_then(|raw| Self::parse(raw))
        {
            Some(mut counters) => {
                if let Some(step) = retried_step {
                    counters.retract_visit(step);
                }
                counters
            }
            // Runs paused before the counters were stored: rebuild what the
            // kept history shows rather than starting from zero.
            None => Self::from_history(steps, history),
        }
    }

    /// Whether `state` holds readable counters (false for runs from before them).
    pub fn is_stored(state: &HashMap<String, String>) -> bool {
        state
            .get(GUARD_COUNTERS_STATE_KEY)
            .and_then(|raw| Self::parse(raw))
            .is_some()
    }

    fn parse(raw: &str) -> Option<Self> {
        let stored: Stored = serde_json::from_str(raw).ok()?;
        (stored.v == 1).then(|| Self {
            iterations: stored.iterations,
            llm_calls: stored.llm_calls,
            visits: stored.visits.into_iter().collect(),
            goto_fires: stored
                .goto_fires
                .into_iter()
                .map(|(from, to, n)| ((from, to), n))
                .collect(),
        })
    }

    fn from_history(steps: &[WorkflowStep], history: &[StepResult]) -> Self {
        let mut counters = Self::default();
        for result in history {
            let Some(step) = steps.iter().find(|s| s.name == result.step_name) else {
                continue;
            };
            counters.iterations += 1;
            *counters.visits.entry(step.name.clone()).or_insert(0) += 1;
            if matches!(step.step_type, StepType::Agent | StepType::BatchQuickPrompt)
                && result.status != RunStatus::WaitingQuota
            {
                counters.llm_calls += 1;
            }
        }
        counters
    }

    fn retract_visit(&mut self, step: &str) {
        self.iterations = self.iterations.saturating_sub(1);
        if let Some(n) = self.visits.get_mut(step) {
            *n = n.saturating_sub(1);
        }
    }

    /// Writes the counters into the run's durable state.
    pub fn store(&self, state: &mut HashMap<String, String>) {
        let mut goto_fires: Vec<(String, String, u32)> = self
            .goto_fires
            .iter()
            .map(|((from, to), n)| (from.clone(), to.clone(), *n))
            .collect();
        goto_fires.sort();
        let stored = Stored {
            v: 1,
            iterations: self.iterations,
            llm_calls: self.llm_calls,
            visits: self.visits.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            goto_fires,
        };
        if let Ok(raw) = serde_json::to_string(&stored) {
            state.insert(GUARD_COUNTERS_STATE_KEY.to_string(), raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, step_type: StepType) -> WorkflowStep {
        serde_json::from_value(serde_json::json!({
            "name": name, "step_type": {"type": format!("{step_type:?}")},
            "agent": "ClaudeCode", "prompt_template": "", "mode": {"type": "Normal"},
        }))
        .unwrap()
    }

    fn result(name: &str, status: RunStatus) -> StepResult {
        serde_json::from_value(serde_json::json!({
            "step_name": name, "status": status, "output": "", "duration_ms": 0,
        }))
        .unwrap()
    }

    #[test]
    fn stored_counters_round_trip_and_a_retried_step_is_not_a_new_visit() {
        let mut counters = GuardCounters {
            iterations: 5,
            llm_calls: 3,
            ..Default::default()
        };
        counters.visits.insert("work".into(), 3);
        counters
            .goto_fires
            .insert(("back".into(), "work".into()), 2);
        let mut state = HashMap::new();
        counters.store(&mut state);

        assert_eq!(GuardCounters::resume(&state, &[], &[], None), counters);
        let retried = GuardCounters::resume(&state, &[], &[], Some("work"));
        assert_eq!(retried.visits["work"], 2);
        assert_eq!(retried.iterations, 4);
        assert_eq!(retried.llm_calls, 3);
        assert_eq!(retried.goto_fires, counters.goto_fires);
    }

    #[test]
    fn a_run_without_stored_counters_rebuilds_them_from_its_history() {
        let steps = [
            step("work", StepType::Agent),
            step("review", StepType::Gate),
        ];
        let history = [
            result("work", RunStatus::Success),
            result("review", RunStatus::Success),
            result("work", RunStatus::Success),
            result("__guard_timeout__", RunStatus::StoppedByGuard),
        ];
        let counters = GuardCounters::resume(&HashMap::new(), &steps, &history, Some("work"));
        assert_eq!(counters.visits["work"], 2);
        assert_eq!(counters.visits["review"], 1);
        assert_eq!(counters.iterations, 3);
        assert_eq!(counters.llm_calls, 2);
    }

    #[test]
    fn unreadable_counters_fall_back_to_the_history() {
        let steps = [step("work", StepType::Agent)];
        let history = [result("work", RunStatus::Success)];
        let state = HashMap::from([(GUARD_COUNTERS_STATE_KEY.to_string(), "{".to_string())]);
        let counters = GuardCounters::resume(&state, &steps, &history, None);
        assert_eq!(counters.visits["work"], 1);
        assert_eq!(counters.llm_calls, 1);
    }
}
