//! `GET /api/activity` — the attention center's aggregation (KT-1658, phase 1
//! of KT-334).
//!
//! One endpoint answers "what is happening in Kronn right now" from the state
//! that already exists: discussion agent turns (`agent_dispatch_jobs`), workflow
//! runs (`workflow_runs`), task-worker executions (`task_executions`) and audits
//! (the audit tracker). Nothing is stored: there is no notification table to
//! keep in step with those, a row exists exactly as long as its source says it
//! does (KT-334's architecture principle).
//!
//! Every source implements [`ActivitySource`]; [`collect_activity`] only folds
//! them. A new kind of row is a new source in [`default_sources`], and the panel
//! needs no change: it renders the generic [`ActivityRow`] facts.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use axum::{extract::State, Json};
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;

use crate::models::{
    ActivityAction, ActivityActionKind, ActivityCompartment, ActivityLink, ActivityLinkTarget,
    ActivityOverview, ActivityRow, ActivityState, ApiResponse, AuditProgress, RunStatus,
};
use crate::AppState;

/// A running row with no sign of life for this long is flagged `stalled`. Long
/// enough that a model thinking through a hard step is not flagged on every
/// poll, short enough that a wedged one is noticed within the quarter hour.
pub const STALL_AFTER_SECS: u32 = 15 * 60;

/// What a source may read: the database, the in-memory trackers, and the clock.
pub struct ActivityContext<'a> {
    pub conn: &'a Connection,
    pub audits: &'a [AuditProgress],
    pub now: DateTime<Utc>,
}

/// One origin of rows. Sources are independent: one that fails is reported in
/// `unavailable_sources` and never blanks the others.
pub trait ActivitySource {
    /// The state it reads (`workflow_runs`, …): the `source` of its rows.
    fn name(&self) -> &'static str;
    fn collect(&self, ctx: &ActivityContext<'_>) -> Result<Vec<ActivityRow>>;
}

/// The sources of the attention center, in the order their rows tie-break.
pub fn default_sources() -> Vec<Box<dyn ActivitySource>> {
    vec![
        Box::new(AgentTurnSource),
        Box::new(WorkflowRunSource),
        Box::new(TaskWorkerSource),
        Box::new(AuditSource),
    ]
}

/// Folds the sources into one overview, oldest first. A running row whose last
/// sign of life is older than [`STALL_AFTER_SECS`] is flagged `stalled` here,
/// once, so every source is judged by the same rule.
pub fn collect_activity(
    ctx: &ActivityContext<'_>,
    sources: &[Box<dyn ActivitySource>],
) -> ActivityOverview {
    let mut rows = Vec::new();
    let mut unavailable_sources = Vec::new();
    for source in sources {
        match source.collect(ctx) {
            Ok(found) => rows.extend(found),
            Err(error) => {
                tracing::warn!(source = source.name(), %error, "activity source unreadable");
                unavailable_sources.push(source.name().to_string());
            }
        }
    }
    for row in &mut rows {
        row.stalled = is_stalled(row, ctx.now);
    }
    // Stable: equal start times keep the order of their sources.
    rows.sort_by_key(|row| row.started_at);
    ActivityOverview {
        rows,
        stall_after_secs: STALL_AFTER_SECS,
        generated_at: ctx.now,
        unavailable_sources,
    }
}

fn is_stalled(row: &ActivityRow, now: DateTime<Utc>) -> bool {
    row.state == ActivityState::Running
        && row
            .last_activity_at
            .is_some_and(|at| now - at > Duration::seconds(i64::from(STALL_AFTER_SECS)))
}

/// A running-compartment row; the optional facts start empty.
#[allow(clippy::too_many_arguments)]
fn running_row(
    source: &'static str,
    native_id: &str,
    row_type: &str,
    state: ActivityState,
    title: String,
    started_at: DateTime<Utc>,
    link: Option<ActivityLink>,
    actions: Vec<ActivityAction>,
) -> ActivityRow {
    ActivityRow {
        id: format!("{source}:{native_id}"),
        row_type: row_type.to_string(),
        compartment: ActivityCompartment::Running,
        state,
        source: source.to_string(),
        title,
        detail: None,
        project_name: None,
        started_at,
        last_activity_at: None,
        stalled: false,
        wake_at: None,
        link,
        actions,
    }
}

fn action(id: &str, kind: ActivityActionKind, args: &[(&str, &str)]) -> ActivityAction {
    ActivityAction {
        id: id.to_string(),
        kind,
        args: args
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn link(target: ActivityLinkTarget, id: &str, run_id: Option<&str>) -> ActivityLink {
    ActivityLink {
        target,
        id: id.to_string(),
        run_id: run_id.map(str::to_string),
    }
}

/// A stored timestamp; one that does not parse is no timestamp rather than "now".
fn parse_stored(raw: Option<String>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn project_names(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, name FROM projects")?;
    let names = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<HashMap<_, _>, _>>()?;
    Ok(names)
}

// ─── Discussion agent turns ─────────────────────────────────────────────────

/// An agent answering in a discussion right now: a running dispatch job. A turn
/// that belongs to a task worker is left to the worker's own row.
pub struct AgentTurnSource;

impl ActivitySource for AgentTurnSource {
    fn name(&self) -> &'static str {
        "agent_dispatch_jobs"
    }

    fn collect(&self, ctx: &ActivityContext<'_>) -> Result<Vec<ActivityRow>> {
        let mut stmt = ctx.conn.prepare(
            "SELECT j.id, j.discussion_id, d.title, p.name,
                    j.claimed_at, j.created_at, j.last_progress_at
               FROM agent_dispatch_jobs j
               JOIN discussions d ON d.id = j.discussion_id
               LEFT JOIN projects p ON p.id = d.project_id
              WHERE j.status = 'Running'
                AND NOT EXISTS (
                    SELECT 1 FROM task_executions e
                     WHERE e.sub_discussion_id = j.discussion_id
                       AND e.status NOT IN ('Done', 'Failed', 'Cancelled'))",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(
                |(job_id, discussion_id, title, project, claimed, created, progress)| {
                    let started_at = parse_stored(claimed)
                        .or_else(|| parse_stored(Some(created)))
                        .unwrap_or(ctx.now);
                    let mut row = running_row(
                        self.name(),
                        &job_id,
                        "agent_turn",
                        ActivityState::Running,
                        title,
                        started_at,
                        Some(link(ActivityLinkTarget::Discussion, &discussion_id, None)),
                        vec![action(
                            "stop_dispatch",
                            ActivityActionKind::Stop,
                            &[("discussion_id", &discussion_id), ("dispatch_id", &job_id)],
                        )],
                    );
                    row.project_name = project;
                    row.last_activity_at = parse_stored(progress);
                    row
                },
            )
            .collect())
    }
}

// ─── Workflow runs ──────────────────────────────────────────────────────────

/// A top-level workflow run that is running, queued, or paused on a provider
/// quota. A run waiting for an approval is a decision (KT-334's next
/// compartment); a sub-workflow child shows through its parent; a batch
/// placeholder shows through the discussions it fans out.
pub struct WorkflowRunSource;

impl ActivitySource for WorkflowRunSource {
    fn name(&self) -> &'static str {
        "workflow_runs"
    }

    fn collect(&self, ctx: &ActivityContext<'_>) -> Result<Vec<ActivityRow>> {
        let runs = crate::db::workflows::list_active_runs(ctx.conn, None)?;
        let projects = project_names(ctx.conn)?;
        Ok(runs
            .into_iter()
            .filter(|run| {
                run.parent_run_id.is_none()
                    && !run
                        .workflow_id
                        .starts_with(crate::db::workflows::BATCH_WORKFLOW_PREFIX)
            })
            .filter_map(|run| {
                let (state, actions) = match run.status {
                    RunStatus::Running => (
                        ActivityState::Running,
                        vec![action(
                            "cancel_workflow_run",
                            ActivityActionKind::Stop,
                            &[("workflow_id", &run.workflow_id), ("run_id", &run.run_id)],
                        )],
                    ),
                    RunStatus::Pending => (
                        ActivityState::Queued,
                        vec![action(
                            "cancel_workflow_run",
                            ActivityActionKind::Stop,
                            &[("workflow_id", &run.workflow_id), ("run_id", &run.run_id)],
                        )],
                    ),
                    RunStatus::WaitingQuota => (
                        ActivityState::Paused,
                        vec![action(
                            "resume_workflow_run",
                            ActivityActionKind::ResumeNow,
                            &[("run_id", &run.run_id)],
                        )],
                    ),
                    _ => return None,
                };
                let mut row = running_row(
                    self.name(),
                    &run.run_id,
                    "workflow_run",
                    state,
                    run.workflow_name.clone(),
                    run.started_at,
                    Some(link(
                        ActivityLinkTarget::Workflow,
                        &run.workflow_id,
                        Some(&run.run_id),
                    )),
                    actions,
                );
                row.project_name = run
                    .project_id
                    .as_ref()
                    .and_then(|id| projects.get(id).cloned());
                row.last_activity_at = run.last_activity_at;
                if let Some(quota) = run.quota {
                    row.wake_at = quota.wake_at;
                    if !quota.step_name.is_empty() {
                        row.detail = Some(quota.step_name);
                    }
                }
                Some(row)
            })
            .collect())
    }
}

// ─── Task-worker executions ─────────────────────────────────────────────────

/// A task worker that is provisioning or working, or an integration being
/// applied. One in review, blocked or escalated waits on a person: that is the
/// next compartment's.
pub struct TaskWorkerSource;

impl ActivitySource for TaskWorkerSource {
    fn name(&self) -> &'static str {
        "task_executions"
    }

    fn collect(&self, ctx: &ActivityContext<'_>) -> Result<Vec<ActivityRow>> {
        let mut stmt = ctx.conn.prepare(
            "SELECT e.id, e.task_id, t.task_number, t.title, e.status, e.created_at,
                    j.last_progress_at, p.name
               FROM task_executions e
               JOIN planning_tasks t ON t.id = e.task_id
               JOIN orchestration_runs r ON r.id = e.orchestration_run_id
               LEFT JOIN agent_dispatch_jobs j ON j.id = e.dispatch_job_id
               LEFT JOIN projects p ON p.id = r.project_id
              WHERE e.status IN ('Pending', 'Provisioning', 'Working',
                                 'Integrating', 'Validating', 'Applying')",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(
                |(id, task_id, number, title, status, created, progress, project)| {
                    let state = match status.as_str() {
                        "Pending" | "Provisioning" => ActivityState::Queued,
                        _ => ActivityState::Running,
                    };
                    let mut row = running_row(
                        self.name(),
                        &id,
                        "task_worker",
                        state,
                        format!("KT-{number} · {title}"),
                        parse_stored(Some(created)).unwrap_or(ctx.now),
                        Some(link(ActivityLinkTarget::PlanningTask, &task_id, None)),
                        vec![action(
                            "cancel_task_execution",
                            ActivityActionKind::Cancel,
                            &[("execution_id", &id)],
                        )],
                    );
                    row.project_name = project;
                    // Only the dispatcher's own progress stamp: a worker with
                    // none reports no continuous signal, which is not a stall.
                    row.last_activity_at = parse_stored(progress);
                    row
                },
            )
            .collect())
    }
}

// ─── Audits ─────────────────────────────────────────────────────────────────

/// An audit in flight, from the audit tracker. The `done` entry the tracker
/// keeps for a moment after the end is not running anything.
pub struct AuditSource;

impl ActivitySource for AuditSource {
    fn name(&self) -> &'static str {
        "audit_tracker"
    }

    fn collect(&self, ctx: &ActivityContext<'_>) -> Result<Vec<ActivityRow>> {
        let projects = project_names(ctx.conn)?;
        Ok(ctx
            .audits
            .iter()
            .filter(|audit| audit.phase != "done")
            .map(|audit| {
                let name = projects.get(&audit.project_id).cloned();
                let mut row = running_row(
                    self.name(),
                    &audit.project_id,
                    "audit",
                    ActivityState::Running,
                    name.clone().unwrap_or_else(|| audit.project_id.clone()),
                    audit.started_at,
                    Some(link(ActivityLinkTarget::Project, &audit.project_id, None)),
                    vec![action(
                        "cancel_audit",
                        ActivityActionKind::Stop,
                        &[("project_id", &audit.project_id)],
                    )],
                );
                row.project_name = name;
                row.detail = Some(match (&audit.current_file, audit.total_steps) {
                    (Some(file), total) if total > 0 => {
                        format!("{}/{} · {file}", audit.step_index, total)
                    }
                    (None, total) if total > 0 => format!("{}/{}", audit.step_index, total),
                    (Some(file), _) => file.clone(),
                    (None, _) => audit.phase.clone(),
                });
                row.last_activity_at = audit
                    .recent_activity
                    .as_ref()
                    .and_then(|recent| recent.entries.first())
                    .map(|entry| entry.at);
                row
            })
            .collect())
    }
}

// ─── Route ──────────────────────────────────────────────────────────────────

/// GET /api/activity
///
/// Everything running in Kronn, derived on read. Operator-only: it is not on
/// the bridge-token route table, so an agent cannot enumerate the instance.
pub async fn overview(State(state): State<AppState>) -> Json<ApiResponse<ActivityOverview>> {
    let audits = match state.audit_tracker.lock() {
        Ok(tracker) => tracker.progress.values().cloned().collect::<Vec<_>>(),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    match state
        .db
        .with_read_conn(move |conn| {
            let ctx = ActivityContext {
                conn,
                audits: &audits,
                now: Utc::now(),
            };
            Ok(collect_activity(&ctx, &default_sources()))
        })
        .await
    {
        Ok(overview) => Json(ApiResponse::ok(overview)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
