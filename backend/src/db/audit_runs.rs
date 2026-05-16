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

use crate::models::{AuditRun, AuditRunStep};

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
                health_score, report_path, recommendations_json, last_completed_step
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
                health_score, report_path, recommendations_json, last_completed_step
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
        last_completed_step: row.get::<_, i64>(19).unwrap_or(0).max(0) as u32,
    })
}

/// 0.8.3 (#311) — bump `last_completed_step` on every successful
/// `step_done` event so the resume mechanism knows where to pick up
/// if the SSE stream gets interrupted mid-run. `step` is 1-based.
/// Idempotent: if the new value isn't greater than the current one
/// (rare race where two updates land out of order), the existing
/// value wins. No-op on terminal rows.
pub fn update_last_completed_step(conn: &Connection, id: &str, step: u32) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET last_completed_step = ?2
         WHERE id = ?1 AND status = 'Running' AND last_completed_step < ?2",
        params![id, step as i64],
    )?;
    Ok(())
}

/// 0.8.3 (#311) — mark an audit run as `Interrupted`. Different from
/// `Cancelled` (explicit user action) and `Failed` (terminal error
/// the pipeline can't recover from). `Interrupted` means the SSE
/// stream ended before reaching step 10 without an explicit signal,
/// most often a CLI rate-limit, OOM, or network blip. The frontend
/// surfaces these specifically as resumable: "Reprendre Step N/10".
pub fn mark_interrupted(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            status = 'Interrupted',
            report_path = COALESCE(report_path, ?3)
         WHERE id = ?1 AND status = 'Running'",
        params![id, Utc::now().to_rfc3339(), format!("interrupted: {reason}")],
    )?;
    Ok(())
}

/// 0.8.4 (#317 / B1) — reconcile stale `Running` rows that were never
/// terminated. A backend crash, container restart, kill -9, or any
/// other path that interrupts the SSE loop *without* the cancel flag
/// being set will leave a row stuck in `status = 'Running'` forever.
/// On a project that's been audited many times these accumulate and
/// pollute the recap-panel chip strip (and the "active audits" badge,
/// which trusts `status = 'Running'`).
///
/// Strategy: at boot, any run that's been `Running` for more than
/// `stale_after_secs` seconds is marked as `Interrupted` with a
/// `report_path` prefix `stale: ` for forensic clarity.
///
/// Returns the count of reconciled rows so the caller can log it.
///
/// The threshold defaults to 30 minutes (1800s) which is well above
/// the longest realistic Full-audit duration (~25 min on a 50k-line
/// repo). Callers can override for testing.
pub fn reconcile_stale_runs(conn: &Connection, stale_after_secs: i64) -> Result<u64> {
    let now = Utc::now();
    let cutoff = (now - chrono::Duration::seconds(stale_after_secs)).to_rfc3339();
    let now_rfc = now.to_rfc3339();
    let affected = conn.execute(
        "UPDATE audit_runs SET
            status = 'Interrupted',
            ended_at = ?2,
            duration_ms = CAST(((julianday(?2) - julianday(started_at)) * 86400000) AS INTEGER),
            report_path = COALESCE(report_path, 'stale: backend restarted while audit was Running')
         WHERE status = 'Running' AND started_at < ?1",
        params![cutoff, now_rfc],
    )?;
    Ok(affected as u64)
}

/// 0.8.4 (#317 / B1) — admin endpoint companion. Sister of
/// `reconcile_stale_runs` but force-flags ALL `Running` rows
/// regardless of age. Used by the UI cleanup button when the user
/// knows nothing is actually running (e.g. just rebuilt the
/// containers and wants a clean slate). Returns the count.
pub fn reconcile_all_running(conn: &Connection) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let affected = conn.execute(
        "UPDATE audit_runs SET
            status = 'Interrupted',
            ended_at = ?1,
            duration_ms = CAST(((julianday(?1) - julianday(started_at)) * 86400000) AS INTEGER),
            report_path = COALESCE(report_path, 'stale: cleared by user via admin cleanup')
         WHERE status = 'Running'",
        params![now],
    )?;
    Ok(affected as u64)
}

/// 0.8.3 (#311) — fetch the most-recent resumable run for a project,
/// if any. "Resumable" means `status = 'Interrupted'` AND
/// `last_completed_step` is in 1..=9 (no point resuming if step 10
/// finished, and step 0 means nothing was produced so resume = restart).
/// Returns `None` when the project has no resumable run.
pub fn latest_resumable(conn: &Connection, project_id: &str) -> Result<Option<AuditRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, kind, agent_type, started_at, ended_at, duration_ms, status,
                td_critical, td_high, td_medium, td_low, td_total,
                td_resolved_since_last, td_new_since_last, td_carried_over,
                health_score, report_path, recommendations_json, last_completed_step
         FROM audit_runs
         WHERE project_id = ?1
           AND status = 'Interrupted'
           AND last_completed_step BETWEEN 1 AND 9
         ORDER BY started_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![project_id], row_to_audit_run)?;
    if let Some(r) = rows.next() {
        Ok(Some(r?))
    } else {
        Ok(None)
    }
}

// ─── 0.8.4 (#298) audit_run_steps helpers ─────────────────────────────

/// Insert a fresh step row at `step_start`. The metrics fields are
/// filled in by `finalize_step` at `step_done`. Idempotent on
/// `(audit_run_id, step_index)` so a resumed audit (#311) that skips
/// already-completed steps doesn't crash on UNIQUE violation — we
/// just ignore the conflict.
pub fn insert_audit_step_start(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    file_label: &str,
    started_at: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO audit_run_steps (audit_run_id, step_index, file_label, started_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![audit_run_id, step_index as i64, file_label, started_at.to_rfc3339()],
    )?;
    Ok(())
}

/// Finalize a step row at `step_done` (or step_warning). `success`
/// is `false` when `validate_and_repair_step_output` (#292) emitted
/// a warning OR when the CLI exited non-zero. `step_warning` is the
/// reason string (None on success).
#[allow(clippy::too_many_arguments)]
pub fn finalize_audit_step(
    conn: &Connection,
    audit_run_id: &str,
    step_index: u32,
    ended_at: DateTime<Utc>,
    duration_ms: u64,
    step_tokens: u64,
    cumulative_tokens: u64,
    cli_success: bool,
    step_warning: Option<&str>,
    repaired: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE audit_run_steps SET
            ended_at = ?3,
            duration_ms = ?4,
            step_tokens = ?5,
            cumulative_tokens = ?6,
            cli_success = ?7,
            step_warning = ?8,
            step_repaired_from_template = ?9
         WHERE audit_run_id = ?1 AND step_index = ?2",
        params![
            audit_run_id,
            step_index as i64,
            ended_at.to_rfc3339(),
            duration_ms as i64,
            step_tokens as i64,
            cumulative_tokens as i64,
            cli_success as i32,
            step_warning,
            repaired as i32,
        ],
    )?;
    Ok(())
}

/// All steps of an audit run, ordered by step_index. Used by the
/// ProjectCard recap panel.
pub fn list_audit_steps(conn: &Connection, audit_run_id: &str) -> Result<Vec<AuditRunStep>> {
    let mut stmt = conn.prepare(
        "SELECT audit_run_id, step_index, file_label, started_at, ended_at,
                duration_ms, step_tokens, cumulative_tokens, cli_success,
                step_warning, step_repaired_from_template
         FROM audit_run_steps
         WHERE audit_run_id = ?1
         ORDER BY step_index ASC"
    )?;
    let rows = stmt.query_map(params![audit_run_id], |row| {
        let started_str: String = row.get(3)?;
        let ended_str: Option<String> = row.get(4)?;
        Ok(AuditRunStep {
            audit_run_id: row.get(0)?,
            step_index: row.get::<_, i64>(1)? as u32,
            file_label: row.get(2)?,
            started_at: DateTime::parse_from_rfc3339(&started_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            ended_at: ended_str.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            duration_ms: row.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as u64),
            step_tokens: row.get::<_, Option<i64>>(6)?.map(|v| v.max(0) as u64),
            cumulative_tokens: row.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as u64),
            cli_success: row.get::<_, i64>(8)? != 0,
            step_warning: row.get(9)?,
            step_repaired_from_template: row.get::<_, i64>(10)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
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
                recommendations_json TEXT,
                last_completed_step INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE audit_run_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                audit_run_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                file_label TEXT NOT NULL,
                started_at DATETIME NOT NULL,
                ended_at DATETIME,
                duration_ms INTEGER,
                step_tokens INTEGER,
                cumulative_tokens INTEGER,
                cli_success INTEGER NOT NULL DEFAULT 1,
                step_warning TEXT,
                step_repaired_from_template INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (audit_run_id) REFERENCES audit_runs(id) ON DELETE CASCADE
            );
            CREATE UNIQUE INDEX idx_audit_run_steps_run
                ON audit_run_steps(audit_run_id, step_index);
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
    fn update_last_completed_step_bumps_only_forward_on_running_rows() {
        // 0.8.3 (#311) — the bump must be strictly monotonic
        // (out-of-order updates from a glitchy SSE shouldn't rewind
        // progress) AND scoped to Running rows (a terminal status
        // shouldn't accept new step updates).
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "rr-1", "p1", "Full", "ClaudeCode", start).unwrap();
        // Bumps forward.
        update_last_completed_step(&conn, "rr-1", 3).unwrap();
        let after1 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(after1[0].last_completed_step, 3);
        update_last_completed_step(&conn, "rr-1", 5).unwrap();
        let after2 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(after2[0].last_completed_step, 5);
        // Out-of-order: lower value must NOT win.
        update_last_completed_step(&conn, "rr-1", 2).unwrap();
        let after3 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(after3[0].last_completed_step, 5, "monotonic — lower step ignored");
        // Terminal: mark complete then try to bump — must not move.
        complete(&conn, "rr-1", start + chrono::Duration::seconds(30), "Completed",
                 0, 0, 0, 0, 0, 0, 0, 100, None, None).unwrap();
        update_last_completed_step(&conn, "rr-1", 9).unwrap();
        let after4 = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(after4[0].last_completed_step, 5, "terminal rows must not accept new bumps");
    }

    #[test]
    fn mark_interrupted_writes_status_and_preserves_last_completed_step() {
        // 0.8.3 (#311) — interrupted run keeps last_completed_step
        // so the resume mechanism knows where to pick up.
        let conn = fresh_conn();
        let start = Utc::now();
        insert_running(&conn, "ri-1", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "ri-1", 5).unwrap();
        mark_interrupted(&conn, "ri-1", "rate-limit hit").unwrap();

        let runs = list_recent(&conn, "p1", 1).unwrap();
        assert_eq!(runs[0].status, "Interrupted");
        assert_eq!(runs[0].last_completed_step, 5,
            "last_completed_step must survive the mark_interrupted call so resume knows where to restart");
        assert!(runs[0].report_path.as_deref().unwrap_or("").contains("rate-limit"),
            "interruption reason must be captured for forensics");
    }

    #[test]
    fn latest_resumable_only_returns_interrupted_partial_runs() {
        // Eligible: Interrupted AND last_completed_step in 1..=9.
        let conn = fresh_conn();
        let start = Utc::now();
        // Completed run — not resumable.
        insert_running(&conn, "r-done", "p1", "Full", "Codex", start).unwrap();
        complete(&conn, "r-done", start + chrono::Duration::seconds(10),
                 "Completed", 0, 0, 0, 0, 0, 0, 0, 90, None, None).unwrap();
        // Interrupted but no step done — restart, not resume.
        insert_running(&conn, "r-empty", "p1", "Full", "Codex", start).unwrap();
        mark_interrupted(&conn, "r-empty", "crashed at step 1").unwrap();
        // Interrupted with step 5 done — resumable!
        insert_running(&conn, "r-good", "p1", "Full", "ClaudeCode", start).unwrap();
        update_last_completed_step(&conn, "r-good", 5).unwrap();
        mark_interrupted(&conn, "r-good", "rate-limit").unwrap();

        let res = latest_resumable(&conn, "p1").unwrap();
        let row = res.expect("must find r-good as resumable");
        assert_eq!(row.id, "r-good");
        assert_eq!(row.last_completed_step, 5);
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

    // ─── 0.8.4 (#298) audit_run_steps ────────────────────────────────

    #[test]
    fn insert_step_start_then_finalize_round_trip() {
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-x", "p1", "Full", "ClaudeCode", t0).unwrap();

        let started = t0;
        insert_audit_step_start(&conn, "run-x", 1, "docs/glossary.md", started).unwrap();

        // Half-finalized state: a step that started but didn't end yet
        // should be visible in the recap with `ended_at = None`.
        let steps = list_audit_steps(&conn, "run-x").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_index, 1);
        assert_eq!(steps[0].file_label, "docs/glossary.md");
        assert!(steps[0].ended_at.is_none());
        assert!(steps[0].duration_ms.is_none());
        assert!(steps[0].step_tokens.is_none());
        assert!(steps[0].cli_success, "default cli_success=true while running");

        // Finalize with success.
        let ended = started + chrono::Duration::seconds(42);
        finalize_audit_step(&conn, "run-x", 1, ended, 42_000, 1_234, 1_234, true, None, false).unwrap();
        let steps = list_audit_steps(&conn, "run-x").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].duration_ms, Some(42_000));
        assert_eq!(steps[0].step_tokens, Some(1_234));
        assert_eq!(steps[0].cumulative_tokens, Some(1_234));
        assert!(steps[0].cli_success);
        assert!(steps[0].step_warning.is_none());
        assert!(!steps[0].step_repaired_from_template);
    }

    #[test]
    fn insert_step_start_is_idempotent_on_resume() {
        // 0.8.4 (#298) — on a resumed audit (#311), the SSE pipeline
        // re-fires step_start for the FIRST replayed step (we resume
        // at last_completed_step + 1 but `for step in 1..=N { ... }`
        // re-iterates from 1). The DB row already exists, so the
        // second insert must be a no-op — `INSERT OR IGNORE` over the
        // UNIQUE (audit_run_id, step_index) index handles this.
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-r", "p1", "Full", "ClaudeCode", t0).unwrap();

        insert_audit_step_start(&conn, "run-r", 3, "docs/repo-map.md", t0).unwrap();
        // Same step fired again — must NOT crash, must NOT overwrite started_at.
        let later = t0 + chrono::Duration::seconds(60);
        insert_audit_step_start(&conn, "run-r", 3, "docs/repo-map.md", later).unwrap();

        let steps = list_audit_steps(&conn, "run-r").unwrap();
        assert_eq!(steps.len(), 1, "second insert must be a no-op");
        // started_at must keep the first value (not the re-fire timestamp).
        let drift = (steps[0].started_at - t0).num_seconds().abs();
        assert!(drift < 2, "started_at must NOT be rewritten by the second insert (drift={drift}s)");
    }

    #[test]
    fn finalize_step_with_warning_marks_failure_and_repaired() {
        // 0.8.4 (#298) — a step that triggered #292 validation must
        // surface in the recap with cli_success=false AND the warning
        // reason AND the repaired flag, so the UI can paint the row
        // red + show the warning text.
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-w", "p1", "Full", "ClaudeCode", t0).unwrap();
        insert_audit_step_start(&conn, "run-w", 5, "docs/tech-debt/inventory.md", t0).unwrap();
        finalize_audit_step(
            &conn,
            "run-w",
            5,
            t0 + chrono::Duration::seconds(7),
            7_000,
            500,
            12_000,
            false,
            Some("target file is empty — repaired from template"),
            true,
        ).unwrap();

        let steps = list_audit_steps(&conn, "run-w").unwrap();
        assert_eq!(steps.len(), 1);
        assert!(!steps[0].cli_success);
        assert_eq!(steps[0].step_warning.as_deref(),
            Some("target file is empty — repaired from template"));
        assert!(steps[0].step_repaired_from_template);
    }

    #[test]
    fn list_audit_steps_is_ordered_by_step_index() {
        let conn = fresh_conn();
        let t0 = Utc::now();
        insert_running(&conn, "run-o", "p1", "Full", "ClaudeCode", t0).unwrap();
        // Insert out of order — list must still return 1, 2, 3, 4.
        insert_audit_step_start(&conn, "run-o", 3, "docs/architecture.md", t0).unwrap();
        insert_audit_step_start(&conn, "run-o", 1, "docs/glossary.md", t0).unwrap();
        insert_audit_step_start(&conn, "run-o", 4, "docs/api.md", t0).unwrap();
        insert_audit_step_start(&conn, "run-o", 2, "docs/repo-map.md", t0).unwrap();

        let steps = list_audit_steps(&conn, "run-o").unwrap();
        let indexes: Vec<u32> = steps.iter().map(|s| s.step_index).collect();
        assert_eq!(indexes, vec![1, 2, 3, 4], "must be sorted ASC by step_index");
    }

    #[test]
    fn list_audit_steps_returns_empty_for_unknown_run() {
        // Legacy case — pre-0.8.4 runs that have no audit_run_steps
        // rows. The frontend must render the panel as "no per-step
        // data" instead of crashing.
        let conn = fresh_conn();
        let steps = list_audit_steps(&conn, "nonexistent").unwrap();
        assert!(steps.is_empty());
    }

    // ─── 0.8.4 (#317 / B1) reconcile_stale_runs ─────────────────────

    #[test]
    fn reconcile_stale_runs_marks_old_running_rows_as_interrupted() {
        // Two stale runs (3h + 1h old) + one fresh (5 min old) + one
        // already terminal. Cutoff is 30 min → first two should flip,
        // third stays Running, fourth is unchanged.
        let conn = fresh_conn();
        let now = Utc::now();
        insert_running(&conn, "stale-3h", "p1", "Full", "ClaudeCode", now - chrono::Duration::hours(3)).unwrap();
        insert_running(&conn, "stale-1h", "p1", "Full", "ClaudeCode", now - chrono::Duration::hours(1)).unwrap();
        insert_running(&conn, "fresh",    "p1", "Full", "ClaudeCode", now - chrono::Duration::minutes(5)).unwrap();
        insert_running(&conn, "terminal", "p1", "Full", "ClaudeCode", now - chrono::Duration::hours(2)).unwrap();
        complete(&conn, "terminal", now - chrono::Duration::hours(1), "Completed",
                 0, 0, 0, 0, 0, 0, 0, 100, None, None).unwrap();

        let affected = reconcile_stale_runs(&conn, 30 * 60).unwrap();
        assert_eq!(affected, 2, "exactly 2 stale Running rows should have been flipped");

        let runs = list_recent(&conn, "p1", 10).unwrap();
        let by_id: std::collections::HashMap<_, _> = runs.iter().map(|r| (r.id.clone(), r.status.clone())).collect();
        assert_eq!(by_id["stale-3h"], "Interrupted");
        assert_eq!(by_id["stale-1h"], "Interrupted");
        assert_eq!(by_id["fresh"],    "Running",   "fresh run must NOT be touched");
        assert_eq!(by_id["terminal"], "Completed", "terminal rows must NEVER be touched");

        // Idempotent: a second call does nothing.
        let again = reconcile_stale_runs(&conn, 30 * 60).unwrap();
        assert_eq!(again, 0, "second reconcile run must be a no-op");
    }

    #[test]
    fn reconcile_stale_runs_preserves_last_completed_step() {
        // A run can be Running with partial progress (last_completed_step=5).
        // After reconcile it becomes Interrupted AND keeps the step
        // pointer so the resume mechanism (#311) can pick up from
        // where it left off. Without this guard, a backend restart
        // mid-audit would lose all progress and force a from-scratch
        // re-run on resume — burning 30k+ tokens.
        let conn = fresh_conn();
        let old = Utc::now() - chrono::Duration::hours(2);
        insert_running(&conn, "partial", "p1", "Full", "ClaudeCode", old).unwrap();
        update_last_completed_step(&conn, "partial", 5).unwrap();

        reconcile_stale_runs(&conn, 30 * 60).unwrap();

        // The run is now Interrupted AND resumable.
        let resumable = latest_resumable(&conn, "p1").unwrap().expect("must be resumable post-reconcile");
        assert_eq!(resumable.id, "partial");
        assert_eq!(resumable.last_completed_step, 5,
            "last_completed_step must survive reconcile so resume picks up at step 6");
    }

    #[test]
    fn reconcile_all_running_flips_everything_regardless_of_age() {
        // For the admin "force cleanup" button: even a 5-second-old
        // Running row gets flipped. Used when the operator KNOWS
        // nothing is running (just rebuilt docker, etc).
        let conn = fresh_conn();
        let now = Utc::now();
        insert_running(&conn, "fresh-1", "p1", "Full", "ClaudeCode", now - chrono::Duration::seconds(5)).unwrap();
        insert_running(&conn, "fresh-2", "p1", "Full", "ClaudeCode", now - chrono::Duration::seconds(30)).unwrap();

        let affected = reconcile_all_running(&conn).unwrap();
        assert_eq!(affected, 2);

        let runs = list_recent(&conn, "p1", 10).unwrap();
        for r in &runs {
            assert_eq!(r.status, "Interrupted", "every Running row must be flipped (got {} for {})", r.status, r.id);
        }
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
