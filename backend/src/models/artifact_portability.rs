//! Versioned Artifact bundles. Runtime histories and credentials are not part
//! of the portable definition; retained dataset observations are explicitly included.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{LivePageDatasetKind, QuickApi, QuickExec, QuickPrompt, Workflow};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactBundlePoint {
    pub observed_at: DateTime<Utc>,
    #[ts(type = "any")]
    pub payload: serde_json::Value,
    pub dedupe_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactBundleDataset {
    pub name: String,
    pub kind: LivePageDatasetKind,
    /// Distinguishes a dataset that has never been populated from a JSON null.
    pub has_current: bool,
    #[ts(type = "any")]
    pub current: serde_json::Value,
    #[ts(type = "any")]
    pub schema: Option<serde_json::Value>,
    pub max_points: u32,
    pub max_age_days: Option<u32>,
    pub updated_at: DateTime<Utc>,
    pub points: Vec<ArtifactBundlePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactBundlePage {
    pub id: String,
    pub title: String,
    pub slug: String,
    pub html: String,
    pub created_by_agent: Option<String>,
    pub datasets: Vec<ArtifactBundleDataset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactBundle {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub artifact: ArtifactBundlePage,
    #[serde(default)]
    pub referenced_artifacts: Vec<ArtifactBundlePage>,
    #[serde(default)]
    pub referenced_workflows: Vec<Workflow>,
    #[serde(default)]
    pub referenced_quick_prompts: Vec<QuickPrompt>,
    #[serde(default)]
    pub referenced_quick_apis: Vec<QuickApi>,
    #[serde(default)]
    pub referenced_quick_execs: Vec<QuickExec>,
    /// Literal credentials replaced before export; locations only, never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_fields: Vec<crate::core::export_secrets::RedactedField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ArtifactResourceKind {
    Artifact,
    Workflow,
    QuickPrompt,
    QuickApi,
    QuickExec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ArtifactImportAction {
    Create,
    Reuse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactImportChoice {
    pub kind: ArtifactResourceKind,
    pub source_id: String,
    pub action: ArtifactImportAction,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactImportRequest {
    pub content: String,
    pub project_id: Option<String>,
    #[serde(default)]
    pub choices: Vec<ArtifactImportChoice>,
    /// Source identities of new Quick Execs explicitly reviewed by the user.
    #[serde(default)]
    pub approved_quick_exec_ids: Vec<String>,
    /// The preview digest is required at commit; stale decisions are rejected.
    pub preview_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ArtifactImportDisposition {
    Create,
    Reuse,
    Conflict,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ArtifactImportExecReview {
    pub command: String,
    pub args: Vec<String>,
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ArtifactImportApiReview {
    pub method: Option<String>,
    pub endpoint: String,
    pub plugin: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ArtifactImportEntry {
    pub kind: ArtifactResourceKind,
    pub source_id: String,
    pub name: String,
    pub disposition: ArtifactImportDisposition,
    pub existing_id: Option<String>,
    /// UI translation key suffix: missing, identical, changed, retargeted, chosen.
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quick_exec: Option<ArtifactImportExecReview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quick_api: Option<ArtifactImportApiReview>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ArtifactImportPreview {
    pub title: String,
    pub entries: Vec<ArtifactImportEntry>,
    pub issues: Vec<String>,
    pub warnings: Vec<ArtifactImportWarning>,
    pub digest: String,
    pub can_import: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ArtifactImportResult {
    pub artifact: super::LivePage,
    pub entries: Vec<ArtifactImportEntry>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ArtifactImportWarning {
    pub kind: String,
    pub id: String,
}
