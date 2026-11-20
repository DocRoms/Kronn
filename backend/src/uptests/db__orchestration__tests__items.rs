//! Tests of `crate::db::orchestration` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::room_lang::RoomLang;
use crate::db::agent_dispatch::agent_type_from_db;
use crate::db::execution_activity::parse_dt;
use crate::db::execution_activity::{
    default_activity_timeout_secs, default_review_timeout_secs, discussion_has_cli_worker,
    execution_dod_sentence, note_dispatch_activity, note_room_activity, touch_execution_activity,
    DEFAULT_ACTIVITY_TIMEOUT_SECS, DEFAULT_REVIEW_TIMEOUT_SECS,
};
use crate::db::execution_activity::{
    effective_activity_secs, effective_review_secs, timeout_secs_from_env,
};
use crate::db::orchestration::*;
use crate::db::run_state;
use crate::models::{
    AgentType, BlockedReasonCode, CampaignTaskCandidate, CampaignTaskReason,
    CampaignWorkerSelection, CancellationCleanupPolicy, DiscussionMessage, ExecutionRecoveryAction,
    ExecutionTimeoutKind, IntegrationStrategy, LaunchOutcome, LaunchSingleTaskInput,
    MessageChannel, MessageRole, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor,
    OrchestrationControlState, OrchestrationResiliencePolicy, OrchestrationRun,
    OrchestrationRunInput, OrchestrationRunKind, OrchestrationRunStatus, PlanningActorKind,
    PlanningPlacement, PlanningTaskStatus, PrincipalAttention, ReviewVerdict, TaskExecution,
    TaskExecutionEvent, TaskExecutionLineage, TaskExecutionRecovery, TaskExecutionStatus,
    TaskExecutionValidationRun, ValidationSpec,
};
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::str::FromStr;
use uuid::Uuid;
#[path = "../db/orchestration_tests_up.rs"]
mod tests;
