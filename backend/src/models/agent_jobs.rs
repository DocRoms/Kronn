use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AgentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum QuickExecStatus {
    /// Exited 0. The ONLY value that means success.
    Passed,
    /// Exited non-zero, or died on a signal.
    Failed,
    TimedOut,
    Cancelled,
    /// Refused before spawning. Distinct from `Failed` so "we did not run this"
    /// is never read as "this ran and found nothing".
    Rejected,
}

impl QuickExecStatus {
    /// Success is exit 0 and nothing else. A timeout, a cancellation and a
    /// rejection all produce no findings, and none of them is a pass.
    pub fn is_success(self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Diagnostic {
    pub path: Option<String>,
    pub line: Option<i64>,
    pub message: String,
}

/// Where the full streams were kept. Bytes are the real size on disk; `truncated`
/// says the process produced more than that.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactRef {
    pub path: String,
    pub bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickExecResult {
    pub status: QuickExecStatus,
    /// `None` when the process died on a signal or never started. Not 0 — an
    /// unknown exit code must not read as a clean one.
    pub exit_code: Option<i32>,
    pub summary: String,
    pub failed_tests: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub artifact: Option<ArtifactRef>,
    pub duration_ms: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    /// Whether `failed_tests` and `diagnostics` can be treated as exhaustive.
    /// False when a stream was truncated or never reached EOF: an empty list
    /// drawn from a partial log is not evidence of no failures.
    pub findings_complete: bool,
}

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

#[derive(Clone, Deserialize, TS)]
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

crate::debug_without_secrets!(StartAgentBackgroundJobRequest { quick_exec_id, reason, dedupe_key, task_execution_id; masked: variables });

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ScheduleAgentWakeRequest {
    pub delay_seconds: u32,
    pub reason: String,
    pub dedupe_key: String,
    pub task_execution_id: Option<String>,
}
