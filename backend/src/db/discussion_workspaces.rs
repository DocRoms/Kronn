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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;
    use crate::db::orchestration::launch_single_task;
    use crate::models::{LaunchSingleTaskInput, OrchestrationActor, PlanningActorKind};

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p1', 'Kronn', '/repo', 'now', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode
             ) VALUES ('d1', 'p1', 'Room', 'now', 'now', 'Direct')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (7, 'd1', 'Codex', 'sess-1', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn external_session_workspace_is_updated_in_place() {
        let conn = fixture();
        let first = upsert_external(
            &conn,
            "d1",
            7,
            None,
            "p1",
            "/repo-wt",
            "/repo-wt",
            "feature/one",
            "abc",
        )
        .unwrap();
        let second = upsert_external(
            &conn,
            "d1",
            7,
            None,
            "p1",
            "/repo-wt",
            "/repo-wt",
            "feature/two",
            "def",
        )
        .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(second.branch, "feature/two");
        assert_eq!(list_for_discussion(&conn, "d1").unwrap().len(), 1);
    }

    #[test]
    fn canonical_path_cannot_belong_to_two_discussions() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode
             ) VALUES ('d2', 'p1', 'Other', 'now', 'now', 'Direct')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd2', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        upsert_external(
            &conn, "d1", 7, None, "p1", "/repo-wt", "/repo-wt", "a", "abc",
        )
        .unwrap();

        let error = upsert_external(
            &conn, "d2", 8, None, "p1", "/repo-wt", "/repo-wt", "a", "abc",
        )
        .unwrap_err();
        assert!(error.to_string().contains("another discussion"));
    }

    #[test]
    fn missing_external_workspace_keeps_its_path_and_ownership() {
        let conn = fixture();
        let declared = upsert_external(
            &conn,
            "d1",
            7,
            None,
            "p1",
            "/repo-wt",
            "/repo-wt",
            "feature/missing",
            "abc",
        )
        .unwrap();

        let missing = mark_missing(&conn, "d1", 7).unwrap().unwrap();

        assert_eq!(missing.id, declared.id);
        assert_eq!(missing.state, "missing");
        assert_eq!(missing.ownership, "external");
        assert_eq!(missing.workspace_path.as_deref(), Some("/repo-wt"));
    }

    #[test]
    fn same_room_sessions_share_a_workspace_but_another_room_is_rejected() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd1', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        upsert_external(
            &conn, "d1", 7, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
        )
        .unwrap();
        upsert_external(
            &conn, "d1", 8, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
        )
        .expect("the same discussion may intentionally share one checkout");
        assert_eq!(list_for_discussion(&conn, "d1").unwrap().len(), 2);

        conn.execute(
            "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode
             ) VALUES ('d2', 'p1', 'Other', 'now', 'now', 'Direct')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (9, 'd2', 'Gemini', 'sess-3', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        let error = upsert_external(
            &conn, "d2", 9, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
        )
        .unwrap_err();
        assert!(error.to_string().contains("another discussion"));
    }

    #[test]
    fn history_rewrite_lease_is_exclusive_renewable_and_releasable() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd1', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        for session_pk in [7, 8] {
            upsert_external(
                &conn,
                "d1",
                session_pk,
                None,
                "p1",
                "/repo-wt",
                "/repo-wt",
                "release/0.9.7",
                "abc",
            )
            .unwrap();
        }

        let first = acquire_history_lease(
            &conn,
            "d1",
            7,
            "refs/kronn-backup/release-0.9.7-before-squash",
        )
        .unwrap();
        let HistoryLeaseAcquire::Acquired(first) = first else {
            panic!("first holder must acquire")
        };
        let renewed = acquire_history_lease(
            &conn,
            "d1",
            7,
            "refs/kronn-backup/release-0.9.7-before-squash",
        )
        .unwrap();
        let HistoryLeaseAcquire::Acquired(renewed) = renewed else {
            panic!("same holder must renew idempotently")
        };
        assert_eq!(renewed.id, first.id);

        let blocked = acquire_history_lease(
            &conn,
            "d1",
            8,
            "refs/kronn-backup/release-0.9.7-before-squash-2",
        )
        .unwrap();
        let HistoryLeaseAcquire::Blocked(owner) = blocked else {
            panic!("the second session must be refused")
        };
        assert_eq!(owner.session_pk, 7);
        assert_eq!(owner.session_agent_type, "Codex");

        assert!(!release_history_lease(&conn, "d1", 8).unwrap());
        assert!(release_history_lease(&conn, "d1", 7).unwrap());
        assert!(matches!(
            acquire_history_lease(
                &conn,
                "d1",
                8,
                "refs/kronn-backup/release-0.9.7-before-squash-2"
            )
            .unwrap(),
            HistoryLeaseAcquire::Acquired(_)
        ));
    }

    #[test]
    fn expired_history_lease_never_blocks_the_next_session() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO discussion_sessions (
                 id, disc_id, agent_type, session_id, role, status, joined_at
             ) VALUES (8, 'd1', 'ClaudeCode', 'sess-2', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        for session_pk in [7, 8] {
            upsert_external(
                &conn, "d1", session_pk, None, "p1", "/repo-wt", "/repo-wt", "shared", "abc",
            )
            .unwrap();
        }
        acquire_history_lease(&conn, "d1", 7, "refs/kronn-backup/first").unwrap();
        conn.execute(
            "UPDATE discussion_workspace_history_leases SET expires_at = '2000-01-01T00:00:00Z'",
            [],
        )
        .unwrap();
        assert!(matches!(
            acquire_history_lease(&conn, "d1", 8, "refs/kronn-backup/second").unwrap(),
            HistoryLeaseAcquire::Acquired(_)
        ));
    }

    // ─── KT-318 socle: backend-owned managed workspaces ──────────────────────

    /// Seed the parent discussion + a planning task, then launch a REAL execution
    /// in it (Pending) so the managed-workspace FK `task_execution_id →
    /// task_executions(id)` is satisfied. Returns the real execution id — the
    /// managed writer is exercised against a genuine execution row, not a literal.
    fn seed_execution(conn: &Connection, task_id: &str, number: i64) -> String {
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at, workspace_mode) \
             VALUES ('d-parent', 'p1', 'Principal', 'now', 'now', 'Direct')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
             VALUES (?1, ?2, 'T', 'now', 'now')",
            params![task_id, number],
        )
        .unwrap();
        launch_single_task(
            conn,
            &LaunchSingleTaskInput::new(task_id, "d-parent"),
            &OrchestrationActor {
                kind: PlanningActorKind::Backend,
                id: Some("orchestrator".into()),
                session_id: None,
                source_message_id: None,
            },
        )
        .unwrap()
        .execution
        .id
    }

    #[test]
    fn managed_workspace_is_backend_owned_and_idempotent_by_execution() {
        let conn = fixture();
        let exec = seed_execution(&conn, "task-1", 1);

        // No joined CLI session at all — the backend provisioner owns the row.
        let ws = upsert_managed(
            &conn,
            &exec,
            "d1",
            "d-parent",
            Some("task-1"),
            "p1",
            "/repo/.kronn/worktrees/child",
            "/repo/.kronn/worktrees/child",
            "kronn/task/KT-1-abcdef",
            "headsha",
            "basesha",
        )
        .unwrap();
        assert_eq!(ws.ownership, "managed");
        assert_eq!(ws.state, "attached");
        assert_eq!(ws.session_pk, None, "a managed row carries no CLI session");
        assert_eq!(ws.task_execution_id.as_deref(), Some(exec.as_str()));
        assert_eq!(ws.parent_discussion_id.as_deref(), Some("d-parent"));
        assert_eq!(ws.base_sha.as_deref(), Some("basesha"));

        // Idempotent by task_execution_id: a compensable retry re-attaches its OWN
        // row (same id) and refreshes the mutable fields, never a duplicate.
        let again = upsert_managed(
            &conn,
            &exec,
            "d1",
            "d-parent",
            Some("task-1"),
            "p1",
            "/repo/.kronn/worktrees/child",
            "/repo/.kronn/worktrees/child",
            "kronn/task/KT-1-abcdef",
            "newhead",
            "basesha",
        )
        .unwrap();
        assert_eq!(
            again.id, ws.id,
            "same execution reuses the same managed row"
        );
        assert_eq!(again.head_sha.as_deref(), Some("newhead"));
        assert_eq!(
            get_managed_for_execution(&conn, &exec).unwrap().unwrap().id,
            ws.id
        );

        // Mark-missing keeps the auditable row (compensation-friendly, no orphan).
        let missing = mark_missing_for_execution(&conn, &exec).unwrap().unwrap();
        assert_eq!(missing.state, "missing");
        assert_eq!(missing.ownership, "managed");

        // Successful physical cleanup retires the checkout without erasing the
        // room ↔ execution ↔ branch/commit evidence.
        let retired = retire_managed_for_execution(&conn, &exec, Some("delivered-head"))
            .unwrap()
            .unwrap();
        assert_eq!(retired.state, "detached");
        assert_eq!(retired.canonical_path, None);
        assert_eq!(
            retired.workspace_path.as_deref(),
            Some("/repo/.kronn/worktrees/child")
        );
        assert_eq!(retired.head_sha.as_deref(), Some("delivered-head"));
        assert_eq!(retired.task_execution_id.as_deref(), Some(exec.as_str()));

        let principal_rows = list_visible_for_discussion(&conn, "d-parent").unwrap();
        assert_eq!(principal_rows.len(), 1);
        assert_eq!(principal_rows[0].disc_id, "d1");
        assert_eq!(principal_rows[0].state, "detached");
        assert_eq!(
            get_visible_for_discussion(&conn, "d-parent", &ws.id)
                .unwrap()
                .unwrap()
                .id,
            ws.id
        );

        // Compensation still has an ownership-aware destructive primitive for
        // workspaces which never reached a deliverable execution.
        assert!(delete_managed_for_execution(&conn, &exec).unwrap());
        assert!(get_managed_for_execution(&conn, &exec).unwrap().is_none());
    }

    #[test]
    fn managed_workspace_set_null_orphan_is_explicitly_collectable() {
        let conn = fixture();
        let exec = seed_execution(&conn, "task-orphan", 2);
        let workspace = upsert_managed(
            &conn,
            &exec,
            "d1",
            "d-parent",
            Some("task-orphan"),
            "p1",
            "/repo/.kronn/worktrees/orphan",
            "/repo/.kronn/worktrees/orphan",
            "kronn/task/KT-2-orphan",
            "head",
            "base",
        )
        .unwrap();

        conn.execute("DELETE FROM task_executions WHERE id = ?1", [&exec])
            .unwrap();
        let orphans = list_orphaned_managed(&conn).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].id, workspace.id);
        assert_eq!(orphans[0].task_execution_id, None);
        assert!(delete_orphaned_managed(&conn, &workspace.id).unwrap());
        assert!(list_orphaned_managed(&conn).unwrap().is_empty());
    }

    #[test]
    fn one_managed_row_per_execution_and_managed_coexists_with_external() {
        let conn = fixture();
        let exec = seed_execution(&conn, "task-1", 1);
        upsert_managed(
            &conn,
            &exec,
            "d1",
            "d-parent",
            None,
            "p1",
            "/wt/a",
            "/wt/a",
            "kronn/task/KT-1-a",
            "h",
            "b",
        )
        .unwrap();

        // A SECOND managed row for the SAME execution (different physical path) is
        // rejected by the unique partial index — a retry cannot fork a second row.
        let dup = conn.execute(
            "INSERT INTO discussion_workspaces \
             (id, disc_id, session_pk, project_id, workspace_path, canonical_path, branch, \
              head_sha, ownership, state, created_at, updated_at, task_execution_id) \
             VALUES ('other', 'd1', NULL, 'p1', '/wt/b', '/wt/b', 'br', 'h', 'managed', \
                     'attached', 'now', 'now', ?1)",
            params![exec],
        );
        assert!(
            dup.is_err(),
            "a second managed row for one execution must be rejected"
        );

        // A managed row (session_pk NULL) and an external row (session_pk set)
        // coexist: the external unique index is (disc_id, session_pk) WHERE
        // session_pk IS NOT NULL, so a NULL-session managed row never collides.
        // Session id 9 (fixture already holds id 7) avoids a PK/uniqueness clash.
        conn.execute(
            "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (9, 'd1', 'Codex', 'sess-9', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        upsert_external(
            &conn, "d1", 9, None, "p1", "/wt/ext", "/wt/ext", "br2", "h2",
        )
        .unwrap();
        assert_eq!(list_for_discussion(&conn, "d1").unwrap().len(), 2);
    }

    #[test]
    fn get_managed_for_discussion_detects_backend_owned_row_regardless_of_path() {
        let conn = fixture();
        let exec = seed_execution(&conn, "task-1", 1);

        // No managed row yet → nothing to refuse.
        assert!(get_managed_for_discussion(&conn, "d1").unwrap().is_none());

        upsert_managed(
            &conn,
            &exec,
            "d1",
            "d-parent",
            Some("task-1"),
            "p1",
            "/repo/.kronn/worktrees/child",
            "/repo/.kronn/worktrees/child",
            "kronn/task/KT-1-abcdef",
            "h",
            "b",
        )
        .unwrap();

        // The refusal keys on (disc_id, ownership='managed'), so it fires whether a
        // CLI re-declares the SAME canonical path or a DIFFERENT one — both resolve
        // to "this room already has a backend-owned worktree, read it".
        let found = get_managed_for_discussion(&conn, "d1").unwrap().unwrap();
        assert_eq!(found.ownership, "managed");
        assert_eq!(found.task_execution_id.as_deref(), Some(exec.as_str()));

        // An external-only room is NOT refused — the detection must not over-fire.
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at, workspace_mode) \
             VALUES ('d-ext', 'p1', 'Ext', 'now', 'now', 'Direct')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (11, 'd-ext', 'Codex', 'sess-11', 'peer', 'active', 'now')",
            [],
        )
        .unwrap();
        upsert_external(
            &conn, "d-ext", 11, None, "p1", "/wt/ext", "/wt/ext", "br", "h",
        )
        .unwrap();
        assert!(get_managed_for_discussion(&conn, "d-ext")
            .unwrap()
            .is_none());
    }
}
