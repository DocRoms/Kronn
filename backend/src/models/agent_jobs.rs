use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::quick_exec::QuickExecResult;

use super::AgentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentResumeJobKind {
    Command,
    Wake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentResumeJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    QuotaExhausted,
    Escalated,
}

impl AgentResumeJobStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentResumeFailureKind {
    CommandFailed,
    BackendRestarted,
    DispatchStalled,
    QuotaExhausted,
    RuntimeUnavailable,
}

/// Redacted, bounded state exposed to agents and the attention UI. The command
/// snapshot and variable values intentionally remain backend-only.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentResumeJobView {
    pub id: String,
    pub discussion_id: String,
    pub target_agent: AgentType,
    pub source_dispatch_job_id: Option<String>,
    pub task_execution_id: Option<String>,
    pub quick_exec_id: Option<String>,
    pub kind: AgentResumeJobKind,
    pub status: AgentResumeJobStatus,
    pub reason: String,
    pub scheduled_at: DateTime<Utc>,
    pub chain_depth: u32,
    pub wake_budget: u32,
    pub watchdog_redispatches: u32,
    pub completion_dispatch_id: Option<String>,
    pub result: Option<QuickExecResult>,
    pub failure_kind: Option<AgentResumeFailureKind>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct StartAgentBackgroundJobRequest {
    /// A user-saved, shell-free Quick Exec. Its definition is snapshotted when
    /// the job is created so a later edit cannot mutate queued work.
    pub quick_exec_id: String,
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: HashMap<String, String>,
    pub reason: String,
    pub dedupe_key: String,
    pub task_execution_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ScheduleAgentWakeRequest {
    pub delay_seconds: u32,
    pub reason: String,
    pub dedupe_key: String,
    pub task_execution_id: Option<String>,
}
