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
fn parse_kind(v: String) -> SharedRunKind {
    match v.as_str() {
        "quick_prompt" => SharedRunKind::QuickPrompt,
        "quick_api" => SharedRunKind::QuickApi,
        "quick_exec" => SharedRunKind::QuickExec,
        _ => SharedRunKind::Workflow,
    }
}
fn parse_status(v: String) -> SharedRunStatus {
    match v.as_str() {
        "preflight_failed" => SharedRunStatus::PreflightFailed,
        "queued" => SharedRunStatus::Queued,
        "running" => SharedRunStatus::Running,
        "success" => SharedRunStatus::Success,
        "cancelled" => SharedRunStatus::Cancelled,
        "timeout" => SharedRunStatus::Timeout,
        _ => SharedRunStatus::Failed,
    }
}
fn row(r: &Row<'_>) -> rusqlite::Result<SharedRun> {
    let result: Option<String> = r.get(8)?;
    Ok(SharedRun {
        id: r.get(0)?,
        kind: parse_kind(r.get(1)?),
        source_id: r.get(2)?,
        discussion_id: r.get(3)?,
        status: parse_status(r.get(4)?),
        started_at: r.get::<_, Option<String>>(5)?.and_then(|v| {
            DateTime::parse_from_rfc3339(&v)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }),
        finished_at: r.get::<_, Option<String>>(6)?.and_then(|v| {
            DateTime::parse_from_rfc3339(&v)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }),
        duration_ms: r.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as u64),
        result: result.and_then(|v| serde_json::from_str(&v).ok()),
        diagnostic: r.get(9)?,
        created_at: DateTime::parse_from_rfc3339(&r.get::<_, String>(10)?)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&r.get::<_, String>(11)?)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}
pub fn upsert(conn: &Connection, run: &SharedRun) -> Result<()> {
    conn.execute("INSERT INTO shared_runs(id,kind,source_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(id) DO UPDATE SET discussion_id=excluded.discussion_id,status=excluded.status,started_at=excluded.started_at,finished_at=excluded.finished_at,duration_ms=excluded.duration_ms,result_json=excluded.result_json,diagnostic=excluded.diagnostic,updated_at=excluded.updated_at",params![run.id,kind(&run.kind),run.source_id,run.discussion_id,status(&run.status),run.started_at.map(|v|v.to_rfc3339()),run.finished_at.map(|v|v.to_rfc3339()),run.duration_ms.map(|v|v as i64),run.result.as_ref().map(|v|v.to_string()),run.diagnostic,run.created_at.to_rfc3339(),run.updated_at.to_rfc3339()])?;
    Ok(())
}
pub fn get(conn: &Connection, id: &str) -> Result<Option<SharedRun>> {
    let mut s=conn.prepare("SELECT id,kind,source_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at FROM shared_runs WHERE id=?1")?;
    Ok(s.query_row([id], row).optional()?)
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
        conn.execute_batch(include_str!("sql/154_shared_runs.sql"))
            .unwrap();
        let now = Utc::now();
        let expected = SharedRun {
            id: "run-1".into(),
            kind: SharedRunKind::QuickApi,
            source_id: "qa-1".into(),
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
}
