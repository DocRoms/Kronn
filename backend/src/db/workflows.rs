use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::*;

// ─── Helper ─────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_run_status(s: &str) -> RunStatus {
    match s {
        "Pending" => RunStatus::Pending,
        "Running" => RunStatus::Running,
        "Success" => RunStatus::Success,
        "Failed" => RunStatus::Failed,
        "Cancelled" => RunStatus::Cancelled,
        "WaitingApproval" => RunStatus::WaitingApproval,
        _ => RunStatus::Pending,
    }
}

fn run_status_str(s: &RunStatus) -> &'static str {
    match s {
        RunStatus::Pending => "Pending",
        RunStatus::Running => "Running",
        RunStatus::Success => "Success",
        RunStatus::Failed => "Failed",
        RunStatus::Cancelled => "Cancelled",
        RunStatus::WaitingApproval => "WaitingApproval",
    }
}

// ─── Workflows CRUD ─────────────────────────────────────────────────────────

pub fn list_workflows(conn: &Connection) -> Result<Vec<Workflow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, trigger_json, steps_json, actions_json,
                safety_json, workspace_config_json, concurrency_limit, enabled,
                created_at, updated_at
         FROM workflows ORDER BY updated_at DESC"
    )?;

    let workflows = stmt.query_map([], |row| {
        Ok(row_to_workflow(row))
    })?.filter_map(|r| r.ok())
    .collect();

    Ok(workflows)
}

pub fn get_workflow(conn: &Connection, id: &str) -> Result<Option<Workflow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, trigger_json, steps_json, actions_json,
                safety_json, workspace_config_json, concurrency_limit, enabled,
                created_at, updated_at
         FROM workflows WHERE id = ?1"
    )?;

    let wf = stmt.query_row(params![id], |row| {
        Ok(row_to_workflow(row))
    }).ok();

    Ok(wf)
}

pub fn insert_workflow(conn: &Connection, wf: &Workflow) -> Result<()> {
    conn.execute(
        "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
         safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            wf.id,
            wf.name,
            wf.project_id,
            serde_json::to_string(&wf.trigger)?,
            serde_json::to_string(&wf.steps)?,
            serde_json::to_string(&wf.actions)?,
            serde_json::to_string(&wf.safety)?,
            wf.workspace_config.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()),
            wf.concurrency_limit,
            wf.enabled as i32,
            wf.created_at.to_rfc3339(),
            wf.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_workflow(conn: &Connection, wf: &Workflow) -> Result<()> {
    conn.execute(
        "UPDATE workflows SET name = ?2, trigger_json = ?3, steps_json = ?4,
         actions_json = ?5, safety_json = ?6, workspace_config_json = ?7,
         concurrency_limit = ?8, enabled = ?9, updated_at = ?10
         WHERE id = ?1",
        params![
            wf.id,
            wf.name,
            serde_json::to_string(&wf.trigger)?,
            serde_json::to_string(&wf.steps)?,
            serde_json::to_string(&wf.actions)?,
            serde_json::to_string(&wf.safety)?,
            wf.workspace_config.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()),
            wf.concurrency_limit,
            wf.enabled as i32,
            wf.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn delete_workflow(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM workflows WHERE id = ?1", params![id])?;
    Ok(())
}

// ─── Workflow Runs CRUD ─────────────────────────────────────────────────────

pub fn list_runs(conn: &Connection, workflow_id: &str) -> Result<Vec<WorkflowRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, status, trigger_context, step_results_json,
                tokens_used, workspace_path, started_at, finished_at
         FROM workflow_runs WHERE workflow_id = ?1
         ORDER BY started_at DESC"
    )?;

    let runs = stmt.query_map(params![workflow_id], |row| {
        Ok(row_to_run(row))
    })?.filter_map(|r| r.ok())
    .collect();

    Ok(runs)
}

pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<WorkflowRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, status, trigger_context, step_results_json,
                tokens_used, workspace_path, started_at, finished_at
         FROM workflow_runs WHERE id = ?1"
    )?;

    let run = stmt.query_row(params![run_id], |row| {
        Ok(row_to_run(row))
    }).ok();

    Ok(run)
}

pub fn insert_run(conn: &Connection, run: &WorkflowRun) -> Result<()> {
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, status, trigger_context,
         step_results_json, tokens_used, workspace_path, started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            run.id,
            run.workflow_id,
            run_status_str(&run.status),
            run.trigger_context.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()),
            serde_json::to_string(&run.step_results)?,
            run.tokens_used as i64,
            run.workspace_path,
            run.started_at.to_rfc3339(),
            run.finished_at.map(|d| d.to_rfc3339()),
        ],
    )?;
    Ok(())
}

pub fn update_run(conn: &Connection, run: &WorkflowRun) -> Result<()> {
    conn.execute(
        "UPDATE workflow_runs SET status = ?2, step_results_json = ?3,
         tokens_used = ?4, workspace_path = ?5, finished_at = ?6
         WHERE id = ?1",
        params![
            run.id,
            run_status_str(&run.status),
            serde_json::to_string(&run.step_results)?,
            run.tokens_used as i64,
            run.workspace_path,
            run.finished_at.map(|d| d.to_rfc3339()),
        ],
    )?;
    Ok(())
}

/// Delete a single run.
pub fn delete_run(conn: &Connection, run_id: &str) -> Result<()> {
    conn.execute("DELETE FROM workflow_runs WHERE id = ?1", params![run_id])?;
    Ok(())
}

/// Delete all runs for a workflow.
pub fn delete_all_runs(conn: &Connection, workflow_id: &str) -> Result<()> {
    conn.execute("DELETE FROM workflow_runs WHERE workflow_id = ?1", params![workflow_id])?;
    Ok(())
}

/// Get the last run for a workflow (for summaries).
pub fn get_last_run(conn: &Connection, workflow_id: &str) -> Result<Option<WorkflowRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, status, trigger_context, step_results_json,
                tokens_used, workspace_path, started_at, finished_at
         FROM workflow_runs WHERE workflow_id = ?1
         ORDER BY started_at DESC LIMIT 1"
    )?;

    let run = stmt.query_row(params![workflow_id], |row| {
        Ok(row_to_run(row))
    }).ok();

    Ok(run)
}

/// Count active runs for a workflow (for concurrency limiting).
pub fn count_active_runs(conn: &Connection, workflow_id: &str) -> Result<u32> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_runs WHERE workflow_id = ?1 AND status IN ('Pending', 'Running')",
        params![workflow_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

// ─── Tracker reconciliation ─────────────────────────────────────────────────

pub fn is_issue_processed(conn: &Connection, workflow_id: &str, issue_id: &str) -> Result<bool> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_tracker_processed WHERE workflow_id = ?1 AND issue_id = ?2",
        params![workflow_id, issue_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn mark_issue_processed(conn: &Connection, workflow_id: &str, issue_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO workflow_tracker_processed (workflow_id, issue_id, processed_at)
         VALUES (?1, ?2, ?3)",
        params![workflow_id, issue_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

// ─── Row mappers ────────────────────────────────────────────────────────────

fn row_to_workflow(row: &rusqlite::Row) -> Workflow {
    let trigger_str: String = row.get(3).unwrap_or_default();
    let steps_str: String = row.get(4).unwrap_or_default();
    let actions_str: String = row.get(5).unwrap_or_default();
    let safety_str: String = row.get(6).unwrap_or_default();
    let ws_config_str: Option<String> = row.get(7).unwrap_or(None);
    let concurrency: Option<u32> = row.get(8).unwrap_or(None);

    Workflow {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        project_id: row.get(2).unwrap_or(None),
        trigger: serde_json::from_str(&trigger_str).unwrap_or(WorkflowTrigger::Manual),
        steps: serde_json::from_str(&steps_str).unwrap_or_default(),
        actions: serde_json::from_str(&actions_str).unwrap_or_default(),
        safety: serde_json::from_str(&safety_str).unwrap_or(WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        }),
        workspace_config: ws_config_str.and_then(|s| serde_json::from_str(&s).ok()),
        concurrency_limit: concurrency,
        enabled: row.get::<_, i32>(9).unwrap_or(1) != 0,
        created_at: parse_dt(row.get::<_, String>(10).unwrap_or_default()),
        updated_at: parse_dt(row.get::<_, String>(11).unwrap_or_default()),
    }
}

fn row_to_run(row: &rusqlite::Row) -> WorkflowRun {
    let status_str: String = row.get(2).unwrap_or_default();
    let ctx_str: Option<String> = row.get(3).unwrap_or(None);
    let results_str: String = row.get(4).unwrap_or_default();

    WorkflowRun {
        id: row.get(0).unwrap_or_default(),
        workflow_id: row.get(1).unwrap_or_default(),
        status: parse_run_status(&status_str),
        trigger_context: ctx_str.and_then(|s| serde_json::from_str(&s).ok()),
        step_results: serde_json::from_str(&results_str).unwrap_or_default(),
        tokens_used: row.get::<_, i64>(5).unwrap_or(0) as u64,
        workspace_path: row.get(6).unwrap_or(None),
        started_at: parse_dt(row.get::<_, String>(7).unwrap_or_default()),
        finished_at: row.get::<_, Option<String>>(8).unwrap_or(None).map(parse_dt),
    }
}
