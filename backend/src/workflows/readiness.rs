//! Whether a saved workflow can start (KT-1138): every refusal Kronn can tell
//! before a run, read from the runtime's own validators, across the workflow,
//! its sub-workflows and every rollback chain. Read-only: it never approves,
//! rewrites or clears anything.

use std::collections::HashMap;

use crate::models::{
    StepType, Workflow, WorkflowBlocker, WorkflowBlockerKind, WorkflowReadiness, WorkflowStep,
};

const HUMAN_APPROVAL_ACTION: &str = "Ask a human to review and approve this line in the \
     workflow editor. An agent cannot approve it: never rewrite the line to dodge the approval.";
const SCRIPT_APPROVAL_ACTION: &str = "Ask a human to open the step in the workflow editor and \
     save it, which approves the script's current content. An agent cannot pin a script hash.";

/// The readiness of `root`, resolving sub-workflows through `workflows`
/// (`root` itself need not be in it).
pub fn assess(root: &Workflow, workflows: &HashMap<String, Workflow>) -> WorkflowReadiness {
    let mut walk = Walk {
        workflows,
        root,
        path: Vec::new(),
        checked: Vec::new(),
        blockers: Vec::new(),
    };
    walk.visit(root);
    finish(root, walk.checked, walk.blockers)
}

/// A readiness that says the diagnostic itself failed: never `ready`.
pub fn collection_failure(root: &Workflow, error: &str) -> WorkflowReadiness {
    let blocker = WorkflowBlocker {
        workflow_id: root.id.clone(),
        workflow_name: root.name.clone(),
        step: None,
        on_failure: false,
        kind: WorkflowBlockerKind::CollectionError,
        phase: None,
        reason: None,
        message: format!("The readiness check could not run: {error}"),
        action: "Retry the check (workflow_validate); do not report the workflow as ready.".into(),
        human_only: false,
    };
    finish(root, vec![root.id.clone()], vec![blocker])
}

/// The blocker counts a workflow card shows: (all, human-only).
pub fn counts(readiness: &WorkflowReadiness) -> (u32, u32) {
    (
        readiness.blockers.len() as u32,
        readiness.human_approval_count,
    )
}

struct Walk<'a> {
    workflows: &'a HashMap<String, Workflow>,
    root: &'a Workflow,
    path: Vec<String>,
    checked: Vec<String>,
    blockers: Vec<WorkflowBlocker>,
}

impl Walk<'_> {
    fn lookup(&self, id: &str) -> Option<&Workflow> {
        if id == self.root.id {
            Some(self.root)
        } else {
            self.workflows.get(id)
        }
    }

    fn visit(&mut self, wf: &Workflow) {
        // A child reached twice (diamond) is reported once.
        if self.checked.contains(&wf.id) {
            return;
        }
        self.checked.push(wf.id.clone());
        for (chain, on_failure) in [(&wf.steps, false), (&wf.on_failure, true)] {
            for step in chain {
                self.blockers.extend(step_blockers(wf, step, on_failure));
            }
        }
        self.path.push(wf.id.clone());
        for (chain, on_failure) in [(&wf.steps, false), (&wf.on_failure, true)] {
            for step in chain {
                self.child(wf, step, on_failure);
            }
        }
        self.path.pop();
    }

    fn child(&mut self, wf: &Workflow, step: &WorkflowStep, on_failure: bool) {
        let nested = match step.step_type {
            StepType::SubWorkflow => true,
            StepType::TriggerWorkflow => false,
            _ => return,
        };
        let Some(target) = step
            .sub_workflow_id
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
        else {
            return;
        };
        let blocker = |kind, message: String, action: &str| WorkflowBlocker {
            workflow_id: wf.id.clone(),
            workflow_name: wf.name.clone(),
            step: Some(step.name.clone()),
            on_failure,
            kind,
            phase: None,
            reason: None,
            message,
            action: action.to_string(),
            human_only: false,
        };
        if nested && self.path.iter().any(|ancestor| ancestor == target) {
            let cycle = blocker(
                WorkflowBlockerKind::ChildCycle,
                format!("Sub-workflow cycle: {} → {target}.", self.path.join(" → ")),
                "Point the SubWorkflow step at a workflow that does not call this one back.",
            );
            self.blockers.push(cycle);
            return;
        }
        let Some(child) = self.lookup(target).cloned() else {
            let missing = blocker(
                WorkflowBlockerKind::MissingChild,
                format!("The workflow `{target}` this step calls does not exist."),
                "Point the step at an existing workflow id (workflow_list).",
            );
            self.blockers.push(missing);
            return;
        };
        if !nested {
            // A triggered run is its own chain: its blockers count, but it
            // starts a fresh nesting path; `checked` bounds trigger loops.
            let outer = std::mem::take(&mut self.path);
            self.visit(&child);
            self.path = outer;
            return;
        }
        if self.path.len() >= crate::api::workflows::MAX_SUBWORKFLOW_DEPTH {
            let deep = blocker(
                WorkflowBlockerKind::ChildCycle,
                format!(
                    "Sub-workflows nest deeper than {}: {} → {target}.",
                    crate::api::workflows::MAX_SUBWORKFLOW_DEPTH,
                    self.path.join(" → ")
                ),
                "Flatten the sub-workflow chain.",
            );
            self.blockers.push(deep);
            return;
        }
        self.visit(&child);
    }
}

/// Every known refusal of one step, from the validators the run applies.
fn step_blockers(wf: &Workflow, step: &WorkflowStep, on_failure: bool) -> Vec<WorkflowBlocker> {
    let base = |kind, message: String, action: String, human_only| WorkflowBlocker {
        workflow_id: wf.id.clone(),
        workflow_name: wf.name.clone(),
        step: Some(step.name.clone()),
        on_failure,
        kind,
        phase: None,
        reason: None,
        message,
        action,
        human_only,
    };
    let mut found = Vec::new();
    if let Err(message) = crate::api::workflows::validate_step_required_fields(step) {
        found.push(base(
            WorkflowBlockerKind::MisconfiguredStep,
            message,
            "Complete the step's required configuration (workflow_update).".into(),
            false,
        ));
    }
    if step.step_type == StepType::Exec {
        for (phase, message) in
            crate::workflows::exec_step::static_refusals(step, &wf.exec_allowlist)
        {
            let mut refused = base(
                WorkflowBlockerKind::ValidationError,
                message,
                "Use a bare binary listed in the workflow's `exec_allowlist`, and drop the \
                 irreversible operation (force push, recursive forced rm) from the step."
                    .into(),
                false,
            );
            refused.phase = Some(phase);
            found.push(refused);
        }
        for file in step
            .exec_script_files
            .iter()
            .filter(|f| f.sha256.is_empty())
        {
            let mut pending = base(
                WorkflowBlockerKind::HumanApproval,
                format!("`{}` has no approved hash yet.", file.path),
                SCRIPT_APPROVAL_ACTION.into(),
                true,
            );
            pending.phase = Some("script".into());
            found.push(pending);
        }
    }
    for line in crate::core::inline_code::classify_step(step, on_failure) {
        let approval = line.reason == "unmodelled_program";
        let action = if approval {
            HUMAN_APPROVAL_ACTION.to_string()
        } else if let Some(args) = &line.suggested_args {
            format!(
                "Rewrite `exec_args` as {} (the value then stays data).",
                serde_json::to_string(args).unwrap_or_default()
            )
        } else {
            "Move the value out of the code: pass it as a separate argument or on stdin read \
             as data, never inside inline code or an option position."
                .into()
        };
        let message = line.manual_fix.clone().unwrap_or_else(|| {
            format!(
                "`{}` places {} inside its code or options.",
                line.command,
                if line.placeholder.is_empty() {
                    "a malformed placeholder".to_string()
                } else {
                    format!("`{}`", line.placeholder)
                }
            )
        });
        let mut blocker = base(
            if approval {
                WorkflowBlockerKind::HumanApproval
            } else {
                WorkflowBlockerKind::UnsafeInterpolation
            },
            message,
            action,
            approval,
        );
        blocker.phase = Some(line.phase.clone());
        blocker.reason = Some(line.reason.clone());
        found.push(blocker);
    }
    found
}

fn finish(
    root: &Workflow,
    checked: Vec<String>,
    blockers: Vec<WorkflowBlocker>,
) -> WorkflowReadiness {
    let human = blockers.iter().filter(|b| b.human_only).count() as u32;
    let state = if root.enabled {
        "Saved and enabled"
    } else {
        "Saved and disabled"
    };
    let summary = if blockers.is_empty() {
        format!(
            "READY: no known blocker in {} workflow(s) checked. {state}. This is a pre-run \
             check, not a guarantee: a run can still fail on credentials, changed scripts, \
             external services or agent output, so report the real run result.",
            checked.len()
        )
    } else {
        let fixable = blockers.len() as u32 - human;
        format!(
            "NOT READY: {} blocker(s) across {} workflow(s) checked; {human} need a human \
             approval (an agent cannot give it: tell the user exactly which steps), {fixable} \
             can be fixed by editing the workflow. {state}; neither means a run can start.",
            blockers.len(),
            checked.len()
        )
    };
    WorkflowReadiness {
        workflow_id: root.id.clone(),
        workflow_name: root.name.clone(),
        enabled: root.enabled,
        ready: blockers.is_empty(),
        human_approval_count: human,
        blockers,
        checked_workflow_ids: checked,
        summary,
    }
}

#[cfg(test)]
#[path = "readiness_test.rs"]
mod tests;
