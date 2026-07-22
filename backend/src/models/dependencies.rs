use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub enum DependencyCheckStatus {
    UpToDate,
    UpdatesAvailable,
    Unsupported,
    Unavailable,
    Error,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DependencyUpdatePackage {
    pub name: String,
    pub current: String,
    pub latest: String,
    pub major: bool,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DependencyManagerUpdate {
    pub manager: String,
    pub manifest: String,
    pub status: DependencyCheckStatus,
    pub outdated: u32,
    pub major: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<DependencyUpdatePackage>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DependencyUpdateSummary {
    pub managers: Vec<DependencyManagerUpdate>,
    pub total_outdated: u32,
    pub total_major: u32,
    pub checked_at: DateTime<Utc>,
    pub cached: bool,
}
