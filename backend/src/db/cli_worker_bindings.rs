//! KT-620: the live CLI row and its reload-stable source binding are independent.
//! Pin the server-derived identity inside offer acceptance, then reuse it in the
//! caller-owned terminal/reassignment savepoint. Never select an owner by room.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

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
