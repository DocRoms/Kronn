//! KT-1270 — the projects a room may reach besides its primary one. Only the
//! human routes call the write functions; an agent may only file a request.

use std::collections::HashSet;

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::models::discussion_scope::{
    DiscussionProjectGrant, DiscussionScope, DiscussionScopeRequest, ScopeAccess, ScopeGrantSource,
    ScopeRequestStatus,
};

/// Pending requests a room may hold at once: an agent cannot flood the human.
pub const MAX_PENDING_REQUESTS: usize = 5;

/// What the active grants of a set of discussions open, the primary project
/// excepted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantedProjects {
    pub read: HashSet<String>,
    pub write: HashSet<String>,
    pub all: Option<ScopeAccess>,
}

const GRANT_COLUMNS: &str = "g.id, g.discussion_id, g.project_id, p.name, g.access, g.source, \
     g.request_id, g.granted_by, g.granted_at, g.revoked_by, g.revoked_at, g.revoked_reason";

fn grant_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscussionProjectGrant> {
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

const REQUEST_COLUMNS: &str = "r.id, r.discussion_id, r.project_id, p.name, r.access, r.reason, \
     r.requested_by, r.status, r.decided_by, r.decided_at, r.granted_access, r.created_at";

fn request_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscussionScopeRequest> {
    let parse = |index: usize, value: String| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            format!("invalid value {value}").into(),
        )
    };
    let access: String = row.get(4)?;
    let status: String = row.get(7)?;
    let granted: Option<String> = row.get(10)?;
    Ok(DiscussionScopeRequest {
        id: row.get(0)?,
        discussion_id: row.get(1)?,
        project_id: row.get(2)?,
        project_name: row.get(3)?,
        access: ScopeAccess::parse(&access).map_err(|_| parse(4, access.clone()))?,
        reason: row.get(5)?,
        requested_by: row.get(6)?,
        status: ScopeRequestStatus::parse(&status).map_err(|_| parse(7, status.clone()))?,
        decided_by: row.get(8)?,
        decided_at: row.get(9)?,
        granted_access: granted
            .map(|value| ScopeAccess::parse(&value).map_err(|_| parse(10, value.clone())))
            .transpose()?,
        created_at: row.get(11)?,
    })
}

/// The active grants of `discussion_ids`, read on every bridge request so a
/// revoke applies on the next call. Grants on the primary project add nothing.
pub fn granted_projects(conn: &Connection, discussion_ids: &[String]) -> Result<GrantedProjects> {
    let mut granted = GrantedProjects::default();
    if discussion_ids.is_empty() {
        return Ok(granted);
    }
    let placeholders = vec!["?"; discussion_ids.len()].join(", ");
    let sql = format!(
        "SELECT project_id, access FROM discussion_project_grants \
         WHERE revoked_at IS NULL AND discussion_id IN ({placeholders})"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(discussion_ids), |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (project, access) = row?;
        let access = ScopeAccess::parse(&access)?;
        match project {
            None => {
                granted.all = Some(match (granted.all, access) {
                    (Some(ScopeAccess::Write), _) | (_, ScopeAccess::Write) => ScopeAccess::Write,
                    _ => ScopeAccess::Read,
                });
            }
            Some(project) => {
                if access == ScopeAccess::Write {
                    granted.write.insert(project.clone());
                }
                granted.read.insert(project);
            }
        }
    }
    Ok(granted)
}

/// Whether a human granted `discussion_id` write on `project_id` (or on every
/// project): the room may then host that project's workflow steps.
pub fn room_writes(conn: &Connection, discussion_id: &str, project_id: &str) -> Result<bool> {
    let granted = granted_projects(conn, &[discussion_id.to_owned()])?;
    Ok(granted.all == Some(ScopeAccess::Write) || granted.write.contains(project_id))
}

fn primary_project(conn: &Connection, discussion_id: &str) -> Result<Option<Option<String>>> {
    Ok(conn
        .query_row(
            "SELECT project_id FROM discussions WHERE id = ?1",
            [discussion_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?)
}

fn project_exists(conn: &Connection, project_id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", [project_id], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}

/// A project named by id or, failing that, by its unique name.
pub fn resolve_project(conn: &Connection, project: &str) -> Result<Option<String>> {
    let project = project.trim();
    if project.is_empty() {
        return Ok(None);
    }
    if project_exists(conn, project)? {
        return Ok(Some(project.to_owned()));
    }
    let mut statement = conn.prepare("SELECT id FROM projects WHERE name = ?1 LIMIT 2")?;
    let ids: Vec<String> = statement
        .query_map([project], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(match ids.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    })
}

fn active_grants(conn: &Connection, discussion_id: &str) -> Result<Vec<DiscussionProjectGrant>> {
    let sql = format!(
        "SELECT {GRANT_COLUMNS} FROM discussion_project_grants g \
         LEFT JOIN projects p ON p.id = g.project_id \
         WHERE g.discussion_id = ?1 AND g.revoked_at IS NULL \
         ORDER BY g.granted_at, g.id"
    );
    let mut statement = conn.prepare(&sql)?;
    let grants = statement
        .query_map([discussion_id], grant_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(grants)
}

/// Digest of the active grants, in a stable order.
pub fn fingerprint(grants: &[DiscussionProjectGrant]) -> String {
    let mut lines: Vec<String> = grants
        .iter()
        .map(|grant| {
            format!(
                "{}|{}|{}",
                grant.id,
                grant.project_id.as_deref().unwrap_or("*"),
                grant.access.as_str()
            )
        })
        .collect();
    lines.sort();
    Sha256::digest(lines.join("\n").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The room's scope; `None` when the discussion does not exist.
pub fn scope_of(conn: &Connection, discussion_id: &str) -> Result<Option<DiscussionScope>> {
    let Some(primary) = primary_project(conn, discussion_id)? else {
        return Ok(None);
    };
    let grants = active_grants(conn, discussion_id)?;
    let revoked_sql = format!(
        "SELECT {GRANT_COLUMNS} FROM discussion_project_grants g \
         LEFT JOIN projects p ON p.id = g.project_id \
         WHERE g.discussion_id = ?1 AND g.revoked_at IS NOT NULL \
         ORDER BY g.revoked_at DESC, g.id LIMIT 50"
    );
    let revoked = conn
        .prepare(&revoked_sql)?
        .query_map([discussion_id], grant_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let requests_sql = format!(
        "SELECT {REQUEST_COLUMNS} FROM discussion_scope_requests r \
         LEFT JOIN projects p ON p.id = r.project_id \
         WHERE r.discussion_id = ?1 \
         ORDER BY (r.status = 'pending') DESC, r.created_at DESC, r.id LIMIT 50"
    );
    let requests = conn
        .prepare(&requests_sql)?
        .query_map([discussion_id], request_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let fingerprint = fingerprint(&grants);
    Ok(Some(DiscussionScope {
        discussion_id: discussion_id.to_owned(),
        primary_project_id: primary,
        grants,
        revoked,
        requests,
        fingerprint,
    }))
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

fn invalid<T>(message: &str) -> Result<T> {
    Err(ScopeError::Invalid(message.into()).into())
}

fn check_target(
    conn: &Connection,
    discussion_id: &str,
    project: Option<&str>,
    source: ScopeGrantSource,
) -> Result<()> {
    let Some(primary) = primary_project(conn, discussion_id)? else {
        return Err(ScopeError::NotFound("discussion not found".into()).into());
    };
    let Some(primary) = primary else {
        return invalid("a discussion without a project cannot reach other projects");
    };
    match (project, source) {
        (None, ScopeGrantSource::All) => Ok(()),
        (None, _) | (Some(_), ScopeGrantSource::All) => {
            invalid("every project is granted with source `all` only")
        }
        (Some(project), _) if project == primary => {
            invalid("the room's own project needs no grant")
        }
        (Some(project), _) if !project_exists(conn, project)? => {
            Err(ScopeError::NotFound("project not found".into()).into())
        }
        _ => Ok(()),
    }
}

/// Revoke the active grant on the same target, then record the new one, in
/// the caller's transaction.
#[allow(clippy::too_many_arguments)]
fn insert_grant(
    conn: &Connection,
    discussion_id: &str,
    project: Option<&str>,
    access: ScopeAccess,
    source: ScopeGrantSource,
    request_id: Option<&str>,
    granted_by: &str,
) -> Result<String> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE discussion_project_grants \
         SET revoked_at = ?3, revoked_by = ?4, revoked_reason = 'replaced' \
         WHERE discussion_id = ?1 AND COALESCE(project_id, '*') = COALESCE(?2, '*') \
           AND revoked_at IS NULL",
        params![discussion_id, project, now, granted_by],
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO discussion_project_grants \
         (id, discussion_id, project_id, access, source, request_id, granted_by, granted_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            discussion_id,
            project,
            access.as_str(),
            source.as_str(),
            request_id,
            granted_by,
            now
        ],
    )?;
    Ok(id)
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
fn receipt(conn: &Connection, discussion_id: &str, receipt_id: String, body: String) -> Result<()> {
    let message = crate::models::DiscussionMessage {
        recovered_partial: false,
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

fn grant_receipt(conn: &Connection, grant: &DiscussionProjectGrant) -> Result<()> {
    let target = project_label(conn, grant.project_id.as_deref())?;
    let what = match grant.access {
        ScopeAccess::Read => "read",
        ScopeAccess::Write => "write (planning)",
    };
    receipt(
        conn,
        &grant.discussion_id,
        format!("scope-grant:{}", grant.id),
        format!(
            "{} granted {what} access to {target} — credentials and API configs stay this room's own project's",
            grant.granted_by
        ),
    )
}

fn grant_by_id(conn: &Connection, grant_id: &str) -> Result<DiscussionProjectGrant> {
    let sql = format!(
        "SELECT {GRANT_COLUMNS} FROM discussion_project_grants g \
         LEFT JOIN projects p ON p.id = g.project_id WHERE g.id = ?1"
    );
    Ok(conn.query_row(&sql, [grant_id], grant_from_row)?)
}

/// Open `project` (or every project when `None`) to the room. An active grant
/// on the same target is replaced: revoked, then granted again, so the journal
/// keeps both.
pub fn grant(
    conn: &Connection,
    discussion_id: &str,
    project: Option<&str>,
    access: ScopeAccess,
    source: ScopeGrantSource,
    granted_by: &str,
) -> Result<DiscussionProjectGrant> {
    check_target(conn, discussion_id, project, source)?;
    let tx = conn.unchecked_transaction()?;
    let id = insert_grant(
        &tx,
        discussion_id,
        project,
        access,
        source,
        None,
        granted_by,
    )?;
    let granted = grant_by_id(&tx, &id)?;
    grant_receipt(&tx, &granted)?;
    tx.commit()?;
    Ok(granted)
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
    let target = project_label(&tx, revoked.project_id.as_deref())?;
    receipt(
        &tx,
        discussion_id,
        format!("scope-revoke:{grant_id}"),
        format!("{revoked_by} revoked this room's access to {target}"),
    )?;
    tx.commit()?;
    Ok(())
}

/// What filing a request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// A new pending request.
    New(DiscussionScopeRequest),
    /// The same project is already pending: that request stands.
    AlreadyPending(DiscussionScopeRequest),
    /// The room already reaches the project at that level.
    AlreadyGranted,
}

fn request_by_id(conn: &Connection, request_id: &str) -> Result<Option<DiscussionScopeRequest>> {
    let sql = format!(
        "SELECT {REQUEST_COLUMNS} FROM discussion_scope_requests r \
         LEFT JOIN projects p ON p.id = r.project_id WHERE r.id = ?1"
    );
    Ok(conn
        .query_row(&sql, [request_id], request_from_row)
        .optional()?)
}

/// An agent's request: only ever pending until a human decides.
pub fn file_request(
    conn: &Connection,
    discussion_id: &str,
    project_id: &str,
    access: ScopeAccess,
    reason: &str,
    requested_by: &str,
) -> Result<Filed> {
    let reason = reason.trim();
    if reason.is_empty() {
        return invalid("say why the room needs this project");
    }
    let reason: String = reason.chars().take(1000).collect();
    let Some(primary) = primary_project(conn, discussion_id)? else {
        return Err(ScopeError::NotFound("discussion not found".into()).into());
    };
    let Some(primary) = primary else {
        return invalid("a discussion without a project cannot reach other projects");
    };
    if primary == project_id {
        return Ok(Filed::AlreadyGranted);
    }
    let granted = granted_projects(conn, &[discussion_id.to_owned()])?;
    let reached = match access {
        ScopeAccess::Read => granted.all.is_some() || granted.read.contains(project_id),
        ScopeAccess::Write => {
            granted.all == Some(ScopeAccess::Write) || granted.write.contains(project_id)
        }
    };
    if reached {
        return Ok(Filed::AlreadyGranted);
    }
    let pending: Option<String> = conn
        .query_row(
            "SELECT id FROM discussion_scope_requests \
             WHERE discussion_id = ?1 AND project_id = ?2 AND status = 'pending'",
            params![discussion_id, project_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(pending) = pending {
        if let Some(request) = request_by_id(conn, &pending)? {
            return Ok(Filed::AlreadyPending(request));
        }
    }
    let open: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discussion_scope_requests \
         WHERE discussion_id = ?1 AND status = 'pending'",
        [discussion_id],
        |row| row.get(0),
    )?;
    if open as usize >= MAX_PENDING_REQUESTS {
        return Err(ScopeError::Conflict(format!(
            "this room already has {MAX_PENDING_REQUESTS} access requests waiting for a human"
        ))
        .into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO discussion_scope_requests \
         (id, discussion_id, project_id, access, reason, requested_by, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
        params![
            id,
            discussion_id,
            project_id,
            access.as_str(),
            reason,
            requested_by,
            Utc::now().to_rfc3339()
        ],
    )?;
    let target = project_label(&tx, Some(project_id))?;
    receipt(
        &tx,
        discussion_id,
        format!("scope-request:{id}"),
        format!(
            "{requested_by} asks for {} access to {target}: {reason} — waiting for a human",
            access.as_str()
        ),
    )?;
    tx.commit()?;
    match request_by_id(conn, &id)? {
        Some(request) => Ok(Filed::New(request)),
        None => bail!("scope request vanished after insert"),
    }
}

/// A human's decision on a pending request. Approving grants at most the
/// access asked for.
pub fn decide(
    conn: &Connection,
    discussion_id: &str,
    request_id: &str,
    approve: Option<ScopeAccess>,
    decided_by: &str,
) -> Result<(DiscussionScopeRequest, Option<DiscussionProjectGrant>)> {
    let Some(request) = request_by_id(conn, request_id)? else {
        return Err(ScopeError::NotFound("scope request not found".into()).into());
    };
    if request.discussion_id != discussion_id {
        return Err(ScopeError::NotFound("scope request not found".into()).into());
    }
    if request.status != ScopeRequestStatus::Pending {
        return Err(ScopeError::Conflict("this request was already decided".into()).into());
    }
    let granted_access = match (approve, request.access) {
        (Some(ScopeAccess::Write), ScopeAccess::Read) => {
            return invalid("a request for read access cannot be approved for write")
        }
        (access, _) => access,
    };
    if granted_access.is_some() {
        check_target(
            conn,
            discussion_id,
            Some(&request.project_id),
            ScopeGrantSource::Request,
        )?;
    }
    let tx = conn.unchecked_transaction()?;
    let status = if granted_access.is_some() {
        ScopeRequestStatus::Approved
    } else {
        ScopeRequestStatus::Declined
    };
    let changed = tx.execute(
        "UPDATE discussion_scope_requests \
         SET status = ?2, decided_by = ?3, decided_at = ?4, granted_access = ?5 \
         WHERE id = ?1 AND status = 'pending'",
        params![
            request_id,
            status.as_str(),
            decided_by,
            Utc::now().to_rfc3339(),
            granted_access.map(ScopeAccess::as_str)
        ],
    )?;
    if changed == 0 {
        return Err(ScopeError::Conflict("this request was already decided".into()).into());
    }
    let grant_id = match granted_access {
        Some(access) => Some(insert_grant(
            &tx,
            discussion_id,
            Some(&request.project_id),
            access,
            ScopeGrantSource::Request,
            Some(request_id),
            decided_by,
        )?),
        None => None,
    };
    let grant = grant_id.map(|id| grant_by_id(&tx, &id)).transpose()?;
    match &grant {
        Some(granted) => grant_receipt(&tx, granted)?,
        None => {
            let target = project_label(&tx, Some(&request.project_id))?;
            receipt(
                &tx,
                discussion_id,
                format!("scope-decline:{request_id}"),
                format!("{decided_by} declined the access request to {target}"),
            )?;
        }
    }
    tx.commit()?;
    let decided = request_by_id(conn, request_id)?.unwrap_or(request);
    Ok((decided, grant))
}

#[cfg(test)]
#[path = "discussion_scope_tests.rs"]
mod discussion_scope_tests;
