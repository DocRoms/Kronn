//! KT-1492 — trust levels: where a human set them, which one is in force for a
//! room or a workflow, and the journal of what a level granted on its own.
//!
//! Only the human routes call [`set_level`] and [`clear_level`]; an agent's
//! token has no route to them without a valid strong-confirmation code. Gates
//! call [`grant`], which fails closed: an unreadable setting never grants.
//! What a level approves runs under the vault sandbox when the host has one
//! (`core::trust_sandbox`).
//!
//! Lowering a level withdraws what it granted: every grant is journalled with
//! the kind and id of the object it approved, stays `active` while that object
//! holds the approval, and [`set_level`] / [`clear_level`] / [`forget_scope`]
//! take the approval back and journal a `withdrawn` line for each. A grant is
//! judged by the level now in force for the room or workflow it came from, not
//! by the scope that first gave it: a room set to Standard takes back what it
//! got from the instance, and an unrelated room keeps its own.
//!
//! A gate that approves an object grants and writes in one transaction
//! ([`grant_and_write`]) on the one database connection, so a level lowered
//! meanwhile either comes first (nothing is approved) or comes after (the
//! approval is withdrawn): no approval escapes the journal.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::core::trust as policy;
use crate::models::{
    EffectiveTrust, TrustApproval, TrustAuditEntry, TrustLevel, TrustScope, TrustSetting,
};

/// Who is asking a gate: the rooms and the workflow run of an agent's launch.
/// Nothing known (a human, a peer) resolves to the instance level alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustOrigin {
    pub discussion_ids: Vec<String>,
    pub workflow_run_id: Option<String>,
    pub workflow_id: Option<String>,
    pub project_id: Option<String>,
}

impl TrustOrigin {
    pub fn room(discussion_id: &str) -> Self {
        Self {
            discussion_ids: vec![discussion_id.to_owned()],
            ..Self::default()
        }
    }

    pub fn workflow(workflow_id: &str) -> Self {
        Self {
            workflow_id: Some(workflow_id.to_owned()),
            ..Self::default()
        }
    }

    /// The launch behind a bridge token.
    pub fn from_caller(caller: &crate::core::bridge_token::BridgeCaller) -> Self {
        Self {
            discussion_ids: caller.own_discussions.clone(),
            workflow_run_id: caller.own_run.clone(),
            workflow_id: None,
            project_id: caller.project.clone(),
        }
    }
}

fn scope_key(scope: TrustScope, scope_id: Option<&str>) -> String {
    match scope {
        TrustScope::Instance => String::new(),
        _ => scope_id.unwrap_or_default().to_owned(),
    }
}

/// A scope's own setting, if a human set one.
pub fn get_setting(
    conn: &Connection,
    scope: TrustScope,
    scope_id: Option<&str>,
) -> Result<TrustSetting> {
    let key = scope_key(scope, scope_id);
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT level, set_by, set_at FROM trust_settings WHERE scope = ?1 AND scope_id = ?2",
            params![scope.as_str(), key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (level, set_by, set_at) = match row {
        Some((level, by, at)) => (TrustLevel::parse(&level), Some(by), Some(at)),
        None => (None, None, None),
    };
    Ok(TrustSetting {
        scope,
        scope_id: (scope != TrustScope::Instance).then_some(key),
        level,
        set_by,
        set_at,
    })
}

/// The instance level: Standard until a human chooses another.
pub fn instance_level(conn: &Connection) -> Result<TrustLevel> {
    Ok(get_setting(conn, TrustScope::Instance, None)?
        .level
        .unwrap_or_default())
}

/// A human sets a scope's level. Journalled as a `setting` line. Lowering it
/// withdraws what the scope granted at the higher level; the withdrawn
/// approvals are returned (each is journalled too).
pub fn set_level(
    conn: &Connection,
    scope: TrustScope,
    scope_id: Option<&str>,
    level: TrustLevel,
    set_by: &str,
) -> Result<Vec<Withdrawn>> {
    let previous = get_setting(conn, scope, scope_id)?.level;
    let key = scope_key(scope, scope_id);
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO trust_settings (scope, scope_id, level, set_by, set_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(scope, scope_id) DO UPDATE SET \
           level = excluded.level, set_by = excluded.set_by, set_at = excluded.set_at",
        params![scope.as_str(), key, level.as_str(), set_by, now],
    )?;
    journal(
        conn,
        Entry {
            event: "setting",
            level,
            source: scope,
            source_id: (scope != TrustScope::Instance).then_some(key.as_str()),
            subject: &format!("trust level set to {}", level.as_str()),
            detail: &format!(
                "by {set_by}; was {}",
                previous.map_or("inherited", TrustLevel::as_str)
            ),
            ..Entry::default()
        },
    )?;
    withdraw(conn, set_by)
}

/// A human drops a scope's own setting: it inherits again. `false` when it had
/// none. What the scope granted and the inherited level does not grant is
/// withdrawn.
pub fn clear_level(
    conn: &Connection,
    scope: TrustScope,
    scope_id: Option<&str>,
    cleared_by: &str,
) -> Result<bool> {
    let previous = get_setting(conn, scope, scope_id)?.level;
    let key = scope_key(scope, scope_id);
    let removed = conn.execute(
        "DELETE FROM trust_settings WHERE scope = ?1 AND scope_id = ?2",
        params![scope.as_str(), key],
    )?;
    if removed > 0 {
        journal(
            conn,
            Entry {
                event: "setting",
                level: TrustLevel::Standard,
                source: scope,
                source_id: (scope != TrustScope::Instance).then_some(key.as_str()),
                subject: "trust level override removed",
                detail: &format!(
                    "by {cleared_by}; was {}",
                    previous.map_or("inherited", TrustLevel::as_str)
                ),
                ..Entry::default()
            },
        )?;
        withdraw(conn, cleared_by)?;
    }
    Ok(removed > 0)
}

fn workflow_of_run(conn: &Connection, run_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT workflow_id FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// The level in force for `origin`: the narrowest explicit setting (a room,
/// then the workflow, then the instance), else Standard. When an agent's
/// rooms disagree, the lowest of their levels wins. The workflow is the one
/// `origin` runs in (a run, or a workflow's own launch): a room that merely
/// edits a workflow never takes that workflow's level.
pub fn effective(conn: &Connection, origin: &TrustOrigin) -> Result<EffectiveTrust> {
    let mut room: Option<(TrustLevel, String)> = None;
    for id in &origin.discussion_ids {
        if let Some(level) = get_setting(conn, TrustScope::Room, Some(id))?.level {
            if room.as_ref().is_none_or(|(lowest, _)| level < *lowest) {
                room = Some((level, id.clone()));
            }
        }
    }
    if let Some((level, id)) = room {
        return Ok(EffectiveTrust {
            level,
            source: TrustScope::Room,
            source_id: Some(id),
            explicit: true,
        });
    }
    let workflow_id = match (&origin.workflow_id, &origin.workflow_run_id) {
        (Some(id), _) => Some(id.clone()),
        (None, Some(run)) => workflow_of_run(conn, run)?,
        (None, None) => None,
    };
    if let Some(id) = workflow_id {
        if let Some(level) = get_setting(conn, TrustScope::Workflow, Some(&id))?.level {
            return Ok(EffectiveTrust {
                level,
                source: TrustScope::Workflow,
                source_id: Some(id),
                explicit: true,
            });
        }
    }
    let instance = get_setting(conn, TrustScope::Instance, None)?;
    Ok(EffectiveTrust {
        level: instance.level.unwrap_or_default(),
        source: TrustScope::Instance,
        source_id: None,
        explicit: instance.level.is_some(),
    })
}

/// What a grant approved, so lowering the level can take it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustObject {
    pub kind: &'static str,
    pub id: String,
    /// A step name, or the room of a scope grant.
    pub reference: Option<String>,
}

impl TrustObject {
    pub const QUICK_EXEC: &'static str = "quick_exec";
    pub const WORKFLOW_STEP: &'static str = "workflow_step";
    pub const ROOM_SCOPE_GRANT: &'static str = "room_scope_grant";

    pub fn quick_exec(id: &str) -> Self {
        Self {
            kind: Self::QUICK_EXEC,
            id: id.to_owned(),
            reference: None,
        }
    }

    pub fn workflow_step(workflow_id: &str, step: &str) -> Self {
        Self {
            kind: Self::WORKFLOW_STEP,
            id: workflow_id.to_owned(),
            reference: Some(step.to_owned()),
        }
    }

    pub fn room_scope_grant(grant_id: &str, discussion_id: &str) -> Self {
        Self {
            kind: Self::ROOM_SCOPE_GRANT,
            id: grant_id.to_owned(),
            reference: Some(discussion_id.to_owned()),
        }
    }
}

/// Whether the level in force for `origin` grants `approval` on its own, with
/// nothing journalled yet. `None`: ask as before. A host with no sandbox does
/// not stop a level: what it grants then runs without the vault protection,
/// which the panel and the acknowledgement named when the level was raised.
pub fn level_grants(
    conn: &Connection,
    origin: &TrustOrigin,
    approval: TrustApproval,
) -> Result<Option<EffectiveTrust>> {
    let trust = effective(conn, origin)?;
    Ok(policy::grants(trust.level, approval).then_some(trust))
}

/// A gate asks: does the level in force for `origin` grant `approval` on its
/// own? When it does, the grant is journalled ("granted by trust level X") with
/// the object it approved, and the level is returned; otherwise `None` and the
/// gate asks as before. Fails closed: an unreadable setting is an error the
/// gate treats as "ask".
pub fn grant(
    conn: &Connection,
    origin: &TrustOrigin,
    approval: TrustApproval,
    subject: &str,
    project_id: Option<&str>,
    object: Option<TrustObject>,
) -> Result<Option<EffectiveTrust>> {
    let Some(trust) = level_grants(conn, origin, approval)? else {
        return Ok(None);
    };
    let mut origin = origin.clone();
    if project_id.is_some() {
        origin.project_id = project_id.map(str::to_owned);
    }
    journal(
        conn,
        Entry {
            event: "grant",
            approval: Some(approval),
            level: trust.level,
            source: trust.source,
            source_id: trust.source_id.as_deref(),
            origin: &origin,
            subject,
            detail: &format!(
                "granted by trust level {} ({})",
                trust.level.as_str(),
                trust.source.as_str()
            ),
            object: object.as_ref(),
            active: object.is_some(),
        },
    )?;
    Ok(Some(trust))
}

/// A gate that approves an object: the level is consulted, the grant journalled
/// and `write` (the object's save, handed the open transaction) run as one unit,
/// on the one database connection, so a level lowered meanwhile cannot slip
/// between them. `Ok(false)`: the level no longer grants it, nothing was
/// journalled or written, and the gate asks as before. A failed `write` rolls
/// the grant back with it.
pub fn grant_and_write(
    conn: &Connection,
    origin: &TrustOrigin,
    approval: TrustApproval,
    subject: &str,
    project_id: Option<&str>,
    object: TrustObject,
    write: impl FnOnce(&Connection) -> Result<()>,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let granted = grant(&tx, origin, approval, subject, project_id, Some(object))?;
    if granted.is_none() {
        return Ok(false);
    }
    write(&tx)?;
    tx.commit()?;
    Ok(true)
}

/// What lowering a level took back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withdrawn {
    pub approval: TrustApproval,
    pub kind: String,
    pub id: String,
    pub subject: String,
}

/// One row of an active grant, as [`withdraw`] reads it.
struct ActiveGrant {
    log_id: String,
    approval: Option<String>,
    kind: Option<String>,
    object_id: Option<String>,
    reference: Option<String>,
    subject: String,
    level: String,
    source: String,
    source_id: Option<String>,
    discussion_id: Option<String>,
    /// Every room the grant's caller was bound to (JSON), NULL before 281.
    discussion_ids: Option<String>,
    workflow_id: Option<String>,
    project_id: Option<String>,
}

impl ActiveGrant {
    /// The rooms the grant came from: the full set when journalled, else the
    /// single room older rows kept.
    fn rooms(&self) -> Vec<String> {
        self.discussion_ids
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
            .filter(|rooms| !rooms.is_empty())
            .unwrap_or_else(|| self.discussion_id.iter().cloned().collect())
    }
}

/// Take back every active grant the level now in force for its origin no
/// longer gives: the approval is removed from its object, the grant stops
/// being active, and a `withdrawn` line names it in the journal. A grant is
/// judged by the room or workflow it came from, so lowering a room to Standard
/// takes back what it got from the instance, and a room with a setting of its
/// own keeps what it was granted when the instance level goes down.
pub fn withdraw(conn: &Connection, by: &str) -> Result<Vec<Withdrawn>> {
    let mut statement = conn.prepare(
        "SELECT id, approval, object_kind, object_id, object_ref, subject, level, \
                source, source_id, discussion_id, workflow_id, project_id, discussion_ids \
         FROM trust_audit_log \
         WHERE event = 'grant' AND active = 1 \
         ORDER BY at, rowid",
    )?;
    let grants: Vec<ActiveGrant> = statement
        .query_map([], |row| {
            Ok(ActiveGrant {
                log_id: row.get(0)?,
                approval: row.get(1)?,
                kind: row.get(2)?,
                object_id: row.get(3)?,
                reference: row.get(4)?,
                subject: row.get(5)?,
                level: row.get(6)?,
                source: row.get(7)?,
                source_id: row.get(8)?,
                discussion_id: row.get(9)?,
                workflow_id: row.get(10)?,
                project_id: row.get(11)?,
                discussion_ids: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);
    let mut withdrawn = Vec::new();
    for grant in grants {
        let Some(approval) = grant.approval.as_deref().and_then(TrustApproval::parse) else {
            continue;
        };
        let origin = TrustOrigin {
            discussion_ids: grant.rooms(),
            workflow_id: grant.workflow_id.clone(),
            project_id: grant.project_id.clone(),
            ..TrustOrigin::default()
        };
        let in_force = effective(conn, &origin)?;
        if policy::grants(in_force.level, approval) {
            continue;
        }
        if let (Some(kind), Some(object_id)) = (grant.kind.as_deref(), grant.object_id.as_deref()) {
            take_back(conn, kind, object_id, grant.reference.as_deref())?;
        }
        conn.execute(
            "UPDATE trust_audit_log SET active = 0 WHERE id = ?1",
            [&grant.log_id],
        )?;
        let object = grant
            .kind
            .as_deref()
            .zip(grant.object_id.as_deref())
            .and_then(|(kind, id)| {
                Some(TrustObject {
                    kind: static_kind(kind)?,
                    id: id.to_owned(),
                    reference: grant.reference.clone(),
                })
            });
        journal(
            conn,
            Entry {
                event: "withdrawn",
                approval: Some(approval),
                level: TrustLevel::parse(&grant.level).unwrap_or_default(),
                source: TrustScope::parse(&grant.source).unwrap_or(TrustScope::Instance),
                source_id: grant.source_id.as_deref(),
                origin: &origin,
                subject: &grant.subject,
                detail: &format!(
                    "withdrawn by {by}: the trust level in force for it is now {} ({})",
                    in_force.level.as_str(),
                    in_force.source.as_str()
                ),
                object: object.as_ref(),
                active: false,
            },
        )?;
        withdrawn.push(Withdrawn {
            approval,
            kind: grant.kind.unwrap_or_default(),
            id: grant.object_id.unwrap_or_default(),
            subject: grant.subject,
        });
    }
    Ok(withdrawn)
}

fn static_kind(kind: &str) -> Option<&'static str> {
    [
        TrustObject::QUICK_EXEC,
        TrustObject::WORKFLOW_STEP,
        TrustObject::ROOM_SCOPE_GRANT,
    ]
    .into_iter()
    .find(|known| *known == kind)
}

/// Remove the approval a level gave to one object. An object a human changed
/// since (the `by trust` mark is gone) or deleted is left as it is.
fn take_back(
    conn: &Connection,
    kind: &str,
    object_id: &str,
    reference: Option<&str>,
) -> Result<()> {
    match kind {
        TrustObject::QUICK_EXEC => {
            conn.execute(
                "UPDATE quick_execs SET unmodelled_args_approved = 0, approved_by_trust = 0 \
                 WHERE id = ?1 AND approved_by_trust = 1",
                [object_id],
            )?;
        }
        TrustObject::WORKFLOW_STEP => {
            let Some(step_name) = reference else {
                return Ok(());
            };
            let Some(mut workflow) = crate::db::workflows::get_workflow(conn, object_id)? else {
                return Ok(());
            };
            let mut changed = false;
            for step in workflow
                .steps
                .iter_mut()
                .chain(workflow.on_failure.iter_mut())
                .filter(|step| step.name == step_name && step.exec_approved_by_trust == Some(true))
            {
                step.exec_unmodelled_args_approved = None;
                step.exec_approved_by_trust = None;
                changed = true;
            }
            if changed {
                crate::db::workflows::update_workflow_as_agent(conn, &workflow)?;
            }
        }
        TrustObject::ROOM_SCOPE_GRANT => {
            let Some(room) = reference else {
                return Ok(());
            };
            // Already revoked by a human: nothing is left to take back.
            let _ = crate::db::discussion_scope::revoke(
                conn,
                room,
                object_id,
                "trust level",
                Some("the trust level that granted it was lowered"),
            );
        }
        _ => {}
    }
    Ok(())
}

/// A room or a workflow is deleted: what its level granted is taken back, and
/// its setting goes.
pub fn forget_scope(conn: &Connection, scope: TrustScope, scope_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM trust_settings WHERE scope = ?1 AND scope_id = ?2",
        params![scope.as_str(), scope_id],
    )?;
    withdraw(conn, "Kronn (the scope was deleted)")?;
    Ok(())
}

/// Rows kept in the journal, and days they are kept. An active grant is
/// exempt: it is what a lowered level withdraws.
pub const AUDIT_MAX_ROWS: i64 = 10_000;
pub const AUDIT_MAX_DAYS: i64 = 180;

fn prune(conn: &Connection) -> Result<()> {
    prune_at(conn, Utc::now())
}

fn prune_at(conn: &Connection, now: chrono::DateTime<Utc>) -> Result<()> {
    let cutoff = (now - chrono::Duration::days(AUDIT_MAX_DAYS)).to_rfc3339();
    conn.execute(
        "DELETE FROM trust_audit_log WHERE active = 0 AND at < ?1",
        [cutoff],
    )?;
    conn.execute(
        "DELETE FROM trust_audit_log WHERE active = 0 AND rowid <= \
         (SELECT MAX(rowid) FROM trust_audit_log) - ?1",
        [AUDIT_MAX_ROWS],
    )?;
    Ok(())
}

/// One journal line to write.
struct Entry<'a> {
    event: &'a str,
    approval: Option<TrustApproval>,
    level: TrustLevel,
    source: TrustScope,
    source_id: Option<&'a str>,
    origin: &'a TrustOrigin,
    subject: &'a str,
    detail: &'a str,
    object: Option<&'a TrustObject>,
    active: bool,
}

impl Default for Entry<'_> {
    fn default() -> Self {
        static NO_ORIGIN: TrustOrigin = TrustOrigin {
            discussion_ids: Vec::new(),
            workflow_run_id: None,
            workflow_id: None,
            project_id: None,
        };
        Self {
            event: "setting",
            approval: None,
            level: TrustLevel::Standard,
            source: TrustScope::Instance,
            source_id: None,
            origin: &NO_ORIGIN,
            subject: "",
            detail: "",
            object: None,
            active: false,
        }
    }
}

fn journal(conn: &Connection, entry: Entry<'_>) -> Result<()> {
    let origin = entry.origin;
    let workflow_id = match (&origin.workflow_id, &origin.workflow_run_id) {
        (Some(id), _) => Some(id.clone()),
        (None, Some(run)) => workflow_of_run(conn, run)?,
        (None, None) => None,
    };
    conn.execute(
        "INSERT INTO trust_audit_log \
         (id, at, event, approval, level, source, source_id, discussion_id, workflow_id, \
          project_id, subject, detail, object_kind, object_id, object_ref, active, \
          discussion_ids) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            uuid::Uuid::new_v4().to_string(),
            Utc::now().to_rfc3339(),
            entry.event,
            entry.approval.map(TrustApproval::as_str),
            entry.level.as_str(),
            entry.source.as_str(),
            entry.source_id,
            origin.discussion_ids.first(),
            workflow_id,
            origin.project_id,
            entry.subject,
            entry.detail,
            entry.object.map(|o| o.kind),
            entry.object.map(|o| o.id.as_str()),
            entry.object.and_then(|o| o.reference.as_deref()),
            i32::from(entry.active),
            (!origin.discussion_ids.is_empty())
                .then(|| serde_json::to_string(&origin.discussion_ids))
                .transpose()
                .map_err(|e| anyhow::anyhow!(e))?,
        ],
    )?;
    prune(conn)
}

/// A strong-confirmation code was spent (`proof`) or refused (`proof_refused`)
/// on a call, whatever channel carried it. The code itself is never stored: the
/// line names the channel, the route and the object it was spent on. A refusal
/// for a lockout is not journalled (it would let a flood fill the journal).
pub fn record_proof(
    conn: &Connection,
    accepted: bool,
    binding: &crate::core::trust_proof::Binding,
    reason: &str,
) -> Result<()> {
    journal(
        conn,
        Entry {
            event: if accepted { "proof" } else { "proof_refused" },
            subject: &format!("{} · {}", binding.route, binding.object),
            detail: &format!(
                "confirmation code {reason} through the {} channel",
                binding.channel.as_str()
            ),
            ..Entry::default()
        },
    )
}

/// An enrolment step the admin secret authorised (`begin` or `confirm`): the
/// journal names the route and the channel, never the secret or a code.
pub fn record_enrolment_step(
    conn: &Connection,
    route: &str,
    channel: crate::core::trust_proof::Channel,
) -> Result<()> {
    journal(
        conn,
        Entry {
            event: "proof",
            subject: &format!("{route} · proof"),
            detail: &format!(
                "enrolment step authorised by the admin secret through the {} channel",
                channel.as_str()
            ),
            ..Entry::default()
        },
    )
}

/// The journal, newest first.
pub fn audit_entries(conn: &Connection, limit: usize) -> Result<Vec<TrustAuditEntry>> {
    let mut statement = conn.prepare(
        "SELECT id, at, event, approval, level, source, source_id, discussion_id, workflow_id, \
                project_id, subject, detail, object_kind, object_id, object_ref, active \
         FROM trust_audit_log ORDER BY at DESC, rowid DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        let approval: Option<String> = row.get(3)?;
        let level: String = row.get(4)?;
        let source: String = row.get(5)?;
        Ok(TrustAuditEntry {
            id: row.get(0)?,
            at: row.get(1)?,
            event: row.get(2)?,
            approval: approval.as_deref().and_then(TrustApproval::parse),
            level: TrustLevel::parse(&level).unwrap_or_default(),
            source: TrustScope::parse(&source).unwrap_or(TrustScope::Instance),
            source_id: row.get(6)?,
            discussion_id: row.get(7)?,
            workflow_id: row.get(8)?,
            project_id: row.get(9)?,
            subject: row.get(10)?,
            detail: row.get(11)?,
            object_kind: row.get(12)?,
            object_id: row.get(13)?,
            object_ref: row.get(14)?,
            active: row.get::<_, i32>(15)? != 0,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod trust_tests;
