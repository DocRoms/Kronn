use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub enum DependencyCheckStatus {
    UpToDate,
    UpdatesAvailable,
    Unsupported,
    Unavailable,
    Error,
    TimedOut,
    /// A JS manifest whose dependencies are not installed: npm cannot know
    /// the installed versions, so "nothing outdated" would be a guess.
    NotInstalled,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DependencyUpdatePackage {
    pub name: String,
    pub current: String,
    pub latest: String,
    pub major: bool,
    /// The package is declared but not installed (no `node_modules`): `current`
    /// is empty because no installed version exists to report, and the row
    /// only proves the declared range cannot reach `latest`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub declared_only: bool,
    /// The lockfile `current` was read from when there is no installed tree
    /// (KT-1325), e.g. `package-lock.json`. `None`: `current` is what the
    /// package manager reports as installed (or the lock it was run against).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DependencyManagerUpdate {
    pub manager: String,
    pub manifest: String,
    pub status: DependencyCheckStatus,
    pub outdated: u32,
    pub major: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<DependencyUpdatePackage>,
    /// Why the manifest could not be checked (missing binary, network, parse
    /// error, timeout…). Set for every status that is not a verdict (KT-1325).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct DependencyUpdateSummary {
    pub managers: Vec<DependencyManagerUpdate>,
    pub total_outdated: u32,
    pub total_major: u32,
    pub checked_at: DateTime<Utc>,
    pub cached: bool,
    /// `None` means manual checks only; otherwise the persisted result is
    /// refreshed opportunistically when the project overview is opened after
    /// this many days. No package update command is ever executed.
    pub monitoring_interval_days: Option<u16>,
    pub next_check_at: Option<DateTime<Utc>>,
}
