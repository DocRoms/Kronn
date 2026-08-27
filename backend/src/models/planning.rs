use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::deserialize_optional_field;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanningTaskStatus {
    #[default]
    Idea,
    Todo,
    InProgress,
    Blocked,
    Done,
    Archived,
}

impl PlanningTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idea => "idea",
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }
}

impl std::str::FromStr for PlanningTaskStatus {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "idea" => Ok(Self::Idea),
            "todo" => Ok(Self::Todo),
            "in_progress" => Ok(Self::InProgress),
            "blocked" => Ok(Self::Blocked),
            "done" => Ok(Self::Done),
            "archived" => Ok(Self::Archived),
            _ => anyhow::bail!("Unknown planning task status: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanningTaskPriority {
    Critical,
    High,
    #[default]
    Normal,
    Low,
}

impl PlanningTaskPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

impl std::str::FromStr for PlanningTaskPriority {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "critical" => Ok(Self::Critical),
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            _ => anyhow::bail!("Unknown planning task priority: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanningPlacement {
    #[default]
    Active,
    Later,
}

impl PlanningPlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Later => "later",
        }
    }
}

impl std::str::FromStr for PlanningPlacement {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "later" => Ok(Self::Later),
            _ => anyhow::bail!("Unknown planning placement: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanningActorKind {
    #[default]
    Human,
    Agent,
    /// 0.11.0 (KT-317) — an autonomous Kronn backend transition (claim,
    /// integrate, reconcile). Distinct from `Agent` so a task change made by the
    /// orchestrator is attributable and cannot be spoofed by a chat message.
    Backend,
    /// 0.11.0 (KT-317) — a system/maintenance transition (boot reconcile).
    System,
}

impl PlanningActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::Backend => "backend",
            Self::System => "system",
        }
    }
}

impl std::str::FromStr for PlanningActorKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "human" => Ok(Self::Human),
            "agent" => Ok(Self::Agent),
            "backend" => Ok(Self::Backend),
            "system" => Ok(Self::System),
            _ => anyhow::bail!("Unknown planning actor kind: {value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct PlanningActor {
    #[serde(default)]
    pub kind: PlanningActorKind,
    #[serde(default)]
    pub id: Option<String>,
    /// Durable joined-CLI source session. Two sessions of the same provider
    /// keep the same `id` but are never conflated in the audit trail.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub source_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningTaskSummary {
    pub id: String,
    pub reference: String,
    pub parent_id: Option<String>,
    pub parent_reference: Option<String>,
    pub parent_title: Option<String>,
    pub title: String,
    pub status: PlanningTaskStatus,
    pub priority: PlanningTaskPriority,
    pub rank: i64,
    pub completed_subtasks: u32,
    pub total_subtasks: u32,
    pub project_ids: Vec<String>,
    pub discussion_ids: Vec<String>,
    pub tags: Vec<String>,
    pub blocker_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningDodItem {
    pub id: String,
    pub sentence: String,
    pub completed: bool,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningTaskLink {
    pub id: String,
    pub label: String,
    pub url: String,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningTaskEvent {
    pub id: String,
    pub action: String,
    pub actor_kind: PlanningActorKind,
    pub actor_id: Option<String>,
    pub actor_session_id: Option<String>,
    pub changes: serde_json::Value,
    pub source_message_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningWorkspaceSummary {
    pub id: String,
    pub disc_id: String,
    pub branch: String,
    pub state: String,
    pub ownership: String,
    pub session_agent_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningTaskChange {
    pub task_id: String,
    pub task_reference: String,
    pub task_title: String,
    #[serde(flatten)]
    pub event: PlanningTaskEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningTaskDetail {
    #[serde(flatten)]
    pub summary: PlanningTaskSummary,
    pub subtasks: Vec<PlanningTaskSummary>,
    pub description: String,
    pub blocked_reason: Option<String>,
    pub definition_of_done: Vec<PlanningDodItem>,
    pub links: Vec<PlanningTaskLink>,
    pub blockers: Vec<PlanningTaskSummary>,
    pub blocking: Vec<PlanningTaskSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<PlanningWorkspaceSummary>,
    pub events: Vec<PlanningTaskEvent>,
}

/// KT-30 — minimal, read-only dependency reference for the plan projection.
/// Just enough to render a blocked task's active blockers (and, later, the
/// dependency neighbourhood) WITHOUT loading each blocker's full detail. Never
/// recursive: a blocker's own blockers are not expanded here.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningDependencySummary {
    pub id: String,
    pub reference: String,
    pub title: String,
    pub status: PlanningTaskStatus,
    pub project_ids: Vec<String>,
    pub discussion_ids: Vec<String>,
}

/// KT-30 — bucketed counts over the ACTIVE relations of a discussion plan,
/// under a strict precedence so every Active task lands in exactly one bucket:
/// `done` > `blocked` > `in_progress` > `ideas` > `ready`. The five Active
/// buckets sum to `DiscussionPlan::total_active`; `later` is the separate Later
/// count.
///
/// `ready` is exactly the `actionable` relations — a `Todo` with no active
/// blocker — so a UI "X ready" label never overcounts. A nascent `Idea` (not
/// yet started, not actionable) has its own `ideas` bucket rather than
/// inflating `ready`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningPlanStats {
    pub ready: u32,
    pub blocked: u32,
    pub in_progress: u32,
    pub ideas: u32,
    pub done: u32,
    pub later: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningDiscussionRelation {
    pub placement: PlanningPlacement,
    pub is_primary: bool,
    pub position: i64,
    pub task: PlanningTaskSummary,
    /// KT-30 — this task's ACTIVE blockers (status not done/archived), loaded in
    /// one batched pass for the whole plan. Empty when nothing blocks it.
    pub active_blockers: Vec<PlanningDependencySummary>,
    /// KT-30 — ready to pick up: an ACTIVE relation whose task is `Todo` with no
    /// active blocker. A Later relation, or one that is done/blocked/in-progress
    /// (or a mere `Idea`), is never `actionable`. The API does NOT pre-select a
    /// "next" set — it delivers plan order + this flag; the UI takes the first N.
    pub actionable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionPlan {
    pub discussion_id: String,
    pub primary_objective: Option<PlanningTaskSummary>,
    pub active: Vec<PlanningDiscussionRelation>,
    pub later: Vec<PlanningDiscussionRelation>,
    /// Retained aliases (== `stats.done` / sum of the five Active buckets) so
    /// existing MCP/UI consumers keep working during the KT-30 split.
    pub completed_active: u32,
    pub total_active: u32,
    /// KT-30 — bucketed Active counts + the Later count.
    pub stats: PlanningPlanStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatePlanningDodItem {
    #[serde(default)]
    pub id: Option<String>,
    pub sentence: String,
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatePlanningTaskLink {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatePlanningTaskRequest {
    pub title: String,
    /// Optional discussion to link atomically as an active plan item.
    #[serde(default)]
    pub discussion_id: Option<String>,
    /// Opaque caller-scoped retry key. The same key and content returns the
    /// existing task; reusing it for different content is a conflict.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: PlanningTaskStatus,
    #[serde(default)]
    pub priority: PlanningTaskPriority,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub definition_of_done: Vec<CreatePlanningDodItem>,
    #[serde(default)]
    pub links: Vec<CreatePlanningTaskLink>,
    #[serde(default)]
    pub actor: PlanningActor,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdatePlanningTaskRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<PlanningTaskStatus>,
    #[serde(default)]
    pub priority: Option<PlanningTaskPriority>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub parent_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub blocked_reason: Option<Option<String>>,
    #[serde(default)]
    pub rank: Option<i64>,
    #[serde(default)]
    pub project_ids: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub definition_of_done: Option<Vec<CreatePlanningDodItem>>,
    #[serde(default)]
    pub links: Option<Vec<CreatePlanningTaskLink>>,
    #[serde(default)]
    pub actor: PlanningActor,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LinkPlanningDiscussionRequest {
    pub discussion_id: String,
    #[serde(default)]
    pub placement: PlanningPlacement,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub actor: PlanningActor,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddPlanningBlockerRequest {
    pub blocker_task_id: String,
    #[serde(default)]
    pub actor: PlanningActor,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemovePlanningBlockerRequest {
    #[serde(default)]
    pub actor: PlanningActor,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdatePlanningDodItemRequest {
    pub completed: bool,
    #[serde(default)]
    pub actor: PlanningActor,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanningTaskListQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub status: Option<PlanningTaskStatus>,
    #[serde(default)]
    pub priority: Option<PlanningTaskPriority>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub discussion_id: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub with_discussion: Option<bool>,
    #[serde(default)]
    pub cursor: Option<i64>,
    #[serde(default = "default_planning_limit")]
    pub limit: u32,
}

fn default_planning_limit() -> u32 {
    50
}

impl Default for PlanningTaskListQuery {
    fn default() -> Self {
        Self {
            search: None,
            status: None,
            priority: None,
            project_id: None,
            discussion_id: None,
            tag: None,
            with_discussion: None,
            cursor: None,
            limit: default_planning_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningTaskListResponse {
    pub items: Vec<PlanningTaskSummary>,
    pub next_cursor: Option<i64>,
}
