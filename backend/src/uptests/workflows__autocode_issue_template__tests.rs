//! Tests of `crate::workflows::autocode_issue_template` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::contribution::{
    DEFAULT_MAX_REVIEW_ROUNDS, DEFAULT_REPOSITORY, DEFAULT_TARGET_BRANCH, PYTHON_PROGRAM,
    REVIEWER_VARIABLE_FORMS,
};
use crate::models::{
    AgentType, CreateWorkflowRequest, PromptVariable, PromptVariableControl, StepType,
    WorkflowStep, WorkflowTrigger,
};
use crate::workflows::autocode_issue_template::*;
use crate::workflows::big_ticket_template::blank_step;
use std::collections::HashMap;
#[path = "../workflows/autocode_issue_template_test.rs"]
pub(crate) mod tests;
