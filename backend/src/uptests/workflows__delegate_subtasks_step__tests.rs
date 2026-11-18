//! Tests of `crate::workflows::delegate_subtasks_step` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::orchestration::{
    decide_review_as_delegation_step, provision_campaign_task_execution, run_integration_waited_on,
    CampaignProvisionInput, IntegrationOutcome, ReviewOutcome,
};
use crate::core::worktree;
use crate::db::Database;
use crate::models::{
    AgentType, CampaignWorkerSelection, ConditionAction, DelegateSubtasksConfig, DelegateWorker,
    Discussion, DiscussionMessage, MessageChannel, MessageRole, MessageTarget,
    OrchestrationControlState, OrchestrationRun, OrchestrationRunKind, PlanningActor,
    PlanningActorKind, PlanningTaskStatus, QuotaWait, RunStatus, StepResult, SummaryStrategy,
    TaskExecution, TaskExecutionStatus, WorkflowStep,
};
use crate::workflows::delegate_subtasks_step::*;
use crate::workflows::runner::SharedBudget;
use crate::workflows::steps::StepOutcome;
use crate::workflows::template::TemplateContext;
use crate::AppState;
use anyhow::Context;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
#[path = "../workflows/delegate_subtasks_step_tests.rs"]
pub(crate) mod tests;
