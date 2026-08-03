//! KT-140 — durable multi-worktree bindings for discussions and joined CLIs.

use anyhow::{bail, Result};
use chrono::Utc;
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
    pub created_at: String,
    pub updated_at: String,
}

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
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

const SELECT_WORKSPACE: &str = "
    SELECT dw.id, dw.disc_id, dw.session_pk, ds.agent_type,
           dw.task_id, pt.task_number,
           dw.project_id, dw.workspace_path, dw.canonical_path, dw.branch,
           dw.head_sha, dw.ownership, dw.state, dw.created_at, dw.updated_at
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
             workspace_path, canonical_path, branch, head_sha,
             ownership, state, created_at, updated_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

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
        assert!(error.to_string().contains("UNIQUE constraint failed"));
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
}
