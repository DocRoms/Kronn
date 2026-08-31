use crate::models::{SharedRun, SharedRunKind, SharedRunStatus};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};

fn kind(v: &SharedRunKind) -> &'static str {
    match v {
        SharedRunKind::QuickPrompt => "quick_prompt",
        SharedRunKind::QuickApi => "quick_api",
        SharedRunKind::QuickExec => "quick_exec",
        SharedRunKind::Workflow => "workflow",
    }
}
fn status(v: &SharedRunStatus) -> &'static str {
    match v {
        SharedRunStatus::PreflightFailed => "preflight_failed",
        SharedRunStatus::Queued => "queued",
        SharedRunStatus::Running => "running",
        SharedRunStatus::Success => "success",
        SharedRunStatus::Failed => "failed",
        SharedRunStatus::Cancelled => "cancelled",
        SharedRunStatus::Timeout => "timeout",
    }
}
fn parse_kind(v: String) -> rusqlite::Result<SharedRunKind> {
    match v.as_str() {
        "quick_prompt" => Ok(SharedRunKind::QuickPrompt),
        "quick_api" => Ok(SharedRunKind::QuickApi),
        "quick_exec" => Ok(SharedRunKind::QuickExec),
        "workflow" => Ok(SharedRunKind::Workflow),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, format!("unknown shared run kind: {v}").into())),
    }
}
fn parse_status(v: String) -> rusqlite::Result<SharedRunStatus> {
    match v.as_str() {
        "preflight_failed" => Ok(SharedRunStatus::PreflightFailed),
        "queued" => Ok(SharedRunStatus::Queued),
        "running" => Ok(SharedRunStatus::Running),
        "success" => Ok(SharedRunStatus::Success),
        "failed" => Ok(SharedRunStatus::Failed),
        "cancelled" => Ok(SharedRunStatus::Cancelled),
        "timeout" => Ok(SharedRunStatus::Timeout),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, format!("unknown shared run status: {v}").into())),
    }
}
fn timestamp(r: &Row<'_>, index: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    r.get::<_, Option<String>>(index)?.map(|value| DateTime::parse_from_rfc3339(&value).map(|date| date.with_timezone(&Utc)).map_err(|error| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error)))).transpose()
}
fn row(r: &Row<'_>) -> rusqlite::Result<SharedRun> {
    let result: Option<String> = r.get(9)?;
    Ok(SharedRun {
        id: r.get(0)?,
        kind: parse_kind(r.get(1)?)?,
        source_id: r.get(2)?,
        project_id: r.get(3)?,
        discussion_id: r.get(4)?,
        status: parse_status(r.get(5)?)?,
        started_at: timestamp(r, 6)?,
        finished_at: timestamp(r, 7)?,
        duration_ms: r.get::<_, Option<i64>>(8)?.map(|v| v.max(0) as u64),
        result: result.and_then(|v| serde_json::from_str(&v).ok()),
        diagnostic: r.get(10)?,
        created_at: timestamp(r, 11)?.ok_or_else(|| rusqlite::Error::InvalidColumnType(11, "created_at".into(), rusqlite::types::Type::Null))?,
        updated_at: timestamp(r, 12)?.ok_or_else(|| rusqlite::Error::InvalidColumnType(12, "updated_at".into(), rusqlite::types::Type::Null))?,
    })
}
pub fn upsert(conn: &Connection, run: &SharedRun) -> Result<()> {
    conn.execute("INSERT INTO shared_runs(id,kind,source_id,project_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,discussion_id=excluded.discussion_id,status=excluded.status,started_at=excluded.started_at,finished_at=excluded.finished_at,duration_ms=excluded.duration_ms,result_json=excluded.result_json,diagnostic=excluded.diagnostic,updated_at=excluded.updated_at",params![run.id,kind(&run.kind),run.source_id,run.project_id,run.discussion_id,status(&run.status),run.started_at.map(|v|v.to_rfc3339()),run.finished_at.map(|v|v.to_rfc3339()),run.duration_ms.map(|v|v as i64),run.result.as_ref().map(|v|v.to_string()),run.diagnostic,run.created_at.to_rfc3339(),run.updated_at.to_rfc3339()])?;
    Ok(())
}
pub fn get(conn: &Connection, id: &str) -> Result<Option<SharedRun>> {
    let mut s=conn.prepare("SELECT id,kind,source_id,project_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at FROM shared_runs WHERE id=?1")?;
    Ok(s.query_row([id], row).optional()?)
}
pub fn list(
    conn: &Connection,
    kind_filter: Option<&str>,
    source_id: Option<&str>,
    project_id: Option<&str>,
    discussion_id: Option<&str>,
    limit: u32,
) -> Result<Vec<SharedRun>> {
    let mut statement = conn.prepare(
        "SELECT id,kind,source_id,project_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at
         FROM shared_runs
         WHERE (?1 IS NULL OR kind=?1) AND (?2 IS NULL OR source_id=?2)
           AND (?3 IS NULL OR project_id=?3) AND (?4 IS NULL OR discussion_id=?4)
         ORDER BY created_at DESC LIMIT ?5",
    )?;
    let rows = statement.query_map(params![kind_filter, source_id, project_id, discussion_id, limit.min(200)], row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn sync_workflow(conn: &Connection, run: &crate::models::WorkflowRun) -> Result<()> {
    let project_id: Option<String> = conn.query_row(
        "SELECT project_id FROM workflows WHERE id=?1",
        [&run.workflow_id],
        |row| row.get(0),
    ).optional()?;
    let status = match run.status {
        crate::models::RunStatus::Pending => SharedRunStatus::Queued,
        crate::models::RunStatus::Running | crate::models::RunStatus::WaitingApproval => SharedRunStatus::Running,
        crate::models::RunStatus::Success => SharedRunStatus::Success,
        crate::models::RunStatus::Cancelled => SharedRunStatus::Cancelled,
        crate::models::RunStatus::StoppedByGuard => SharedRunStatus::Timeout,
        crate::models::RunStatus::Partial | crate::models::RunStatus::Failed | crate::models::RunStatus::Interrupted => SharedRunStatus::Failed,
    };
    let now = Utc::now();
    let duration_ms = run.finished_at.map(|finished| (finished - run.started_at).num_milliseconds().max(0) as u64);
    let completed = run.step_results.iter().filter(|step| !matches!(step.status, crate::models::RunStatus::Pending | crate::models::RunStatus::Running | crate::models::RunStatus::WaitingApproval)).count();
    let current = run.step_results.iter().find(|step| matches!(step.status, crate::models::RunStatus::Pending | crate::models::RunStatus::Running | crate::models::RunStatus::WaitingApproval)).map(|step| step.step_name.clone());
    let shared = SharedRun {
        id: run.id.clone(), kind: SharedRunKind::Workflow, source_id: run.workflow_id.clone(), project_id,
        discussion_id: None, status, started_at: Some(run.started_at), finished_at: run.finished_at,
        duration_ms, result: Some(serde_json::json!({"progress":{"completed":completed,"total":run.step_results.len(),"current_label":current},"steps":run.step_results})),
        diagnostic: None, created_at: run.started_at, updated_at: now,
    };
    upsert(conn, &shared)
}
use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_measured_run_without_inventing_progress() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY);")
            .unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql"))
            .unwrap();
        let now = Utc::now();
        let expected = SharedRun {
            id: "run-1".into(),
            kind: SharedRunKind::QuickApi,
            source_id: "qa-1".into(),
            project_id: None,
            discussion_id: None,
            status: SharedRunStatus::Success,
            started_at: Some(now),
            finished_at: Some(now),
            duration_ms: Some(42),
            result: Some(serde_json::json!({"ok":true})),
            diagnostic: None,
            created_at: now,
            updated_at: now,
        };
        upsert(&conn, &expected).unwrap();
        let actual = get(&conn, "run-1").unwrap().unwrap();
        assert_eq!(actual.id, "run-1");
        assert_eq!(actual.duration_ms, Some(42));
        assert_eq!(actual.result, expected.result);
    }

    #[test]
    fn corrupt_values_are_rejected_instead_of_invented() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY); CREATE TABLE projects(id TEXT PRIMARY KEY);").unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql")).unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints=ON; INSERT INTO shared_runs(id,kind,source_id,status,created_at,updated_at) VALUES('bad','mystery','x','unknown','not-a-date','not-a-date');").unwrap();
        assert!(get(&conn, "bad").is_err());
    }
}
