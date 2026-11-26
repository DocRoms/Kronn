//! KT-1270 — the projects a room may reach besides its primary one. Only the
//! human routes call the write functions; an agent may only file a request.

use std::collections::HashSet;

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::models::discussion_scope::{
    edit_access, CheckoutState, DiscussionProjectGrant, DiscussionScope, DiscussionScopeRequest,
    EditAccess, EditBlock, ScopeAccess, ScopeGrantSource, ScopeRequestStatus, WorkflowScope,
    WorkflowScopeEntry,
};

use super::discussion_scope_grants::{
    grant_by_id, grant_from_row, grant_target, receipt, GRANT_COLUMNS,
};
pub use super::discussion_scope_grants::{revoke, ScopeError};

/// The workflow scope row that stands for every project (KT-1336).
pub const ALL_PROJECTS: &str = "*";

/// Pending requests a room may hold at once: an agent cannot flood the human.
pub const MAX_PENDING_REQUESTS: usize = 5;

pub use crate::core::bridge_token::GrantedProjects;

const REQUEST_COLUMNS: &str = "r.id, r.discussion_id, r.project_id, p.name, r.access, r.reason, \
     r.requested_by, r.status, r.decided_by, r.decided_at, r.granted_access, r.created_at, \
     r.requested_name";

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
        requested_name: row.get(12)?,
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
        "SELECT project_id, access, source FROM discussion_project_grants \
         WHERE revoked_at IS NULL AND discussion_id IN ({placeholders})"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(discussion_ids), |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (project, access, source) = row?;
        let access = ScopeAccess::parse(&access)?;
        if ScopeGrantSource::parse(&source)? == ScopeGrantSource::General {
            granted.general |= access.plans();
            continue;
        }
        match project {
            None => {
                granted.all = Some(
                    granted
                        .all
                        .map_or(access, |current| current.stronger(access)),
                );
            }
            Some(project) => {
                if access == ScopeAccess::Code {
                    granted.code.insert(project.clone());
                }
                if access.plans() {
                    granted.write.insert(project.clone());
                }
                granted.read.insert(project);
            }
        }
    }
    let mut statement = conn.prepare(&format!(
        "SELECT kind, resource_id FROM general_resource_origins \
         WHERE discussion_id IN ({placeholders})"
    ))?;
    let own = statement.query_map(rusqlite::params_from_iter(discussion_ids), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in own {
        granted.general_own.insert(row?);
    }
    Ok(granted)
}

/// A registered project a room reaches through an active grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachedProject {
    pub id: String,
    pub name: String,
    /// The checkout registered for the project.
    pub path: String,
    pub access: ScopeAccess,
}

/// What a room's agent may be told it reaches (KT-1406): the projects granted
/// now, and the registered ones it could ask for. Read on every turn, so a
/// grant or a revoke applies at the next one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomReach {
    pub granted: Vec<ReachedProject>,
    /// Names only: nothing else about a project nobody granted. Never put in a
    /// prompt (KT-1417): the agent reads them through `room_scope()`.
    pub requestable: Vec<String>,
}

/// The projects `discussion_id` reaches besides its primary one. A room
/// without a project reaches none and may not ask for any (see
/// `check_target`), so it gets an empty reach. Every path comes from the
/// `projects` table, never from a request.
pub fn room_reach(conn: &Connection, discussion_id: &str) -> Result<RoomReach> {
    let Some(Some(primary)) = primary_project(conn, discussion_id)? else {
        return Ok(RoomReach::default());
    };
    let mut every: Option<ScopeAccess> = None;
    let mut by_project: std::collections::HashMap<String, ScopeAccess> = Default::default();
    for grant in active_grants(conn, discussion_id)? {
        if grant.source == ScopeGrantSource::General {
            continue;
        }
        let stronger = |current: Option<ScopeAccess>| {
            current.map_or(grant.access, |current| current.stronger(grant.access))
        };
        match &grant.project_id {
            None => every = Some(stronger(every)),
            Some(project) => {
                let access = stronger(by_project.get(project).copied());
                by_project.insert(project.clone(), access);
            }
        }
    }
    let mut statement = conn.prepare("SELECT id, name, path FROM projects ORDER BY name, id")?;
    let projects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut reach = RoomReach::default();
    for (id, name, path) in projects {
        if id == primary {
            continue;
        }
        let access = match (by_project.get(&id).copied(), every) {
            (Some(own), Some(all)) => Some(own.stronger(all)),
            (own, all) => own.or(all),
        };
        match access {
            Some(access) => reach.granted.push(ReachedProject {
                id,
                name,
                path,
                access,
            }),
            None => reach.requestable.push(name),
        }
    }
    Ok(reach)
}

/// Record the room a general resource was created from (KT-1346).
pub fn record_general_origin(
    conn: &Connection,
    kind: &str,
    resource_id: &str,
    discussion_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO general_resource_origins(kind, resource_id, discussion_id, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![kind, resource_id, discussion_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Drop every room's provenance of a general resource: an id that can be
/// reused (a skill's slug) must not hand a new resource to an old room.
pub fn forget_general_origin(conn: &Connection, kind: &str, resource_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM general_resource_origins WHERE kind = ?1 AND resource_id = ?2",
        params![kind, resource_id],
    )?;
    Ok(())
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
                "{}|{}|{}|{}",
                grant.id,
                grant.project_id.as_deref().unwrap_or("*"),
                grant.access.as_str(),
                grant.source.as_str()
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
    let requestable = room_reach(conn, discussion_id)?.requestable;
    Ok(Some(DiscussionScope {
        discussion_id: discussion_id.to_owned(),
        primary_project_id: primary,
        grants,
        revoked,
        requests,
        fingerprint,
        requestable,
    }))
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(ScopeError::Invalid(message.into()).into())
}

fn check_target(
    conn: &Connection,
    discussion_id: &str,
    project: Option<&str>,
    source: ScopeGrantSource,
    access: ScopeAccess,
) -> Result<()> {
    // Code opens a checkout for the agent to edit: one named project, decided by
    // a human for it, never every project nor the general resources.
    if access == ScopeAccess::Code && (project.is_none() || source == ScopeGrantSource::General) {
        return invalid("code access names one project: it is never granted for every project");
    }
    let Some(primary) = primary_project(conn, discussion_id)? else {
        return Err(ScopeError::NotFound("discussion not found".into()).into());
    };
    // The general resources are a room without a project's own reach; a
    // project's room writes its project.
    match (primary.as_deref(), source) {
        (None, ScopeGrantSource::General) if project.is_none() => return Ok(()),
        (None, ScopeGrantSource::General) => {
            return invalid("the general resources are granted without a project")
        }
        (Some(_), ScopeGrantSource::General) => {
            return invalid("a room with a project writes its project, not the general resources")
        }
        _ => {}
    }
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

/// KT-2102 — what the receipt of a `code` grant adds when the agent that asked
/// cannot edit the project as it runs now: without it the grant read as usable
/// and the agent worked on a copy. `asked_by` is a request's `requested_by`.
fn cannot_edit_yet(asked_by: &str) -> Option<String> {
    let agent = crate::db::discussions::parse_agent_type(asked_by.strip_prefix('@')?).ok()?;
    // The agent's own saved setting, read now: its next launch runs with it.
    let full_access = crate::core::config::saved_effective_full_access(&agent);
    let label = crate::agents::runner::agent_settings_label(&agent);
    // A checkout is looked at by a launch only: one it cannot open says so there.
    let why = match edit_access(
        &agent,
        full_access,
        ScopeAccess::Code,
        CheckoutState::Openable,
    ) {
        EditAccess::Blocked(EditBlock::CodexSandbox) => format!(
            "with its full access off it edits no project; enable Config › Agents › {label} › \
             Full access, or ask another agent"
        ),
        EditAccess::Blocked(EditBlock::NativeBroker | EditBlock::HttpTools) => {
            "it reaches the files of this room's own project only; ask Claude Code, or Codex \
             with full access"
                .to_string()
        }
        _ => return None,
    };
    Some(format!(". {label} cannot edit it yet: {why}"))
}

/// KT-2102 — what the reply of a request granted at once tells the agent that asked. One
/// that cannot edit a `code` grant as it runs now learns it here, before it tries: the
/// room's scope block only lists the project at its next turn.
pub fn trust_grant_next(granted: ScopeAccess, asked_by: &str) -> &'static str {
    if granted == ScopeAccess::Code && cannot_edit_yet(asked_by).is_some() {
        "Granted by this room's trust level and journalled, but you cannot edit this project \
         as you run now: the grant line in the room says why and what to do. Do not \
         clone or copy the project elsewhere to work around it: say so and stop."
    } else {
        "Granted by this room's trust level and journalled. The grant shows in the room; \
         continue."
    }
}

fn grant_receipt(
    conn: &Connection,
    grant: &DiscussionProjectGrant,
    asked_by: Option<&str>,
) -> Result<()> {
    let target = grant_target(conn, grant)?;
    let blocked = asked_by
        .filter(|_| grant.access == ScopeAccess::Code && grant.source != ScopeGrantSource::General)
        .and_then(cannot_edit_yet)
        .unwrap_or_default();
    let what = match (grant.source, grant.access) {
        (ScopeGrantSource::General, _) => "write",
        (_, ScopeAccess::Read) => "read",
        (_, ScopeAccess::Plan) => "plan (planning in Kronn's tools; its files stay read-only)",
        (_, ScopeAccess::Code) => "code (the agent edits its checkout like its own)",
    };
    receipt(
        conn,
        &grant.discussion_id,
        format!("scope-grant:{}", grant.id),
        format!(
            "{} granted {what} access to {target} — {}{blocked}",
            grant.granted_by,
            if grant.source == ScopeGrantSource::General {
                "no project's credentials or API configs"
            } else {
                "credentials and API configs stay this room's own project's"
            }
        ),
    )
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
    check_target(conn, discussion_id, project, source, access)?;
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
    // A human's direct grant has no asking agent to answer.
    grant_receipt(&tx, &granted, None)?;
    tx.commit()?;
    Ok(granted)
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

/// One line of agent text inside a System `[kronn-scope: …]` line: no line
/// break or bracket that could close it and forge a decision, bounded.
fn clean_line(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| match c {
            '[' | ']' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    let joined = words.join(" ");
    if joined.chars().count() > max {
        format!("{}…", joined.chars().take(max).collect::<String>())
    } else {
        joined
    }
}

/// An agent's request: only ever pending until a human decides. `project_id`
/// is the project `requested_name` names, or `None` when it names no single
/// project: the request is recorded and shown the same way, so the reply
/// never tells the agent whether a project it cannot read exists.
pub fn file_request(
    conn: &Connection,
    discussion_id: &str,
    project_id: Option<&str>,
    requested_name: &str,
    access: ScopeAccess,
    reason: &str,
    requested_by: &str,
) -> Result<Filed> {
    let reason = clean_line(reason, 500);
    if reason.is_empty() {
        return invalid("say why the room needs this project");
    }
    let requested_name = clean_line(requested_name, 120);
    if requested_name.is_empty() {
        return invalid("name the project");
    }
    let Some(primary) = primary_project(conn, discussion_id)? else {
        return Err(ScopeError::NotFound("discussion not found".into()).into());
    };
    let Some(primary) = primary else {
        return invalid("a discussion without a project cannot reach other projects");
    };
    if let Some(project_id) = project_id {
        if primary == project_id {
            return Ok(Filed::AlreadyGranted);
        }
        let granted = granted_projects(conn, &[discussion_id.to_owned()])?;
        let reached = match access {
            ScopeAccess::Read => granted.all.is_some() || granted.read.contains(project_id),
            ScopeAccess::Plan => {
                granted.all.is_some_and(|all| all.plans()) || granted.write.contains(project_id)
            }
            ScopeAccess::Code => granted.code.contains(project_id),
        };
        if reached {
            return Ok(Filed::AlreadyGranted);
        }
    }
    let pending: Option<String> = conn
        .query_row(
            "SELECT id FROM discussion_scope_requests \
             WHERE discussion_id = ?1 AND status = 'pending' \
               AND COALESCE(project_id, 'name:' || requested_name) \
                 = COALESCE(?2, 'name:' || ?3)",
            params![discussion_id, project_id, requested_name],
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
         (id, discussion_id, project_id, requested_name, access, reason, requested_by, status, \
          created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
        params![
            id,
            discussion_id,
            project_id,
            requested_name,
            access.as_str(),
            reason,
            requested_by,
            Utc::now().to_rfc3339()
        ],
    )?;
    // The agent's own words, never the project's resolved name: the line is
    // the same whether the project exists or not.
    receipt(
        &tx,
        discussion_id,
        format!("scope-request:{id}"),
        format!(
            "{} asks for {} access to \u{201c}{requested_name}\u{201d}: {reason} \u{2014} waiting for a human",
            clean_line(requested_by, 60),
            access.as_str()
        ),
    )?;
    tx.commit()?;
    match request_by_id(conn, &id)? {
        Some(request) => Ok(Filed::New(request)),
        None => bail!("scope request vanished after insert"),
    }
}

/// [`file_request`], then KT-1492: when the room's trust level grants this kind
/// of request on its own (Trusted: read, and planning, on one of the user's
/// registered projects; Full: code, KT-1491), the request is decided at once, as the level,
/// and the grant is journalled. A name that matches no project stays pending
/// for a human to decline, like any request; so does a request the level does
/// not cover, or one that cannot be decided. A request already pending for the
/// same scope is decided like a new one. Returns the level that granted it.
pub fn file_request_with_trust(
    conn: &Connection,
    discussion_id: &str,
    project_id: Option<&str>,
    requested_name: &str,
    access: ScopeAccess,
    reason: &str,
    requested_by: &str,
) -> Result<(Filed, Option<crate::models::TrustLevel>)> {
    file_request_as(
        conn,
        None,
        discussion_id,
        project_id,
        requested_name,
        access,
        reason,
        requested_by,
    )
}

/// [`file_request_with_trust`] for the launch that asks: the level is the one
/// in force for every room and the run of `asker`, not for the room it names
/// alone, so a room it just opened never lends it a higher level (KT-1964).
#[allow(clippy::too_many_arguments)]
pub fn file_request_as(
    conn: &Connection,
    asker: Option<&crate::db::trust::TrustOrigin>,
    discussion_id: &str,
    project_id: Option<&str>,
    requested_name: &str,
    access: ScopeAccess,
    reason: &str,
    requested_by: &str,
) -> Result<(Filed, Option<crate::models::TrustLevel>)> {
    let filed = file_request(
        conn,
        discussion_id,
        project_id,
        requested_name,
        access,
        reason,
        requested_by,
    )?;
    // A request already pending for the same scope is decided the same way: the
    // owner may have raised the level after the card was filed, and the agent's
    // retry must not stay blocked on it. What the level grants is the narrower
    // of what the pending card asks for and what this call asks for: a retry
    // for read on a pending write card is granted read when the level covers
    // read (even if it does not cover write), and a retry for write on a read
    // card never widens it.
    let (Filed::New(request) | Filed::AlreadyPending(request)) = &filed else {
        return Ok((filed, None));
    };
    let Some(project) = request.project_id.as_deref() else {
        return Ok((filed, None));
    };
    let access = if request.access.covers(access) {
        access
    } else {
        request.access
    };
    let approval = match access {
        ScopeAccess::Read => crate::models::TrustApproval::RoomScopeRead,
        ScopeAccess::Plan => crate::models::TrustApproval::RoomScopePlan,
        // Only the Full level opens a checkout for editing on its own; under
        // Standard and Trusted a human decides each one.
        ScopeAccess::Code => crate::models::TrustApproval::CodeWriteOtherProject,
    };
    let mut origin = crate::db::trust::TrustOrigin::room(discussion_id);
    if let Some(asker) = asker {
        for room in &asker.discussion_ids {
            if !origin.discussion_ids.contains(room) {
                origin.discussion_ids.push(room.clone());
            }
        }
        origin.workflow_run_id.clone_from(&asker.workflow_run_id);
        origin.workflow_id.clone_from(&asker.workflow_id);
    }
    // The room's own level still has to grant it: the asker's run or other
    // rooms only ever lower what the room would give.
    let room_alone = crate::db::trust::TrustOrigin::room(discussion_id);
    if crate::db::trust::level_grants(conn, &room_alone, approval)?.is_none() {
        return Ok((filed, None));
    }
    let Some(trust) = crate::db::trust::level_grants(conn, &origin, approval)? else {
        return Ok((filed, None));
    };
    let decided_by = format!("trust level {}", trust.level.as_str());
    match decide(conn, discussion_id, &request.id, Some(access), &decided_by) {
        Ok((decided, Some(granted))) => {
            let subject = format!(
                "Room scope {} on \u{201c}{}\u{201d}",
                access.as_str(),
                clean_line(&request.requested_name, 120)
            );
            // The journal names the grant: lowering the level revokes it.
            crate::db::trust::grant(
                conn,
                &origin,
                approval,
                &subject,
                Some(project),
                Some(crate::db::trust::TrustObject::room_scope_grant(
                    &granted.id,
                    discussion_id,
                )),
            )?;
            // The reply carries the decided request, with the access granted.
            let filed = match filed {
                Filed::New(_) => Filed::New(decided),
                _ => Filed::AlreadyPending(decided),
            };
            Ok((filed, Some(trust.level)))
        }
        // The request stays pending for a human.
        _ => Ok((filed, None)),
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
    let granted_access = match approve {
        Some(access) if !request.access.covers(access) => {
            return invalid(&format!(
                "a request for {} access cannot be approved for {}",
                request.access.as_str(),
                access.as_str()
            ))
        }
        access => access,
    };
    if granted_access.is_some() {
        let Some(project) = request.project_id.as_deref() else {
            return invalid("this request names no known project: decline it");
        };
        check_target(
            conn,
            discussion_id,
            Some(project),
            ScopeGrantSource::Request,
            granted_access.unwrap_or(request.access),
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
            request.project_id.as_deref(),
            access,
            ScopeGrantSource::Request,
            Some(request_id),
            decided_by,
        )?),
        None => None,
    };
    let grant = grant_id.map(|id| grant_by_id(&tx, &id)).transpose()?;
    match &grant {
        Some(granted) => grant_receipt(&tx, granted, Some(&request.requested_by))?,
        None => {
            receipt(
                &tx,
                discussion_id,
                format!("scope-decline:{request_id}"),
                format!(
                    "{decided_by} declined the access request to \u{201c}{}\u{201d}",
                    request.requested_name
                ),
            )?;
        }
    }
    tx.commit()?;
    let decided = request_by_id(conn, request_id)?.unwrap_or(request);
    Ok((decided, grant))
}

/// The revision a human validates when setting a workflow's scope: the
/// workflow and everything it executes (Quick Prompts, Quick APIs,
/// sub-workflows, skills, directives, profiles), as a run pins them
/// (`run_pins::revision_fingerprint`), repository profiles left out.
pub fn workflow_fingerprint(conn: &Connection, workflow_id: &str) -> Result<Option<String>> {
    let Some(workflow) = crate::db::workflows::get_workflow(conn, workflow_id)? else {
        return Ok(None);
    };
    Ok(Some(crate::workflows::run_pins::revision_fingerprint(
        conn,
        &workflow,
        workflow.project_id.as_deref(),
    )?))
}

/// Only a workflow of one project can carry a scope: its runs pin the
/// revision a human validated. A global or multi-project workflow resolves its
/// references per launch project, so its runs would pin another revision.
pub fn scope_supported(workflow: &crate::models::workflows::Workflow) -> bool {
    workflow.project_id.is_some() && workflow.project_scope.is_none()
}

/// A workflow's scope and whether it still holds (the workflow unchanged since
/// a human validated it); `None` when the workflow does not exist.
pub fn workflow_scope(conn: &Connection, workflow_id: &str) -> Result<Option<WorkflowScope>> {
    let Some(current) = workflow_fingerprint(conn, workflow_id)? else {
        return Ok(None);
    };
    let mut statement = conn.prepare(
        "SELECT s.project_id, p.name, s.access, s.workflow_fingerprint, s.set_by, s.set_at \
         FROM workflow_scopes s LEFT JOIN projects p ON p.id = s.project_id \
         WHERE s.workflow_id = ?1 ORDER BY p.name, s.project_id",
    )?;
    let rows = statement.query_map([workflow_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut entries = Vec::new();
    let mut all = None;
    let mut valid = true;
    let mut validated_by = None;
    let mut validated_at = None;
    let mut digest_lines = Vec::new();
    for row in rows {
        let (project_id, project_name, access, fingerprint, set_by, set_at) = row?;
        valid &= fingerprint == current;
        digest_lines.push(format!("{project_id}|{access}|{fingerprint}|{set_at}"));
        validated_by = Some(set_by);
        validated_at = Some(set_at);
        let access = ScopeAccess::parse(&access)?;
        if project_id == ALL_PROJECTS {
            all = Some(access);
        } else {
            entries.push(WorkflowScopeEntry {
                project_id,
                project_name,
                access,
            });
        }
    }
    digest_lines.sort();
    // What the human confirms covers the workflow as it is now, not only the
    // stored rows: an edit after the screen was read makes its fingerprint stale.
    digest_lines.push(format!("workflow|{workflow_id}|{current}"));
    let scope_fingerprint: String = Sha256::digest(digest_lines.join("\n").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let supported = crate::db::workflows::get_workflow(conn, workflow_id)?
        .as_ref()
        .is_some_and(scope_supported);
    Ok(Some(WorkflowScope {
        workflow_id: workflow_id.to_owned(),
        supported,
        valid: supported && valid && (!entries.is_empty() || all.is_some()),
        entries,
        all,
        fingerprint: scope_fingerprint,
        validated_by,
        validated_at,
    }))
}

/// A human validates (or clears) a workflow's scope, pinned to the workflow
/// as it is now.
pub fn set_workflow_scope(
    conn: &Connection,
    workflow_id: &str,
    entries: &[(String, ScopeAccess)],
    all: Option<ScopeAccess>,
    set_by: &str,
) -> Result<WorkflowScope> {
    let Some(workflow) = crate::db::workflows::get_workflow(conn, workflow_id)? else {
        return Err(ScopeError::NotFound("workflow not found".into()).into());
    };
    if !scope_supported(&workflow) {
        return invalid(
            "a project scope needs a workflow of one project: a global or multi-project workflow \
             resolves another revision for each project it runs in, so no validation could hold",
        );
    }
    let Some(fingerprint) = workflow_fingerprint(conn, workflow_id)? else {
        return Err(ScopeError::NotFound("workflow not found".into()).into());
    };
    let mut seen = HashSet::new();
    for (project, _) in entries {
        if !seen.insert(project.as_str()) {
            return invalid("each project appears once");
        }
        if !project_exists(conn, project)? {
            return Err(ScopeError::NotFound("project not found".into()).into());
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM workflow_scopes WHERE workflow_id = ?1",
        [workflow_id],
    )?;
    let now = Utc::now().to_rfc3339();
    let every = all.map(|access| (ALL_PROJECTS.to_owned(), access));
    for (project, access) in entries.iter().chain(every.iter()) {
        tx.execute(
            "INSERT INTO workflow_scopes \
             (workflow_id, project_id, access, workflow_fingerprint, set_by, set_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                workflow_id,
                project,
                access.as_str(),
                fingerprint,
                set_by,
                now
            ],
        )?;
    }
    tx.commit()?;
    workflow_scope(conn, workflow_id)?
        .ok_or_else(|| ScopeError::NotFound("workflow not found".into()).into())
}

/// Withdraw one project (or every one, `None`) from a workflow's scope.
/// Always allowed, whatever the workflow became: it only removes, never
/// validates the rest again (the other entries keep their own validation).
pub fn revoke_workflow_scope(
    conn: &Connection,
    workflow_id: &str,
    project_id: Option<&str>,
    all: bool,
) -> Result<Option<WorkflowScope>> {
    let project_id = if all { Some(ALL_PROJECTS) } else { project_id };
    let removed = match project_id {
        Some(project) => conn.execute(
            "DELETE FROM workflow_scopes WHERE workflow_id = ?1 AND project_id = ?2",
            params![workflow_id, project],
        )?,
        None => conn.execute(
            "DELETE FROM workflow_scopes WHERE workflow_id = ?1",
            [workflow_id],
        )?,
    };
    if removed == 0 && project_id.is_some() {
        return Err(
            ScopeError::NotFound("this project is not in the workflow's scope".into()).into(),
        );
    }
    workflow_scope(conn, workflow_id)
}

/// What a workflow run's tokens reach through their workflow's scope. The run
/// is judged on what it pinned, never on the live workflow: a run that pinned
/// anything else than the validated revision (a definition changed, then
/// restored; a Quick Prompt edited) reaches nothing, and so does a run without
/// its own pin (a child run).
pub fn run_granted_projects(conn: &Connection, run_id: &str) -> Result<GrantedProjects> {
    let workflow_id: Option<String> = conn
        .query_row(
            "SELECT workflow_id FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    let mut granted = GrantedProjects::default();
    let Some(workflow_id) = workflow_id else {
        return Ok(granted);
    };
    let Some(pinned) = crate::workflows::run_pins::pinned_scope_revision(conn, run_id)? else {
        return Ok(granted);
    };
    let validated: Vec<(String, String)> = conn
        .prepare(
            "SELECT project_id, access FROM workflow_scopes \
             WHERE workflow_id = ?1 AND workflow_fingerprint = ?2",
        )?
        .query_map(params![workflow_id, pinned], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (project_id, access) in validated {
        let access = ScopeAccess::parse(&access)?;
        if project_id == ALL_PROJECTS {
            granted.all = Some(access);
            continue;
        }
        if access.plans() {
            granted.write.insert(project_id.clone());
        }
        granted.read.insert(project_id);
    }
    Ok(granted)
}

#[cfg(test)]
#[path = "discussion_scope_tests.rs"]
mod discussion_scope_tests;
