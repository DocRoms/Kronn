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
}

impl PlanningActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
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
    pub changes: serde_json::Value,
    pub source_message_id: Option<String>,
    pub created_at: DateTime<Utc>,
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
    pub events: Vec<PlanningTaskEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningDiscussionRelation {
    pub placement: PlanningPlacement,
    pub is_primary: bool,
    pub position: i64,
    pub task: PlanningTaskSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionPlan {
    pub discussion_id: String,
    pub primary_objective: Option<PlanningTaskSummary>,
    pub active: Vec<PlanningDiscussionRelation>,
    pub later: Vec<PlanningDiscussionRelation>,
    pub completed_active: u32,
    pub total_active: u32,
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
