//! Room sessions joined by a workflow Agent step through its runner-owned
//! capability (KT-793). The capability itself is checked in memory by
//! `workflows::step_room`; this module owns the durable membership side.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{
    ActiveWorkflowStep, WorkflowAgentAttempt, WorkflowAgentProvenance, WorkflowStepIdentity,
};

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
        model: None,
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
            progress_run_id: None,
            preparing: false,
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
                a.step_key, a.step_name, m.timestamp, s.agent_type, s.model
           FROM message_cli_authors mca
           JOIN messages m ON m.id = mca.message_id
           JOIN workflow_step_room_sessions link ON link.session_pk = mca.cli_session_id
           JOIN workflow_step_room_activities a
             ON a.run_id = link.run_id AND a.step_key = link.step_key
           LEFT JOIN discussion_sessions s ON s.id = mca.cli_session_id
          WHERE m.discussion_id = ?1 AND a.disc_id = ?1",
    )?;
    let rows = statement
        .query_map([disc_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                identity_from_row(row, 1)?,
                PostedBy::from_row(row, 6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // One read of a step's attempts, however many messages it posted.
    let mut attempts = std::collections::HashMap::<(String, String), Vec<_>>::new();
    let mut authors = std::collections::HashMap::with_capacity(rows.len());
    for (message_id, mut identity, posted) in rows {
        let key = (identity.run_id.clone(), identity.step_name.clone());
        if !attempts.contains_key(&key) {
            let read = step_attempts(conn, &key.0, &key.1)?;
            attempts.insert(key.clone(), read);
        }
        identity.model = posted.model(&attempts[&key]);
        authors.insert(message_id, identity);
    }
    Ok(authors)
}

/// Workflow-step provenance of one message, plus whether that exact step is
/// still alive now. Finished rows intentionally remain queryable.
pub fn message_author(
    conn: &Connection,
    disc_id: &str,
    message_id: &str,
) -> Result<Option<(WorkflowStepIdentity, bool)>> {
    let found = conn
        .query_row(
            "SELECT a.run_id, a.workflow_id, a.workflow_name, a.step_key, a.step_name,
                    CASE WHEN a.finished_at IS NULL AND s.status != 'left'
                               AND r.status = 'Running' THEN 1 ELSE 0 END,
                    m.timestamp, s.agent_type, s.model
               FROM message_cli_authors mca
               JOIN messages m ON m.id = mca.message_id
               JOIN discussion_sessions s ON s.id = mca.cli_session_id
               JOIN workflow_step_room_sessions link ON link.session_pk = s.id
               JOIN workflow_step_room_activities a
                 ON a.run_id = link.run_id AND a.step_key = link.step_key
               JOIN workflow_runs r ON r.id = a.run_id
              WHERE mca.message_id = ?1 AND m.discussion_id = ?2 AND a.disc_id = ?2",
            params![message_id, disc_id],
            |row| {
                Ok((
                    identity_from_row(row, 0)?,
                    row.get::<_, bool>(5)?,
                    PostedBy::from_row(row, 6)?,
                ))
            },
        )
        .optional()?;
    let Some((mut identity, active, posted)) = found else {
        return Ok(None);
    };
    let attempts = step_attempts(conn, &identity.run_id, &identity.step_name)?;
    identity.model = posted.model(&attempts);
    Ok(Some((identity, active)))
}

/// When, and by which session, a step message was posted (KT-2009).
struct PostedBy {
    timestamp: String,
    agent_type: Option<String>,
    declared_model: Option<String>,
}

impl PostedBy {
    fn from_row(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            timestamp: row.get(start)?,
            agent_type: row.get(start + 1)?,
            declared_model: row.get(start + 2)?,
        })
    }

    /// The model of the message's author, never guessed: the attempt running
    /// when it was posted, else what its session declared at join.
    fn model(&self, attempts: &[WorkflowAgentAttempt]) -> Option<String> {
        // The attempt's model wins: the runner resolves it at launch for that
        // very attempt, while a session declares once, at join, for itself.
        attempt_running_at(attempts, &self.timestamp, self.agent_type.as_deref())
            .and_then(WorkflowAgentAttempt::served_model)
            .or_else(|| {
                self.declared_model
                    .as_deref()
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
            })
    }
}

/// Every attempt the run recorded for its step `step_name`, over all passes of
/// it. Empty when the run is gone, never ran it, or predates attempt records.
fn step_attempts(
    conn: &Connection,
    run_id: &str,
    step_name: &str,
) -> Result<Vec<WorkflowAgentAttempt>> {
    let mut statement = conn.prepare(
        "SELECT json_extract(e.value, '$.agent_provenance')
           FROM workflow_runs r,
                json_each(CASE WHEN json_valid(r.step_results_json)
                               THEN r.step_results_json ELSE '[]' END) e
          WHERE r.id = ?1
            AND json_extract(e.value, '$.step_name') = ?2
            AND json_type(e.value, '$.agent_provenance') = 'object'",
    )?;
    let provenances = statement
        .query_map(params![run_id, step_name], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(provenances
        .iter()
        .filter_map(|json| serde_json::from_str::<WorkflowAgentProvenance>(json).ok())
        .flat_map(|provenance| provenance.attempts)
        .collect())
}

/// The attempt running at `posted_at`: the latest one started by then and not
/// over yet, preferring one of the posting session's agent.
fn attempt_running_at<'a>(
    attempts: &'a [WorkflowAgentAttempt],
    posted_at: &str,
    agent_type: Option<&str>,
) -> Option<&'a WorkflowAgentAttempt> {
    // An attempt's end is measured once its process has exited; a second
    // absorbs a coarser message clock, and a next attempt still wins by its
    // later start.
    const END_GRACE_MS: i64 = 1_000;
    let posted = chrono::DateTime::parse_from_rfc3339(posted_at)
        .ok()?
        .with_timezone(&Utc);
    attempts
        .iter()
        .filter(|attempt| {
            let ran_ms = i64::try_from(attempt.duration_ms).unwrap_or(i64::MAX);
            let end = chrono::TimeDelta::try_milliseconds(ran_ms.saturating_add(END_GRACE_MS))
                .and_then(|ran| attempt.started_at.checked_add_signed(ran));
            // An end past chrono's range is no end: the attempt covers it.
            attempt.started_at <= posted && end.is_none_or(|end| posted <= end)
        })
        .max_by_key(|attempt| {
            let same_agent =
                agent_type.is_some_and(|agent| format!("{:?}", attempt.agent) == agent);
            (same_agent, attempt.started_at)
        })
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

fn boot_revoke(conn: &Connection, _: &mut super::BootReport) -> Result<()> {
    match revoke_all_after_restart(conn) {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            "Revoked {n} room membership(s) of workflow steps that died with the previous process"
        ),
        Err(e) => tracing::warn!("Failed to revoke stale workflow step room memberships: {e}"),
    }
    Ok(())
}
inventory::submit! { super::BootStep { order: 60, run: boot_revoke } }

#[cfg(all(test, feature = "full-tests"))]
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

    // KT-2009 — the model of a step message comes from the attempt that ran
    // when it was posted.

    const T0: &str = "2026-11-24T10:00:00Z";

    fn at(seconds: i64) -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(T0)
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::seconds(seconds)
    }

    fn attempt(
        id: u32,
        agent: crate::models::AgentType,
        resolved_model: Option<&str>,
        from: i64,
        to: i64,
    ) -> WorkflowAgentAttempt {
        WorkflowAgentAttempt {
            id,
            role: crate::models::WorkflowAgentAttemptRole::Initial,
            retry: id,
            agent,
            tier: crate::models::ModelTier::Default,
            connection_id: None,
            requested_model: resolved_model.map(str::to_string),
            resolved_model: resolved_model.map(str::to_string),
            preflight_warning: None,
            model_applied: resolved_model.map(|_| true),
            observed_models: vec![],
            format_fallback: false,
            npx_fallback_command: None,
            npx_fallback_version: None,
            started_at: at(from),
            duration_ms: u64::try_from((to - from) * 1_000).unwrap(),
            succeeded: true,
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            session_id: None,
            cost_usd: None,
            cost_unknown_reason: None,
        }
    }

    /// The run's step results: one row per pass of the step `Orchestrate`.
    fn record_passes(conn: &Connection, passes: Vec<Vec<WorkflowAgentAttempt>>) {
        let rows: Vec<serde_json::Value> = passes
            .into_iter()
            .map(|attempts| {
                serde_json::json!({
                    "step_name": "Orchestrate",
                    "status": "Success",
                    "output": "done",
                    "duration_ms": 0,
                    "agent_provenance": WorkflowAgentProvenance {
                        attempts,
                        selected_attempt: None,
                    },
                })
            })
            .collect();
        conn.execute(
            "UPDATE workflow_runs SET step_results_json = ?1 WHERE id = 'run'",
            [serde_json::Value::Array(rows).to_string()],
        )
        .unwrap();
    }

    /// The step joined with `agent`, as its bridge does for each attempt.
    fn step_session(conn: &Connection, agent: &str, session: &str) -> i64 {
        join(conn, "run", "orchestrate", "room", agent, session)
            .unwrap()
            .session_pk
    }

    fn post(conn: &Connection, message_id: &str, session_pk: i64, seconds: i64) {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp,
                                   sort_order)
             VALUES (?1, 'room', 'Agent', 'working', 'ClaudeCode', ?2, ?3)",
            params![message_id, at(seconds).to_rfc3339(), seconds],
        )
        .unwrap();
        crate::db::discussions::set_message_cli_author(conn, message_id, session_pk).unwrap();
    }

    /// The model each read gives the message; both reads must agree.
    fn model_of(conn: &Connection, message_id: &str) -> Option<String> {
        let listed = message_authors(conn, "room").unwrap()[message_id]
            .model
            .clone();
        let (single, _) = message_author(conn, "room", message_id)
            .unwrap()
            .expect("the step message keeps its provenance");
        assert_eq!(single.model, listed, "the two reads agree");
        listed
    }

    fn step_room(conn: &Connection) {
        seed(conn, "Running");
        begin_activity(
            conn,
            "run",
            "orchestrate",
            "Orchestrate",
            "room",
            "ClaudeCode",
        )
        .unwrap();
    }

    #[test]
    fn a_step_message_reads_the_model_its_attempt_ran() {
        let conn = conn();
        step_room(&conn);
        let session = step_session(&conn, "ClaudeCode", "s1");
        post(&conn, "m", session, 30);
        record_passes(
            &conn,
            vec![vec![attempt(
                1,
                crate::models::AgentType::ClaudeCode,
                Some("claude-opus-4-7"),
                0,
                60,
            )]],
        );

        assert_eq!(model_of(&conn, "m").as_deref(), Some("claude-opus-4-7"));
        let json = serde_json::to_value(&message_authors(&conn, "room").unwrap()["m"]).unwrap();
        assert_eq!(json["model"], "claude-opus-4-7");
    }

    #[test]
    fn a_fallback_or_retry_attempt_message_reads_that_attempt_model() {
        let conn = conn();
        step_room(&conn);
        let first = step_session(&conn, "ClaudeCode", "s1");
        post(&conn, "initial", first, 5);
        let fallback = step_session(&conn, "Codex", "s2");
        post(&conn, "fallback", fallback, 30);
        let replay = step_session(&conn, "ClaudeCode", "s3");
        post(&conn, "replay", replay, 130);
        record_passes(
            &conn,
            vec![
                vec![
                    attempt(
                        1,
                        crate::models::AgentType::ClaudeCode,
                        Some("claude-opus-4-7"),
                        0,
                        10,
                    ),
                    attempt(
                        2,
                        crate::models::AgentType::Codex,
                        Some("gpt-5-codex"),
                        20,
                        40,
                    ),
                ],
                // The step ran again later in the run, on another model.
                vec![attempt(
                    1,
                    crate::models::AgentType::ClaudeCode,
                    Some("claude-sonnet-4-6"),
                    120,
                    140,
                )],
            ],
        );

        assert_eq!(
            model_of(&conn, "initial").as_deref(),
            Some("claude-opus-4-7")
        );
        assert_eq!(model_of(&conn, "fallback").as_deref(), Some("gpt-5-codex"));
        assert_eq!(
            model_of(&conn, "replay").as_deref(),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn the_attempt_model_wins_over_the_session_declared_one() {
        let conn = conn();
        step_room(&conn);
        let session = step_session(&conn, "Codex", "s1");
        crate::db::discussion_sessions::set_session_model(&conn, session, "declared-model")
            .unwrap();
        post(&conn, "covered", session, 30);
        post(&conn, "uncovered", session, 300);
        record_passes(
            &conn,
            vec![vec![attempt(
                1,
                crate::models::AgentType::Codex,
                Some("gpt-5-codex"),
                0,
                60,
            )]],
        );

        assert_eq!(model_of(&conn, "covered").as_deref(), Some("gpt-5-codex"));
        assert_eq!(
            model_of(&conn, "uncovered").as_deref(),
            Some("declared-model"),
            "a declaration stands when no attempt covers the message"
        );
    }

    #[test]
    fn a_step_message_whose_run_recorded_no_model_stays_unknown() {
        let conn = conn();
        step_room(&conn);
        let session = step_session(&conn, "ClaudeCode", "s1");
        post(&conn, "m", session, 30);

        // The step has not finished: no attempt is recorded yet.
        assert_eq!(model_of(&conn, "m"), None);

        // An attempt whose transport kept its default names no model.
        let mut kept_default = attempt(
            1,
            crate::models::AgentType::ClaudeCode,
            Some("claude-opus-4-7"),
            0,
            60,
        );
        kept_default.model_applied = Some(false);
        record_passes(&conn, vec![vec![kept_default]]);
        assert_eq!(model_of(&conn, "m"), None);

        // No attempt ran when the message was posted.
        record_passes(
            &conn,
            vec![vec![attempt(
                1,
                crate::models::AgentType::ClaudeCode,
                Some("claude-opus-4-7"),
                100,
                160,
            )]],
        );
        assert_eq!(model_of(&conn, "m"), None);

        // A run recorded before attempts existed.
        conn.execute(
            "UPDATE workflow_runs SET step_results_json =
                 '[{\"step_name\":\"Orchestrate\",\"status\":\"Success\",\"output\":\"\",\"duration_ms\":0,\"step_model\":\"opus\"}]'
              WHERE id = 'run'",
            [],
        )
        .unwrap();
        assert_eq!(model_of(&conn, "m"), None);

        // Its step results unreadable.
        conn.execute(
            "UPDATE workflow_runs SET step_results_json = '{not json' WHERE id = 'run'",
            [],
        )
        .unwrap();
        assert_eq!(model_of(&conn, "m"), None);
    }

    #[test]
    fn a_purged_run_leaves_no_model_to_read() {
        let conn = conn();
        step_room(&conn);
        let session = step_session(&conn, "ClaudeCode", "s1");
        post(&conn, "m", session, 30);
        record_passes(
            &conn,
            vec![vec![attempt(
                1,
                crate::models::AgentType::ClaudeCode,
                Some("claude-opus-4-7"),
                0,
                60,
            )]],
        );
        conn.execute("DELETE FROM workflow_runs WHERE id = 'run'", [])
            .unwrap();

        assert!(
            !message_authors(&conn, "room").unwrap().contains_key("m"),
            "nothing of the purged run is read back"
        );
        assert!(message_author(&conn, "room", "m").unwrap().is_none());
    }
}
