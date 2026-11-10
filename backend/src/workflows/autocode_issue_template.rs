//! The "autoCode Issue" workflows, shipped with Kronn (KT-1643).
//!
//! Anyone with Kronn opens the contribution board, clicks an issue and
//! contributes. The workflow therefore cannot be instance data: it is built
//! here and installed on any instance by
//! `POST /api/workflows/templates/autocode-issue`, the same way the
//! feasibility AutoPilot template is.
//!
//! Nothing instance-specific is written into a step:
//!
//! - the Kronn address is `{{run.kronn_url}}`, a built-in of the run;
//! - the Kronn project is the one whose remote is the repository, resolved by
//!   the preflight (`core::contribution`), never a pasted id;
//! - the repository, the target branch (default `main`) and the review rounds
//!   (default 10) are workflow variables, shown on the launch form;
//! - the first step of each workflow is the preflight, which fails with an
//!   actionable message per missing prerequisite;
//! - the publish workflow carries out the publication plan the preflight
//!   returns: straight to the repository for a maintainer, through the
//!   contributor's fork otherwise, the pull request always targeting the
//!   repository.
//!
//! The steps are deterministic scripts (no token spent): `autocode_issue/*.py`.

use crate::core::contribution::{
    DEFAULT_MAX_REVIEW_ROUNDS, DEFAULT_REPOSITORY, DEFAULT_TARGET_BRANCH,
};
use crate::models::{
    AgentType, CreateWorkflowRequest, PromptVariable, PromptVariableControl, StepType,
    WorkflowStep, WorkflowTrigger,
};
use crate::workflows::big_ticket_template::blank_step;
use std::collections::HashMap;

/// What the installer may choose; everything else is fixed by the template.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutocodeIssueParams {
    /// Scope the workflows to a Kronn project. Optional: they find their
    /// project by the repository at run time either way.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// The two workflows the template installs.
pub struct AutocodeIssueTemplate {
    /// Takes the issue, creates the task, room and campaign, launches it.
    pub contribute: CreateWorkflowRequest,
    /// Once the campaign delivered: fork if needed, push, open the pull request.
    pub publish: CreateWorkflowRequest,
}

pub const CONTRIBUTE_NAME: &str = "autoCode Issue - start the campaign (contribution board)";
pub const PUBLISH_NAME: &str = "autoCode Issue - publish the pull request (direct or from a fork)";

const SEP: &str = "\n===SEP===\n";

/// A script of `autocode_issue/`, with the defaults the template shares.
fn script(source: &str) -> String {
    source
        .replace("__DEFAULT_REPO__", DEFAULT_REPOSITORY)
        .replace("__DEFAULT_BRANCH__", DEFAULT_TARGET_BRANCH)
        .replace("__DEFAULT_ROUNDS__", &DEFAULT_MAX_REVIEW_ROUNDS.to_string())
}

fn stdin(parts: &[&str]) -> String {
    parts.join(SEP)
}

fn exec_step(
    name: &str,
    description: &str,
    source: &str,
    input: String,
    secs: u32,
) -> WorkflowStep {
    let mut step = blank_step(name, StepType::Exec, AgentType::ClaudeCode);
    step.description = Some(description.into());
    step.exec_command = Some("python3".into());
    step.exec_args = vec!["-c".into(), script(source)];
    step.exec_stdin = Some(input);
    step.exec_timeout_secs = Some(secs);
    step
}

/// The five values every step reads, in the order the scripts split them.
const KRONN_URL: &str = "{{run.kronn_url}}";
const REPO: &str = "{{repo}}";
const TARGET: &str = "{{targetBranch}}";
const ROUNDS: &str = "{{maxReviewRounds}}";
const ISSUE: &str = "{{issueNumber}}";

fn preflight_step() -> WorkflowStep {
    exec_step(
        "preflight",
        "Checks this machine can contribute: gh installed and signed in with the repo scope, \
         access to the repository (direct or through a fork), the Kronn project that is a \
         clone of it, a local worker. Each failure says what to run to fix it.",
        include_str!("autocode_issue/preflight.py"),
        stdin(&[KRONN_URL, REPO, TARGET, ROUNDS, ISSUE]),
        120,
    )
}

fn text_variable(
    name: &str,
    label: &str,
    placeholder: &str,
    description: &str,
    required: bool,
    pattern: Option<&str>,
) -> PromptVariable {
    PromptVariable {
        name: name.into(),
        label: label.into(),
        placeholder: placeholder.into(),
        description: Some(description.into()),
        required,
        pattern: pattern.map(String::from),
        source: None,
        source_ref: None,
        allow_manual_override: false,
        control: Some(PromptVariableControl::Text),
    }
}

/// The settings every autoCode Issue workflow shows on the board.
fn variables() -> Vec<PromptVariable> {
    vec![
        text_variable(
            "issueNumber",
            "Issue number",
            "",
            "Number of the GitHub issue of the repository to take and code",
            true,
            Some(r"^[0-9]+$"),
        ),
        text_variable(
            "repo",
            "Repository",
            DEFAULT_REPOSITORY,
            &format!(
                "owner/name of the repository the pull request targets (empty = {DEFAULT_REPOSITORY})"
            ),
            false,
            None,
        ),
        text_variable(
            "targetBranch",
            "Target branch",
            DEFAULT_TARGET_BRANCH,
            &format!("Branch the work is cut from and the pull request targets (empty = {DEFAULT_TARGET_BRANCH})"),
            false,
            None,
        ),
        text_variable(
            "maxReviewRounds",
            "Max review rounds",
            &DEFAULT_MAX_REVIEW_ROUNDS.to_string(),
            &format!(
                "Review rounds allowed per task before the campaign asks the human (empty = {DEFAULT_MAX_REVIEW_ROUNDS})"
            ),
            false,
            None,
        ),
    ]
}

fn request(
    name: &str,
    project_id: &Option<String>,
    steps: Vec<WorkflowStep>,
) -> CreateWorkflowRequest {
    CreateWorkflowRequest {
        refs: Default::default(),
        retention: None,
        shadow: None,
        project_scope: None,
        name: name.into(),
        project_id: project_id.clone(),
        trigger: WorkflowTrigger::Manual,
        steps,
        actions: vec![],
        safety: None,
        workspace_config: None,
        // One run at a time: two clicks never start two campaigns at once.
        concurrency_limit: Some(1),
        concurrency_key: None,
        guards: None,
        artifacts: HashMap::new(),
        on_failure: vec![],
        exec_allowlist: vec!["gh".into(), "git".into(), "python3".into()],
        variables: variables(),
        enabled: None,
    }
}

pub fn build_autocode_issue(params: &AutocodeIssueParams) -> AutocodeIssueTemplate {
    let contribute = vec![
        preflight_step(),
        exec_step(
            "take",
            "Deterministic: guard (open, not taken by someone else, not a pull request), then \
             assign the issue to the account of the gh login.",
            include_str!("autocode_issue/take.py"),
            stdin(&[ISSUE, REPO]),
            120,
        ),
        exec_step(
            "orchestrate",
            "100% deterministic: creates (or resumes) the Kronn task, its room and its \
             campaign, names who reviews each delivery, then launches the execution. Plan, \
             arbitration, code, reviews and merge happen in the standard Kronn campaign.",
            include_str!("autocode_issue/orchestrate.py"),
            stdin(&[
                "{{steps.take.data_json}}",
                "{{steps.preflight.data_json}}",
                KRONN_URL,
                REPO,
                TARGET,
                ROUNDS,
            ]),
            300,
        ),
    ];
    let publish = vec![
        preflight_step(),
        exec_step(
            "publish",
            "Carries out the publication plan of the preflight: fork the repository when the \
             contributor cannot push to it, push the issue branch (to the repository, or to \
             the fork), open the pull request against the repository.",
            include_str!("autocode_issue/publish.py"),
            stdin(&[
                "{{steps.preflight.data_json}}",
                KRONN_URL,
                REPO,
                TARGET,
                ISSUE,
            ]),
            300,
        ),
    ];
    AutocodeIssueTemplate {
        contribute: request(CONTRIBUTE_NAME, &params.project_id, contribute),
        publish: request(PUBLISH_NAME, &params.project_id, publish),
    }
}

#[cfg(test)]
#[path = "autocode_issue_template_test.rs"]
mod tests;
