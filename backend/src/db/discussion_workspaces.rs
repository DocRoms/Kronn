//! KT-140 — durable multi-worktree bindings for discussions and joined CLIs.

use anyhow::{bail, Result};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DiscussionWorkspace {
    pub id: String,
    pub disc_id: String,
    pub session_pk: Option<i64>,
    pub session_agent_type: Option<String>,
    pub task_id: Option<String>,
    pub task_reference: Option<String>,
    pub project_id: String,
    pub workspace_path: Option<String>,
    pub canonical_path: Option<String>,
    pub branch: String,
    pub head_sha: Option<String>,
    pub ownership: String,
    pub state: String,
    /// Managed-worktree lineage (KT-318, migration 127). Populated only for a
    /// backend-owned `managed` row: the principal room it was spawned from, the
    /// parent HEAD it was pinned at, and the owning TaskExecution. All NULL for a
    /// CLI-declared `external` workspace.
    #[serde(default)]
    pub parent_discussion_id: Option<String>,
    #[serde(default)]
    pub base_sha: Option<String>,
    #[serde(default)]
    pub task_execution_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub const HISTORY_LEASE_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct WorkspaceHistoryLease {
    pub id: String,
    pub disc_id: String,
    pub session_pk: i64,
    pub session_agent_type: String,
    pub session_id: Option<String>,
    pub canonical_path: String,
    pub branch: String,
    pub backup_ref: String,
    pub head_sha: String,
    pub acquired_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryLeaseAcquire {
    Acquired(WorkspaceHistoryLease),
    Blocked(WorkspaceHistoryLease),
}

fn map_history_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceHistoryLease> {
    Ok(WorkspaceHistoryLease {
        id: row.get(0)?,
        disc_id: row.get(1)?,
        session_pk: row.get(2)?,
        session_agent_type: row.get(3)?,
        session_id: row.get(4)?,
        canonical_path: row.get(5)?,
        branch: row.get(6)?,
        backup_ref: row.get(7)?,
        head_sha: row.get(8)?,
        acquired_at: row.get(9)?,
        expires_at: row.get(10)?,
    })
}

const SELECT_HISTORY_LEASE: &str = "
    SELECT lease.id, lease.disc_id, lease.session_pk, session.agent_type,
           session.session_id, lease.canonical_path, lease.branch,
           lease.backup_ref, lease.head_sha, lease.acquired_at, lease.expires_at
      FROM discussion_workspace_history_leases lease
      JOIN discussion_sessions session ON session.id = lease.session_pk
";

fn map_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscussionWorkspace> {
    Ok(DiscussionWorkspace {
        id: row.get(0)?,
        disc_id: row.get(1)?,
        session_pk: row.get(2)?,
        session_agent_type: row.get(3)?,
        task_id: row.get(4)?,
        task_reference: row
            .get::<_, Option<i64>>(5)?
            .map(|number| format!("KT-{number}")),
        project_id: row.get(6)?,
        workspace_path: row.get(7)?,
        canonical_path: row.get(8)?,
        branch: row.get(9)?,
        head_sha: row.get(10)?,
        ownership: row.get(11)?,
        state: row.get(12)?,
        parent_discussion_id: row.get(15)?,
        base_sha: row.get(16)?,
        task_execution_id: row.get(17)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

const SELECT_WORKSPACE: &str = "
    SELECT dw.id, dw.disc_id, dw.session_pk, ds.agent_type,
           dw.task_id, pt.task_number,
           dw.project_id, dw.workspace_path, dw.canonical_path, dw.branch,
           dw.head_sha, dw.ownership, dw.state, dw.created_at, dw.updated_at,
           dw.parent_discussion_id, dw.base_sha, dw.task_execution_id
      FROM discussion_workspaces dw
      LEFT JOIN discussion_sessions ds ON ds.id = dw.session_pk
      LEFT JOIN planning_tasks pt ON pt.id = dw.task_id
";

pub fn list_for_discussion(conn: &Connection, disc_id: &str) -> Result<Vec<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE} WHERE dw.disc_id = ?1
         ORDER BY CASE WHEN dw.session_pk IS NULL THEN 0 ELSE 1 END,
                  dw.updated_at DESC"
    );
    let mut statement = conn.prepare(&sql)?;
    let workspaces = statement
        .query_map([disc_id], map_workspace)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(workspaces)
}

/// Workspaces whose code belongs in a discussion's UI.
///
/// A principal room owns the managed workspaces of its orchestrated child
/// discussions through `parent_discussion_id`. Keeping this projection in the
/// database layer prevents the frontend from reconstructing execution lineage
/// from chat messages, and keeps detached (cleaned) evidence visible.
pub fn list_visible_for_discussion(
    conn: &Connection,
    disc_id: &str,
) -> Result<Vec<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE}
         WHERE dw.disc_id = ?1 OR dw.parent_discussion_id = ?1
         ORDER BY CASE dw.state WHEN 'attached' THEN 0 WHEN 'detached' THEN 1 ELSE 2 END,
                  CASE WHEN dw.disc_id = ?1 THEN 0 ELSE 1 END,
                  dw.updated_at DESC"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map([disc_id], map_workspace)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_visible_for_discussion(
    conn: &Connection,
    disc_id: &str,
    workspace_id: &str,
) -> Result<Option<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE}
         WHERE dw.id = ?2 AND (dw.disc_id = ?1 OR dw.parent_discussion_id = ?1)
         LIMIT 1"
    );
    Ok(conn
        .query_row(&sql, params![disc_id, workspace_id], map_workspace)
        .optional()?)
}

pub fn get_for_session(
    conn: &Connection,
    disc_id: &str,
    session_pk: i64,
) -> Result<Option<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE}
         WHERE dw.disc_id = ?1 AND dw.session_pk = ?2
         LIMIT 1"
    );
    Ok(conn
        .query_row(&sql, params![disc_id, session_pk], map_workspace)
        .optional()?)
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_external(
    conn: &Connection,
    disc_id: &str,
    session_pk: i64,
    task_id: Option<&str>,
    project_id: &str,
    workspace_path: &str,
    canonical_path: &str,
    branch: &str,
    head_sha: &str,
) -> Result<DiscussionWorkspace> {
    if workspace_path.trim().is_empty()
        || canonical_path.trim().is_empty()
        || branch.trim().is_empty()
        || head_sha.trim().is_empty()
    {
        bail!("workspace path, canonical path, branch and HEAD are required");
    }

    let now = Utc::now().to_rfc3339();
    let existing_id = conn
        .query_row(
            "SELECT id FROM discussion_workspaces
              WHERE disc_id = ?1 AND session_pk = ?2",
            params![disc_id, session_pk],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());

    conn.execute(
        "INSERT INTO discussion_workspaces (
             id, disc_id, session_pk, task_id, project_id,
             workspace_path, canonical_path, branch, head_sha, base_sha,
             ownership, state, created_at, updated_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9,
             'external', 'attached', ?10, ?10
         )
         ON CONFLICT(disc_id, session_pk) WHERE session_pk IS NOT NULL DO UPDATE SET
             task_id = excluded.task_id,
             project_id = excluded.project_id,
             workspace_path = excluded.workspace_path,
             canonical_path = excluded.canonical_path,
             branch = excluded.branch,
             head_sha = excluded.head_sha,
             ownership = 'external',
             state = 'attached',
             updated_at = excluded.updated_at",
        params![
            id,
            disc_id,
            session_pk,
            task_id,
            project_id,
            workspace_path,
            canonical_path,
            branch,
            head_sha,
            now
        ],
    )?;

    get_for_session(conn, disc_id, session_pk)?
        .ok_or_else(|| anyhow::anyhow!("workspace upsert did not return a row"))
}

pub fn mark_missing(
    conn: &Connection,
    disc_id: &str,
    session_pk: i64,
) -> Result<Option<DiscussionWorkspace>> {
    conn.execute(
        "UPDATE discussion_workspaces
            SET state = 'missing', updated_at = ?3
          WHERE disc_id = ?1 AND session_pk = ?2",
        params![disc_id, session_pk, Utc::now().to_rfc3339()],
    )?;
    get_for_session(conn, disc_id, session_pk)
}

// ─── Managed workspaces (KT-318 — backend-owned, no joined CLI session) ───────

/// Create or re-attach the backend-owned `managed` workspace for a TaskExecution.
///
/// Unlike [`upsert_external`], this needs NO joined CLI session and NO `kr-join`
/// token: the backend provisioner owns the row (audit gap #3 — the native
/// principal cannot `disc_workspace_set`). Idempotent by `task_execution_id` (the
/// unique partial index), so a compensable retry re-attaches its own row rather
/// than creating a second. `disc_id` is the sub-discussion the worktree serves;
/// `parent_discussion_id` is the principal room; `base_sha` is the pinned parent
/// HEAD the child branch was cut from (ADR §4).
#[allow(clippy::too_many_arguments)]
pub fn upsert_managed(
    conn: &Connection,
    task_execution_id: &str,
    disc_id: &str,
    parent_discussion_id: &str,
    task_id: Option<&str>,
    project_id: &str,
    workspace_path: &str,
    canonical_path: &str,
    branch: &str,
    head_sha: &str,
    base_sha: &str,
) -> Result<DiscussionWorkspace> {
    if workspace_path.trim().is_empty()
        || canonical_path.trim().is_empty()
        || branch.trim().is_empty()
        || head_sha.trim().is_empty()
        || base_sha.trim().is_empty()
    {
        bail!("managed workspace path, canonical path, branch, HEAD and base SHA are required");
    }

    let now = Utc::now().to_rfc3339();
    let existing_id = conn
        .query_row(
            "SELECT id FROM discussion_workspaces WHERE task_execution_id = ?1",
            params![task_execution_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());

    conn.execute(
        "INSERT INTO discussion_workspaces (
             id, disc_id, session_pk, task_id, project_id,
             workspace_path, canonical_path, branch, head_sha,
             ownership, state, created_at, updated_at,
             parent_discussion_id, base_sha, task_execution_id
         ) VALUES (
             ?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8,
             'managed', 'attached', ?9, ?9, ?10, ?11, ?12
         )
         ON CONFLICT(task_execution_id) WHERE task_execution_id IS NOT NULL DO UPDATE SET
             disc_id = excluded.disc_id,
             task_id = excluded.task_id,
             project_id = excluded.project_id,
             workspace_path = excluded.workspace_path,
             canonical_path = excluded.canonical_path,
             branch = excluded.branch,
             head_sha = excluded.head_sha,
             ownership = 'managed',
             state = 'attached',
             parent_discussion_id = excluded.parent_discussion_id,
             base_sha = excluded.base_sha,
             updated_at = excluded.updated_at",
        params![
            id,
            disc_id,
            task_id,
            project_id,
            workspace_path,
            canonical_path,
            branch,
            head_sha,
            now,
            parent_discussion_id,
            base_sha,
            task_execution_id,
        ],
    )?;

    get_managed_for_execution(conn, task_execution_id)?
        .ok_or_else(|| anyhow::anyhow!("managed workspace upsert did not return a row"))
}

/// The managed workspace owned by a TaskExecution, if any (unique by the index).
pub fn get_managed_for_execution(
    conn: &Connection,
    task_execution_id: &str,
) -> Result<Option<DiscussionWorkspace>> {
    let sql = format!("{SELECT_WORKSPACE} WHERE dw.task_execution_id = ?1 LIMIT 1");
    Ok(conn
        .query_row(&sql, params![task_execution_id], map_workspace)
        .optional()?)
}

/// The backend-owned `managed` workspace serving a discussion, if any (KT-328). A
/// CLI attached to a sub-discussion with a managed worktree must READ this row, not
/// re-declare it via `disc_workspace_set`: re-declaring the same checkout would trip
/// the UNIQUE `canonical_path` index with an opaque constraint error, and a
/// different path would create a second (external) row with no designated authority
/// for the ownership-aware teardown. The handler uses this to refuse cleanly.
pub fn get_managed_for_discussion(
    conn: &Connection,
    disc_id: &str,
) -> Result<Option<DiscussionWorkspace>> {
    let sql =
        format!("{SELECT_WORKSPACE} WHERE dw.disc_id = ?1 AND dw.ownership = 'managed' LIMIT 1");
    Ok(conn
        .query_row(&sql, params![disc_id], map_workspace)
        .optional()?)
}

/// Mark the managed workspace as `missing` (compensation-friendly): the auditable
/// row and its ownership are preserved so the execution stays resumable rather
/// than becoming a silent orphan.
pub fn mark_missing_for_execution(
    conn: &Connection,
    task_execution_id: &str,
) -> Result<Option<DiscussionWorkspace>> {
    conn.execute(
        "UPDATE discussion_workspaces
            SET state = 'missing', updated_at = ?2
          WHERE task_execution_id = ?1",
        params![task_execution_id, Utc::now().to_rfc3339()],
    )?;
    get_managed_for_execution(conn, task_execution_id)
}

/// The managed workspaces Kronn still records as `attached` to a checkout on
/// disk (KT-1676): the ones whose directory a reconcile pass must find again.
pub fn list_attached_managed_with_path(conn: &Connection) -> Result<Vec<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE} WHERE dw.ownership = 'managed' AND dw.state = 'attached' \
         AND dw.canonical_path IS NOT NULL AND dw.task_execution_id IS NOT NULL \
         ORDER BY dw.created_at, dw.id"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map([], map_workspace)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Whether Kronn's workspace registry records `path` as one of its own
/// checkouts (KT-1748): a managed discussion workspace, or the workspace of a
/// workflow run. Only the exact path counts; an `external` workspace a user
/// declared stays foreign. A database error answers `false`, never "managed".
pub fn is_registered_managed_checkout(conn: &Connection, path: &std::path::Path) -> bool {
    let path = path.to_string_lossy();
    let known = |sql: &str| {
        conn.query_row(sql, params![path.as_ref()], |_| Ok(()))
            .optional()
            .map(|row| row.is_some())
            .unwrap_or(false)
    };
    known(
        "SELECT 1 FROM discussion_workspaces WHERE ownership = 'managed' \
         AND (canonical_path = ?1 OR workspace_path = ?1) LIMIT 1",
    ) || known("SELECT 1 FROM workflow_runs WHERE workspace_path = ?1 LIMIT 1")
}

/// Who is using the checkout at `path` right now (KT-1748), as a phrase naming
/// the run, or `None` when nothing recorded holds it. A holder is: a workflow run
/// that has not finished, a task execution (other than `own_execution_id`, the
/// integration asking) that has not reached Done/Failed/Cancelled, or a pending or
/// running agent dispatch / resume job of the discussion bound to that path.
/// A run in `waiting_runs` is not a holder: it waits on the caller and writes
/// nothing meanwhile (KT-1795, a DelegateSubtasks step and its parent runs).
/// A database error answers "busy": an unreadable registry never says idle.
pub fn active_holder_of_checkout(
    conn: &Connection,
    path: &std::path::Path,
    own_execution_id: Option<&str>,
    waiting_runs: &[String],
) -> Option<String> {
    let path = path.to_string_lossy();
    let own = own_execution_id.unwrap_or("");
    let first = |sql: &str, params: &[&dyn rusqlite::ToSql], label: &dyn Fn(String) -> String| {
        conn.query_row(sql, params, |row| row.get::<_, String>(0))
            .optional()
            .map(|row| row.map(label))
    };
    let active_run = || -> rusqlite::Result<Option<String>> {
        let mut statement = conn.prepare(
            "SELECT id FROM workflow_runs WHERE workspace_path = ?1 \
             AND status IN ('Pending', 'Running', 'WaitingApproval', 'WaitingQuota')",
        )?;
        let ids = statement.query_map(params![path.as_ref()], |row| row.get::<_, String>(0))?;
        for id in ids {
            let id = id?;
            if !waiting_runs.contains(&id) {
                return Ok(Some(format!("workflow run {id} is still active")));
            }
        }
        Ok(None)
    };
    let probes = [
        active_run(),
        first(
            "SELECT te.id FROM discussion_workspaces dw \
               JOIN task_executions te ON te.id = dw.task_execution_id \
              WHERE dw.ownership = 'managed' AND (dw.canonical_path = ?1 OR dw.workspace_path = ?1) \
                AND te.id != ?2 AND te.status NOT IN ('Done', 'Failed', 'Cancelled') LIMIT 1",
            params![path.as_ref(), own],
            &|id| format!("task execution {id} is still active"),
        ),
        first(
            "SELECT job.id FROM discussion_workspaces dw \
               JOIN agent_dispatch_jobs job ON job.discussion_id = dw.disc_id \
              WHERE (dw.canonical_path = ?1 OR dw.workspace_path = ?1) \
                AND job.status IN ('Pending', 'Running') LIMIT 1",
            params![path.as_ref()],
            &|id| format!("agent dispatch {id} is pending or running"),
        ),
        first(
            "SELECT job.id FROM discussion_workspaces dw \
               JOIN agent_resume_jobs job ON job.discussion_id = dw.disc_id \
              WHERE (dw.canonical_path = ?1 OR dw.workspace_path = ?1) \
                AND job.status IN ('Pending', 'Running') LIMIT 1",
            params![path.as_ref()],
            &|id| format!("agent resume job {id} is pending or running"),
        ),
    ];
    for probe in probes {
        match probe {
            Ok(Some(holder)) => return Some(holder),
            Ok(None) => {}
            Err(error) => return Some(format!("the registry could not be read: {error}")),
        }
    }
    None
}

/// Retire a successfully removed managed checkout without erasing its lineage.
///
/// `canonical_path` is cleared so the historical row cannot claim ownership of
/// a path which no longer exists (or block a future declaration). The original
/// display path, branch, base SHA, final delivered HEAD and execution FK remain
/// durable and therefore survive backend restarts and physical cleanup.
pub fn retire_managed_for_execution(
    conn: &Connection,
    task_execution_id: &str,
    final_head_sha: Option<&str>,
) -> Result<Option<DiscussionWorkspace>> {
    conn.execute(
        "UPDATE discussion_workspaces
            SET state = 'detached', canonical_path = NULL,
                head_sha = COALESCE(?2, head_sha), updated_at = ?3
          WHERE task_execution_id = ?1 AND ownership = 'managed'",
        params![task_execution_id, final_head_sha, Utc::now().to_rfc3339()],
    )?;
    get_managed_for_execution(conn, task_execution_id)
}

/// Ownership-aware compensation: remove ONLY the `managed` row this execution
/// owns. The `ownership = 'managed'` guard makes it impossible to delete a
/// CLI-declared external checkout. Returns whether a row was removed. The physical
/// worktree teardown is the caller's separate ownership-checked step; this drops
/// only the DB intent row.
pub fn delete_managed_for_execution(conn: &Connection, task_execution_id: &str) -> Result<bool> {
    let removed = conn.execute(
        "DELETE FROM discussion_workspaces
          WHERE task_execution_id = ?1 AND ownership = 'managed'",
        params![task_execution_id],
    )?;
    Ok(removed > 0)
}

/// Managed rows whose owning execution was deleted. Migration 127 deliberately
/// uses `ON DELETE SET NULL` so evidence is not destroyed by a cascade; KT-322's
/// boot collector consumes these rows only after ownership and cleanliness have
/// been proved against Git.
pub fn list_orphaned_managed(conn: &Connection) -> Result<Vec<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE} WHERE dw.ownership = 'managed' \
         AND dw.task_execution_id IS NULL ORDER BY dw.created_at, dw.id"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map([], map_workspace)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The orphaned managed rows of `disc_id` that still name a checkout (KT-2084):
/// deleting the discussion would cascade away the only row leading to them.
pub fn orphaned_checkouts_of_discussion(
    conn: &Connection,
    disc_id: &str,
) -> Result<Vec<DiscussionWorkspace>> {
    let sql = format!(
        "{SELECT_WORKSPACE} WHERE dw.disc_id = ?1 AND dw.ownership = 'managed' \
         AND dw.task_execution_id IS NULL AND dw.canonical_path IS NOT NULL \
         ORDER BY dw.created_at, dw.id"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map(params![disc_id], map_workspace)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Delete one exact orphan intent after its physical checkout was safely
/// removed (or was already absent). The full predicate prevents a raced owner
/// reattachment from being erased by a stale boot scan.
pub fn delete_orphaned_managed(conn: &Connection, workspace_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM discussion_workspaces \
         WHERE id = ?1 AND ownership = 'managed' AND task_execution_id IS NULL",
        [workspace_id],
    )? > 0)
}

/// Acquire or renew the advisory history-rewrite lease for a declared
/// workspace. The caller must first create `backup_ref` at the declared HEAD;
/// we persist both values as an auditable proof, not a model assertion.
pub fn acquire_history_lease(
    conn: &Connection,
    disc_id: &str,
    session_pk: i64,
    backup_ref: &str,
) -> Result<HistoryLeaseAcquire> {
    let tx = conn.unchecked_transaction()?;
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    tx.execute(
        "UPDATE discussion_workspace_history_leases
            SET released_at = ?1, release_reason = 'expired'
          WHERE released_at IS NULL AND unixepoch(expires_at) <= unixepoch(?1)",
        [&now_text],
    )?;

    let workspace = get_for_session(&tx, disc_id, session_pk)?.ok_or_else(|| {
        anyhow::anyhow!("declare this session workspace before acquiring a lease")
    })?;
    if workspace.state != "attached" {
        bail!("the declared workspace is not attached");
    }
    let canonical_path = workspace
        .canonical_path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the declared workspace has no canonical path"))?;
    let head_sha = workspace
        .head_sha
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the declared workspace has no HEAD"))?;

    let active_sql = format!(
        "{SELECT_HISTORY_LEASE}
         WHERE lease.canonical_path = ?1 AND lease.branch = ?2
           AND lease.released_at IS NULL
         LIMIT 1"
    );
    if let Some(active) = tx
        .query_row(
            &active_sql,
            params![&canonical_path, &workspace.branch],
            map_history_lease,
        )
        .optional()?
    {
        if active.session_pk != session_pk {
            tx.commit()?;
            return Ok(HistoryLeaseAcquire::Blocked(active));
        }
        let expires_at = (now + Duration::seconds(HISTORY_LEASE_SECONDS)).to_rfc3339();
        tx.execute(
            "UPDATE discussion_workspace_history_leases
                SET backup_ref = ?2, head_sha = ?3, expires_at = ?4
              WHERE id = ?1 AND released_at IS NULL",
            params![active.id, backup_ref, head_sha, expires_at],
        )?;
        let renewed = tx.query_row(
            &active_sql,
            params![&canonical_path, &workspace.branch],
            map_history_lease,
        )?;
        tx.commit()?;
        return Ok(HistoryLeaseAcquire::Acquired(renewed));
    }

    let id = Uuid::new_v4().to_string();
    let expires_at = (now + Duration::seconds(HISTORY_LEASE_SECONDS)).to_rfc3339();
    tx.execute(
        "INSERT INTO discussion_workspace_history_leases (
             id, disc_id, session_pk, canonical_path, branch, backup_ref,
             head_sha, acquired_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            disc_id,
            session_pk,
            canonical_path,
            workspace.branch,
            backup_ref,
            head_sha,
            now_text,
            expires_at,
        ],
    )?;
    let lease = tx.query_row(
        &active_sql,
        params![canonical_path, workspace.branch],
        map_history_lease,
    )?;
    tx.commit()?;
    Ok(HistoryLeaseAcquire::Acquired(lease))
}

pub fn release_history_lease(conn: &Connection, disc_id: &str, session_pk: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE discussion_workspace_history_leases
            SET released_at = ?3, release_reason = 'released'
          WHERE disc_id = ?1 AND session_pk = ?2 AND released_at IS NULL",
        params![disc_id, session_pk, Utc::now().to_rfc3339()],
    )?;
    Ok(changed > 0)
}
