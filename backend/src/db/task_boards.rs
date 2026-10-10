//! KT-1030 — storage for task boards: the planning tasks sharing one tag, and
//! the order of their open cards (the planning `rank` is renumbered across a
//! whole priority band, so it cannot hold a per-board order).

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

/// Most tasks one board reads; a board is a personal list, not a backlog.
pub const MAX_BOARD_TASKS: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
pub struct BoardTask {
    pub id: String,
    pub reference: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    /// Every tag of the task, board tag included, in name order.
    pub tags: Vec<String>,
    /// The first discussion linked to the task, if any.
    pub discussion_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// The non-archived tasks carrying `tag` (case-insensitive), oldest first.
pub fn load_tasks(conn: &Connection, tag: &str) -> Result<Vec<BoardTask>> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.task_number, t.title, t.description, t.status, t.priority, t.created_at, t.updated_at
           FROM planning_tasks t
          WHERE t.status <> 'archived'
            AND EXISTS (SELECT 1 FROM planning_task_tags tt
                         WHERE tt.task_id = t.id AND tt.tag = ?1 COLLATE NOCASE)
          ORDER BY t.created_at, t.task_number
          LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![tag, MAX_BOARD_TASKS as i64], |row| {
            Ok(BoardTask {
                id: row.get(0)?,
                reference: format!("KT-{}", row.get::<_, i64>(1)?),
                title: row.get(2)?,
                description: row.get(3)?,
                status: row.get(4)?,
                priority: row.get(5)?,
                tags: Vec::new(),
                discussion_id: None,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut tags = conn.prepare(
        "SELECT tag FROM planning_task_tags WHERE task_id = ?1 ORDER BY tag COLLATE NOCASE",
    )?;
    let mut discussions = conn.prepare(
        "SELECT discussion_id FROM planning_task_discussions
          WHERE task_id = ?1 ORDER BY created_at, discussion_id LIMIT 1",
    )?;
    let mut tasks = rows;
    for task in &mut tasks {
        task.tags = tags
            .query_map([&task.id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        task.discussion_id = discussions
            .query_row([&task.id], |row| row.get(0))
            .optional()?;
    }
    Ok(tasks)
}

/// The stored order of a board's open cards; empty when never saved or unreadable.
pub fn get_order(conn: &Connection, tag: &str) -> Result<Vec<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT task_ids FROM task_board_orders WHERE tag = ?1",
            [tag],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default())
}

pub fn set_order(conn: &Connection, tag: &str, task_ids: &[String]) -> Result<()> {
    conn.execute(
        "INSERT INTO task_board_orders (tag, task_ids, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(tag) DO UPDATE SET task_ids = excluded.task_ids, updated_at = excluded.updated_at",
        params![tag, serde_json::to_string(task_ids)?, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CreatePlanningTaskRequest, PlanningTaskStatus};

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn task(conn: &Connection, title: &str, tags: &[&str]) -> String {
        crate::db::planning::create_task(
            conn,
            &CreatePlanningTaskRequest {
                title: title.into(),
                discussion_id: None,
                idempotency_key: None,
                description: String::new(),
                status: PlanningTaskStatus::Todo,
                priority: Default::default(),
                parent_id: None,
                project_ids: vec![],
                tags: tags.iter().map(|tag| tag.to_string()).collect(),
                definition_of_done: vec![],
                links: vec![],
                actor: Default::default(),
            },
        )
        .unwrap()
        .summary
        .id
    }

    #[test]
    fn loads_only_tasks_of_the_tag_whatever_its_case() {
        let conn = conn();
        let mine = task(&conn, "Mine", &["Todo", "front"]);
        task(&conn, "Other", &["backlog"]);
        let tasks = load_tasks(&conn, "todo").unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, mine);
        assert_eq!(tasks[0].tags, vec!["front".to_string(), "Todo".to_string()]);
        assert!(tasks[0].reference.starts_with("KT-"));
    }

    #[test]
    fn order_round_trips_and_a_corrupt_row_reads_empty() {
        let conn = conn();
        assert!(get_order(&conn, "todo").unwrap().is_empty());
        set_order(&conn, "todo", &["b".into(), "a".into()]).unwrap();
        set_order(&conn, "TODO", &["a".into(), "b".into()]).unwrap();
        assert_eq!(get_order(&conn, "todo").unwrap(), vec!["a", "b"]);
        conn.execute("UPDATE task_board_orders SET task_ids = 'nope'", [])
            .unwrap();
        assert!(get_order(&conn, "todo").unwrap().is_empty());
    }
}
