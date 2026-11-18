//! The room-scope grant read helpers and the revocation, kept below
//! `db::discussion_scope` so the trust withdrawal can revoke a grant.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::discussion_scope::{
    DiscussionProjectGrant, ScopeAccess, ScopeGrantSource,
};

pub const GRANT_COLUMNS: &str = "g.id, g.discussion_id, g.project_id, p.name, g.access, g.source, \
     g.request_id, g.granted_by, g.granted_at, g.revoked_by, g.revoked_at, g.revoked_reason";

pub fn grant_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscussionProjectGrant> {
    let parse = |index: usize, value: String| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            format!("invalid value {value}").into(),
        )
    };
    let access: String = row.get(4)?;
    let source: String = row.get(5)?;
    Ok(DiscussionProjectGrant {
        id: row.get(0)?,
        discussion_id: row.get(1)?,
        project_id: row.get(2)?,
        project_name: row.get(3)?,
        access: ScopeAccess::parse(&access).map_err(|_| parse(4, access.clone()))?,
        source: ScopeGrantSource::parse(&source).map_err(|_| parse(5, source.clone()))?,
        request_id: row.get(6)?,
        granted_by: row.get(7)?,
        granted_at: row.get(8)?,
        revoked_by: row.get(9)?,
        revoked_at: row.get(10)?,
        revoked_reason: row.get(11)?,
    })
}

/// Why a scope change was refused: shown to the human as is.
#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
}

pub fn grant_target(conn: &Connection, grant: &DiscussionProjectGrant) -> Result<String> {
    if grant.source == ScopeGrantSource::General {
        return Ok("the general resources (seen by every project)".into());
    }
    project_label(conn, grant.project_id.as_deref())
}

fn project_label(conn: &Connection, project: Option<&str>) -> Result<String> {
    let Some(project) = project else {
        return Ok("every project".into());
    };
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            [project],
            |row| row.get(0),
        )
        .optional()?;
    Ok(name.unwrap_or_else(|| project.to_owned()))
}

/// A non-dispatching System line in the room: the agents and the human see
/// every change of scope where they work.
pub fn receipt(conn: &Connection, discussion_id: &str, receipt_id: String, body: String) -> Result<()> {
    let message = crate::models::DiscussionMessage {
        recovered_partial: false,
        kind: Default::default(),
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        session_continuity: None,
        model: None,
        lint_report: None,
        id: receipt_id,
        role: crate::models::MessageRole::System,
        channel: crate::models::MessageChannel::Main,
        content: format!("[kronn-scope: {body}]"),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    crate::db::discussions::insert_message(conn, discussion_id, &message)?;
    Ok(())
}

pub fn grant_by_id(conn: &Connection, grant_id: &str) -> Result<DiscussionProjectGrant> {
    let sql = format!(
        "SELECT {GRANT_COLUMNS} FROM discussion_project_grants g \
         LEFT JOIN projects p ON p.id = g.project_id WHERE g.id = ?1"
    );
    Ok(conn.query_row(&sql, [grant_id], grant_from_row)?)
}

/// Revoke an active grant. A revoked grant stays revoked.
pub fn revoke(
    conn: &Connection,
    discussion_id: &str,
    grant_id: &str,
    revoked_by: &str,
    reason: Option<&str>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let changed = tx.execute(
        "UPDATE discussion_project_grants \
         SET revoked_at = ?3, revoked_by = ?4, revoked_reason = ?5 \
         WHERE id = ?1 AND discussion_id = ?2 AND revoked_at IS NULL",
        params![
            grant_id,
            discussion_id,
            Utc::now().to_rfc3339(),
            revoked_by,
            reason
        ],
    )?;
    if changed == 0 {
        return Err(
            ScopeError::NotFound("no active grant with this id in this room".into()).into(),
        );
    }
    let revoked = grant_by_id(&tx, grant_id)?;
    let target = grant_target(&tx, &revoked)?;
    receipt(
        &tx,
        discussion_id,
        format!("scope-revoke:{grant_id}"),
        format!("{revoked_by} revoked this room's access to {target}"),
    )?;
    tx.commit()?;
    Ok(())
}
