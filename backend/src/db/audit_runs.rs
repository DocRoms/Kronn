//! CRUD for the `audit_runs` table (migration 050, 0.8.2).
//!
//! Used by:
//!   - the audit pipeline to insert at start + update at end;
//!   - the API to read latest N runs for a project (powers the
//!     health-badge sparkline + the audit-history doc generator);
//!   - the cluster detector to read previous recommendations.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::AuditRun;

/// Insert a new audit run with `status = Running` and zeroed counts.
/// Returns the row id for later update calls.
pub fn insert_running(
    conn: &Connection,
    id: &str,
    project_id: &str,
    kind: &str,
    agent_type: &str,
    started_at: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_runs (id, project_id, kind, agent_type, started_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, 'Running')",
        params![id, project_id, kind, agent_type, started_at.to_rfc3339()],
    )?;
    Ok(())
}

/// Final update for a completed run. Computes the duration from the
/// stored `started_at` so callers don't have to round-trip the value.
#[allow(clippy::too_many_arguments)]
pub fn complete(
    conn: &Connection,
    id: &str,
    ended_at: DateTime<Utc>,
    status: &str,
    td_critical: u32,
    td_high: u32,
    td_medium: u32,
    td_low: u32,
    td_resolved_since_last: u32,
    td_new_since_last: u32,
    td_carried_over: u32,
    health_score: u8,
    report_path: Option<&str>,
    recommendations_json: Option<&str>,
) -> Result<()> {
    let td_total = td_critical + td_high + td_medium + td_low;
    let ended_str = ended_at.to_rfc3339();

    // Pull started_at to compute the duration in the same statement,
    // avoiding a separate round-trip.
    let started_at: String = conn.query_row(
        "SELECT started_at FROM audit_runs WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let started: DateTime<Utc> = DateTime::parse_from_rfc3339(&started_at)?.with_timezone(&Utc);
    let duration_ms: i64 = (ended_at - started).num_milliseconds().max(0);

    conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = ?3,
            status = ?4,
            td_critical = ?5,
            td_high = ?6,
            td_medium = ?7,
            td_low = ?8,
            td_total = ?9,
            td_resolved_since_last = ?10,
            td_new_since_last = ?11,
            td_carried_over = ?12,
            health_score = ?13,
            report_path = ?14,
            recommendations_json = ?15
         WHERE id = ?1",
        params![
            id,
            ended_str,
            duration_ms as u32,
            status,
            td_critical,
            td_high,
            td_medium,
            td_low,
            td_total,
            td_resolved_since_last,
            td_new_since_last,
            td_carried_over,
            health_score,
            report_path,
            recommendations_json,
        ],
    )?;
    Ok(())
}

/// Mark a running audit as Failed without computing counts (called when
/// the pipeline aborts early). Idempotent on already-terminal rows.
pub fn mark_failed(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    // We don't have a `reason` column; we encode it in the report_path
    // field with a `failure:` prefix for the (rare) inspection.
    conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Failed',
            report_path = COALESCE(report_path, ?3)
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339(), format!("failure: {reason}")],
    )?;
    Ok(())
}

/// Mark as Cancelled (Ctrl+C / cancel button). Same shape as `mark_failed`.
pub fn mark_cancelled(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Cancelled'
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Latest N runs for a project, most recent first. Used by the health
/// badge to render the sparkline + delta chip.
pub fn list_recent(conn: &Connection, project_id: &str, limit: u32) -> Result<Vec<AuditRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, kind, agent_type, started_at, ended_at, duration_ms, status,
                td_critical, td_high, td_medium, td_low, td_total,
                td_resolved_since_last, td_new_since_last, td_carried_over,
                health_score, report_path, recommendations_json
         FROM audit_runs
         WHERE project_id = ?1
         ORDER BY started_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project_id, limit], row_to_audit_run)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// The most recent COMPLETED audit run, used to bootstrap the health
/// badge in places that only need the latest snapshot (project list
/// page). Returns None when no run has finished yet.
pub fn latest_completed(conn: &Connection, project_id: &str) -> Result<Option<AuditRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, kind, agent_type, started_at, ended_at, duration_ms, status,
                td_critical, td_high, td_medium, td_low, td_total,
                td_resolved_since_last, td_new_since_last, td_carried_over,
                health_score, report_path, recommendations_json
         FROM audit_runs
         WHERE project_id = ?1 AND status = 'Completed'
         ORDER BY ended_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![project_id], row_to_audit_run)?;
    if let Some(r) = rows.next() {
        Ok(Some(r?))
    } else {
        Ok(None)
    }
}

fn row_to_audit_run(row: &rusqlite::Row) -> rusqlite::Result<AuditRun> {
    let started_str: String = row.get(4)?;
    let ended_str: Option<String> = row.get(5)?;
    let duration_ms: Option<i64> = row.get(6)?;
    Ok(AuditRun {
        id: row.get(0)?,
        project_id: row.get(1)?,
        kind: row.get(2)?,
        agent_type: row.get(3)?,
        started_at: DateTime::parse_from_rfc3339(&started_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        ended_at: ended_str.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }),
        duration_ms: duration_ms.map(|d| d.max(0) as u32),
        status: row.get(7)?,
        td_critical: row.get::<_, i64>(8)? as u32,
        td_high: row.get::<_, i64>(9)? as u32,
        td_medium: row.get::<_, i64>(10)? as u32,
        td_low: row.get::<_, i64>(11)? as u32,
        td_total: row.get::<_, i64>(12)? as u32,
        td_resolved_since_last: row.get::<_, i64>(13)? as u32,
        td_new_since_last: row.get::<_, i64>(14)? as u32,
        td_carried_over: row.get::<_, i64>(15)? as u32,
        health_score: row.get::<_, Option<i64>>(16)?.map(|v| v.clamp(0, 100) as u8),
        report_path: row.get(17)?,
        recommendations_json: row.get(18)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Project FK is checked at row insert time, so spin up the bare
        // minimum tables.
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT, path TEXT);
             CREATE TABLE audit_runs (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                kind TEXT NOT NULL DEFAULT 'Full',
                agent_type TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                duration_ms INTEGER,
                status TEXT NOT NULL DEFAULT 'Running',
                td_critical INTEGER NOT NULL DEFAULT 0,
                td_high INTEGER NOT NULL DEFAULT 0,
                td_medium INTEGER NOT NULL DEFAULT 0,
                td_low INTEGER NOT NULL DEFAULT 0,
                td_total INTEGER NOT NULL DEFAULT 0,
                td_resolved_since_last INTEGER NOT NULL DEFAULT 0,
                td_new_since_last INTEGER NOT NULL DEFAULT 0,
                td_carried_over INTEGER NOT NULL DEFAULT 0,
                health_score INTEGER,
                report_path TEXT,
                recommendations_json TEXT
            );
            INSERT INTO projects (id, name, path) VALUES ('p1', 'Test', '/tmp/test');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn insert_and_complete_round_trip() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "run-1", "p1", "Full", "ClaudeCode", start).unwrap();
        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "Running");
        assert!(runs[0].ended_at.is_none());
        assert_eq!(runs[0].td_total, 0);

        let end = start + chrono::Duration::seconds(120);
        complete(
            &conn, "run-1", end, "Completed",
            1, 6, 6, 5,    // 1C/6H/6M/5L → DOCROMS_WEB calibration
            3, 8, 7,       // resolved / new / carried
            53,            // health score
            Some("docs/tech-debt/_reconciliation-2026-05-13.md"),
            Some("[{\"kind\":\"Security\",\"reason\":\"3 findings touch secrets\",\"cluster_size\":3}]"),
        )
        .unwrap();

        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "Completed");
        assert_eq!(runs[0].td_total, 18);
        assert_eq!(runs[0].health_score, Some(53));
        // Duration ~120s = 120000 ms, allow a small clock-skew margin.
        let dur = runs[0].duration_ms.unwrap();
        assert!((119_500..=120_500).contains(&dur), "got duration_ms={dur}");
        assert_eq!(
            runs[0].report_path.as_deref(),
            Some("docs/tech-debt/_reconciliation-2026-05-13.md")
        );
        assert!(runs[0].recommendations_json.is_some());
    }

    #[test]
    fn latest_completed_skips_running_rows() {
        let conn = fresh_conn();
        // First run completes.
        let t1 = Utc::now();
        insert_running(&conn, "r1", "p1", "Full", "Codex", t1).unwrap();
        complete(&conn, "r1", t1 + chrono::Duration::seconds(60), "Completed",
                 0, 1, 2, 3, 0, 6, 0, 84, None, None).unwrap();
        // Second run is still Running.
        let t2 = Utc::now();
        insert_running(&conn, "r2", "p1", "Full", "Kiro", t2).unwrap();

        let latest = latest_completed(&conn, "p1").unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.id, "r1", "must be the completed one, not the running one");
        assert_eq!(latest.health_score, Some(84));
    }

    #[test]
    fn mark_failed_is_idempotent_on_terminal() {
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "r1", "p1", "Full", "Codex", start).unwrap();
        // First completion sets status=Completed.
        complete(&conn, "r1", start + chrono::Duration::seconds(30), "Completed",
                 0, 0, 0, 0, 0, 0, 0, 100, None, None).unwrap();
        // mark_failed must NOT downgrade an already-completed run.
        mark_failed(&conn, "r1", "spurious").unwrap();
        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(runs[0].status, "Completed", "terminal row must not be overwritten");
    }

    #[test]
    fn list_recent_returns_newest_first() {
        let conn = fresh_conn();
        let t0 = Utc::now() - chrono::Duration::hours(2);
        let t1 = Utc::now() - chrono::Duration::hours(1);
        let t2 = Utc::now();
        insert_running(&conn, "old", "p1", "Full", "Codex", t0).unwrap();
        insert_running(&conn, "mid", "p1", "Security", "Codex", t1).unwrap();
        insert_running(&conn, "new", "p1", "Docker", "ClaudeCode", t2).unwrap();
        let runs = list_recent(&conn, "p1", 5).unwrap();
        assert_eq!(runs[0].id, "new");
        assert_eq!(runs[1].id, "mid");
        assert_eq!(runs[2].id, "old");
    }
}
