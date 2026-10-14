//! Room sessions joined by a workflow Agent step through its runner-owned
//! capability (KT-793). The capability itself is checked in memory by
//! `workflows::step_room`; this module owns the durable membership side.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{ActiveWorkflowStep, WorkflowStepIdentity};

fn identity_from_row(
    row: &rusqlite::Row<'_>,
    start: usize,
) -> rusqlite::Result<WorkflowStepIdentity> {
    Ok(WorkflowStepIdentity {
        run_id: row.get(start)?,
        workflow_id: row.get(start + 1)?,
        workflow_name: row.get(start + 2)?,
        step_key: row.get(start + 3)?,
        step_name: row.get(start + 4)?,
    })
}

/// Persist the identity before the provider starts, so the room can render a
/// running Agent step even before its bridge makes the first MCP call.
pub fn begin_activity(
    conn: &Connection,
    run_id: &str,
    step_key: &str,
    step_name: &str,
    disc_id: &str,
    agent_type: &str,
) -> Result<()> {
    let (workflow_id, workflow_name): (String, String) = conn
        .query_row(
            "SELECT w.id, w.name
               FROM workflow_runs r JOIN workflows w ON w.id = r.workflow_id
              WHERE r.id = ?1 AND r.status = 'Running'",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("workflow run is not running"))?;
    conn.execute(
        "INSERT INTO workflow_step_room_activities (
             run_id, step_key, workflow_id, workflow_name, step_name, disc_id,
             agent_type, started_at, finished_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)
         ON CONFLICT(run_id, step_key) DO UPDATE SET
             workflow_id = excluded.workflow_id,
             workflow_name = excluded.workflow_name,
             step_name = excluded.step_name,
             disc_id = excluded.disc_id,
             agent_type = excluded.agent_type,
             started_at = excluded.started_at,
             finished_at = NULL",
        params![
            run_id,
            step_key,
            workflow_id,
            workflow_name,
            step_name,
            disc_id,
            agent_type,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn finish_activity(conn: &Connection, run_id: &str, step_key: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE workflow_step_room_activities
            SET finished_at = COALESCE(finished_at, ?3)
          WHERE run_id = ?1 AND step_key = ?2 AND finished_at IS NULL",
        params![run_id, step_key, Utc::now().to_rfc3339()],
    )?)
}

pub fn list_active_for_discussion(
    conn: &Connection,
    disc_id: &str,
) -> Result<Vec<ActiveWorkflowStep>> {
    let mut statement = conn.prepare(
        "SELECT a.run_id, a.workflow_id, a.workflow_name, a.step_key, a.step_name,
                a.agent_type, a.started_at
           FROM workflow_step_room_activities a
           JOIN workflow_runs r ON r.id = a.run_id
          WHERE a.disc_id = ?1 AND a.finished_at IS NULL AND r.status = 'Running'
          ORDER BY a.started_at, a.run_id, a.step_key",
    )?;
    let rows = statement.query_map([disc_id], |row| {
        let agent = row.get::<_, String>(5)?;
        Ok(ActiveWorkflowStep {
            identity: identity_from_row(row, 0)?,
            agent_type: crate::db::discussions::parse_agent_type(&agent)?,
            started_at: row.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn message_authors(
    conn: &Connection,
    disc_id: &str,
) -> Result<std::collections::HashMap<String, WorkflowStepIdentity>> {
    let mut statement = conn.prepare(
        "SELECT mca.message_id, a.run_id, a.workflow_id, a.workflow_name,
                a.step_key, a.step_name
           FROM message_cli_authors mca
           JOIN messages m ON m.id = mca.message_id
           JOIN workflow_step_room_sessions link ON link.session_pk = mca.cli_session_id
           JOIN workflow_step_room_activities a
             ON a.run_id = link.run_id AND a.step_key = link.step_key
          WHERE m.discussion_id = ?1 AND a.disc_id = ?1",
    )?;
    let rows = statement.query_map([disc_id], |row| {
        Ok((row.get::<_, String>(0)?, identity_from_row(row, 1)?))
    })?;
    rows.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
        .map_err(Into::into)
}

/// Workflow-step provenance of one message, plus whether that exact step is
/// still alive now. Finished rows intentionally remain queryable.
pub fn message_author(
    conn: &Connection,
    disc_id: &str,
    message_id: &str,
) -> Result<Option<(WorkflowStepIdentity, bool)>> {
    conn.query_row(
        "SELECT a.run_id, a.workflow_id, a.workflow_name, a.step_key, a.step_name,
                CASE WHEN a.finished_at IS NULL AND s.status != 'left'
                           AND r.status = 'Running' THEN 1 ELSE 0 END
           FROM message_cli_authors mca
           JOIN messages m ON m.id = mca.message_id
           JOIN discussion_sessions s ON s.id = mca.cli_session_id
           JOIN workflow_step_room_sessions link ON link.session_pk = s.id
           JOIN workflow_step_room_activities a
             ON a.run_id = link.run_id AND a.step_key = link.step_key
           JOIN workflow_runs r ON r.id = a.run_id
          WHERE mca.message_id = ?1 AND m.discussion_id = ?2 AND a.disc_id = ?2",
        params![message_id, disc_id],
        |row| Ok((identity_from_row(row, 0)?, row.get::<_, bool>(5)?)),
    )
    .optional()
    .map_err(Into::into)
}

pub fn session_identity(
    conn: &Connection,
    session_pk: i64,
) -> Result<Option<WorkflowStepIdentity>> {
    conn.query_row(
        "SELECT a.run_id, a.workflow_id, a.workflow_name, a.step_key, a.step_name
           FROM workflow_step_room_sessions link
           JOIN workflow_step_room_activities a
             ON a.run_id = link.run_id AND a.step_key = link.step_key
          WHERE link.session_pk = ?1",
        [session_pk],
        |row| identity_from_row(row, 0),
    )
    .optional()
    .map_err(Into::into)
}

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
    let transaction = conn.unchecked_transaction()?;
    let revoked = transaction.execute(
        "UPDATE discussion_sessions SET status = 'left', left_at = COALESCE(left_at, ?1)
          WHERE status != 'left'
            AND id IN (SELECT session_pk FROM workflow_step_room_sessions)",
        [Utc::now().to_rfc3339()],
    )?;
    transaction.execute(
        "UPDATE workflow_step_room_activities
            SET finished_at = COALESCE(finished_at, ?1)
          WHERE finished_at IS NULL",
        [Utc::now().to_rfc3339()],
    )?;
    transaction.commit()?;
    Ok(revoked)
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
    fn activity_and_message_provenance_present_the_step_as_a_discussion_agent() {
        let conn = conn();
        seed(&conn, "Running");
        begin_activity(
            &conn,
            "run",
            "orchestrate",
            "Orchestrate",
            "room",
            "ClaudeCode",
        )
        .unwrap();
        let active = list_active_for_discussion(&conn, "room").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].identity.workflow_name, "wf");
        assert_eq!(active[0].identity.step_name, "Orchestrate");

        let joined = join(
            &conn,
            "run",
            "orchestrate",
            "room",
            "ClaudeCode",
            "step-session",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp)
             VALUES ('step-message', 'room', 'Agent', 'working', 'ClaudeCode',
                     '2026-09-28T08:00:00Z')",
            [],
        )
        .unwrap();
        crate::db::discussions::set_message_cli_author(&conn, "step-message", joined.session_pk)
            .unwrap();

        let participants =
            crate::db::discussion_sessions::list_participant_views(&conn, "room").unwrap();
        let participant = participants
            .iter()
            .find(|participant| participant.id == joined.session_pk)
            .unwrap();
        assert_eq!(participant.role, "agent");
        assert_eq!(participant.cli_ordinal, None);
        assert_eq!(
            participant.presence_state,
            crate::db::discussion_sessions::PresenceState::Running
        );
        assert_eq!(
            participant.workflow_step.as_ref().unwrap().step_name,
            "Orchestrate"
        );
        assert_eq!(
            message_authors(&conn, "room").unwrap()["step-message"].workflow_name,
            "wf"
        );
        assert_eq!(
            crate::db::discussions::list_messages(&conn, "room").unwrap()[0].author_cli_ordinal,
            None,
            "a workflow-owned process is never exposed as CLI N"
        );

        finish_activity(&conn, "run", "orchestrate").unwrap();
        revoke(&conn, &[joined.session_pk]).unwrap();
        assert!(list_active_for_discussion(&conn, "room")
            .unwrap()
            .is_empty());
        let (_, active) = message_author(&conn, "room", "step-message")
            .unwrap()
            .expect("finished provenance stays queryable");
        assert!(!active);
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
