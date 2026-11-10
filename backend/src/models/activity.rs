// The attention center's row model (KT-1658, phase 1 of KT-334).
//
// One generic row says what is happening in Kronn whatever produced it: a
// `type` (what kind of thing), a `state`, a `source` (the table or tracker it
// is derived from) and the few facts every row shares — title, elapsed
// reference, last sign of life, deep link, actions. The panel renders those
// facts only, so a row of a type it has never seen still shows up. Nothing here
// is stored: every row is derived from the state it describes.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a row sits in the panel. Phase 1 fills `Running` only; the other two
/// are the KT-334 compartments the next phase fills, reserved so adding them
/// reworks neither the endpoint nor the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ActivityCompartment {
    /// Something is working or waiting on its own: nothing is asked of the owner.
    Running,
    /// "Attend une décision" — reserved (KT-334).
    NeedsDecision,
    /// "Terminé non-vu" — reserved (KT-334).
    UnseenDone,
}

/// What a `Running`-compartment row is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Running,
    /// Admitted but not started: a worker still being provisioned, a run not picked up.
    Queued,
    /// Parked until something outside Kronn changes (a provider quota); never "running".
    Paused,
}

/// The kinds of place a row's deep link can open. Resolved to an address by the
/// UI's route table, so a path is never spelled in the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLinkTarget {
    Discussion,
    /// `id` is the workflow, `run_id` the run to reveal.
    Workflow,
    Project,
    PlanningTask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityLink {
    pub target: ActivityLinkTarget,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<String>,
}

/// How an action reads on its button. The label follows this, never the row's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ActivityActionKind {
    Stop,
    Cancel,
    ResumeNow,
}

/// An existing action a row offers. `id` names the UI handler that performs it
/// with `args` (the same call the page's own button makes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityAction {
    pub id: String,
    pub kind: ActivityActionKind,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

/// One thing in the attention center.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityRow {
    /// `<source>:<native id>`: stable across polls, unique across sources.
    pub id: String,
    /// What the row is (`agent_turn`, `workflow_run`, `audit`, `task_worker`, …).
    /// An open set: the panel shows a type it does not know without special-casing.
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub row_type: String,
    pub compartment: ActivityCompartment,
    pub state: ActivityState,
    /// The state the row is derived from (`workflow_runs`, `agent_dispatch_jobs`,
    /// `task_executions`, `audit_tracker`).
    pub source: String,
    pub title: String,
    /// A second line of context that is data, not prose (project, step, progress).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    /// The project it belongs to, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_name: Option<String>,
    /// Elapsed time is `now - started_at`, counted by the UI.
    pub started_at: DateTime<Utc>,
    /// The last sign of life Kronn has for the row. `None` is "no continuous
    /// signal" and is never read as a stall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_activity_at: Option<DateTime<Utc>>,
    /// A running row whose last sign of life is older than the overview's
    /// `stall_after_secs`. A hint to look, not a verdict: the row may be thinking.
    pub stalled: bool,
    /// When a `Paused` row resumes on its own; `None` while it waits for a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wake_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub link: Option<ActivityLink>,
    pub actions: Vec<ActivityAction>,
}

/// `GET /api/activity`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityOverview {
    pub rows: Vec<ActivityRow>,
    /// Inactivity after which a running row is flagged `stalled`.
    pub stall_after_secs: u32,
    pub generated_at: DateTime<Utc>,
    /// Sources that could not be read this time: the list is partial, and says so.
    pub unavailable_sources: Vec<String>,
}
