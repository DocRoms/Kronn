//! Tests of `crate::api::orchestration` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::orchestration::*;
use crate::core::room_lang::RoomLang;
use crate::core::task_worker_env::{Capability, ToolchainCapabilities};
use crate::core::{scanner, worktree};
use crate::db::orchestration::{CheckpointOutcome, ProvisioningCheckpoint};
use crate::db::planning::StartTaskCheckpoint;
use crate::db::Database;
use crate::models::{
    AgentType, ApiErrorCode, ApiResponse, BlockedReasonCode, DeliveryManifestV1, Discussion,
    DiscussionMessage, ExecutionRecoveryAction, FileChangeKind, ManifestFile, MessageChannel,
    MessageRole, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor, PlanningActor,
    PlanningActorKind, PlanningDodItem, PlanningDodVerification, PlanningTaskStatus,
    ReviewDecisionV1, ReviewFinding, ReviewVerdict, SagaResumeAction, SummaryStrategy,
    TaskExecution, TaskExecutionStatus, TaskWorkerScope, DELIVERY_CONTRACT_VERSION,
};
use crate::AppState;
use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[path = "../api/orchestration_tests_up.rs"]
pub(crate) mod tests;
