//! Tests of `crate::db::planning` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::uplink::{ExecutionOf, TaskExecutionLookup};
use crate::db::planning::*;
use crate::models::{
    AddPlanningBlockerRequest, AgentType, AssignPlanningTaskRequest, CreatePlanningDodItem,
    CreatePlanningTaskLink, CreatePlanningTaskRequest, DiscussionPlan,
    LinkPlanningDiscussionRequest, MessageTargetKind, ModelTier, PlanningActor, PlanningActorKind,
    PlanningAssignee, PlanningAssigneeInput, PlanningAssigneeRole, PlanningAssigneeSource,
    PlanningAwaitingMerge, PlanningDependencySummary, PlanningDiscussionRelation, PlanningDodItem,
    PlanningDodVerification, PlanningLiveCheck, PlanningPlacement, PlanningPlanStats,
    PlanningTaskChange, PlanningTaskDetail, PlanningTaskEvent, PlanningTaskLink,
    PlanningTaskListQuery, PlanningTaskListResponse, PlanningTaskPriority, PlanningTaskStatus,
    PlanningTaskSummary, PlanningWorkspaceSummary, TaskExecutionStatus,
    UnassignPlanningTaskRequest, UnlinkPlanningDiscussionRequest, UpdatePlanningDodItemRequest,
    UpdatePlanningTaskRequest,
};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::str::FromStr;
use uuid::Uuid;
#[path = "../db/planning_tests_up.rs"]
mod tests;
