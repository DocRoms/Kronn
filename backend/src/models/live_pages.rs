use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePage {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub slug: String,
    pub current_revision_id: String,
    pub data_revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_published_at: Option<DateTime<Utc>>,
    /// User-pinned / favorite Page — favorites surface first in the library.
    pub pinned: bool,
    /// Archived Pages remain addressable by workflows and can be restored.
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageRevision {
    pub id: String,
    pub page_id: String,
    pub revision: u64,
    pub html: String,
    pub created_by_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LivePageDatasetKind {
    Snapshot,
    TimeSeries,
    Collection,
}

impl LivePageDatasetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::TimeSeries => "time_series",
            Self::Collection => "collection",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageDataset {
    pub id: String,
    pub page_id: String,
    pub name: String,
    pub kind: LivePageDatasetKind,
    #[ts(type = "any")]
    pub current: Option<serde_json::Value>,
    #[ts(type = "any")]
    pub schema: Option<serde_json::Value>,
    pub max_points: u32,
    pub max_age_days: Option<u32>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageDatasetPoint {
    pub id: String,
    pub dataset_id: String,
    pub observed_at: DateTime<Utc>,
    #[ts(type = "any")]
    pub payload: serde_json::Value,
    pub workflow_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageDatasetView {
    #[serde(flatten)]
    pub dataset: LivePageDataset,
    pub points: Vec<LivePageDatasetPoint>,
    /// Compact UTF-8 JSON bytes currently retained for this dataset. This
    /// includes the snapshot/collection value and retained time-series point
    /// payloads, but excludes SQLite row metadata and the optional schema.
    pub data_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageDetail {
    #[serde(flatten)]
    pub page: LivePage,
    pub revision: LivePageRevision,
    pub datasets: Vec<LivePageDatasetView>,
}

/// Live workflow configurations that publish into this Page. A Page is not
/// owned by one workflow: several workflows may feed the same destination.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageWorkflowLink {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub step_names: Vec<String>,
}

/// One successful data refresh recorded in the Page publication ledger.
/// Workflow fields remain optional because Pages may also be published
/// directly and workflow deletion preserves the historical publication.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePagePublication {
    pub id: String,
    pub page_id: String,
    pub data_revision: u64,
    pub workflow_id: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_run_id: Option<String>,
    pub datasets_updated: Vec<String>,
    pub content_changed: bool,
    pub changed_datasets: Vec<String>,
    pub unchanged_datasets: Vec<String>,
    pub points_added: u32,
    pub points_removed: u32,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageDiscussionLink {
    pub discussion_id: String,
    pub title: String,
    pub relation: LivePageDiscussionRelation,
    pub archived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LivePageDiscussionRelation {
    CreatedFrom,
    Attached,
}

impl LivePageDiscussionRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreatedFrom => "created_from",
            Self::Attached => "attached",
        }
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct CreateLivePageDataset {
    pub name: String,
    pub kind: LivePageDatasetKind,
    /// Optional mock/seed value used by Page Studio previews. For a
    /// time-series dataset this may be an array of point payloads.
    #[ts(type = "any")]
    pub initial: Option<serde_json::Value>,
    #[ts(type = "any")]
    pub schema: Option<serde_json::Value>,
    pub max_points: Option<u32>,
    pub max_age_days: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct CreateLivePageRequest {
    pub title: String,
    pub slug: Option<String>,
    pub project_id: Option<String>,
    pub html: String,
    pub created_by_agent: Option<String>,
    /// Optional discussion that originated this Page. Agents set this to the
    /// current room so the artifact remains discoverable from both places.
    pub discussion_id: Option<String>,
    #[serde(default)]
    pub datasets: Vec<CreateLivePageDataset>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct UpdateLivePageRequest {
    pub title: Option<String>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct LinkLivePageDiscussionRequest {
    pub discussion_id: String,
    pub relation: Option<LivePageDiscussionRelation>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct UpdateLivePageHtmlRequest {
    pub html: String,
    pub created_by_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LivePageWriteOperation {
    Replace,
    Append,
    Upsert,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageWrite {
    pub dataset: String,
    pub operation: LivePageWriteOperation,
    #[ts(type = "any")]
    pub value: serde_json::Value,
    pub observed_at: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub key_field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishLivePageRequest {
    pub workflow_id: Option<String>,
    pub workflow_run_id: Option<String>,
    pub writes: Vec<LivePageWrite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishLivePageResult {
    pub page_id: String,
    pub data_revision: u64,
    pub datasets_updated: Vec<String>,
    pub content_changed: bool,
    pub changed_datasets: Vec<String>,
    pub unchanged_datasets: Vec<String>,
    pub points_added: u32,
    pub points_removed: u32,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePagesCapability {
    pub activated: bool,
    pub activated_at: Option<DateTime<Utc>>,
}

/// Which run of the bound workflow the Page should mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LivePageRunSelector {
    /// Most recently started run, regardless of status.
    Latest,
    /// Most recent non-terminal run (Pending / Running / WaitingApproval),
    /// falling back to the latest run when none is active.
    LatestActive,
}

impl LivePageRunSelector {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::LatestActive => "latest_active",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "latest" => Some(Self::Latest),
            "latest_active" => Some(Self::LatestActive),
            _ => None,
        }
    }
}

/// Binds a Page dataset to a workflow so the Page can mirror that workflow's
/// live run state (read) and, from Phase 3, decide its gates (write). The
/// `phase_map` / `meta_map` blobs are interpreted client-side (they describe how
/// to fold a run's `step_results` into the Page's pipeline shape); the backend
/// stores them verbatim and owns the authorization boundary via `workflow_id`
/// and `allowed_gate_steps`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageWorkflowBinding {
    pub id: String,
    pub page_id: String,
    pub workflow_id: String,
    pub dataset: String,
    pub run_selector: LivePageRunSelector,
    #[ts(type = "any")]
    pub phase_map: serde_json::Value,
    #[ts(type = "any")]
    pub meta_map: serde_json::Value,
    /// Gate step names this Page is allowed to decide. Empty = no gate is
    /// decidable from the Page (read-only mirror).
    pub allowed_gate_steps: Vec<String>,
    /// Launch variables the Page is allowed to pass when triggering the bound
    /// workflow (Phase 4). `None` = triggering is not allowed from the Page;
    /// `Some([])` = triggerable with no variables; `Some(list)` = triggerable and
    /// provided variables must be a subset of `list`.
    pub trigger_variable_allowlist: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A Live Page's request to decide the gate its bound run is waiting on. The
/// backend authorizes it against the `(page, dataset)` binding: the run must
/// belong to the bound workflow, be `WaitingApproval`, and its waiting gate step
/// must be listed in the binding's `allowed_gate_steps`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PageGateDecisionRequest {
    pub dataset: String,
    pub run_id: String,
    /// `approve` | `request_changes` | `reject` (case-insensitive).
    pub decision: String,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Create or replace the binding for `(page, dataset)`. Idempotent on that pair.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct UpsertLivePageBindingRequest {
    pub workflow_id: String,
    pub dataset: String,
    pub run_selector: Option<LivePageRunSelector>,
    /// Client-side phase grouping. Optional: a read-only mirror can omit it
    /// (defaults to JSON `null`, folded to an empty phase list downstream).
    #[serde(default)]
    #[ts(type = "any")]
    pub phase_map: serde_json::Value,
    /// Client-side meta resolution spec. Optional (defaults to JSON `null`).
    #[serde(default)]
    #[ts(type = "any")]
    pub meta_map: serde_json::Value,
    #[serde(default)]
    pub allowed_gate_steps: Vec<String>,
    /// Optional trigger authorization (Phase 4). Omitted / `null` leaves the
    /// binding non-triggerable; a (possibly empty) array makes it triggerable and
    /// bounds the launch variables a Page may pass.
    #[serde(default)]
    pub trigger_variable_allowlist: Option<Vec<String>>,
}

/// A Live Page's request to trigger its bound workflow (Phase 4). Authorized
/// against the `(page, dataset)` binding: the binding must carry a
/// `trigger_variable_allowlist` (else triggering is refused) and every provided
/// variable key must be listed in it.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PageTriggerRequest {
    pub dataset: String,
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: std::collections::HashMap<String, String>,
}

/// The id of the run spawned by a Page trigger. The Page doesn't stream the run;
/// its auto-refresh / mirror surfaces the run's progress.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PageTriggerResponse {
    pub run_id: String,
}
