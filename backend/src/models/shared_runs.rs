use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SharedRunKind {
    QuickPrompt,
    QuickApi,
    QuickExec,
    Workflow,
    /// Media generation (image or video). ONE kind, not one per modality:
    /// the execution family is identical and only the output differs, so the
    /// modality lives in `result.modality`.
    Media,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SharedRunStatus {
    PreflightFailed,
    Queued,
    Running,
    Success,
    Failed,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickExecDiagnostics {
    /// None when the process did not start, timed out or died on a signal.
    pub exit_code: Option<i32>,
    /// Captured, bounded stderr; None means unavailable, not an empty stream.
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharedRun {
    pub id: String,
    pub kind: SharedRunKind,
    pub source_id: String,
    pub project_id: Option<String>,
    pub discussion_id: Option<String>,
    pub status: SharedRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    #[ts(type = "unknown")]
    pub result: Option<serde_json::Value>,
    pub diagnostic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exec_details: Option<QuickExecDiagnostics>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
