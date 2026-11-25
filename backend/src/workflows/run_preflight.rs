//! KT-2055 — the Agent steps a run's preflight judges: every one the run will
//! certainly start whose failure fails the run, read as it will run.
//!
//! - each Agent step of the workflow, hydrated from its Quick Prompt (the QP's
//!   connection lands on a step naming none only at hydration, so the raw step
//!   is not the one that runs);
//! - each Agent step of the sub-workflows its `SubWorkflow` steps start,
//!   transitively, each child once (cycle-safe) and within the depth cap.
//!
//! Optional agents — fallback links, `on_failure` steps, shadows whose failure
//! is tolerated — are left to the dispatch rule of `steps::execute_step`.

use std::collections::{HashSet, VecDeque};

use crate::models::{StepType, Workflow, WorkflowStep};
use crate::AppState;

/// Mirrors `sub_workflow_step::MAX_SUBWORKFLOW_DEPTH`: deeper children are
/// refused by the run itself, never started.
const MAX_SUBWORKFLOW_DEPTH: usize = 5;

/// One Agent step the run will certainly start.
pub struct PlannedStep {
    /// The step as it will run: hydrated from its Quick Prompt when it loads.
    pub step: WorkflowStep,
    /// `false` when its Quick Prompt did not load: the step fails at dispatch
    /// in its own words, so its connection is not judged here.
    pub hydrated: bool,
    /// `None` for a step of the run's own workflow; for a sub-workflow's
    /// step, the way the run reaches it.
    pub within: Option<String>,
}

impl PlannedStep {
    /// `reason`, prefixed with where the step lives when it is a child's.
    pub fn describe(&self, reason: String) -> String {
        match &self.within {
            None => reason,
            Some(within) => format!("{within}: {reason}"),
        }
    }
}

/// The Agent steps `workflow`'s run `run_id` will certainly start, the
/// workflow's own first, then its sub-workflows' (pinned revisions, KT-1096).
/// A disabled or missing child is left to the step that starts it.
pub async fn planned_agent_steps(
    state: &AppState,
    workflow: &Workflow,
    run_id: &str,
) -> Vec<PlannedStep> {
    let mut planned = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([workflow.id.clone()]);
    let mut queue: VecDeque<(Workflow, Option<String>, usize)> =
        VecDeque::from([(workflow.clone(), None, 0)]);
    while let Some((current, within, depth)) = queue.pop_front() {
        for step in &current.steps {
            match step.step_type {
                StepType::Agent => {
                    let mut hydrated = step.clone();
                    let loaded = super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
                        &mut hydrated,
                        &state.db,
                        Some(run_id),
                    )
                    .await
                    .is_ok();
                    planned.push(PlannedStep {
                        step: if loaded { hydrated } else { step.clone() },
                        hydrated: loaded,
                        within: within.clone(),
                    });
                }
                StepType::SubWorkflow if depth < MAX_SUBWORKFLOW_DEPTH => {
                    let Some(target) = step
                        .sub_workflow_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    // Each child once: a cycle, or a child started twice, is
                    // judged a single time.
                    if !seen.insert(target.clone()) {
                        continue;
                    }
                    let parent_run = run_id.to_string();
                    let child = state
                        .db
                        .with_read_conn(move |conn| {
                            super::run_pins::sub_workflow_for(conn, &parent_run, &target)
                        })
                        .await;
                    if let Ok(Ok(Some(child))) = child {
                        if !child.enabled {
                            continue;
                        }
                        let here = format!("sub-workflow `{}` (step '{}')", child.name, step.name);
                        let path = match &within {
                            None => here,
                            Some(parent) => format!("{parent} › {here}"),
                        };
                        queue.push_back((child, Some(path), depth + 1));
                    }
                }
                _ => {}
            }
        }
    }
    planned
}
