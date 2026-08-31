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
pub struct SharedRun {
    pub id: String,
    pub kind: SharedRunKind,
    pub source_id: String,
    pub discussion_id: Option<String>,
    pub status: SharedRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    #[ts(type = "unknown")]
    pub result: Option<serde_json::Value>,
    pub diagnostic: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
