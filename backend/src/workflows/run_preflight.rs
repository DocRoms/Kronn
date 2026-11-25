//! KT-2055 — the Agent steps a run's preflight judges: every one the run will
//! certainly start whose failure fails the run, read as it will run.
//!
//! - each Agent step of the workflow, hydrated from its Quick Prompt (the QP's
//!   connection lands on a step naming none only at hydration, so the raw step
//!   is not the one that runs);
//! - each `BatchQuickPrompt` step, read as the step its items run: the QP's
//!   agent on the QP's connection (KT-2057);
//! - each Agent step of the sub-workflows its `SubWorkflow` steps start,
//!   transitively, each child once per project (cycle-safe), within the depth cap counted
//!   from the run's own depth, in the project the child will run in (KT-2057).
//!
//! Optional agents — fallback links, `on_failure` steps, shadows whose failure
//! is tolerated — are left to the dispatch rule of `steps::execute_step`.

use std::collections::{HashSet, VecDeque};

use crate::models::{AgentType, AgentsConfig, StepType, Workflow, WorkflowRun, WorkflowStep};
use crate::AppState;

/// One Agent step the run will certainly start.
pub struct PlannedStep {
    /// The step as it will run: hydrated from its Quick Prompt when it loads.
    pub step: WorkflowStep,
    /// Why its Quick Prompt did not load: the step fails at dispatch in these
    /// words, so its connection is not judged here.
    pub unloaded: Option<String>,
    /// A `BatchQuickPrompt` step, read as the step its items run.
    pub batch: bool,
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

    /// The agent the step, or its `multi_agent_review` reviewer (KT-2057),
    /// needs and cannot launch here; `None` when both can start.
    pub fn missing_agent(
        &self,
        usable: &[AgentType],
        agents_config: &AgentsConfig,
    ) -> Option<String> {
        let step = &self.step;
        if !super::step_agents::step_can_launch(step, usable, agents_config) {
            return Some(self.describe(format!("'{}' needs {:?}", step.name, step.agent)));
        }
        let reviewer = super::steps::reviewer_step_for(step, step.multi_agent_review.as_ref()?);
        (!super::step_agents::step_can_launch(&reviewer, usable, agents_config)).then(|| {
            self.describe(format!(
                "'{}' reviewer needs {:?}",
                step.name, reviewer.agent
            ))
        })
    }
}

/// The depth `run` sits at in its sub-workflow tree, as its own `SubWorkflow`
/// steps read it.
fn run_depth(run: &WorkflowRun) -> usize {
    run.trigger_context
        .as_ref()
        .and_then(|context| context.get("__subwf_depth__"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |depth| usize::try_from(depth).unwrap_or(usize::MAX))
}

/// The Agent steps `run` of `workflow` will certainly start, the workflow's
/// own first, then its sub-workflows' (pinned revisions, KT-1096). A disabled
/// or missing child, or one whose references do not resolve in its project,
/// is left to the step that starts it.
pub async fn planned_agent_steps(
    state: &AppState,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> Vec<PlannedStep> {
    let run_id = run.id.as_str();
    let mut planned = Vec::new();
    // A child is judged once per project it runs in: a multi-project child
    // runs in its parent's project (`child_project_id`).
    let mut seen: HashSet<(String, Option<String>)> =
        HashSet::from([(workflow.id.clone(), workflow.project_id.clone())]);
    let mut queue: VecDeque<(Workflow, Option<String>, usize)> =
        VecDeque::from([(workflow.clone(), None, run_depth(run))]);
    while let Some((current, within, depth)) = queue.pop_front() {
        for step in &current.steps {
            match step.step_type {
                StepType::Agent => {
                    let mut hydrated = step.clone();
                    let unloaded = super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
                        &mut hydrated,
                        &state.db,
                        Some(run_id),
                    )
                    .await
                    .err();
                    planned.push(PlannedStep {
                        step: if unloaded.is_none() {
                            hydrated
                        } else {
                            step.clone()
                        },
                        unloaded,
                        batch: false,
                        within: within.clone(),
                    });
                }
                StepType::BatchQuickPrompt => {
                    let Some(qp_id) = step
                        .batch_quick_prompt_id
                        .as_deref()
                        .filter(|id| !id.is_empty())
                    else {
                        continue;
                    };
                    let loaded =
                        super::batch_step::load_batch_quick_prompt(state, qp_id, run_id).await;
                    planned.push(PlannedStep {
                        step: match &loaded {
                            Ok(qp) => super::batch_step::batch_item_step(step, qp),
                            Err(_) => step.clone(),
                        },
                        unloaded: loaded.err(),
                        batch: true,
                        within: within.clone(),
                    });
                }
                StepType::SubWorkflow if !super::sub_workflow_step::child_depth_exceeds(depth) => {
                    let Some(target) = step
                        .sub_workflow_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    let parent_run = run_id.to_string();
                    let parent_project = current.project_id.clone();
                    let child = state
                        .db
                        .with_read_conn(move |conn| {
                            let Ok(Some(mut child)) =
                                super::run_pins::sub_workflow_for(conn, &parent_run, &target)?
                            else {
                                return Ok(None);
                            };
                            // Read as the child run will read it: in its
                            // project, its references resolved as the run
                            // pinned them (a child run inherits its parent's
                            // pins), else in that project.
                            child.project_id = super::sub_workflow_step::child_project_id(
                                &child,
                                parent_project.as_deref(),
                            );
                            if crate::core::resource_refs::has_structured_references(&child)
                                && !super::run_pins::apply_pinned_structured(
                                    conn,
                                    &parent_run,
                                    &mut child,
                                )?
                            {
                                let project_id = child.project_id.clone();
                                crate::core::resource_refs::resolve_run_structured_references(
                                    conn,
                                    &mut child,
                                    project_id.as_deref(),
                                )?;
                            }
                            Ok(Some(child))
                        })
                        .await;
                    if let Ok(Some(child)) = child {
                        // Each child once in each project it runs in: a cycle,
                        // or a child started twice, is judged a single time.
                        if !child.enabled
                            || !seen.insert((child.id.clone(), child.project_id.clone()))
                        {
                            continue;
                        }
                        let here = format!("sub-workflow `{}` (step '{}')", child.name, step.name);
                        let path = match &within {
                            None => here,
                            Some(parent) => format!("{parent} › {here}"),
                        };
                        queue.push_back((child, Some(path), depth.saturating_add(1)));
                    }
                }
                _ => {}
            }
        }
    }
    planned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentSettings, MultiAgentReviewConfig};

    fn planned(agent: AgentType, reviewer: Option<AgentType>) -> PlannedStep {
        let step = WorkflowStep {
            name: "auteur".into(),
            step_type: StepType::Agent,
            agent,
            agent_settings: Some(AgentSettings::default()),
            multi_agent_review: reviewer.map(|reviewer_agent| MultiAgentReviewConfig {
                reviewer_agent,
                reviewer_tier: None,
                debate_prompt: "Review it".into(),
                max_rounds: Some(1),
            }),
            ..WorkflowStep::default()
        };
        PlannedStep {
            step,
            unloaded: None,
            batch: false,
            within: None,
        }
    }

    // KT-2057 — a reviewer that cannot launch refuses the run as its author would.
    #[test]
    fn a_reviewer_that_cannot_launch_is_missing_like_its_author() {
        let agents = crate::core::config::default_config().agents;
        let usable = [AgentType::ClaudeCode];
        assert_eq!(
            planned(AgentType::ClaudeCode, None).missing_agent(&usable, &agents),
            None
        );
        assert_eq!(
            planned(AgentType::ClaudeCode, Some(AgentType::ClaudeCode))
                .missing_agent(&usable, &agents),
            None
        );
        assert_eq!(
            planned(AgentType::ClaudeCode, Some(AgentType::Codex)).missing_agent(&usable, &agents),
            Some("'auteur' reviewer needs Codex".into())
        );
        let mut child = planned(AgentType::Codex, Some(AgentType::ClaudeCode));
        child.within = Some("sub-workflow `enfant` (step 'lancer')".into());
        assert_eq!(
            child.missing_agent(&usable, &agents),
            Some("sub-workflow `enfant` (step 'lancer'): 'auteur' needs Codex".into())
        );
    }
}
