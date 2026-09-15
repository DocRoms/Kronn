//! KT-620: the live CLI row and its reload-stable source binding are independent.
//! Pin the server-derived identity inside offer acceptance, then reuse it in the
//! caller-owned terminal/reassignment savepoint. Never select an owner by room.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use super::discussion_sessions::JoinViaTokenResult;
use crate::models::{OrchestrationActor, PlanningActorKind};

#[cfg(test)]
#[path = "cli_worker_bindings_tests.rs"]
mod tests;

fn audit(conn: &Connection, execution_id: &str, session_pk: i64, action: &str) -> Result<()> {
    super::orchestration::record_execution_event(
        conn,
        execution_id,
        action,
        None,
        None,
        &OrchestrationActor {
            kind: PlanningActorKind::Backend,
            id: Some("orchestrator".into()),
            session_id: None,
            source_message_id: None,
        },
        serde_json::json!({ "cli_session_id": session_pk }),
    )
}

/// Called only after the exact offer target and source binding have been
/// validated. Composes in accept_worker_offer's transaction: no accepted offer
/// or phase-2 transfer can exist without its durable return identity.
pub(crate) fn pin(
    conn: &Connection,
    execution_id: &str,
    session_pk: i64,
    source_agent: &str,
    source_session_id: &str,
) -> Result<()> {
    if source_agent.trim().is_empty() || source_session_id.trim().is_empty() {
        bail!("CLI worker binding identity must be nonempty");
    }
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT source_agent, source_session_id FROM task_execution_cli_bindings \
             WHERE task_execution_id = ?1 AND cli_session_id = ?2",
            params![execution_id, session_pk],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((agent, session)) = existing {
        if agent != source_agent || session != source_session_id {
            bail!("CLI worker binding identity changed for execution {execution_id}, session {session_pk}");
        }
        return Ok(());
    }
    conn.execute(
        "INSERT INTO task_execution_cli_bindings \
         (task_execution_id, cli_session_id, source_agent, source_session_id, pinned_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            execution_id,
            session_pk,
            source_agent,
            source_session_id,
            Utc::now().to_rfc3339()
        ],
    )?;
    audit(conn, execution_id, session_pk, "cli_worker_binding_pinned")
}

/// Return only the pinned worker's ownership, in the caller's transaction.
/// Legacy executions may use an exact matching active key, but an unresolved
/// identity must never be guessed from other bindings in the child room.
pub(crate) fn return_to_origin(
    conn: &Connection,
    execution_id: &str,
    session_pk: i64,
    expected_agent: Option<&str>,
    origin: &str,
    child: &str,
) -> Result<()> {
    let active: Option<(String, String, String)> = conn
        .query_row(
            "SELECT agent_type, session_id, disc_id FROM discussion_sessions WHERE id = ?1",
            [session_pk],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let pinned: Option<(String, String)> = conn
        .query_row(
            "SELECT source_agent, source_session_id FROM task_execution_cli_bindings \
             WHERE task_execution_id = ?1 AND cli_session_id = ?2",
            params![execution_id, session_pk],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    if let Some((agent, _, room)) = &active {
        if expected_agent.is_some_and(|expected| expected != agent) {
            bail!("CLI worker return refused: exact session agent changed");
        }
        if room != child && room != origin {
            bail!("CLI worker return refused: session membership moved to {room}");
        }
    }
    let was_pinned = pinned.is_some();
    let source = pinned.or_else(|| {
        active
            .as_ref()
            .map(|(agent, session, _)| (agent.clone(), session.clone()))
    });
    let Some((source_agent, source_session_id)) = source else {
        audit(
            conn,
            execution_id,
            session_pk,
            "cli_worker_return_identity_missing",
        )?;
        tracing::warn!(
            execution_id,
            session_pk,
            "CLI worker return has no retained source identity"
        );
        return Ok(());
    };
    if expected_agent.is_some_and(|expected| expected != source_agent)
        || active
            .as_ref()
            .is_some_and(|(agent, _, _)| *agent != source_agent)
    {
        bail!("CLI worker return refused: pinned source agent differs from the exact worker");
    }

    let current =
        super::disc_source::find_disc_by_source_session(conn, &source_agent, &source_session_id)?;
    match current.as_deref() {
        Some(room) if room == child => {
            super::disc_source::bind_to_source(conn, origin, &source_agent, &source_session_id)?;
        }
        Some(room) if room == origin => {}
        Some(room) => {
            bail!("CLI worker return refused: session ownership moved from child {child} to {room}")
        }
        None => {
            if !was_pinned {
                let unresolved_child_binding: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM disc_source_history \
                     WHERE disc_id = ?1 AND source_agent = ?2 AND unlinked_at IS NULL)",
                    params![child, source_agent],
                    |row| row.get(0),
                )?;
                if unresolved_child_binding {
                    bail!(
                        "CLI worker return refused: legacy execution {execution_id} has no pinned \
                         durable identity; child bindings cannot identify session {session_pk}. \
                         Explicit source-binding recovery is required"
                    );
                }
            }
            // An explicitly closed pinned source must stay closed. For a legacy
            // execution with no child binding, retain the old non-blocking return
            // behavior, but make the missing ownership evidence visible in audit.
            audit(
                conn,
                execution_id,
                session_pk,
                "cli_worker_return_binding_missing",
            )?;
            tracing::warn!(
                execution_id,
                session_pk,
                was_pinned,
                "CLI worker return has no open exact source binding"
            );
        }
    }
    if active.is_some() {
        super::discussion_sessions::move_session_to_discussion(conn, session_pk, origin)?;
    }
    Ok(())
}

/// Resume only an exact CLI session whose orchestrator-recorded execution has
/// already returned it from `expected_child` to its origin. This is separate
/// from ordinary peer resume so its expected-room refusal stays unchanged.
pub(crate) fn resume_after_orchestrator_return(
    conn: &Connection,
    agent_type: &str,
    resume_token: &str,
    new_session_id: &str,
    next_resume_token: Option<&str>,
    expected_child: &str,
) -> Result<JoinViaTokenResult> {
    let old_hash = super::discussion_sessions::sha256_hex(resume_token);
    let next_token = next_resume_token.unwrap_or(resume_token).to_string();
    let next_hash = super::discussion_sessions::sha256_hex(&next_token);
    let now = Utc::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;

    let proof: Option<(i64, String, String, String, String)> = tx
        .query_row(
            "SELECT s.id, s.disc_id, s.resume_token_hash, b.source_agent, b.source_session_id
               FROM discussion_sessions s
               JOIN task_execution_cli_bindings b ON b.cli_session_id = s.id
               JOIN task_executions e ON e.id = b.task_execution_id
              WHERE s.resume_token_hash IN (?1, ?2)
                AND s.status != 'left'
                AND s.agent_type = ?3
                AND b.source_agent = ?3
                AND e.worker_target_kind = 'cli'
                AND e.worker_cli_session_id = s.id
                AND e.worker_agent_type = ?3
                AND e.sub_discussion_id = ?4
                AND e.parent_discussion_id = s.disc_id
                AND e.status IN ('Done', 'Failed', 'Cancelled')
                AND NOT EXISTS (
                    SELECT 1 FROM task_execution_cli_bindings newer_b
                    JOIN task_executions newer_e ON newer_e.id = newer_b.task_execution_id
                    WHERE newer_b.cli_session_id = s.id
                      AND newer_e.status NOT IN ('Done', 'Failed', 'Cancelled')
                )
                AND EXISTS (SELECT 1 FROM messages m WHERE m.discussion_id = e.sub_discussion_id
                            AND m.id = 'orch-return-child:' || e.id || ':' || e.status)
                AND EXISTS (SELECT 1 FROM messages m WHERE m.discussion_id = e.parent_discussion_id
                            AND m.id = 'orch-return-origin:' || e.id || ':' || e.status)
              ORDER BY CASE WHEN s.resume_token_hash = ?1 THEN 0 ELSE 1 END
              LIMIT 1",
            params![old_hash, next_hash, agent_type, expected_child],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let (session_pk, origin, stored_hash, source_agent, source_session_id) =
        proof.ok_or_else(|| anyhow::anyhow!("orchestrator return resume refused"))?;
    if super::disc_source::find_disc_by_source_session(&tx, &source_agent, &source_session_id)?
        .as_deref()
        != Some(origin.as_str())
    {
        bail!("orchestrator return resume refused");
    }

    let updated = if stored_hash == old_hash {
        tx.execute(
            "UPDATE discussion_sessions SET session_id=?2, last_seen=?3, activity=NULL,
                    activity_expires_at=NULL, resume_token_hash=?4, resume_rotated_at=?3
              WHERE id=?1 AND resume_token_hash=?5 AND status!='left' AND disc_id=?6",
            params![session_pk, new_session_id, now, next_hash, old_hash, origin],
        )?
    } else {
        tx.execute(
            "UPDATE discussion_sessions SET session_id=?2, last_seen=?3, activity=NULL,
                    activity_expires_at=NULL
              WHERE id=?1 AND resume_token_hash=?4 AND status!='left' AND disc_id=?5",
            params![session_pk, new_session_id, now, next_hash, origin],
        )?
    };
    if updated != 1 {
        bail!("orchestrator return resume refused");
    }
    tx.commit()?;
    Ok(JoinViaTokenResult {
        disc_id: origin,
        session_pk,
        resume_token: next_token,
    })
}
