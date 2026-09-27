//! Room sessions joined by a workflow Agent step through its runner-owned
//! capability (KT-793). The capability itself is checked in memory by
//! `workflows::step_room`; this module owns the durable membership side.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

/// Outcome of [`join`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepRoomJoin {
    pub session_pk: i64,
    /// Live executions whose principal moved to this session.
    pub repinned: usize,
}

/// Make `(agent_type, session_id)` an active member of `disc_id` for a step of
/// the running `run_id`. The run's earlier sessions in that room leave, and
/// their live executions now address this session, so a replayed step is woken
/// by their deliveries.
pub fn join(
    conn: &Connection,
    run_id: &str,
    step_key: &str,
    disc_id: &str,
    agent_type: &str,
    session_id: &str,
) -> Result<StepRoomJoin> {
    let tx = conn.unchecked_transaction()?;
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    if status.as_deref() != Some("Running") {
        bail!("workflow run is not running");
    }
    let room_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM discussions WHERE id = ?1)",
        [disc_id],
        |row| row.get(0),
    )?;
    if !room_exists {
        bail!("room not found");
    }
    let now = Utc::now().to_rfc3339();
    let existing: Option<(i64, String, Option<String>)> = tx
        .query_row(
            "SELECT s.id, s.disc_id, l.run_id FROM discussion_sessions s
               LEFT JOIN workflow_step_room_sessions l ON l.session_pk = s.id
              WHERE s.agent_type = ?1 AND s.session_id = ?2 AND s.status != 'left'",
            params![agent_type, session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    // Only a retry of this run's own join reuses a row: the capability must
    // never adopt, and later revoke, a session someone else holds.
    let session_pk = match existing {
        Some((pk, room, linked)) if room == disc_id && linked.as_deref() == Some(run_id) => pk,
        Some(_) => bail!("session is already active outside this workflow step"),
        None => crate::db::discussion_sessions::create_session(
            &tx,
            disc_id,
            agent_type,
            Some(session_id),
            "peer",
        )?,
    };
    tx.execute(
        "INSERT INTO workflow_step_room_sessions (session_pk, run_id, step_key, disc_id, joined_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_pk) DO UPDATE SET
            run_id = excluded.run_id, step_key = excluded.step_key, disc_id = excluded.disc_id",
        params![session_pk, run_id, step_key, disc_id, now],
    )?;
    let repinned = tx.execute(
        "UPDATE task_executions SET principal_cli_session_id = ?1
          WHERE parent_discussion_id = ?2
            AND status NOT IN ('Done', 'Failed', 'Cancelled')
            AND principal_cli_session_id IN (
                SELECT session_pk FROM workflow_step_room_sessions
                 WHERE run_id = ?3 AND disc_id = ?2 AND session_pk <> ?1)",
        params![session_pk, disc_id, run_id],
    )?;
    tx.execute(
        "UPDATE discussion_sessions SET status = 'left', left_at = COALESCE(left_at, ?4)
          WHERE status != 'left' AND id IN (
                SELECT session_pk FROM workflow_step_room_sessions
                 WHERE run_id = ?1 AND disc_id = ?2 AND session_pk <> ?3)",
        params![run_id, disc_id, session_pk, now],
    )?;
    tx.commit()?;
    Ok(StepRoomJoin {
        session_pk,
        repinned,
    })
}

/// End the membership of sessions a step joined. Only linked rows are touched.
pub fn revoke(conn: &Connection, session_pks: &[i64]) -> Result<usize> {
    let now = Utc::now().to_rfc3339();
    let mut revoked = 0;
    for pk in session_pks {
        revoked += conn.execute(
            "UPDATE discussion_sessions SET status = 'left', left_at = COALESCE(left_at, ?2)
              WHERE id = ?1 AND status != 'left'
                AND id IN (SELECT session_pk FROM workflow_step_room_sessions)",
            params![pk, now],
        )?;
    }
    Ok(revoked)
}

/// No step survives a restart, so neither does a membership it joined.
pub fn revoke_all_after_restart(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE discussion_sessions SET status = 'left', left_at = COALESCE(left_at, ?1)
          WHERE status != 'left'
            AND id IN (SELECT session_pk FROM workflow_step_room_sessions)",
        [Utc::now().to_rfc3339()],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn seed(conn: &Connection, run_status: &str) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('room', 'Room', ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES ('wf', 'wf', '{}', '[]', ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, started_at) VALUES ('run', 'wf', ?1, ?2)",
            params![run_status, now],
        )
        .unwrap();
    }

    fn status(conn: &Connection, pk: i64) -> String {
        conn.query_row(
            "SELECT status FROM discussion_sessions WHERE id = ?1",
            [pk],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_step_joins_only_a_running_run_and_an_existing_room() {
        let conn = conn();
        seed(&conn, "Interrupted");
        let refused = join(&conn, "run", "orchestrate", "room", "ClaudeCode", "s1").unwrap_err();
        assert!(refused.to_string().contains("not running"), "{refused}");
        conn.execute("UPDATE workflow_runs SET status = 'Running'", [])
            .unwrap();
        let missing = join(&conn, "run", "orchestrate", "other", "ClaudeCode", "s1").unwrap_err();
        assert!(missing.to_string().contains("room not found"), "{missing}");
        let joined = join(&conn, "run", "orchestrate", "room", "ClaudeCode", "s1").unwrap();
        let again = join(&conn, "run", "orchestrate", "room", "ClaudeCode", "s1").unwrap();
        assert_eq!(
            joined.session_pk, again.session_pk,
            "a retried join is idempotent"
        );
        assert_eq!(status(&conn, joined.session_pk), "active");
    }

    #[test]
    fn revocation_touches_only_step_sessions() {
        let conn = conn();
        seed(&conn, "Running");
        let step = join(&conn, "run", "orchestrate", "room", "ClaudeCode", "s1").unwrap();
        let human = crate::db::discussion_sessions::create_session(
            &conn,
            "room",
            "Codex",
            Some("h"),
            "peer",
        )
        .unwrap();
        assert_eq!(revoke(&conn, &[step.session_pk, human]).unwrap(), 1);
        assert_eq!(status(&conn, step.session_pk), "left");
        assert_eq!(status(&conn, human), "active");

        let other = join(&conn, "run", "orchestrate", "room", "ClaudeCode", "s3").unwrap();
        assert_eq!(revoke_all_after_restart(&conn).unwrap(), 1);
        assert_eq!(status(&conn, other.session_pk), "left");
        assert_eq!(status(&conn, human), "active");
    }

    #[test]
    fn a_step_never_adopts_a_session_held_elsewhere() {
        let conn = conn();
        seed(&conn, "Running");
        let human = crate::db::discussion_sessions::create_session(
            &conn,
            "room",
            "Codex",
            Some("h"),
            "peer",
        )
        .unwrap();
        let adopted = join(&conn, "run", "orchestrate", "room", "Codex", "h").unwrap_err();
        assert!(adopted.to_string().contains("already active"), "{adopted}");
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('elsewhere', 'E', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let away = crate::db::discussion_sessions::create_session(
            &conn,
            "elsewhere",
            "ClaudeCode",
            Some("busy"),
            "peer",
        )
        .unwrap();
        assert!(join(&conn, "run", "orchestrate", "room", "ClaudeCode", "busy").is_err());
        assert_eq!(status(&conn, human), "active");
        assert_eq!(
            status(&conn, away),
            "active",
            "no eviction from another room"
        );
    }
}
